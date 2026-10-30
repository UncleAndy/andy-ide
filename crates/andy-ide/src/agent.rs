//! The conversation in the panel, kept off the thread that draws the window.
//!
//! An agent is a process, and a turn can take minutes. So the session lives on a worker
//! thread of its own: the panel posts what was typed, the worker pumps what the agent
//! says, and this module is the desk between them — a transcript, an input line, and the
//! question that is waiting for an answer.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ide_agents::{
    AgentCommand, ChoiceRequest, ConfigOption, Event as AgentEvent, Mode, Model, PermissionPolicy,
    PermissionRequest, REASONING, Registry, Session, THOUGHT_LEVEL, Usage,
};

/// How long the worker waits for the next thing before checking whether it was told to go.
const PUMP: std::time::Duration = std::time::Duration::from_millis(20);

/// How long the panel waits before starting a crashed agent again.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// How many times in a row an agent that keeps dying is started before the panel gives up.
/// A crash loop is not a conversation, and retrying it forever spends the person's time.
const RECONNECT_ATTEMPTS: u32 = 5;

/// The setting that carries how hard the model thinks, if the agent offers one.
///
/// ACP keeps this apart from the mode, which is what the agent may do without asking, and
/// the agent names its own levels: Hermes' are `none`, `minimal`, `low`, `medium`, `high`,
/// `xhigh`, `max` and `ultra`, and a panel that offered a table of its own would offer
/// levels the agent has never heard of.
pub fn thought_level(options: &[ConfigOption]) -> Option<&ConfigOption> {
    options
        .iter()
        .find(|option| option.category.as_deref() == Some(THOUGHT_LEVEL) || option.id == REASONING)
}

/// The provider a model id belongs to: the part before the first `/` or `:`.
///
/// ACP gives a model an id and a name, and nothing about who serves it. Agents that have
/// more than one provider say so in the id — `nous/hermes-4`, `openrouter:claude` — and a
/// menu that ignores it asks a person to read forty models in one flat list.
pub fn provider_of(id: &str) -> Option<&str> {
    let cut = id.find(['/', ':'])?;
    let provider = id[..cut].trim();
    (!provider.is_empty()).then_some(provider)
}

/// The models in the order the agent listed them, grouped by provider.
///
/// Groups keep the order they first appear in, and a model whose id names no provider
/// goes to the group that has no name: it is drawn without a header when it is the only
/// one, and under one when it is not.
pub fn grouped(models: &[Model]) -> Vec<(Option<String>, Vec<&Model>)> {
    let mut groups: Vec<(Option<String>, Vec<&Model>)> = Vec::new();
    for model in models {
        let provider = provider_of(&model.id).map(str::to_owned);
        match groups.iter_mut().find(|(name, _)| *name == provider) {
            Some((_, group)) => group.push(model),
            None => groups.push((provider, vec![model])),
        }
    }
    groups
}

/// What to call a model where the provider is already on screen.
///
/// Hermes labels its rows `Provider · model` (its inventory builds them that way), and a
/// list grouped by provider would then print the same word on every line. The label
/// itself is left alone: the status line has no group to lean on.
pub fn listed_name(name: &str) -> &str {
    match name.split_once(" · ") {
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => model,
        _ => name,
    }
}

/// One line of the transcript.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Line {
    /// What the person said.
    You(String),
    /// What the agent said, or is saying.
    Agent(String),
    /// What the agent was working out on the way: folded away in the panel.
    Thinking(String),
    /// A tool the agent ran, as it described it.
    Tool(String),
    /// Something the panel says on its own account.
    Note(String),
}

/// What the panel tells the worker.
enum Command {
    /// Write this to the agent, and start a turn.
    Prompt(String),
    /// Answer the question the agent is waiting on.
    Permission { id: u64, option: Option<String> },
    /// Answer a choice among variants: the one picked, or the ones picked when several could
    /// be, or a refusal.
    Choice {
        id: u64,
        field: String,
        multiple: bool,
        values: Option<Vec<String>>,
    },
    /// Switch models, and say so through the status line.
    Model(String),
    /// Switch mode: what the agent may do without asking.
    Mode(String),
    /// Change one of the settings the agent offers: the level of thinking, in this panel.
    Setting { config: String, value: String },
}

/// What the worker tells the panel.
enum Reply {
    /// The session is up: its id, and whether it was picked up from an earlier run.
    Ready { session: String, resumed: bool },
    /// Something the agent said.
    Event(AgentEvent),
    /// The session could not be started, or the worker gave up.
    Failed(String),
}

/// The worker thread, seen from the panel's side.
struct Worker {
    commands: Sender<Command>,
    replies: Receiver<Reply>,
}

