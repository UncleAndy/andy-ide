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
a session that comes back — the tabs and the layout, though not yet the position of the
caret — atomic save, filesystem watching, Markdown preview, dark and light themes.
Acceptance report: [`docs/M1_acceptance.md`](docs/M1_acceptance.md), whose remainder that
last item is.

Milestone M2 (navigation through a language server) is under way: the LSP client in
`ide-lsp` speaks the protocol to a real child process, `F12` and `Ctrl+Click` go to the
definition, `Shift+F12` lists the places a symbol is used, `Ctrl+F12` shows the structure
of the file and `Ctrl+Alt+Shift+N` searches the symbols of the project. Resting the
pointer on a symbol shows its signature, and the other uses of the symbol under the
caret are dimmed behind the text. `Shift+F6` renames the symbol under the caret: the
server's answer is shown as a list of the places it would change, and nothing is written
until Enter is pressed on that list. The problems panel shows what the server published.
What is not there yet: the list of the implementations of a trait, the hierarchy of
incoming calls, and completion. The client today asks a server for a definition, the
places a symbol is used, the symbols of a file and of the project, a hover, the other
uses under the caret and a rename, and for nothing else.
Plan and state:
[`.hermes/plans/2026-10-28_andy-ide-m2.md`](.hermes/plans/2026-10-28_andy-ide-m2.md).

Milestone M3 (terminal) is done: `ide-term` runs a shell on a real pseudo-terminal
(`portable-pty`) and emulates the screen (`vt100`, 2000 lines of scrollback); the bottom
strip of the window is one row of tabs, so `Terminal` and `Problems` share it and whichever
tab is in front uses the whole width. Keys go to the panel that was clicked last, arrows
and `Home`/`End` follow the cursor-key mode the program asked for, and `F1`..`F12` are
sent as xterm sends them, and clicks reach a program that asked for the mouse
(`mc` selects the file under the pointer). With `Shift` held the mouse works for the
terminal again: a drag picks text even out of a program that wants the mouse, and
`Ctrl+Shift+C` or `Ctrl+Insert` copies it, and `Shift+Insert` pastes. `Alt+F12` brings
the terminal forward, `View` names both panels. Sessions live in a strip of their own
above the screen: `+` opens another shell, `×` closes it with its process. A URL or a
`file:line` in the output is a link: `Ctrl+Click` opens it — the page in the browser,
the file at its line in the editor.

Every panel of the window is a place, not a document, and every one of them can be put
away: the `View` menu lists them all with the key that calls each one, and the `×` on a tab
means the same thing. Putting a panel away is not throwing it out — a shell keeps running,
a conversation keeps its words, the changes of a repository keep their places — and the
panel comes back where its kind belongs: the files on the left, the code in the middle,
everything else along the bottom beside the problems. Which panels are on screen, and where
each of them stands, are remembered with the project in `.andy-ide`: the layout is written
out together with the list of what was put away, so the next launch is the window that was
left. A layout is read back, not rebuilt — a panel that was dragged somewhere of its own
stays there, and only a panel the layout has never heard of is put where its kind belongs.
A project opened for the first time starts without the `Diff` and `History` panels: the Git
panel says the same things from inside itself, and a window with fewer tabs is a window
where the ones that are there can be read.

Plan and state:
[`.hermes/plans/2026-10-28_andy-ide-m3.md`](.hermes/plans/2026-10-28_andy-ide-m3.md).

Milestone M4 (git) is done: every one of its acceptance criteria is met — the panel
follows the changes, a diff opens with the lines coloured, a commit is written and is
found again in the history, a branch is switched only where the real `git` agrees the
work is safe, and a failed `push` is shown in the words `git` printed. What the milestone
has not got is an acceptance report of its own, the way M1 has one.

