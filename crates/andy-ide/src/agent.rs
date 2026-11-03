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
    PermissionRequest, REASONING, Registry, Session, SessionInfo, THOUGHT_LEVEL, Usage, first_line,
};

/// How long the worker waits for the next thing before checking whether it was told to go.
const PUMP: std::time::Duration = std::time::Duration::from_millis(20);

/// How long a quiet agent is still taken to be working.
///
/// A session restored while a turn was running comes back writing without a turn of the
/// client's asking, so the words arriving are the whole account of that work — and nothing
/// will ever say it ended. A moment of silence is the end of it: long enough to cover the
/// gap between two chunks, short enough that the button is not a lie a second afterwards.
const SILENCE: Duration = Duration::from_millis(1200);

/// How long a turn in flight may be quiet before the panel says how long it has been quiet.
///
/// A tool call, a slow model and a turn that compacts its own context all take their time without a
/// word, and a line that says nothing but `answering…` for minutes is what a hang looks like. A
/// minute is the first silence a person would not call ordinary, and it is where the panel starts
/// saying the one thing it knows: that nothing has come for that long, and for how long.
const QUIET: Duration = Duration::from_secs(60);

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

/// The words of a line, whichever kind of line it is.
impl Line {}

/// One line of the transcript.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Line {
    /// What the person said.
    You(String),
    /// A row the runtime wrote itself and replayed as a `role="user"` row: a receipt of a
    /// background process, a note from its gateway, a summary of a context it compacted. Told
    /// apart by the shape of its opening words, and a line of its own, so that it is never read
    /// as the words of the person.
    Notice {
        /// Which shape of service row it is.
        kind: String,
        /// The short line kept for a surface to paint.
        text: String,
    },
    /// What the agent said, or is saying.
    Agent(String),
    /// What the agent was working out on the way: folded away in the panel.
    Thinking(String),
    /// A tool the agent ran, as it described it.
    Tool(String),
    /// Something the panel says on its own account.
    Note(String),
}

/// The shapes of a service row of the runtime, the name each kind goes by, and the one line to
/// draw for it.
///
/// The runtime hands its own rows — a receipt of a background process, a note from its gateway, a
/// summary of a context it compacted — to the model as `role="user"` rows, so they replay as user
/// chunks with nothing on them that says they are not the person. Hermes itself tells such rows
/// apart by their opening words (`agent/conversation_compression.py::_SYNTHETIC_USER_PREFIXES`,
/// `hermes_state_timeline.py::_SYNTHETIC_PROMPT`); this is the same rule. A row that is not one
/// of these is the words of the person — among them `[OUT-OF-BAND USER MESSAGE`, which is how the
/// person themselves arrives mid-turn.
/// One shape of a service row: the words it opens with, the name its kind goes by, and how the
/// single line to draw for it is made.
type ServiceRow = (&'static str, &'static str, fn(&str) -> String);

const SERVICE_ROWS: [ServiceRow; 3] = [
    (
        "[IMPORTANT: Background process",
        "background process",
        first_line,
    ),
    (
        "[INTERNAL NOTIFICATION — generated by the Hermes gateway",
        "gateway note",
        first_line,
    ),
    ("[CONTEXT COMPACTION", "context compaction", summary_line),
];

/// The one line to draw for a summary of a compacted context.
///
/// The summary is a wall meant for the model — every turn that was folded into it, verbatim —
/// and what a person needs of it is that it happened and how much it took, not the wall.
fn summary_line(text: &str) -> String {
    format!(
        "context compacted: a summary of the earlier turns is kept in its place ({} lines)",
        text.lines().count()
    )
}

/// The kind of service row the words make, and the line to draw for it.
///
/// Only the opening of a row counts: a message that quotes one of these lines further down is
/// still the words of the person.
fn service_line(text: &str) -> Option<(&'static str, String)> {
    let opening = text.trim_start();
    let (_, kind, line) = SERVICE_ROWS
        .iter()
        .find(|(shape, _, _)| opening.starts_with(shape))?;
    Some((kind, line(text)))
}

/// A length of time, as a person counts it: `1:12`, or `1:02:03` past the hour.
fn clock(took: Duration) -> String {
    let seconds = took.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
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
    /// Stop the answer being written. The session stays: this ends the turn, not the agent.
    Cancel,
    /// List the conversations this agent keeps for this project: the panel shows them, and a
    /// choice among them reopens the session.
    Sessions,
    /// Open another conversation on the agent that is already up. The agent is not restarted: the
    /// conversations it holds in memory would go with the process, and one that has not been written
    /// down yet would vanish from its own list.
    SwitchTo(String),
    /// Begin a fresh conversation on the agent that is already up, for the same reason.
    Fresh,
}

/// What the worker tells the panel.
enum Reply {
    /// The session is up: its id, and whether it was picked up from an earlier run.
    Ready { session: String, resumed: bool },
    /// Something the agent said.
    Event(AgentEvent),
    /// The session could not be started, or the worker gave up.
    Failed(String),
    /// What the desk said about one notification: the id it took it under, or why it refused.
    Told {
        title: String,
        told: Result<u32, String>,
    },
}

/// The worker thread, seen from the panel's side.
/// The window, once it has drawn a frame, so that the worker can ask it for another.
///
/// A panel that is not on screen is not drawn: egui draws on demand, so an event that arrives
/// while the window is hidden sits unread in the channel until something makes a frame — which is
/// exactly what a person switching back to the window does. A notification arriving at that moment
/// is a notification nobody needed, so the worker wakes the window itself, the moment it has
/// something to say.
#[derive(Clone, Default)]
pub struct Waker(Arc<Mutex<Option<egui::Context>>>);

impl Waker {
    /// Points the waker at the window that has just drawn a frame.
    pub fn watch(&self, ctx: &egui::Context) {
        if let Ok(mut window) = self.0.lock()
            && window.is_none()
        {
            *window = Some(ctx.clone());
        }
    }