/// The agent conversation shown in the panel.
pub struct AgentState {
    /// The agents `agents.toml` lists.
    pub registry: Registry,
    /// The agent the panel is talking to, by name.
    pub chosen: Option<String>,
    /// What has been said so far.
    pub transcript: Vec<Line>,
    /// What is being typed.
    pub input: String,
    /// What was typed while the agent was still answering, to send when the turn ends.
    pub queued: Option<String>,
    /// Whether the field is asking for the keyboard back after a send.
    pub wants_focus: bool,
    /// Set when the person asked to copy from the panel: the window turns the shortcut
    /// into the copy event egui's own text selection listens for.
    pub wants_copy: bool,
    /// Where the question card was drawn, when one is up: the tests hold the panel to it,
    /// and nothing else may read it as a place to click.
    pub card_rect: Option<egui::Rect>,
    /// The answers picked so far in a question that allows several. Kept here, beside the
    /// question, because the card is drawn again every frame and a click is one of them.
    pub picked: Vec<String>,
    /// Where the message field was drawn, and what the rows under the conversation took
    /// together last frame. Measured rather than counted: the field grows a line at a time as
    /// the person writes, and the conversation above it has to be told what to leave — a fixed
    /// number of points is a promise the field breaks the moment a second line is typed.
    pub field_rect: Option<egui::Rect>,
    pub below: f32,
    /// Whether a start is in flight: the process is coming up, and the status line says
    /// which step it is at rather than that it is on its way. The step itself is written
    /// by the worker through `progress`, so the panel can hold the line while the worker
    /// is busy starting.
    pub connecting: bool,
    /// The step of the start in flight ("Launching\u{2026}", "Getting agent
    /// parameters\u{2026}", "Getting model parameters\u{2026}"), written by the worker.
    progress: Arc<Mutex<String>>,
    /// Whether the last line of the transcript is an answer still growing. The pieces of one
    /// streamed answer share a line; the person's own words end it, because what follows them
    /// is the next answer, not another piece of the one before.
    agent_line_open: bool,
    /// What is being written into a question that has no answers to pick, and whether the
    /// card is asking for the keyboard to write it with.
    pub card_text: String,
    pub card_wants_focus: bool,
    /// Which of the offered commands the keyboard is on, while a slash is being typed.
    pub command_pick: usize,
    /// The commands the agent offers — `/help`, `/model` and the rest — as it named them.
    /// Read from the agent, never assumed: a panel that offered its own list would offer
    /// commands the agent has never heard of.
    pub commands: Vec<AgentCommand>,
    /// The command line that will be run: the source of truth for which agent starts.
    pub command: String,
    /// What the menu calls it: two agents from one binary with different arguments are
    /// two things to a person, and a name taken from the program cannot say which.
    pub name: String,
    /// What the panel is doing, in one line.
    pub status: String,
    /// The models the agent offers, and the one in use.
    pub models: Vec<Model>,
    pub current_model: Option<String>,
    /// Whether the model list is on screen. The panel owns this flag instead of the menu
    /// watching clicks, so that folding a provider group does not dismiss the list.
    pub models_open: bool,
    /// Where the model button was drawn, so the list can sit above it.
    pub model_button: Option<egui::Rect>,
    /// Where the row of facts under the field was drawn. It is one row by design: the foot of
    /// the panel must not grow when a figure in it gains a character.
    pub status_rect: Option<egui::Rect>,
    /// The modes it offers — what it may do without asking — and the one in use.
    pub modes: Vec<Mode>,
    pub current_mode: Option<String>,
    /// The other settings it offers, of which the level of thinking is the one this panel
    /// shows. Read from the agent, never assumed: an agent that offers no such setting has
    /// no level to change.
    pub options: Vec<ConfigOption>,
    /// How much of the model's context is spent, when the agent says.
    pub usage: Option<Usage>,
    /// Where the panel was last drawn, for the tests that point at it.
    pub shown_rect: Option<egui::Rect>,
    /// Set when the person asked to copy from the panel.
    /// When the turn in flight began, and how long the last one took.
    started: Option<Instant>,
    pub took: Option<Duration>,
    /// Set while a turn is in flight: the panel disables the input rather than queueing.
    pub busy: bool,
    /// The question waiting for a person's answer.
    pub asking: Option<(u64, PermissionRequest)>,
    /// The variants waiting for a person's choice, and what the agent called them.
    pub choice: Option<(u64, ChoiceRequest)>,
    /// Whether a crashed agent is started again without being asked.
    pub auto_reconnect: bool,
    /// How long to wait before the next attempt; zero in tests that cannot wait.
    pub reconnect_delay: Duration,
    /// Attempts made since the last completed turn: a crash loop is given up on.
    attempts: u32,
    /// When the next attempt is due, once a crash has been seen.
    retry_at: Option<Instant>,
    /// Set while `start` is being called by the reconnection, so a manual start and a
    /// retry do not share the same attempt count.
    reconnecting: bool,
    /// When the session last became ready: a crash after a long healthy run is not part
    /// of a crash loop, and the attempts are forgiven.
    connected_at: Option<Instant>,
    worker: Option<Worker>,
    root: Option<PathBuf>,
    /// Where the agents are kept for this project, once a project is known.
    place: Option<PathBuf>,
    /// The id of the conversation in use, once the agent has named it.
    pub session: Option<String>,
    /// An id the project kept from the last run, and the agent it belongs to: a
    /// conversation is a conversation with *that* agent, and naming it to another one
    /// would be asking a stranger to remember somebody else's afternoon.
    resume: Option<String>,
    resumed_from: Option<String>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            registry: Registry::default(),
            chosen: None,
            transcript: Vec::new(),
            input: String::new(),
            queued: None,
            wants_focus: false,
            wants_copy: false,
            card_rect: None,
            picked: Vec::new(),
            commands: Vec::new(),
            command_pick: 0,
            connecting: false,
            progress: Arc::new(Mutex::new(String::new())),
            agent_line_open: false,
            card_text: String::new(),
            card_wants_focus: false,
            field_rect: None,
            below: 64.0,
            command: String::new(),
            name: String::new(),
            status: String::new(),
            models: Vec::new(),
            current_model: None,
            models_open: false,
            model_button: None,
            status_rect: None,
            modes: Vec::new(),
            current_mode: None,
            options: Vec::new(),
            usage: None,
            shown_rect: None,
            started: None,
            took: None,
            busy: false,
            asking: None,
            choice: None,
            // An agent is a long conversation; a process that dies should not end it.
            auto_reconnect: true,
            reconnect_delay: RECONNECT_DELAY,
            attempts: 0,
            retry_at: None,
            reconnecting: false,
            connected_at: None,
            worker: None,
            root: None,
            place: None,
            session: None,
            resume: None,
            resumed_from: None,
        }
    }
}