`ide-git` reads the repository through `gix` — no
process per repaint, and no lie when `git` is absent or wrapped: the branch, the dirty
bit and the list of changes, with `git status`'s two halves kept apart (in the index
against `HEAD`, on disk against the index), so a file staged and then edited shows
twice. The `Git` panel sits beside `Problems` and `Terminal` in the bottom strip
(`Alt+9`), names the branch, groups the changes under the two headings and opens a
file when its line is clicked; a project that is not a repository says so instead of
showing an empty list. A single click selects a change, a double click opens the file,
and `Enter` shows its diff under the list, in the panel itself — both sides read where they
live, `HEAD`'s tree and the index as objects, the work tree as a file, so looking at a
diff cannot disturb the work in progress. `Stage`, `Unstage` and `Stage all` move
changes into the index and `Commit` writes them with the message in the field. Reading
is native; writing is the real `git`, so hooks, signing and commit templates behave
exactly as they do on the command line, and a hook's refusal is shown in its own
words. `History` is the panel's other window — a choice inside the tab, not a tab of its
own: it lists the last two hundred commits newest first, and `Enter` on one shows what it
changed in the same pane under the list. A panel that opened another panel would take the
person away from the list they were reading, and the list is where they were. The
`Branches` menu switches
`HEAD` and starts new ones — through the real `git switch`, so a dirty tree that would
lose work is refused by `git` itself instead of by a guess. In the editor, the margin of
every open file carries a three-pixel stripe beside each line that the index does not
have yet — green for a line the file added, blue for one it changed, red where a line
went away — and staging a file takes the stripes away. A repository with a remote gains
`Fetch`, `Pull` and `Push`: they run on a thread of their own, so the window stays usable
while the network thinks, and `Pull` refuses to do anything but move the branch forward —
a panel with no conflict editor does not start a merge it cannot finish. `Ctrl+Shift+B`
blames the line under the caret — the whole file is walked once, natively, and the status
bar names the commit, its author and its date until the buffer is edited or the chord is
pressed again. A project that is not a repository yet is offered `Initialise a
repository`, which runs `git init -b main` in the project root.

Plan and state:
[`.hermes/plans/2026-10-28_andy-ide-m4.md`](.hermes/plans/2026-10-28_andy-ide-m4.md).

Milestone M5 (agents) is under way: the panel holds a conversation with an agent, a
permission is asked before the agent is allowed to do anything, and a second agent is a
table in `agents.toml`. Two things of the milestone are not there. The context of the
open file does not travel with a request — `prompt_params` carries the text alone, and a
prompt is that one block. And the rule "an edit reaches the file only after it was
confirmed, and shown as a diff" is not the one this editor follows.

`ide-agents` speaks the Agent Client Protocol over a child process's standard input and
output — one JSON-RPC message per line, a deadline on every wait, two threads per agent so
that nothing waits where the window is drawn. Which agents exist is `agents.toml`, not
code: a second agent is a table in that file, and a project's copy wins over the user's.
The code that would serve `fs/read_text_file` and `fs/write_text_file` — reads inside the
project and nowhere else, a write only where the editor has allowed it — is written and
tested, but it is not what happens: the panel claims neither of those capabilities and
never turns a write on, so an edit is the agent's own business, done through the
terminal it was configured with. The paragraph about `_meta.fileAccess` below says the
same thing to the agent, where it looks. `crates/ide-agents/src/bin/acp_test_agent.rs` is
a stand-in agent the tests drive, so the client can be exercised without a model
installed.

The panel is `Alt+0`: a list of the agents in the file, the conversation as it arrives, a
field whose Enter sends, and a permission question that stops the turn until it is
answered. The command that starts it is a field, not a hidden line of a file:
there is no complete list of ACP agents, so `hermes acp`, a path to something built this
afternoon, or `npx …` can be typed and started — read as a command line, with quotes and
escapes, and run without a shell in between, so `;` and `` stay characters of a file
name rather than becoming a second command. `Remember` writes it into `agents.toml`. The
session lives on a worker thread — the panel posts what was typed, the
worker pumps what the agent says — so an answer is drawn while it is still being written.
An agent has two fields on one row: the name the list shows, and the command that starts it.
There is no field for the terminal, because the terminal belongs to the agent — Hermes has
its own, configured there. What the editor does instead is say so: it claims no `fs/*` and
no `terminal/*` in `initialize`, and `session/new` carries `_meta.fileAccess = "terminal"`
with a note that every file operation should go through the agent's own terminal, in the
directory named there.

The hint about the symbol under the pointer belongs to the visible text and nothing else.
The editor allocates a rect the size of the whole document, so "the pointer is inside the
editor's rect" was true over every panel below it; the place is published only when the
pointer is over the part of the text that was actually drawn. Its prose is read as Markdown —
servers document in Markdown, and a hint that shows the asterisks shows the source, not the
documentation — while the signature keeps its monospace face. It is a window rather than a
picture of one: it stays for a moment after the pointer leaves the symbol, so the hand can
reach it, and while the pointer is on it it does not go away at all — the wheel scrolls an
answer too tall for the screen, the text can be selected and copied, links can be followed.
The hint grows to its whole text as long as the window has room; the scroll bar is what
happens when the window is smaller than the answer, not a habit. While the pointer is on
the hint nothing is asked about the code behind it — the hint floats over the text, and the
symbol under a window is not a symbol the person is pointing at.

