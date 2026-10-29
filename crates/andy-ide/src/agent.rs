//! The conversation in the panel, kept off the thread that draws the window.
//!
//! An agent is a process, and a turn can take minutes. So the session lives on a worker
//! thread of its own: the panel posts what was typed, the worker pumps what the agent
//! says, and this module is the desk between them — a transcript, an input line, and the
//! question that is waiting for an answer.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use ide_agents::{
    ChoiceRequest, Event as AgentEvent, Mode, Model, PermissionPolicy, PermissionRequest, Registry,
    Session, Usage,
};

/// How long the worker waits for the next thing before checking whether it was told to go.
const PUMP: std::time::Duration = std::time::Duration::from_millis(20);

/// How long the panel waits before starting a crashed agent again.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// How many times in a row an agent that keeps dying is started before the panel gives up.
/// A crash loop is not a conversation, and retrying it forever spends the person's time.
const RECONNECT_ATTEMPTS: u32 = 5;

/// The levels of thinking an agent may offer, in the order they are shown.
///
/// ACP has one channel for this — `session/set_mode` — and agents use it for two
/// different things: how hard the model thinks, and what it is allowed to do without
/// asking. The names below are the first kind, and an agent that offers them is offering
/// a level rather than a permission.
pub const LEVELS: [(&str, &str); 5] = [
    ("none", "None"),
    ("low", "Low"),
    ("normal", "Normal"),
    ("high", "High"),
    ("extra", "Extra"),
];

/// Whether this mode id is a level of thinking rather than a permission.
pub fn is_level(id: &str) -> bool {
    LEVELS.iter().any(|(level, _)| *level == id)
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
    /// Answer a choice among variants: the one picked, or a refusal.
    Choice {
        id: u64,
        field: String,
        value: Option<String>,
    },
    /// Switch models, and say so through the status line.
    Model(String),
    /// Switch mode: how hard the agent thinks.
    Mode(String),
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
    /// The modes it offers — how hard it thinks — and the one in use.
    pub modes: Vec<Mode>,
    pub current_mode: Option<String>,
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
            command: String::new(),
            name: String::new(),
            status: String::new(),
            models: Vec::new(),
            current_model: None,
            modes: Vec::new(),
            current_mode: None,
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
        let spec = match ide_agents::parse_named(&self.name, &self.command) {
            Ok(spec) => spec,
            Err(error) => {
                self.status = format!("{error}");
                return;
            }
        };

        let name = spec.name.clone();
        // The conversation kept by the project, but only for the agent that kept it.
        let resume = self
            .resume
            .clone()
            .filter(|_| self.resumed_from.as_deref() == Some(name.as_str()));
        let (commands, jobs) = channel();
        let (answers, replies) = channel();
        let started = std::thread::Builder::new()
            .name(format!("agent-{name}"))
            .spawn(move || work(spec, root, resume, jobs, answers));
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

    /// The level of thinking in use, when the agent's current mode is one.
    ///
    /// ACP has a single mode channel and agents spend it on either a level or a
    /// permission, so the panel asks the current mode what it is rather than keeping a
    /// second idea of the state that could disagree with the agent's.
    pub fn level(&self) -> Option<String> {
        self.current_mode.clone().filter(|mode| is_level(mode))
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

    /// Asks the agent to switch mode: how hard it thinks before it answers.
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
    pub fn answer_choice(&mut self, value: Option<&str>) {
        let Some((id, request)) = self.choice.take() else {
            return;
        };
        let Some(worker) = &self.worker else {
            return;
        };
        let _ = worker.commands.send(Command::Choice {
            id,
            field: request.field,
            value: value.map(str::to_owned),
        });
        self.status = match value {
            Some(value) => format!("chose {value}"),
            None => "declined the choice".to_owned(),
        };
    }

    /// Empties what the panel shows, keeping the conversation's id for the restart.
    ///
    /// A restored session replays its history through the agent's own events. Keeping
    /// the lines already on screen would draw every message twice, and the widget ids
    /// behind them would collide. The id stays: that is what makes it the same
    /// conversation rather than a new one.
    pub fn clear_panel(&mut self) {
        self.transcript.clear();
        self.models.clear();
        self.current_model = None;
        self.modes.clear();
        self.current_mode = None;
        self.usage = None;
        self.asking = None;
        self.choice = None;
        self.busy = false;
        self.started = None;
        self.took = None;
        self.queued = None;
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
        self.busy = false;
        self.asking = None;
        self.choice = None;
        // Stopping is a decision: a process ended on purpose is not one to start again.
        self.retry_at = None;
        self.attempts = 0;
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
            }
            Reply::Event(event) => match event {
                AgentEvent::Thinking(thought) => {
                    // One growing line per stretch of reasoning, like the answer below it.
                    if let Some(Line::Thinking(said)) = self.transcript.last_mut() {
                        said.push_str(&thought);
                    } else {
                        self.transcript.push(Line::Thinking(thought));
                    }
                }
                AgentEvent::Text(text) => {
                    // Pieces of one answer join the last line the agent wrote, which is
                    // what makes a stream read as a sentence and not as a column.
                    if let Some(Line::Agent(said)) = self.transcript.last_mut() {
                        said.push_str(&text);
                    } else {
                        self.transcript.push(Line::Agent(text));
                    }
                }
                AgentEvent::ToolCall { title, status, .. } => {
                    self.transcript
                        .push(Line::Tool(format!("{title} ({status})")));
                }
                AgentEvent::Plan(steps) => {
                    for step in steps {
                        self.transcript.push(Line::Tool(format!("· {step}")));
                    }
                }
                AgentEvent::Other { kind } => {
                    tracing::debug!(target: "andy_ide::agent", %kind, "an update this panel does not show");
                }
                AgentEvent::Permission { id, request } => {
                    self.status = request.title();
                    self.asking = Some((id, request));
                }
                AgentEvent::Choice { id, request } => {
                    self.status = request.title().to_owned();
                    self.choice = Some((id, request));
                }
                AgentEvent::Setup(state) => {
                    self.models = state.models;
                    self.current_model = state.current_model;
                    self.modes = state.modes;
                    self.current_mode = state.current_mode;
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
    commands: Receiver<Command>,
    replies: Sender<Reply>,
) {
    // `Ask` and not `Allow`: an agent that can write files without a question can undo
    // an afternoon, and the panel is where that question is asked.
    let mut session = match Session::start_with(&spec, &root, PermissionPolicy::Ask, resume) {
        Ok(session) => session,
        Err(error) => {
            let _ = replies.send(Reply::Failed(format!("{error}")));
            return;
        }
    };
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
            Ok(Command::Choice { id, field, value }) => {
                let _ = session.answer_choice(id, &field, value.as_deref());
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
    fn only_the_five_agreed_names_are_levels_of_thinking() {
        for (id, _) in LEVELS {
            assert!(is_level(id), "{id} was not recognised as a level");
        }
        assert!(
            !is_level("read-only"),
            "a permission was taken for a level of thinking"
        );
        assert!(!is_level("Full access"));
    }
}