    /// Asks the window for a frame, if it has one to ask.
    pub fn wake(&self) {
        if let Ok(window) = self.0.lock()
            && let Some(ctx) = window.as_ref()
        {
            ctx.request_repaint();
        }
    }

    /// Whether the waker knows the window at all.
    #[cfg(test)]
    pub(crate) fn knows_the_window(&self) -> bool {
        self.0.lock().is_ok_and(|window| window.is_some())
    }
}

/// The worker's end of the replies: every reply wakes the window that reads them.
struct Replies {
    outlet: Sender<Reply>,
    waker: Waker,
}

impl Replies {
    /// Hands one reply to the panel and wakes it, answering whether it arrived.
    ///
    /// A plain yes-or-no and not the error the channel gives: that error carries the reply back, which is
    /// over a hundred bytes of conversation to say the panel is gone — and the panel being gone is the
    /// whole of what the worker does with it (it stops).
    fn send(&self, reply: Reply) -> bool {
        let sent = self.outlet.send(reply).is_ok();
        self.waker.wake();
        sent
    }
}

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
    /// When the clipboard was asked for the words egui has selected, while the answer is still on its way.
    ///
    /// The answer is a *later* frame's event: a copy asked for when the button comes up is copied on the
    /// next frame, and the window answers the clipboard request only after that. Waiting exactly one frame
    /// was what kept the notice — and the letting go of the highlight — from ever happening.
    pub wanting_copy_text: Option<std::time::Instant>,
    /// What was copied for the person, and when: the words are put in the clipboard the moment the left
    /// button comes up, and the panel says so for a few seconds — a copy nobody is told about is a copy
    /// nobody trusts.
    pub copied: Option<(usize, std::time::Instant)>,
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
    /// What the foot under the conversation actually took last frame: the field, the facts and the
    /// button, measured rather than counted. A reserve built from guessed parts leaves the difference
    /// on screen — a gap the answer never asked for.
    pub foot: f32,
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
    /// What the provider says is left on the account, as the status line shows it: the figure alone.
    pub credits: Option<String>,
    /// The provider's own lines, for the tooltip: what the agent sent, in its own words.
    pub credits_lines: Vec<String>,
    /// Whether the agent warns that its lines are out of date.
    pub credits_stale: bool,
    /// The conversations the agent keeps for this project, as it last listed them.
    pub sessions: Vec<SessionInfo>,
    /// The names a person gave the conversations, by session id: what the list shows instead of
    /// the name the agent chose.
    pub session_names: std::collections::BTreeMap<String, String>,
    /// The conversation being renamed, and what has been typed for it so far.
    pub session_rename: Option<(String, String)>,
    /// Whether the list of conversations is on screen.
    pub sessions_open: bool,
    /// Where the button that asks for the list was drawn: the list floats above it.
    pub sessions_button: Option<egui::Rect>,
    /// Whether the agent has been asked for the list and has not answered yet.
    pub sessions_loading: bool,
    /// The last figure the agent sent for each conversation, by session id, kept with the
    /// workspace.
    ///
    /// It is not `usage`: that one is what the agent says *now*, and a connection opened through
    /// `Agent::start` clears it — as does moving to another conversation. This is what the panel
    /// falls back to, so a conversation shows where its context stood the last time it was open.
    /// One figure per conversation: moving between them is exactly when the number is wanted, and
    /// a single figure would belong to the wrong one.
    pub contexts: std::collections::BTreeMap<String, Usage>,
    /// Where the panel was last drawn, for the tests that point at it.
    pub shown_rect: Option<egui::Rect>,
    /// Whether the panel was drawn at all in the last frame.
    ///
    /// `shown_rect` alone is not that answer: it is written as the panel draws and **never cleared**, so a
    /// panel standing as a tab *behind* the editor keeps the rectangle of the place it used to occupy —
    /// which is where the editor is now. The window asks this flag before it hands the panel the copy
    /// chord for that very reason: a stale rectangle would take `Ctrl+C` and `Ctrl+Insert` away from the
    /// file being edited, and the editor would copy nothing at all.
    pub shown: bool,
    /// Set when the person asked to copy from the panel.
    /// When the turn in flight began, and how long the last one took.
    started: Option<Instant>,
    pub took: Option<Duration>,
    /// Set while a turn is in flight: the panel disables the input rather than queueing.
    pub busy: bool,
    /// Whether the conversation in hand is up: the load's (or the new session's) answer has
    /// arrived. Words that arrive before it are a replay, not work.
    ready: bool,
    /// When the agent last said something, whatever started it.
    ///
    /// A turn the panel asked for is not the only work there is: a session restored while
    /// something was still running goes on writing, and a panel that watched only its own
    /// turns would offer to talk over the answer — and its timer would sit at zero, or run
    /// for ever, because the end of a turn nobody started is never announced.
    last_word: Option<Instant>,
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
    /// How many times an agent has been brought up. A panel that moves between conversations asks
    /// the agent it has; the count is how a test tells that apart from a restart, and nothing in the
    /// panel needs it.
    #[cfg(test)]
    connects: u32,
    /// The window, once it has drawn a frame: the worker wakes it when it has something to say.
    pub(crate) waker: Waker,
    /// The desk this window would speak to, handed to the worker when the agent is started: the
    /// worker is what tells the person now, wherever the window happens to be.
    pub(crate) notifier: Arc<dyn crate::notify::Surface>,
    /// Where the person is, as far as the window can tell: filled in by every frame it draws, and read
    /// by the worker, which is the thread that decides whether a notice is worth sending.
    pub(crate) looking: Arc<Mutex<crate::notify::Looking>>,
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
    /// The conversation each agent of the project kept, by the name its entry in `agents.toml`
    /// has: one per agent, and not one for the project. A single record was overwritten by
    /// whichever entry ran last, so a second agent left the first with nothing to come back to.
    pub kept: std::collections::BTreeMap<String, String>,
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
            wanting_copy_text: None,
            copied: None,
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
            foot: 0.0,
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
            credits: None,
            credits_lines: Vec::new(),
            credits_stale: false,
            contexts: std::collections::BTreeMap::new(),
            sessions: Vec::new(),
            session_names: std::collections::BTreeMap::new(),
            session_rename: None,
            sessions_open: false,
            sessions_button: None,
            sessions_loading: false,
            shown_rect: None,
            shown: false,
            started: None,
            took: None,
            busy: false,
            last_word: None,
            ready: false,
            asking: None,
            choice: None,
            // An agent is a long conversation; a process that dies should not end it.
            auto_reconnect: true,
            reconnect_delay: RECONNECT_DELAY,
            attempts: 0,
            retry_at: None,
            reconnecting: false,
            connected_at: None,
            #[cfg(test)]
            connects: 0,
            notifier: Arc::new(crate::notify::System),
            looking: Arc::new(Mutex::new(crate::notify::Looking::default())),
            waker: Waker::default(),
            worker: None,
            root: None,
            place: None,
            session: None,
            resume: None,
            resumed_from: None,
            kept: std::collections::BTreeMap::new(),
        }
    }
}