A conversation outlives the window. The project keeps the agent's session id in
`.andy-ide/agent-session.json`, beside the layout, and the next launch asks the agent to
pick that conversation up — `session/load` when the agent says it can, a new session when
it does not remember, and nothing said about the difference an agent's memory cannot
account for. The id is written the moment the agent names it rather than on the way out: a
crash is the case where a conversation is most wanted back. It is kept per agent, since an
id offered to a different agent asks a stranger to remember somebody else's afternoon.

The foot of the panel is the field, and one row under it: the facts about the turn on the left —
how long it is taking, which model is answering, how hard it is thinking, how much of its context
window is spent — and the button that sends on the right. All three choices among the facts are
menus, and all three are answered by the agent, not by us. Anything the agent does not report is a
dash rather than a guess. The facts share the button's row and never wrap: a row that wraps is a
second row, and the second row is the height of the foot, which belongs to the field alone — a
figure gaining a character (the timer does it several times a second) would otherwise make the foot
a line taller, on a panel that is repainted only when something moves. What does not fit is left
out with a mark, which is cheaper than a line the conversation loses. The shell opens in the
project's own directory, and the agent is told that directory the moment the session is opened.

A model is listed under the provider that serves it. An agent wired to several providers
offers a flat list nobody can read, so the models are grouped by the part of their id before
the first `/` or `:`, each group folding away — the provider is the first thing a person
picks by, and the row under it names only the model.

Next to the model is the level of thinking. ACP carries it as a *setting* of its own
(`configOptions`, category `thought_level`), apart from the mode, which is what the agent may
do without asking: the panel shows the levels the agent names — for Hermes, `none`, `minimal`,
`low`, `medium`, `high`, `xhigh`, `max` and `ultra` — and sends the pick back with
`session/set_config_option`. An agent that offers no such setting gets a level button that
says so rather than a menu it would refuse: over ACP that is Hermes 0.22.5, where the level
belongs to the model in the Hermes profile
(`agent.reasoning_overrides`, or `agent.reasoning_effort` for all of them), and is resolved
again whenever a model is switched. The modes on the right are that other channel: `Default`,
`Accept Edits`, `Don't Ask`.

Changing the model, the level or the mode is written into the conversation as a service line
once the agent confirms it — `model: …`, `thinking: …`, `mode: …` — because an answer written
by a model a person did not ask for reads as a mistake.

Selecting text in the conversation and pressing `Ctrl+C` or `Ctrl+Ins` copies it. egui makes a
copy event out of `Ctrl+C` only, so the panel recognises both and re-issues the event after the
editor has had its turn and found nothing; it does so only while the pointer is over the panel,
so `Ctrl+Ins` in the editor still belongs to the editor.

An agent that falls over is started again. The panel waits a second, then two, four, eight —
up to thirty — and gives up after five tries in a row, saying so; a session that ran for a while
before dying does not count against the budget. Before the conversation is restored the panel is
cleared: the old transcript left in place is what made a reconnection look like the same
message twice. The session id and the chosen model survive the clearing, since restoring a
conversation is the point of it. `Stop` cancels the waiting as well as the process.

An agent can ask the person to choose between variants, not just to allow or refuse: the client
claims ACP's `elicitation` in `initialize`, and an `elicitation/create` request becomes buttons
in the panel, answered with the variant's value — the word the agent asked for, which is often
not the words on the button — or declined with `None of these`.

An agent can also ask a question there is nothing to choose from — Hermes asks for free text with
a bare `{"type": "string"}` and no values. That arrives with no variants at all, and a card of
heading and refusal is a question nobody can answer: such a question is drawn with a place to
write the answer, and `Enter` in it answers, the way it sends a message in the field under the
conversation. A question that is neither — a number, an object, a shape this editor has no form
for — is still shown with only the refusal, because refusing is the one answer that always fits.

