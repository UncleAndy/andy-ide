# andy-ide

A desktop IDE in Rust: projects, files, syntax highlighting, code navigation, an embedded
terminal, Git, and AI agents in one window.

## Defining property

Every capability lives in its own library crate that knows nothing about the GUI. Only the
`andy-ide` crate depends on `egui`/`eframe`. Code navigation is delegated to language servers
(LSP) instead of reimplementing a compiler front end, and agents speak ACP, so attaching a new
agent is a configuration entry rather than a code change.

```
                 +---------------------------------------------+
                 |                andy-ide (UI)                |
                 |  dock panels - editor - tabs - status bar   |
                 +----+--------+--------+--------+--------+----+
                      |        |        |        |        |
                 ide-core  ide-buffer ide-syntax ide-lsp  ...
                      |        |        |        |        |
                 projects   rope+undo  tree-    language  terminal
                 config     cursors    sitter   servers   (ide-term)
                 sessions                     agents (ide-agents)
                 fs watch                     git    (ide-git)
```

## Status

Milestone M1 in progress (framework and editing). Specification: `docs/TZ_andy_ide_v0.1.md`.

## Crates

| Crate | Responsibility |
|---|---|
| `ide-core` | Project model, file tree, configuration, sessions, filesystem watching |
| `ide-buffer` | Rope-backed text buffer, cursors, selections, undo history |
| `ide-syntax` | Language registry, tree-sitter grammars, incremental highlighting |
| `ide-lsp` | LSP client, language server registry, document synchronisation |
| `ide-agents` | Agent registry, ACP client, MCP bridge, edit proposals |
| `ide-term` | PTY sessions, screen emulation, link detection |
| `ide-git` | Repository state, diffs, index, commits, branches, history |
| `andy-ide` | The application: window, dock panels, editor |

## Requirements

- Nix with flakes (`nix develop` provides Rust 1.95+, clippy, rustfmt, rust-analyzer, git)
- Linux with Wayland or X11 for the window

## Build and run

```sh
nix develop
cargo build
cargo run -p andy-ide -- --project ~/projects/RUST/andy-ide
```

Build artifacts go to `$HOME/.cache/andy-ide-target`, never into the repository.

Checks:

```sh
nix develop --command bash -c 'cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check'
```

## Layout

```
crates/
  ide-core/    ide-buffer/   ide-syntax/   ide-lsp/
  ide-agents/  ide-term/     ide-git/
  andy-ide/                 the binary
docs/                       specification and acceptance reports
.hermes/plans/              implementation plans
```

## Documentation

- `docs/TZ_andy_ide_v0.1.md` — specification (Russian)

## License

Dual-licensed under MIT or Apache-2.0, at your option. See `LICENSE`.
