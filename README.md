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

Milestone M1 (framework and editing) is complete: window with dock panels, project tree,
rope-backed editor with tree-sitter highlighting for twelve languages, tabs, quick open,
session and layout restore, atomic save, filesystem watching, Markdown preview, dark and
light themes. Acceptance report: [`docs/M1_acceptance.md`](docs/M1_acceptance.md).

Milestone M2 (navigation through a language server) is under way: the LSP client in
`ide-lsp` speaks the protocol to a real child process, `F12` and `Ctrl+Click` go to the
definition, `Shift+F12` lists the places a symbol is used, `Ctrl+F12` shows the structure
of the file and `Ctrl+Alt+Shift+N` searches the symbols of the project. Resting the
pointer on a symbol shows its signature, and the other uses of the symbol under the
caret are dimmed behind the text. `Shift+F6` renames the symbol under the caret: the
server's answer is shown as a list of the places it would change, and nothing is written
until Enter is pressed on that list. The problems panel shows what the server published.
Plan and state:
[`.hermes/plans/2026-10-28_andy-ide-m2.md`](.hermes/plans/2026-10-28_andy-ide-m2.md).

Milestone M3 (terminal) is under way: `ide-term` runs a shell on a real pseudo-terminal
(`portable-pty`) and emulates the screen (`vt100`, 2000 lines of scrollback); the bottom
strip of the window is one row of tabs, so `Terminal` and `Problems` share it and whichever
tab is in front uses the whole width. Keys go to the panel that was clicked last, arrows
and `Home`/`End` follow the cursor-key mode the program asked for, and `F1`..`F12` are
sent as xterm sends them, and clicks reach a program that asked for the mouse
(`mc` selects the file under the pointer). With `Shift` held the mouse works for the
terminal again: a drag picks text even out of a program that wants the mouse, and
`Ctrl+Shift+C` copies it. `Alt+F12` brings the terminal forward, `View` names both
panels.

Plan and state:
[`.hermes/plans/2026-10-28_andy-ide-m3.md`](.hermes/plans/2026-10-28_andy-ide-m3.md).

## Crates

| Crate | Responsibility |
|---|---|
| `ide-core` | Project model, file tree, configuration, sessions, filesystem watching |
| `ide-buffer` | Rope-backed text buffer, cursors, selections, undo history |
| `ide-lsp` | Language server client: transport, handshake, document sync, navigation requests |
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

Started without `--project`, the editor opens the project that was open last, and
skips the remembered ones whose directory is gone. `Ctrl`+`+`, `Ctrl`+`-` and
`Ctrl`+`0` zoom the whole interface, and the size is kept in the `font_size` of
`~/.config/andy-ide/config.toml`, so the next launch opens at the same scale. A
window that was maximized comes back maximized: that flag lives in the same file.

Build artifacts go to `$HOME/.cache/andy-ide-target`, never into the repository.

Checks:

```sh
nix develop --command bash -c 'cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check'
```

The window itself is testable without a display: `crates/andy-ide/src/gui_tests.rs` runs the
real `App` through `egui_kittest`, finds panels and buttons by their labels, clicks
them, types into the editor and reads back what the application did. Editor and preview
text is painted rather than built from widgets, so those are checked through the buffer
and the state instead.

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