A question of several answers is told apart from a question of one by the shape of the schema and
nothing else: Hermes asks for several by making the property an array of the same strings
(`acp_adapter/elicitation.py`). Such a question is drawn with a switch for each answer rather than
a button, because one click cannot be the whole answer, and the answer is sent with `Answer` once
it is complete; it goes back as a list, because the agent asked for a list. A refusal stays a
refusal — which is a different thing from an empty list.

A question is drawn as a card of its own: a heading in the colour of what is being asked (amber
for a permission, blue for a choice), the answers stacked full width below it, and the refusal as
a quieter button on its own line. A permission and a choice look the same, because from the
panel's side they are the same event — the agent has stopped and only a person can start it again.

The card takes the height of what is in it, and at most three quarters of the panel: a question
that covers the whole panel takes the conversation with it, and the conversation is what a person
reads to decide which answer is right. More answers than fit scroll in a window of their own,
while the heading and the refusal stay where they are — the refusal is the way out of a question
whose answers a person wants none of, and a way out has to be on screen. Each answer is one line
whatever its length, with the whole text under the pointer: the real ones are whole sentences, and
answers that wrap make a card as tall as the panel for no answer's sake.

Three things in that paragraph are not decoration, and each was paid for with a bug. The card is
measured by the panel's own drawn rect and not by `ui.available_width()`: in a dock the latter is
the room the panel could be given, and a card laid out to it comes out wider than the panel and
takes the refusal, which is right aligned, off the edge with it. The row the refusal sits in is
given its own height: `with_layout` is handed the whole available size and starts its child at the
top of it, so a right-aligned row left alone stretched the card to the bottom of the panel — it
came out as tall as the panel whatever it held, which is what a person noticed. And both scroll
areas in the panel are named: an unnamed one takes its id from the ui it is drawn in, the
conversation and the card are drawn in the same ui, and two of them under one id share their
state — which egui, in a debug build, says by drawing a red box and a line marked with a flame
over both, all over the panel.

The field takes the width of the panel and starts three lines tall, growing as lines are added:
a message to an agent is rarely one line, and a field of one line makes every such message a
single unreadable one. `Enter` sends it and `Shift`+`Enter` makes a new line, and the button that
sends stands under the field rather than beside it. A key is read as *plainly* pressed or not at
all: egui's own `consume_key` asks for the modifiers a shortcut needs and lets others be held
alongside, so `Shift`+`Enter` answered to a plain `Enter` and sent the half-written message.

The agent's own commands — `/help`, `/model` and the rest — are read from it rather than guessed:
Hermes announces them as an `available_commands_update` with a name, a description, and a hint for
the ones that take an argument. Typing `/` offers them back over the field with the agent's own
words; what is typed narrows the list; `↑`/`↓` move along it; `Enter` or `Tab` completes the one
the keyboard is on rather than sending half a command. A command is sent as a prompt that begins
with it, which is the only way the protocol carries one.

The person's own request is drawn as a block of its own with square corners and in a heavier
weight than the rest, so that an answer can be read against the question it answers. The heavier
weight is drawn rather than asked for: egui's `strong()` brightens the colour and nothing else,
and the fonts it ships have no bold face, so the line is painted twice a fraction of a point
apart.

Sending a message does not move the keyboard: the field keeps it, and a message typed while
the answer is still coming is queued and goes out when the turn ends. Clicking into the
editor does move it, and keeps it there. The answer is read as Markdown as it arrives, and
the agent's reasoning is folded away
behind a `Thinking…` marker: the working out is not the answer, and pouring it into the same
column makes the answer harder to find.

The room kept for the rows under the conversation is the field's own height plus its row, computed
from what those rows are rather than measured against the bottom of the panel: measured to the
bottom, the reserve also carried the panel's own padding, and it was measured against a drawing the
conversation above it had just moved — two figures each leaning on the other. The field grows a
line at a time, and the row under it does not move.

Plan and state:
[`.hermes/plans/2026-10-28_andy-ide-m5.md`](.hermes/plans/2026-10-28_andy-ide-m5.md).

## Crates

| Crate | Responsibility |
|---|---|
| `ide-core` | Project model, file tree, configuration, sessions, filesystem watching |
| `ide-buffer` | Rope-backed text buffer, cursors, selections, undo history |
| `ide-lsp` | Language server client: transport, handshake, document synchronisation, the server registry, navigation requests |
| `ide-syntax` | Language registry, tree-sitter grammars, incremental highlighting |
| `ide-agents` | Agent registry, ACP client, session state, permissions and questions |
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