impl AgentState {
    /// Points the panel at a project, reading its `agents.toml`.
    ///
    /// The worker is left alone when the root has not changed: a conversation survives
    /// the frames that draw it.
    pub fn follow(&mut self, root: Option<&Path>, config: &Path) {
        let root = root.map(Path::to_path_buf);
        if root == self.root {
            return;
        }
        self.root = root;
        self.stop();
        self.chosen = None;
        self.transcript.clear();
        self.asking = None;
        self.choice = None;
        self.picked.clear();
        self.card_text.clear();
        self.card_wants_focus = false;
        self.busy = false;
        self.session = None;
        self.resume = None;
        self.resumed_from = None;
        self.attempts = 0;
        self.retry_at = None;
        let Some(root) = self.root.clone() else {
            self.registry = Registry::defaults();
            self.status = "no project: an agent has nowhere to work".to_owned();
            return;
        };
        let place = ide_agents::registry::locate(Some(&root), config);
        self.place = Some(place.clone());
        match Registry::load_or_write(&place) {
            Ok(registry) => {
                self.chosen = registry.first().map(|agent| agent.name.clone());
                self.command = registry
                    .first()
                    .map(ide_agents::quote_command)
                    .unwrap_or_default();
                self.name = registry
                    .first()
                    .map(|agent| agent.name.clone())
                    .unwrap_or_default();
                self.registry = registry;
                self.status = match &self.chosen {
                    Some(name) => format!("{name}, not started"),
                    None => format!("no agents in {}", place.display()),
                };
            }
            Err(error) => {
                self.registry = Registry::defaults();
                self.status = format!("{error}");
            }
        }
        self.read_session(&root);
    }

    /// Where the project keeps the conversation: `.andy-ide/agent-session.json`.
    ///
    /// Beside `workspace.json`, because it is the same kind of thing — what this project
    /// was doing last time — and not a setting of the editor.
    pub(crate) fn session_file(root: &Path) -> PathBuf {
        root.join(".andy-ide").join("agent-session.json")
    }

    /// Reads the id the project kept, if it kept one.
    fn read_session(&mut self, root: &Path) {
        let Ok(text) = std::fs::read_to_string(Self::session_file(root)) else {
            return;
        };
        let Ok(saved) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let session = saved.get("session").and_then(|id| id.as_str());
        let agent = saved.get("agent").and_then(|name| name.as_str());
        if let (Some(session), Some(agent)) = (session, agent) {
            self.resume = Some(session.to_owned());
            self.resumed_from = Some(agent.to_owned());
        }
    }

    /// Writes the id down, with the agent it belongs to.
    fn remember_session(&mut self, session: &str) {
        let (Some(root), Some(agent)) = (self.root.clone(), self.chosen.clone()) else {
            return;
        };
        let file = Self::session_file(&root);
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let saved = serde_json::json!({ "agent": agent, "session": session });
        let _ = std::fs::write(file, format!("{saved:#}\n"));
        self.resumed_from = Some(agent);
        self.resume = Some(session.to_owned());
    }