/// What a person saw next to a value they chose: the option's own title, because a machine value
/// (`choice-2`) says nothing to a reader coming back to the conversation. A value with no title of its own
/// is shown as it is — better than showing nothing.
fn words_of<'a>(mut options: impl Iterator<Item = (&'a str, &'a str)>, value: &str) -> String {
    options
        .find(|(candidate, _)| *candidate == value)
        .map(|(_, title)| title.to_owned())
        .unwrap_or_else(|| value.to_owned())
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

    /// Reads the ids the project kept, if it kept any.
    ///
    /// The file holds one conversation per agent — `{"sessions": {name: id}}` — and the shape of
    /// the first version, `{"agent": name, "session": id}`, is read as the one entry it is: a
    /// project written by an older build keeps its conversation instead of starting over.
    fn read_session(&mut self, root: &Path) {
        let Ok(text) = std::fs::read_to_string(Self::session_file(root)) else {
            return;
        };
        let Ok(saved) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        if let Some(rows) = saved.get("sessions").and_then(|rows| rows.as_object()) {
            for (agent, session) in rows {
                if let Some(session) = session.as_str() {
                    self.kept.insert(agent.clone(), session.to_owned());
                }
            }
        } else {
            let session = saved.get("session").and_then(|id| id.as_str());
            let agent = saved.get("agent").and_then(|name| name.as_str());
            if let (Some(session), Some(agent)) = (session, agent) {
                self.kept.insert(agent.to_owned(), session.to_owned());
            }
        }
        // The agent the panel opens with, if the project kept a conversation for it.
        if let Some(session) = self.kept.get(&self.name).cloned() {
            self.resume = Some(session);
            self.resumed_from = Some(self.name.clone());
        }
    }

    /// Writes the id down, under the agent it belongs to.
    ///
    /// Every agent keeps its own: the record is a map by name, so opening a second agent adds a
    /// conversation instead of taking the first one away.
    fn remember_session(&mut self, session: &str) {
        let (Some(root), Some(agent)) = (self.root.clone(), self.chosen.clone()) else {
            return;
        };
        let file = Self::session_file(&root);
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        self.kept.insert(agent.clone(), session.to_owned());
        let saved = serde_json::json!({ "sessions": self.kept });
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
        // Whatever the last conversation was doing, this one has not started yet.
        self.ready = false;
        self.last_word = None;
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
        // The conversation kept by the project for this agent: the one a move to another
        // conversation asked for, or else the one this entry kept the last time it ran.
        let resume = self
            .resume
            .clone()
            .filter(|_| self.resumed_from.as_deref() == Some(name.as_str()))
            .or_else(|| self.kept.get(name.as_str()).cloned());
        // A start is another conversation — a new one, or one restored from its id — so nothing
        // of the one on screen outlives it. The words of the last agent left there showed a
        // history the agent in front had never seen, and the next message went to a stranger
        // while the panel looked as though it remembered; a restored conversation drew every
        // replayed line a second time over the one still on screen. The figure goes with them:
        // it belongs to a conversation that is no longer the one in front.
        self.clear_panel();
        self.session = None;
        self.connecting = true;
        self.progress = Arc::new(Mutex::new("Launching\u{2026}".to_owned()));
        let progress = Arc::clone(&self.progress);
        let (commands, jobs) = channel();
        let (answers, replies) = channel();
        let waker = self.waker.clone();
        let telling = Telling {
            desk: Arc::clone(&self.notifier),
            looking: Arc::clone(&self.looking),
        };
        let started = std::thread::Builder::new()
            .name(format!("agent-{name}"))
            .spawn(move || {
                work(
                    spec,
                    root,
                    resume,
                    progress,
                    jobs,
                    Replies {
                        outlet: answers,
                        waker,
                    },
                    telling,
                )
            });
        match started {
            Ok(_) => {
                self.worker = Some(Worker { commands, replies });
                // An agent brought up, not a conversation opened: the count is how a test tells a
                // move between conversations from a restart, and a move sends an answer of its own.
                #[cfg(test)]
                {
                    self.connects += 1;
                }
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
    /// Hands the copy chord to the panel, unless the terminal is the one with the keyboard.
    ///
    /// `terminal_has_the_keys` is the terminal's own "these keys are mine" flag: while it is set, a
    /// `Ctrl+Insert` — or a `Ctrl+C`, which there means *stop* — belongs to the shell, and a window
    /// that took it first would leave the terminal's own copy key doing nothing. Andy's report,
    /// verbatim: "Ctrl-Insert does not copy even right after the selection".
    pub fn take_copy_shortcut(&mut self, ctx: &egui::Context, terminal_has_the_keys: bool) {
        if terminal_has_the_keys {
            return;
        }
        if !self.shown {
            return;
        }
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

    /// Asks the agent to stop the answer it is writing.
    ///
    /// The agent itself stays: this is the door that ends the turn. [`AgentState::stop`] is
    /// the other one — it ends the session and the process with it.
    pub fn cancel(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        if worker.commands.send(Command::Cancel).is_err() {
            self.status = "the agent is gone".to_owned();
            return;
        }
        self.status = "stopping…".to_owned();
    }

    /// The agent said something: a turn is running, whether or not this panel started it.
    ///
    /// The clock is started here for work the panel only hears about — a restored session
    /// that came back mid-turn — because no `send` started it and no ending will stop it.
    fn heard_the_agent(&mut self) {
        // A replay is not work. The history of a restored conversation arrives *before* the
        // load is answered, when there is no conversation to be working in yet; the words
        // that arrive after it is up are the agent working — a turn nobody here asked for.
        if !self.ready {
            return;
        }
        let now = Instant::now();
        self.last_word = Some(now);
        if self.started.is_none() {
            self.started = Some(now);
        }
    }

    /// Whether the agent is working right now, by its own account as much as by ours.
    ///
    /// This is what the foot's button follows: an answer being written can be stopped
    /// whichever side began it.
    /// Asks the agent for the conversations it keeps for this project, and opens the list.
    ///
    /// The list is asked for every time it is opened: a conversation that was started in another
    /// window since should be there, and the agent answers from what it already keeps.
    pub fn open_sessions(&mut self) {
        self.sessions_open = true;
        self.sessions_loading = true;
        let Some(worker) = &self.worker else {
            self.sessions_loading = false;
            return;
        };
        if worker.commands.send(Command::Sessions).is_err() {
            self.sessions_loading = false;
        }
    }

    /// Gives a conversation a name of its own, or takes one away when the name is empty.
    ///
    /// The name lives with the project, not with the agent: ACP has no way to rename a session, and
    /// the name the agent gives is the first thing that was said in it.
    pub fn rename_session(&mut self, session: &str, name: &str) {
        let name = name.trim().to_owned();
        if name.is_empty() {
            self.session_names.remove(session);
        } else {
            self.session_names.insert(session.to_owned(), name);
        }
        self.session_rename = None;
    }

    /// What a conversation is called: the name a person gave it, or the one the agent chose.
    pub fn session_name(&self, session: &str, given: Option<&str>) -> String {
        self.session_names
            .get(session)
            .cloned()
            .or_else(|| given.map(str::to_owned))
            .unwrap_or_else(|| session.to_owned())
    }

    /// Moves the panel to another conversation of this project.
    ///
    /// What is on screen is cleared first: the transcript is about to be replaced by another one,
    /// and half of each would read as a single conversation. The agent is then opened again the
    /// way a restored window opens the session it remembered, and the history of the chosen
    /// conversation arrives through the same replay — the panel fills as it did the first time.
    pub fn switch_session(&mut self, session: &str) {
        if self.session.as_deref() == Some(session) {
            self.sessions_open = false;
            return;
        }
        // The conversation on screen is cleared first: the transcript is about to be replaced by
        // another one, and half of each would read as a single conversation.
        self.clear_panel();
        self.usage = None;
        self.sessions_open = false;
        self.session_rename = None;
        self.ready = false;
        // The agent is asked to open the conversation, not restarted. Restarting it threw away the
        // conversations it was holding in memory — including the one just left, if the agent had not
        // written it down yet — and they vanished from its own list.
        let taken = match &self.worker {
            Some(worker) => worker
                .commands
                .send(Command::SwitchTo(session.to_owned()))
                .is_ok(),
            None => false,
        };
        if !taken {
            // No agent to ask (or one that will not listen): open it the way a start does, from the
            // id written where a restart looks for it.
            self.forget_session();
            self.resume = Some(session.to_owned());
            self.resumed_from = Some(self.name.clone());
            self.start();
        }
    }

    /// What the panel shows about the context: what the agent says now, or — until it says
    /// anything about this conversation — what it said the last time this one was open.
    pub fn context_usage(&self) -> Option<Usage> {
        self.usage.clone().or_else(|| {
            let session = self.session.as_ref()?;
            self.contexts.get(session).cloned()
        })
    }

    pub fn working(&self) -> bool {
        self.busy || self.last_word.is_some_and(|when| when.elapsed() < SILENCE)
    }

    /// How long the turn in flight has been quiet: since the agent last said anything, or since the
    /// turn was sent when it has said nothing at all.
    ///
    /// Nothing else, while a turn this panel asked for is running: the watchdog leaves `last_word`
    /// alone until the turn is over, so this is the silence of the agent and not of the panel.
    pub fn quiet_for(&self) -> Option<Duration> {
        if !self.busy {
            return None;
        }
        Some(self.last_word.or(self.started)?.elapsed())
    }

    /// What the panel says about the work in hand.
    ///
    /// The plain word while the agent answers as usual, and — once the silence has lasted longer
    /// than a person would call ordinary — the length of it, so that a window sitting at
    /// `answering…` and a window that has stalled can be told apart at a glance. The reason for a
    /// silence is not ours to know: a compaction, a tool and a slow model look the same from here,
    /// and the panel says only what it can see.
    pub fn saying(&self) -> String {
        match self.quiet_for() {
            Some(quiet) if quiet >= QUIET => {
                format!(
                    "{} — no word from the agent for {}",
                    self.status,
                    clock(quiet)
                )
            }
            _ => self.status.clone(),
        }
    }

    /// Puts a turn in flight that began a while ago, for the checks that ask what the panel says
    /// about a silence nobody wants to sit through.
    #[cfg(test)]
    pub(crate) fn turn_in_flight_for(&mut self, quiet: Duration) {
        self.busy = true;
        self.last_word = None;
        self.started = Some(Instant::now() - quiet);
    }

    /// Ends the work only heard about, once the words have stopped coming.
    ///
    /// Nothing announces the end of a turn this panel never started, and a timer left
    /// running would run for ever. The silence is the end, and the figure is kept the way
    /// a finished turn's is.
    fn quiet_down(&mut self) {
        if self.busy || self.started.is_none() {
            return;
        }
        let Some(last) = self.last_word else {
            return;
        };
        if last.elapsed() < SILENCE {
            return;
        }
        self.last_word = None;
        self.took = self.started.take().map(|started| started.elapsed());
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
        let title = request.title().to_owned();
        self.picked.clear();
        self.card_text.clear();
        self.card_wants_focus = false;
        // The card leaves no trace of itself once it is answered, and a decision is exactly what a reader
        // comes back to a conversation for: keep the question and the answer in it. Written in the panel's
        // own tone for its own rows (`model: …`, `mode: …`), so it never reads as the agent's words.
        let chosen = match &values {
            Some(values) if !values.is_empty() => values
                .iter()
                .map(|value| {
                    words_of(
                        request
                            .options
                            .iter()
                            .map(|offer| (offer.value.as_str(), offer.title.as_str())),
                        value,
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
            _ => "nothing".to_owned(),
        };
        self.transcript
            .push(Line::Note(format!("asked «{title}»; chose {chosen}")));
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
        // The figures of the other conversations stay: a session of its own is a figure of its
        // own, and coming back to the others should show what they knew.
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

    /// Whether an agent has ever been opened in this panel: a `Ready` has been heard at
    /// least once, which is what makes the project remember who was on duty.
    pub fn has_connected(&self) -> bool {
        self.connected_at.is_some()
    }

    /// How many agents have been brought up in this window: unchanged across a move from one
    /// conversation to another, one greater after a restart.
    #[cfg(test)]
    pub fn connects(&self) -> u32 {
        self.connects
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
        let Some((id, request)) = self.asking.take() else {
            return;
        };
        let Some(worker) = &self.worker else {
            return;
        };
        // The card leaves no trace of itself once it is answered, so the question and the answer are kept
        // here — in the panel's own tone for its own rows. Andy's report: after answering, the conversation
        // showed neither what was asked nor what was chosen. This is the path the `clarify` card takes;
        // `answer_choice` is the other one, for a question of several answers.
        let title = request.title().to_owned();
        let chosen = match &option {
            Some(value) => words_of(
                request
                    .options
                    .iter()
                    .map(|offer| (offer.option_id.as_str(), offer.name.as_str())),
                value,
            ),
            None => "nothing".to_owned(),
        };
        self.transcript
            .push(Line::Note(format!("asked «{title}»; chose {chosen}")));
        let _ = worker.commands.send(Command::Permission { id, option });
        self.status = "answered".to_owned();
    }

    /// Takes everything the worker has said since the last frame.
    pub fn pump(&mut self) {
        // Work the panel only heard about is ended by its silence: no other frame is going
        // to say that a turn nobody started has finished.
        self.quiet_down();
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

    /// Takes what an agent said about its account: the figure for the status line, and the lines the
    /// tooltip shows. An agent that said nothing of an account leaves the last figure alone — that is
    /// not an account with no money, it is one nobody could read.
    pub fn took_account(&mut self, account: Option<ide_agents::AccountUsage>) {
        let Some(account) = account else {
            return;
        };
        self.credits_stale = account.stale;
        self.credits = crate::credits::balance(&account.lines).map(|(_, amount)| amount);
        self.credits_lines = account.lines;
    }

    /// Whether an agent is running.
    pub fn running(&self) -> bool {
        self.worker.is_some()
    }

    /// Ends the conversation.
    pub fn stop(&mut self) {
        self.last_word = None;
        self.ready = false;
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

    /// Forgets the conversation the project kept for the agent in front, so the next start is a
    /// new one. The conversations of the other entries stay: each agent has its own.
    ///
    /// The id is written into the project (`.andy-ide/agent-session.json`) and outlives
    /// the window. Forgetting it only in memory would bring the same conversation back
    /// on the next launch — which is not what a person asking for a new session wants.
    pub fn forget_session(&mut self) {
        if let Some(root) = self.root.clone() {
            // Only the agent in front forgets: the conversations of the other entries are theirs,
            // and a person asking for a new session has not asked for theirs to go.
            self.kept.remove(&self.name);
            if self.kept.is_empty() {
                let _ = std::fs::remove_file(Self::session_file(&root));
            } else {
                let saved = serde_json::json!({ "sessions": self.kept });
                let _ = std::fs::write(Self::session_file(&root), format!("{saved:#}\n"));
            }
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
        // Asked of the agent that is up, not by putting it down and taking it up again: the
        // conversations it holds go with the process, and a fresh one is no reason to lose them.
        self.clear_panel();
        self.usage = None;
        self.sessions_open = false;
        self.session_rename = None;
        self.ready = false;
        let taken = match &self.worker {
            Some(worker) => worker.commands.send(Command::Fresh).is_ok(),
            None => false,
        };
        if !taken {
            self.forget_session();
            self.resume = None;
            self.resumed_from = None;
            self.start();
        }
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
                // The replayed history is not a turn: the clock of the work starts with the
                // words that come after this, and there are none yet.
                self.ready = true;
                self.last_word = None;
                self.started = None;
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
            Reply::Told { title, told } => {
                // What the desk did is said in the window as well as in the log: the person was
                // looking elsewhere when it mattered, so when they come back the only place that can
                // tell them the notification was never shown — and why — is here.
                self.status = match told {
                    Ok(id) => format!("told the desk: {title} (id {id})"),
                    Err(reason) => format!("the desk refused: {reason}"),
                };
            }
            Reply::Event(event) => match event {
                AgentEvent::Thinking(thought) => {
                    self.heard_the_agent();
                    // One growing line per stretch of reasoning, like the answer below it.
                    if let Some(Line::Thinking(said)) = self.transcript.last_mut() {
                        said.push_str(&thought);
                    } else {
                        self.transcript.push(Line::Thinking(thought));
                    }
                    self.agent_line_open = false;
                }
                AgentEvent::Text(text) => {
                    self.heard_the_agent();
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
                    // An agent that does not mark its own service rows leaves them to be
                    // told apart by the shape of their words. Every `You` line heard here is a
                    // replay — what the person types is pushed by the panel itself — so a
                    // service row of a conversation opened later is told apart just the same.
                    match service_line(&text) {
                        Some((kind, line)) => self.transcript.push(Line::Notice {
                            kind: kind.to_owned(),
                            text: line,
                        }),
                        None => self.transcript.push(Line::You(text)),
                    }
                    self.agent_line_open = false;
                }
                AgentEvent::ToolCall { title, status, .. } => {
                    // An update that carries only a new status has no title of its own, and a row for it
                    // said nothing but "(completed)" — the empty lines Andy found under the question. The
                    // question is a row of its own already, and the answer is kept when the card is
                    // answered, so nothing is lost by leaving these out.
                    if !title.trim().is_empty() {
                        self.transcript
                            .push(Line::Tool(format!("{title} ({status})")));
                    }
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
                    // Every turn brings one: the account rides with the same update that carries the
                    // context figure, so the balance follows the conversation at no cost of ours.
                    self.took_account(usage.account.clone());
                    // Written down for the conversation it belongs to: moving away and back is
                    // when a person wants to see the figure again.
                    if let Some(session) = self.session.clone() {
                        self.contexts.insert(session, usage.clone());
                    }
                    self.usage = Some(usage);
                }
                AgentEvent::Sessions(sessions) => {
                    self.sessions = sessions;
                    self.sessions_loading = false;
                }
                AgentEvent::Finished(reason) => {
                    self.last_word = None;
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

/// What an event asks a person for, if anything: one answer for both sides of the channel.
///
/// One place and not two: what the panel shows and what the desk is told are the same question, and
/// an answer written out twice drifts.
/// Who the worker tells, and where the person it tells is.
///
/// One thing and not two arguments: both are handed to the worker when the agent is started, both live
/// in the panel's state, and together they are the whole question of whether a notice is worth sending.
#[derive(Clone)]
struct Telling {
    /// The desk the window itself would speak to.
    desk: Arc<dyn crate::notify::Surface>,
    /// Where the person is, written by the window on every frame it draws.
    looking: Arc<Mutex<crate::notify::Looking>>,
}

impl Telling {
    /// Where the person is now, as far as the window last said.
    fn where_is_the_person(&self) -> crate::notify::Looking {
        self.looking.lock().map(|held| *held).unwrap_or_default()
    }
}

/// Tells the desk about one event, unless the person is looking at the panel already.
///
/// One function, called by the worker where the agent speaks: the decision needs the person's
/// whereabouts, which the window writes on every frame (`Looking`), and the reply goes back to the panel
/// as `Reply::Told`. Returns what the desk said, or `None` when nothing was sent — the caller passes that
/// on, so a held-back notice is visible in the log rather than being silence.
fn tell_the_desk(
    desk: &dyn crate::notify::Surface,
    looking: crate::notify::Looking,
    want: crate::notify::Want,
    detail: &str,
) -> Option<Result<u32, String>> {
    if !crate::notify::should_tell(looking) {
        // Said out loud, and said with the circumstances: without this line a notice nobody received
        // cannot be told from an agent that never asked.
        tracing::info!(
            ?want,
            %detail,
            focused = looking.focused,
            panel_in_front = looking.panel_in_front,
            "the panel wants the person, and they are looking at it"
        );
        return None;
    }
    let (title, body) = want.message(detail);
    let told = desk.show(&title, &body);
    match &told {
        Ok(id) => tracing::info!(
            ?want,
            %detail,
            focused = looking.focused,
            panel_in_front = looking.panel_in_front,
            id,
            "the desk took the notification, told by the agent's own thread"
        ),
        Err(reason) => tracing::warn!(
            ?want,
            %detail,
            focused = looking.focused,
            panel_in_front = looking.panel_in_front,
            %reason,
            "the desk refused the notification"
        ),
    }
    Some(told)
}

fn asking_for(event: &AgentEvent) -> Option<(crate::notify::Want, String)> {
    match event {
        AgentEvent::Permission { request, .. } => {
            Some((crate::notify::Want::Asks, request.title().to_string()))
        }
        AgentEvent::Choice { request, .. } => {
            Some((crate::notify::Want::Asks, request.title().to_string()))
        }
        AgentEvent::Finished(reason) => Some((crate::notify::Want::Finished, reason.clone())),
        _ => None,
    }
}

/// The worker: owns the session, so the panel never waits for one.
fn work(
    spec: ide_agents::AgentSpec,
    root: PathBuf,
    resume: Option<String>,
    progress: Arc<Mutex<String>>,
    commands: Receiver<Command>,
    replies: Replies,
    telling: Telling,
) {
    // `Ask` and not `Allow`: an agent that can write files without a question can undo
    // an afternoon, and the panel is where that question is asked.
    // Every line of this agent is written down as it crosses, by the threads that move it: the
    // protocol is what a session is read back from when the panel has shown something else.
    let wire = crate::log::wire_start(&spec, &root);
    let mut session = match Session::start_with(
        &spec,
        &root,
        PermissionPolicy::Ask,
        resume,
        Some(progress),
        Some(wire),
    ) {
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
        if !replies.send(Reply::Event(event)) {
            return;
        }
    }
    if !replies.send(Reply::Ready {
        session: session.id().to_owned(),
        resumed: session.resumed(),
    }) {
        return;
    }
    // What it offers to choose from, before anything is asked of it.
    if !replies.send(Reply::Event(AgentEvent::Setup(session.state().clone()))) {
        return;
    }

    loop {
        match commands.recv_timeout(PUMP) {
            Ok(Command::Prompt(text)) => {
                if let Err(error) = session.start_turn(&text)
                    && !replies.send(Reply::Failed(format!("{error}")))
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
                // Asked, not waited on: the answer arrives through the event queue as a `Setup`, and a switch of
                // model over a large session is confirmed by hand first (`session/request_permission`) — a
                // worker that sat waiting here could neither show the card nor carry the answer back.
                if let Err(error) = session.ask_model(&model)
                    && !replies.send(Reply::Event(AgentEvent::Failed(format!(
                        "the agent would not switch model: {error}"
                    ))))
                {
                    return;
                }
            }
            Ok(Command::Setting { config, value }) => {
                if let Err(error) = session.ask_option(&config, &value)
                    && !replies.send(Reply::Event(AgentEvent::Failed(format!(
                        "the agent would not change {config}: {error}"
                    ))))
                {
                    return;
                }
            }
            Ok(Command::Fresh) => match session.begin() {
                Ok(()) => {
                    let opened = session.id().to_owned();
                    let _ = replies.send(Reply::Ready {
                        session: opened,
                        resumed: false,
                    });
                    let _ = replies.send(Reply::Event(AgentEvent::Setup(session.state().clone())));
                }
                Err(error) => {
                    let _ = replies.send(Reply::Event(AgentEvent::Failed(format!(
                        "the agent could not begin a conversation: {error}"
                    ))));
                }
            },
            Ok(Command::SwitchTo(wanted)) => match session.load_into(&wanted) {
                Ok(()) => {
                    let opened = session.id().to_owned();
                    // Asked of the session, not assumed: an agent that no longer has the conversation
                    // opens a new one instead, and a panel that says "resumed" afterwards is a panel that
                    // lies about what is on screen.
                    let _ = replies.send(Reply::Ready {
                        session: opened,
                        resumed: session.resumed(),
                    });
                    let _ = replies.send(Reply::Event(AgentEvent::Setup(session.state().clone())));
                }
                Err(error) => {
                    let _ = replies.send(Reply::Event(AgentEvent::Failed(format!(
                        "the agent could not open that conversation: {error}"
                    ))));
                }
            },
            Ok(Command::Sessions) => {
                let answer = match session.list_sessions() {
                    Ok(sessions) => AgentEvent::Sessions(sessions),
                    Err(error) => {
                        AgentEvent::Failed(format!("the agent listed no sessions: {error}"))
                    }
                };
                if !replies.send(Reply::Event(answer)) {
                    return;
                }
            }
            Ok(Command::Cancel) => {
                // Nothing is waited for: the answer to a stopped turn is the turn's own
                // ending (`cancelled`), and it arrives through the event loop like any other.
                let _ = session.cancel();
            }
            Ok(Command::Mode(mode)) => {
                if let Err(error) = session.ask_mode(&mode)
                    && !replies.send(Reply::Event(AgentEvent::Failed(format!(
                        "the agent would not switch mode: {error}"
                    ))))
                {
                    return;
                }
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
            // Told from here, where the agent speaks, and not left to the panel: the window is
            // drawn on demand, and one that is not on screen — another workspace, a person away from
            // the desk — is given no frames at all. A notice waiting for a frame waits for the person
            // to come back to the window, which is the one moment it is of no use.
            if let Some((want, detail)) = asking_for(&event) {
                let looking = telling.where_is_the_person();
                if let Some(told) = tell_the_desk(telling.desk.as_ref(), looking, want, &detail) {
                    let (title, _) = want.message(&detail);
                    if !replies.send(Reply::Told { title, told }) {
                        return;
                    }
                }
            }
            if !replies.send(Reply::Event(event)) {
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

    #[test]
    fn a_service_row_is_told_apart_from_the_words_of_the_person() {
        let (kind, line) = service_line(
            "[IMPORTANT: Background process proc_1 completed normally (exit code 0).\nOutput:\nall good",
        )
        .expect("a receipt is a service row");
        assert_eq!(kind, "background process");
        assert_eq!(
            line, "[IMPORTANT: Background process proc_1 completed normally (exit code 0).",
            "the wall of a receipt is what a person is shown"
        );
        assert_eq!(
            service_line("[INTERNAL NOTIFICATION — generated by the Hermes gateway]\nwoke up")
                .expect("a note of the gateway is a service row")
                .0,
            "gateway note"
        );
        assert_eq!(
            service_line(
                "[OUT-OF-BAND USER MESSAGE — a direct message]\nstop\n[/OUT-OF-BAND USER MESSAGE]"
            ),
            None,
            "the person arriving mid-turn is the person"
        );
        assert_eq!(service_line("why is the sky blue?"), None);
        assert_eq!(
            service_line("see this: [IMPORTANT: Background process proc_1]"),
            None,
            "a line quoting a receipt is still the words of the person"
        );
    }

    /// Whether the frame just drawn was asked for by this panel rather than by egui itself.
    ///
    /// egui remembers what asked for a frame, file and line of the call, so a wake can be told
    /// from the reasons egui has of its own.
    fn the_panel_asked_for_it(ctx: &egui::Context) -> bool {
        ctx.repaint_causes()
            .iter()
            .any(|cause| cause.file.ends_with("agent.rs"))
    }

    #[test]
    fn the_worker_wakes_the_window_it_was_told_about() {
        let ctx = egui::Context::default();
        let mut pass = ctx.run_ui(egui::RawInput::default(), |_| {});
        pass.textures_delta.clear();
        assert!(
            !the_panel_asked_for_it(&ctx),
            "nothing has been said, and yet a frame was asked for"
        );

        // Nobody has drawn a frame: there is no window to wake, and saying so is not an error.
        let waker = Waker::default();
        waker.wake();

        waker.watch(&ctx);
        assert!(
            waker.knows_the_window(),
            "the waker was not pointed at the window"
        );

        waker.wake();
        let mut pass = ctx.run_ui(egui::RawInput::default(), |_| {});
        pass.textures_delta.clear();
        assert!(
            the_panel_asked_for_it(&ctx),
            "the window was not asked for a frame, so an event of the agent waits for the person"
        );
    }

    #[test]
    fn nothing_is_told_while_the_person_is_looking_at_the_panel() {
        let desk = crate::notify::Written::default();
        let want = crate::notify::Want::Asks;
        let question = "Which variant should I write?";

        // Looking at the panel: the question is already on the screen in front of them.
        let here = crate::notify::Looking {
            focused: true,
            panel_in_front: true,
        };
        assert!(
            tell_the_desk(&desk, here, want, question).is_none(),
            "the desk was told about a question already on the screen"
        );
        assert!(
            desk.shown().is_empty(),
            "a notice was sent though the person is looking at the panel: {:?}",
            desk.shown()
        );

        // Walked away from a window that shows the panel: what notices are for.
        let away = crate::notify::Looking {
            focused: false,
            panel_in_front: true,
        };
        assert!(
            tell_the_desk(&desk, away, want, question).is_some(),
            "a person away from a window showing the panel was told nothing"
        );

        // The panel behind another tab, with the keyboard in this window: out of sight, so told.
        let behind = crate::notify::Looking {
            focused: true,
            panel_in_front: false,
        };
        assert!(
            tell_the_desk(&desk, behind, want, question).is_some(),
            "a question behind another tab was not told"
        );

        let shown = desk.shown();
        assert_eq!(shown.len(), 2, "the desk was told {shown:?}");
        assert_eq!(shown[0].1, question, "the body is not the question itself");
    }

    #[test]
    fn a_desk_that_refuses_is_said_in_the_window() {
        // A notification the desk never showed leaves no trace where the person can find it: they were
        // looking elsewhere by definition. The desk is spoken to by the thread of the agent, so its
        // answer comes back to the panel as a reply — and the panel has to put it where the person will
        // look, in the window. Without this, the only report of a broken desk is silence.
        let reason = crate::notify::Surface::show(&crate::notify::Refused, "Ответ готов", "")
            .expect_err("a desk that refuses is the check's own");
        let mut agent = AgentState::default();
        agent.take(Reply::Told {
            title: "Ответ готов".to_owned(),
            told: Err(reason.clone()),
        });

        assert!(
            agent.status.contains("refused"),
            "the window did not say the desk refused: {:?}",
            agent.status
        );
        assert!(
            agent.status.contains(&reason),
            "the window did not say why: {:?}",
            agent.status
        );
    }

    #[test]
    fn every_reply_of_the_worker_wakes_the_window() {
        let ctx = egui::Context::default();
        let waker = Waker::default();
        waker.watch(&ctx);
        let (outlet, kept) = channel();
        let worker = Replies { outlet, waker };

        assert!(
            worker.send(Reply::Failed("nothing to say".to_owned())),
            "the reply did not reach the panel"
        );
        let mut pass = ctx.run_ui(egui::RawInput::default(), |_| {});
        pass.textures_delta.clear();
        assert!(
            the_panel_asked_for_it(&ctx),
            "a reply of the agent did not wake the window, so it waits for the person"
        );
        drop(kept);
    }

    #[test]
    fn a_waker_that_knows_no_window_asks_for_no_frame() {
        let ctx = egui::Context::default();
        let mut pass = ctx.run_ui(egui::RawInput::default(), |_| {});
        pass.textures_delta.clear();

        Waker::default().wake();
        let mut pass = ctx.run_ui(egui::RawInput::default(), |_| {});
        pass.textures_delta.clear();
        assert!(
            !the_panel_asked_for_it(&ctx),
            "a wake from a waker that knows no window reached one anyway"
        );
    }

    #[test]
    fn a_long_silence_of_the_agent_is_said_out_loud() {
        let mut agent = AgentState {
            status: "answering…".to_owned(),
            ..AgentState::default()
        };

        // Nothing in flight, and a turn that has only just been sent: the plain word.
        assert_eq!(agent.quiet_for(), None);
        assert_eq!(agent.saying(), "answering…");
        agent.turn_in_flight_for(Duration::from_secs(30));
        assert_eq!(
            agent.saying(),
            "answering…",
            "half a minute of a slow model is not a silence worth announcing"
        );

        // Past the minute, the panel says the one thing it can see.
        agent.turn_in_flight_for(Duration::from_secs(75));
        assert_eq!(
            agent.saying(),
            "answering… — no word from the agent for 1:15"
        );
    }

    #[test]
    fn the_silence_is_counted_from_the_last_word_and_not_from_the_send() {
        let mut agent = AgentState {
            status: "answering…".to_owned(),
            ..AgentState::default()
        };
        // A turn that streamed for four minutes and has been quiet for two: counting from the send
        // would claim six, and a person told six would go looking for a problem that is not there.
        agent.turn_in_flight_for(Duration::from_secs(360));
        agent.last_word = Some(Instant::now() - Duration::from_secs(120));
        assert_eq!(
            agent.saying(),
            "answering… — no word from the agent for 2:00"
        );
    }

    #[test]
    fn a_long_quiet_is_counted_the_way_a_person_counts_it() {
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(clock(Duration::from_secs(600)), "10:00");
        assert_eq!(clock(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn a_summary_of_a_compacted_context_is_one_line_and_not_the_wall() {
        // The summary of a compacted context arrives as a `role="user"` row like any other, and
        // it is the longest wall of them all: it used to be drawn as the words of the person.
        let summary = format!(
            "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted into the summary \
below.\n{}--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---",
            "a line of the wall\n".repeat(40)
        );
        let (kind, line) = service_line(&summary).expect("a summary is a service row");
        assert_eq!(kind, "context compaction");
        assert_eq!(
            line,
            "context compacted: a summary of the earlier turns is kept in its place (42 lines)",
            "the line a person is shown is not the one the panel makes"
        );
        assert!(
            !line.contains("Earlier turns were compacted"),
            "the wall itself was kept as the line to draw"
        );
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
