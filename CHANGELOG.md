# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-11-04

The first release: an editor that is usable for a day's work on a real project, with a language server,
a shell, git and an agent in the same window. Six milestones of the technical requirements are closed;
what is not in it is listed at the end.

### Editor

- Rope-backed text, tree-sitter highlighting for twelve languages, tabs with a caret kept for every open
  file, and an undo history per file that survives a save.
- Find and replace in the file (`Ctrl+F`, `Ctrl+H`): every match painted, the caret walked through them,
  `Replace all` as one step of undo.
- Indentation that follows the settings — width in columns, spaces or a tab, trailing whitespace taken
  off a saved file, autosave on a period — with each file keeping the indentation it was opened with.
- Markdown preview beside the source (`Ctrl+Shift+M`), its own renderer for tables and code.
- Atomic save, a file watcher that reloads a file changed elsewhere without losing edits, and a status
  bar that says what happened.

### Navigation, through a language server

- The client in `ide-lsp` speaks the protocol to a real child process: definition (`F12`, `Ctrl+Click`),
  implementations of a trait, references, incoming calls, the symbols of a file and of the project, a
  hover, the other uses of the symbol under the caret, a rename previewed before it is applied, a
  completion list, and the parameters of the call being written.
- A server that dies — or never starts — is noticed, dropped, said out loud, and brought back with a
  growing wait (0.5 s, 1 s, 2 s); after three deaths in a row the window stops and says so, and a
  person's own question starts one at once. The server of a project is dropped when the project changes.
- The servers this build knows are rust-analyzer, pyright, gopls and clangd; each one's command and
  arguments can be replaced in the settings, and its presence is reported instead of being fatal.

### Terminal

- A shell on a real pseudo-terminal with 2000 lines of scrollback (the number is a setting), several
  sessions in a strip of tabs, the keys going to the panel that was clicked last, the cursor-key mode
  and the mouse mode the program asked for.
- `Ctrl+Click` on a URL or a `file:line` in the output opens it — the page in the browser, the file at
  its line in the editor.

### Panels and layout

- Every panel is a place, not a document: the `Panels` menu lists them with their keys, the `×` on a tab
  puts a panel away without ending what is in it, and a panel comes back to the leaf it was taken from,
  beside the panels it stood with. Panels, their places and what was put away are remembered with the
  project, so the next launch — or a move to that project through the `Project` menu — is the window that
  was left.
- The light theme is drawn as three surfaces rather than one white sheet, and the window's own chrome
  (menu band, tab strip) has its greys.

### Git

- The panel follows the working tree (`gix`, no process per repaint), stages and unstages, opens a diff
  with the lines coloured, writes a commit, and finds it again in the history with the names beside it,
  a commit's files and the diff of the one in front. A branch is switched only where the real `git`
  agrees the work is safe, and a failed `push` is shown in the words `git` printed.

### Agent

- A panel that drives an ACP agent: conversations, permissions asked as cards, choices, the context
  figure of a conversation, the limits of a subscription, and desktop notifications while the panel is
  not in front. Agents are configuration — `.andy-ide/agents.toml`, and the settings offer it as a form.
  A crashed agent is started again on its own, unless that is switched off.

### Settings, files and projects

- One modal dialog (`Ctrl+,`) with seven sections — `View`, `Editor`, `Panels`, `Tools`, `Agents`,
  `Keys`, `About` — and no `Apply`: the window's part is written the moment it is changed, and the
  project's part with the project. The interface scale is a row of sizes; keys are listed from the one
  registry the window answers to.
- The tree makes, renames and deletes files and folders; a rename takes its tab and its buffer with it,
  and a delete of a file with unsaved edits is refused.
- Projects are opened from a list, from a folder dialog the window draws itself, and from the command
  line; a project can be forgotten from the list without touching its directory.

### Packaging

- Releases for Linux (x86_64 and aarch64 tarballs, and an AppImage), Windows (x86_64) and macOS
  (universal `.app` in a zip), built by `.github/workflows/release.yml` on a tag, with checksums.
- `andy-ide --version` and `andy-ide --help` answer without a window.

### Not in this release

- A file opened on a version from a commit, and a diff against an arbitrary base.
- Regular expressions in the project search.
- Reassigning the keys (the table is read-only).
- The five language servers beyond rust-analyzer are not bundled; they are looked up in `PATH`.
- The macOS bundle is ad-hoc signed unless a Developer ID is in the repository's secrets, and not
  notarised unless a notary profile is too — the README says what that costs on first launch.

[0.1.0]: https://github.com/UncleAndy/andy-ide/releases/tag/v0.1.0