    /// Starts what the command field says, exactly as it is written there.
    ///
    /// The field, and not the menu, is the source of truth: there is no list of ACP
    /// agents that is ever complete, and a panel that can only start what somebody wrote
    /// into a file beforehand is a panel that cannot try an agent a person has just
    /// built. A command that cannot be read — an unclosed quote, an empty line — is
    /// refused in words before anything is spawned.
    pub fn start(&mut self) {
        if self.worker.is_some() {
            self.status = "an agent is already running".to_owned();
            return;
        }
        // A start a person asked for begins a new run of attempts; a retry keeps counting
        // the crashes it is answering.
        if !self.reconnecting {
            self.attempts = 0;
            self.retry_at = None;
        }
        self.reconnecting = false;
        let Some(root) = self.root.clone() else {
            self.status = "no project: an agent has nowhere to work".to_owned();
            return;
        };
        let mut spec = match ide_agents::parse_named(&self.name, &self.command) {
            Ok(spec) => spec,
            Err(error) => {
                self.status = format!("{error}");
                return;
            }
        };
        // The extra environment belongs to the entry in `agents.toml`, not to the command
        // line: the field says how the process is started, the file says what it is started
        // with. The command is re-read on every start, so without this the `[agent.env]`
        // table the file supports would never reach a process.
        if let Some(known) = self.registry.get(&spec.name) {
            spec.env = known.env.clone();
        }

        let name = spec.name.clone();
        // The conversation kept by the project, but only for the agent that kept it.
        let resume = self
            .resume
            .clone()
            .filter(|_| self.resumed_from.as_deref() == Some(name.as_str()));
        self.connecting = true;
        self.progress = Arc::new(Mutex::new("Launching\u{2026}".to_owned()));
        let progress = Arc::clone(&self.progress);
        let (commands, jobs) = channel();
        let (answers, replies) = channel();
        let started = std::thread::Builder::new()
            .name(format!("agent-{name}"))
            .spawn(move || work(spec, root, resume, progress, jobs, answers));
        match started {
            Ok(_) => {
                self.worker = Some(Worker { commands, replies });
                self.chosen = Some(name.clone());
                self.status = format!("{name}: starting…");
            }
            Err(error) => self.status = format!("{name} did not start: {error}"),
        }
    }

