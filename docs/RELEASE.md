# Publishing a release

One tag, one release. Pushing a tag that matches the version in `Cargo.toml` builds every platform on
GitHub runners and gathers the results into one release page; nothing is uploaded by hand.

## Before the tag

1. **The version.** `version = "0.1.0"` stands in `[workspace.package]` of `Cargo.toml`, and every crate
   takes it from there (`version.workspace = true`). The workflow reads that line and refuses to build
   when the tag disagrees with it.
2. **The notes.** `docs/release-notes/<version>.md` becomes the body of the release page. If the file is
   missing the page is a single heading with the version in it — write the file.
3. **The changelog.** `CHANGELOG.md` gets a section for the version, with the date.
4. **The gates, on this machine** (they also run in the workflow, on a machine that is not this one):

   ```sh
   nix develop --command bash -lc '
     export CARGO_TARGET_DIR=$HOME/.cache/andy-ide-target
     cargo fmt --check
     cargo clippy --workspace --all-targets -- -D warnings
     cargo test --workspace --no-fail-fast -- --skip real_
     cargo test -p andy-ide --bin andy-ide real_ -- --test-threads=1'
   ```

## The tag

```sh
git tag -a v0.1.0 -m "andy-ide 0.1.0"
git push origin main
git push origin v0.1.0
```

The second push is the release: `.github/workflows/release.yml` runs on `v*`.

## What the workflow builds

| Job | Runner | Result |
|:--|:--|:--|
| `version` | ubuntu-24.04 | the version, and the check that the tag is that version |
| `test` | ubuntu-22.04 | `fmt`, `clippy -D warnings`, and the deterministic tests |
| `linux` | ubuntu-22.04 and ubuntu-24.04-arm | `andy-ide-<version>-{x86_64,aarch64}-unknown-linux-gnu.tar.gz` |
| `appimage` | ubuntu-22.04 | `andy-ide-<version>-x86_64.AppImage`, unpacked once to prove it holds the program |
| `windows` | windows-latest | `andy-ide-<version>-x86_64-pc-windows-msvc.zip` |
| `macos` | macos-14 | both architectures, `lipo`-ed into one, `andy-ide-<version>-universal.app.zip` |
| `publish` | ubuntu-24.04 | one `SHA256SUMS`, the notes, one release page |

Ubuntu 22.04 is not an accident: glibc 2.35 is the oldest the release is expected to run on, and a binary
built against it also runs on everything newer. A build on `ubuntu-latest` would require the newest glibc
of everybody who downloads it.

The arm runners (`ubuntu-24.04-arm`) are free for public repositories. On a private one that job fails and
the rest of the release still lands.

## Draft or published

A tag push publishes at once. `Actions` → `release` → `Run workflow` builds the same thing from `main`
without a tag and leaves it as a **draft** (`draft: true` by default), which is the way to see the
artifacts before anybody else does. A draft is a real release nobody can see until `Publish` is pressed.

Re-running a failed `publish` job adds to the release that already exists (`gh release upload --clobber`)
instead of failing on its existence; tags are never moved by the workflow.

## Signing macOS

Two secrets, both optional:

- `MACOS_SIGN_IDENTITY` — a Developer ID Application certificate in the runner's keychain
  (`security import` in a step before the bundle, which the workflow does not do: it expects the
  certificate to be there already, e.g. through a keychain step of your own). Without it the bundle is
  signed ad-hoc (`codesign --sign -`) and runs after `xattr -d com.apple.quarantine`.
- `MACOS_NOTARY_PROFILE` — a `notarytool` keychain profile. When it is set the zip is submitted,
  waited on, and the bundle stapled.

Neither is needed for the release to exist. Without them the README tells the person what to do on the
first launch, and says why.

## What is not in the pipeline

- No Windows code signing (an `.exe` signed with a commercial certificate would cost a subscription);
  SmartScreen says so once.
- No `.deb`/`.rpm`/flatpak. The AppImage and the tarball are the two ways in; a package needs a
  maintainer for each distribution, and that is a different job from publishing a build.
- No update channel. The AppImage can carry a `zsync` file later; nothing reads it today.