    /// Takes the copy shortcut when the pointer is over the panel.
    ///
    /// egui listens for a copy *event*, and the keyboard backend only makes one out of
    /// `Ctrl`+`C` — `Ctrl`+`Insert` is a copy only on Windows. So both are recognised
    /// here, and the event is put back into the input by the panel itself, after the
    /// editor has had its turn and found nothing to copy.
    pub fn take_copy_shortcut(&mut self, ctx: &egui::Context) {
        let Some(rect) = self.shown_rect else {
            return;
        };
        let over = ctx
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| rect.contains(pos));
        if !over {
            return;
        }
        ctx.input_mut(|input| {
            // The event, for `Ctrl`+`C`; the key, for `Ctrl`+`Insert`, which the backend
            // leaves in the queue as a plain key press.
            let mut asked = false;
            input.events.retain(|event| {
                if matches!(event, egui::Event::Copy) {
                    asked = true;
                    false
                } else {
                    true
                }
            });
            if input.consume_key(egui::Modifiers::CTRL, egui::Key::Insert) {
                asked = true;
            }
            if asked {
                self.wants_copy = true;
            }
        });
    }

    /// Writes the command into `agents.toml`, so the next start needs no typing.
    ///
    /// Nothing is written until it is asked for: a panel that saved every experiment
    /// would leave a file full of typos to clean up.
    pub fn remember(&mut self) {
        let spec = match ide_agents::parse_named(&self.name, &self.command) {
            Ok(spec) => spec,
            Err(error) => {
                self.status = format!("{error}");
                return;
            }
        };
        let Some(place) = self.place.clone() else {
            self.status = "no project: there is nowhere to keep an agent".to_owned();
            return;
        };
        self.registry.remember(spec);
        match self.registry.save(&place) {
            Ok(()) => self.status = format!("saved to {}", place.display()),
            Err(error) => self.status = format!("{error}"),
        }
    }

    /// Sends what was typed, if there is anything to send and someone to send it to.
    pub fn send(&mut self) {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let Some(worker) = &self.worker else {
            self.status = "start the agent first".to_owned();
            return;
        };
        if self.busy {
            // Typed while the answer is still coming: kept, not refused. The field is
            // where a person writes, and a field that disables itself mid-sentence is
            // one that hands the keyboard to whatever else wants it.
            self.queued = Some(text);
            self.input.clear();
            self.status = "queued: sent when this answer ends".to_owned();
            return;
        }
        if worker.commands.send(Command::Prompt(text.clone())).is_err() {
            self.status = "the agent is gone".to_owned();
            return;
        }
        // The words the person just sent are a boundary: whatever the agent says from here on
        // is the next answer, not another piece of the last one. A restored conversation that
        // ended in an answer would otherwise glue the new answer onto it — the replay arrives
        // without a mark of where it ends, and the load's own answer is not always the last
        // thing a panel hears before this message. The acting person is the mark.
        self.agent_line_open = false;
        self.transcript.push(Line::You(text));
        self.input.clear();
        self.busy = true;
        self.started = Some(Instant::now());
        self.took = None;
        self.status = "answering…".to_owned();
    }

    /// How long the turn in flight has been running, or how long the last one took.
    ///
    /// One number for both states, because it is one question: how long does this agent
    /// take? A timer that vanished between turns would answer it only half the time.
    pub fn elapsed(&self) -> Option<Duration> {
        self.started.map(|started| started.elapsed()).or(self.took)
    }

    /// Asks the agent to switch models; the choice shows up when it agrees.
    pub fn set_model(&mut self, model: &str) {
        let Some(worker) = &self.worker else {
            self.status = "start the agent first".to_owned();
            return;
        };
        if worker
            .commands
            .send(Command::Model(model.to_owned()))
            .is_err()
        {
            self.status = "the agent is gone".to_owned();
        }
    }

    /// Asks the agent for one of the settings it offers: a level of thinking, in this panel.
    pub fn set_setting(&mut self, config: &str, value: &str) {
        let Some(worker) = &self.worker else {
            self.status = "start the agent first".to_owned();
            return;
        };
        if worker
            .commands
            .send(Command::Setting {
                config: config.to_owned(),
                value: value.to_owned(),
            })
            .is_err()
        {
            self.status = "the agent is gone".to_owned();
        }
    }

    /// Asks the agent to switch mode: what it may do without asking.
    pub fn set_mode(&mut self, mode: &str) {
        let Some(worker) = &self.worker else {
            self.status = "start the agent first".to_owned();
            return;
        };
        if worker
            .commands
            .send(Command::Mode(mode.to_owned()))
            .is_err()
        {
            self.status = "the agent is gone".to_owned();
        }
    }

    /// Answers the choice among variants the agent is waiting on.
    ///
    /// `None` refuses: an agent must be able to hear that the person did not want any of
    /// the variants, or it waits forever for an answer that is not coming.
    pub fn answer_choice(&mut self, values: Option<Vec<String>>) {
        let Some((id, request)) = self.choice.take() else {
            return;
        };
        self.picked.clear();
        self.card_text.clear();
        self.card_wants_focus = false;
        let Some(worker) = &self.worker else {
            return;
        };
        let _ = worker.commands.send(Command::Choice {
            id,
            field: request.field,
            multiple: request.multiple,
            values: values.clone(),
        });
        self.status = match values {
            Some(values) => format!("chose {}", values.join(", ")),
            None => "declined the choice".to_owned(),
        };
    }

    /// Says in the conversation what a setup message changed.
    ///
    /// A switch is worth a line: it changes what answers and how hard it thinks, and a
    /// conversation that changed silently reads as if nothing had. Only a value that was
    /// already known can have changed — at the start of a session nothing has — so the
    /// first setup of all says nothing.
    fn note_switches(&mut self, state: &ide_agents::SessionState) {
        if let (Some(before), Some(after)) = (
            self.current_model.as_deref(),
            state.current_model.as_deref(),
        ) && before != after
        {
            let name = state
                .models
                .iter()
                .find(|model| model.id == after)
                .map(|model| model.name.clone())
                .unwrap_or_else(|| after.to_owned());
            self.transcript.push(Line::Note(format!("model: {name}")));
        }
        for before in &self.options {
            let Some(after) = state.options.iter().find(|option| option.id == before.id) else {
                continue;
            };
            if before.current == after.current {
                continue;
            }
            // The agent names its own choices; the level of thinking is the one this panel
            // has a word of its own for.
            let name = after.current_name().unwrap_or_else(|| after.name.clone());
            let what = if thought_level(std::slice::from_ref(after)).is_some() {
                "thinking".to_owned()
            } else {
                after.name.clone()
            };
            self.transcript.push(Line::Note(format!("{what}: {name}")));
        }
        if let (Some(before), Some(after)) =
            (self.current_mode.as_deref(), state.current_mode.as_deref())
            && before != after
        {
            let name = state
                .modes
                .iter()
                .find(|mode| mode.id == after)
                .map(|mode| mode.name.clone())
                .unwrap_or_else(|| after.to_owned());
            self.transcript.push(Line::Note(format!("mode: {name}")));
        }
    }

    /// Empties what the panel shows, keeping the conversation's id for the restart.
    ///
    /// A restored session replays its history through the agent's own events. Keeping
    /// the lines already on screen would draw every message twice, and the widget ids
    /// behind them would collide. The id stays: that is what makes it the same
    /// conversation rather than a new one.
    pub fn clear_panel(&mut self) {
        self.transcript.clear();
        self.agent_line_open = false;
        self.models.clear();
        self.current_model = None;
        self.models_open = false;
        self.model_button = None;
        self.modes.clear();
        self.current_mode = None;
        self.options.clear();
        self.usage = None;
        self.commands.clear();
        self.asking = None;
        self.choice = None;
        self.picked.clear();
        self.card_text.clear();
        self.card_wants_focus = false;
        self.busy = false;
        self.started = None;
        self.took = None;
        self.queued = None;
    }

    /// The step of the start in flight, or nothing when the panel is not starting.
    pub fn connecting_step(&self) -> String {
        self.progress
            .lock()
            .map(|step| step.clone())
            .unwrap_or_default()
    }

    /// Whether the panel is waiting to start a crashed agent again.
    pub fn waiting_to_reconnect(&self) -> bool {
        self.retry_at.is_some()
    }

    /// Schedules a restart after a crash, unless one is already scheduled.
    ///
    /// The wait grows with each attempt in a row: an agent that dies on start would
    /// otherwise be restarted as fast as the window repaints.
    fn schedule_reconnect(&mut self) {
        if !self.auto_reconnect || self.retry_at.is_some() {
            return;
        }
        // A session that ran for a while and then died is not a crash loop; only deaths
        // in quick succession count against the budget.
        if self
            .connected_at
            .is_some_and(|connected| connected.elapsed() > Duration::from_secs(60))
        {
            self.attempts = 0;
        }
        if self.attempts >= RECONNECT_ATTEMPTS {
            self.status = "the agent keeps stopping; not reconnecting again".to_owned();
            return;
        }
        let factor = 1u32 << self.attempts.min(5);
        let delay = (self.reconnect_delay * factor).min(Duration::from_secs(30));
        self.retry_at = Some(Instant::now() + delay);
        self.status = format!(
            "the agent stopped — reconnecting in {:.1} s",
            delay.as_secs_f32()
        );
    }

    /// Starts the agent again, with the panel emptied first.
    fn reconnect_now(&mut self) {
        self.retry_at = None;
        if self.worker.is_some() {
            return;
        }
        self.attempts += 1;
        self.clear_panel();
        let attempt = self.attempts;
        self.reconnecting = true;
        self.start();
        if self.worker.is_some() {
            self.status = format!(
                "the agent stopped — reconnecting (attempt {attempt} of {RECONNECT_ATTEMPTS})"
            );
        }
    }

    /// Answers the question the agent is waiting on.
    pub fn answer(&mut self, option: Option<String>) {
        let Some((id, _)) = self.asking.take() else {
            return;
        };
        let Some(worker) = &self.worker else {
            return;
        };
        let _ = worker.commands.send(Command::Permission { id, option });
        self.status = "answered".to_owned();
    }

    /// Takes everything the worker has said since the last frame.
    pub fn pump(&mut self) {
        // A crash is answered between frames, on the frame that notices it is due: the
        // panel does not keep a timer thread for a process it is about to start anyway.
        if self.retry_at.is_some_and(|at| Instant::now() >= at) {
            self.reconnect_now();
        }
        loop {
            let Some(worker) = &self.worker else {
                return;
            };
            match worker.replies.try_recv() {
                Ok(reply) => self.take(reply),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.busy = false;
                    self.schedule_reconnect();
                    return;
                }
            }
        }
    }

    /// Whether an agent is running.
    pub fn running(&self) -> bool {
        self.worker.is_some()
    }

    /// Ends the conversation.
    pub fn stop(&mut self) {
        if let Some(worker) = self.worker.take() {
            // Dropping the command queue tells the worker to leave; it stops the process
            // on its way out.
            drop(worker.commands);
        }
        self.connecting = false;
        self.busy = false;
        self.asking = None;
        self.choice = None;
        self.picked.clear();
        self.card_text.clear();
        self.card_wants_focus = false;
        // Stopping is a decision: a process ended on purpose is not one to start again.
        self.retry_at = None;
        self.attempts = 0;
    }

    /// Forgets the conversation the project kept, so the next start is a new one.
    ///
    /// The id is written into the project (`.andy-ide/agent-session.json`) and outlives
    /// the window. Forgetting it only in memory would bring the same conversation back
    /// on the next launch — which is not what a person asking for a new session wants.
    pub fn forget_session(&mut self) {
        if let Some(root) = self.root.clone() {
            let _ = std::fs::remove_file(Self::session_file(&root));
        }
        self.session = None;
        self.resume = None;
        self.resumed_from = None;
    }

    /// Ends this conversation and begins a clean one.
    ///
    /// The running process is stopped rather than left behind: two agents on one project
    /// would each keep their own idea of what was said.
    pub fn new_session(&mut self) {
        self.stop();
        self.forget_session();
        self.clear_panel();
        self.start();
    }

    /// Files one reply from the worker.
    fn take(&mut self, reply: Reply) {
        match reply {
            Reply::Ready { session, resumed } => {
                // The conversation's id is written down the moment it is known, not at
                // exit: a crash is exactly the case where a person would like their
                // conversation back, and a file written on the way out is not written.
                self.remember_session(&session);
                self.session = Some(session.clone());
                self.connected_at = Some(Instant::now());
                self.retry_at = None;
                // The replayed history ended with the load's answer: whatever the agent says
                // from here on is its own answer, a line of its own, and not another piece of
                // the last replayed one (`acp_adapter/server.py` replays before responding).
                self.agent_line_open = false;
                self.connecting = false;
                self.status = if resumed {
                    format!("ready, session {session} resumed")
                } else {
                    format!("ready, session {session}")
                };
            }
            Reply::Failed(error) => {
                self.transcript.push(Line::Note(error.clone()));
                self.status = error;
                self.busy = false;
                self.connecting = false;
            }
            Reply::Event(event) => match event {
                AgentEvent::Thinking(thought) => {
                    // One growing line per stretch of reasoning, like the answer below it.
                    if let Some(Line::Thinking(said)) = self.transcript.last_mut() {
                        said.push_str(&thought);
                    } else {
                        self.transcript.push(Line::Thinking(thought));
                    }
                    self.agent_line_open = false;
                }
                AgentEvent::Text(text) => {
                    // Pieces of one answer join the line it is growing into, which is what
                    // makes a stream read as a sentence and not as a column. A line that is
                    // not an answer growing — a replayed message, the person's words — is
                    // not added to.
                    if let Some(Line::Agent(said)) = self.transcript.last_mut()
                        && self.agent_line_open
                    {
                        said.push_str(&text);
                    } else {
                        self.transcript.push(Line::Agent(text));
                        self.agent_line_open = true;
                    }
                }
                AgentEvent::You(text) => {
                    // The person's own words, replayed with a restored session: a block like
                    // the one drawn when they were typed, and the answer that may follow is
                    // not glued to it.
                    self.transcript.push(Line::You(text));
                    self.agent_line_open = false;
                }
                AgentEvent::ToolCall { title, status, .. } => {
                    self.transcript
                        .push(Line::Tool(format!("{title} ({status})")));
                    self.agent_line_open = false;
                }
                AgentEvent::Plan(steps) => {
                    for step in steps {
                        self.transcript.push(Line::Tool(format!("· {step}")));
                    }
                    self.agent_line_open = false;
                }
                AgentEvent::Other { kind } => {
                    tracing::debug!(target: "andy_ide::agent", %kind, "an update this panel does not show");
                }
                AgentEvent::Permission { id, request } => {
                    self.status = request.title();
                    self.asking = Some((id, request));
                }
                AgentEvent::Commands(commands) => {
                    self.commands = commands;
                }
                AgentEvent::Choice { id, request } => {
                    self.status = request.title().to_owned();
                    // A new question starts with nothing picked and nothing written: what was
                    // answered for the last one is not an answer to this one.
                    self.picked.clear();
                    self.card_text.clear();
                    // The question is what the person has to deal with, so the keyboard goes to
                    // the card and not to the field that asks for the next message.
                    self.wants_focus = false;
                    self.card_wants_focus = true;
                    self.choice = Some((id, request));
                }
                AgentEvent::Setup(state) => {
                    self.note_switches(&state);
                    self.models = state.models;
                    self.current_model = state.current_model;
                    self.modes = state.modes;
                    self.current_mode = state.current_mode;
                    self.options = state.options;
                }
                AgentEvent::Usage(usage) => {
                    self.usage = Some(usage);
                }
                AgentEvent::Finished(reason) => {
                    // The timer stops with the turn and keeps its figure: a spinner that
                    // resets to zero tells nobody how long the last answer took.
                    self.took = self.started.take().map(|started| started.elapsed());
                    self.busy = false;
                    // A turn that came back is proof the agent works: a crash after it is
                    // not part of a crash loop.
                    self.attempts = 0;
                    self.status = reason.clone();
                    if let Some(queued) = self.queued.take() {
                        self.input = queued;
                        self.send();
                    }
                }
                AgentEvent::Closed => {
                    self.took = self.started.take().map(|started| started.elapsed());
                    self.busy = false;
                    self.worker = None;
                    // The process is gone. A conversation is not its process, so the panel
                    // starts another one and asks it to load the same session.
                    self.schedule_reconnect();
                }
                AgentEvent::Failed(error) => {
                    self.took = self.started.take().map(|started| started.elapsed());
                    self.transcript.push(Line::Note(error.clone()));
                    self.busy = false;
                    self.status = error;
                }
            },
        }
    }
}

/// The worker: owns the session, so the panel never waits for one.
fn work(
    spec: ide_agents::AgentSpec,
    root: PathBuf,
    resume: Option<String>,
    progress: Arc<Mutex<String>>,
    commands: Receiver<Command>,
    replies: Sender<Reply>,
) {
    // `Ask` and not `Allow`: an agent that can write files without a question can undo
    // an afternoon, and the panel is where that question is asked.
    let mut session =
        match Session::start_with(&spec, &root, PermissionPolicy::Ask, resume, Some(progress)) {
            Ok(session) => session,
            Err(error) => {
                let _ = replies.send(Reply::Failed(format!("{error}")));
                return;
            }
        };
    // A load that resumes a session replays the conversation before it answers, and the
    // replay went into the session's own queue while the load's answer was being waited
    // for. It has to reach the panel before the ready line does: a panel that drew "ready"
    // first and the conversation after would show a restored session as an empty one on the
    // very frame it was looked at, and everything a person had said would seem lost.
    while let Some(event) = session.next_event(Duration::from_millis(1)) {
        if replies.send(Reply::Event(event)).is_err() {
            return;
        }
    }
    if replies
        .send(Reply::Ready {
            session: session.id().to_owned(),
            resumed: session.resumed(),
        })
        .is_err()
    {
        return;
    }
    // What it offers to choose from, before anything is asked of it.
    if replies
        .send(Reply::Event(AgentEvent::Setup(session.state().clone())))
        .is_err()
    {
        return;
    }

    loop {
        match commands.recv_timeout(PUMP) {
            Ok(Command::Prompt(text)) => {
                if let Err(error) = session.start_turn(&text)
                    && replies.send(Reply::Failed(format!("{error}"))).is_err()
                {
                    return;
                }
            }
            Ok(Command::Permission { id, option }) => {
                let _ = session.answer_permission(id, option.as_deref());
            }
            Ok(Command::Choice {
                id,
                field,
                multiple,
                values,
            }) => {
                let _ = session.answer_choice(id, &field, multiple, values.as_deref());
            }
            Ok(Command::Model(model)) => {
                if let Err(error) = session.set_model(&model)
                    && replies
                        .send(Reply::Event(AgentEvent::Failed(format!(
                            "the agent would not switch model: {error}"
                        ))))
                        .is_err()
                {
                    return;
                }
                let _ = replies.send(Reply::Event(AgentEvent::Setup(session.state().clone())));
            }
            Ok(Command::Setting { config, value }) => {
                if let Err(error) = session.set_option(&config, &value)
                    && replies
                        .send(Reply::Event(AgentEvent::Failed(format!(
                            "the agent would not change {config}: {error}"
                        ))))
                        .is_err()
                {
                    return;
                }
                let _ = replies.send(Reply::Event(AgentEvent::Setup(session.state().clone())));
            }
            Ok(Command::Mode(mode)) => {
                if let Err(error) = session.set_mode(&mode)
                    && replies
                        .send(Reply::Event(AgentEvent::Failed(format!(
                            "the agent would not switch mode: {error}"
                        ))))
                        .is_err()
                {
                    return;
                }
                let _ = replies.send(Reply::Event(AgentEvent::Setup(session.state().clone())));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // Whatever has arrived, up to the end of a turn.
        while let Some(event) = session.next_event(PUMP) {
            let finished = matches!(
                event,
                AgentEvent::Finished(_) | AgentEvent::Closed | AgentEvent::Failed(_)
            );
            if replies.send(Reply::Event(event)).is_err() {
                return;
            }
            if finished {
                break;
            }
        }
    }
}

#[cfg(test)]
mod grouping_tests {
    use super::*;

    fn setting(id: &str, name: &str, category: Option<&str>, current: &str) -> ConfigOption {
        ConfigOption {
            id: id.to_owned(),
            name: name.to_owned(),
            category: category.map(str::to_owned),
            current: Some(current.to_owned()),
            choices: Vec::new(),
        }
    }

    fn model(id: &str, name: &str) -> Model {
        Model {
            id: id.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn a_slash_or_a_colon_names_the_provider() {
        assert_eq!(provider_of("nous/hermes-4"), Some("nous"));
        assert_eq!(provider_of("openrouter:claude"), Some("openrouter"));
        assert_eq!(
            provider_of("small"),
            None,
            "an id with no separator names no provider"
        );
        assert_eq!(
            provider_of("a/b/c"),
            Some("a"),
            "only the first separator counts"
        );
        assert_eq!(provider_of("/leading"), None, "an empty provider is none");
    }

    #[test]
    fn models_are_grouped_in_the_order_their_provider_first_appears() {
        let models = vec![
            model("small", "Small"),
            model("nous/hermes-4", "Hermes 4"),
            model("large", "Large"),
            model("openai/gpt-5", "GPT-5"),
        ];
        let groups = grouped(&models);

        assert_eq!(
            groups.len(),
            3,
            "the providers were not separated: {groups:?}"
        );
        assert_eq!(groups[0].0, None, "the unnamed group is not first");
        assert_eq!(
            groups[0]
                .1
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["small", "large"],
            "a later model without a provider was not put back with the first"
        );
        assert_eq!(groups[1].0.as_deref(), Some("nous"));
        assert_eq!(groups[2].0.as_deref(), Some("openai"));
    }

    #[test]
    fn a_model_is_named_without_repeating_its_provider() {
        // Hermes names its rows "Provider · model"; the group header above them already
        // says the provider.
        assert_eq!(listed_name("Nous \u{b7} Hermes 4"), "Hermes 4");
        assert_eq!(
            listed_name("OpenRouter \u{b7} anthropic/claude"),
            "anthropic/claude",
            "only the provider label goes"
        );
        assert_eq!(
            listed_name("Hermes 4"),
            "Hermes 4",
            "a name with no provider was changed"
        );
        assert_eq!(
            listed_name("\u{b7} leading"),
            "\u{b7} leading",
            "an empty provider is not a provider"
        );
    }

    #[test]
    fn the_level_of_thinking_is_the_setting_that_carries_it() {
        let settings = vec![
            setting("edit_approval_policy", "Edits", Some("mode"), "ask"),
            setting(
                "reasoning_effort",
                "Reasoning effort",
                Some("thought_level"),
                "medium",
            ),
        ];
        let level = thought_level(&settings).expect("the level is not among the settings");
        assert_eq!(level.id, "reasoning_effort");
        assert_eq!(
            thought_level(&settings[..1]),
            None,
            "the edit policy was taken for a level"
        );
    }
}
