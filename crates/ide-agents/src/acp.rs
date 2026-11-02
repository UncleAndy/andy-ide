//! The ACP connection as the standard's own SDK makes it, with this project's record kept.
//!
//! Two rules meet here. The panel's rule about the wire is not negotiable ([`crate::wire`]): every line that
//! crosses must be written down as it crosses, because a panel that draws the conversation can hide a line,
//! and a hidden line looks exactly like silence. The standard's rule is that a client brings its own runtime
//! (`agent_client_protocol` carries none), and that its transport is anything that reads and writes.
//!
//! So the child's pipes are wrapped in two small reporting types and the SDK is handed the wrappers
//! (`ByteStreams` takes any pair, splitting the lines itself). The agent's own error output is not a frame of
//! the protocol and goes into the record as a note, the way the session's own reader does it.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, CancelNotification, ContentBlock,
    CreateElicitationRequest, CreateElicitationResponse, CreateTerminalRequest,
    ElicitationAcceptAction, ElicitationAction, ElicitationContentValue, ElicitationMode,
    ErrorCode, Implementation, InitializeRequest, KillTerminalRequest, ListSessionsRequest,
    PromptRequest, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigOptionValue, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TerminalOutputRequest,
    TextContent, ToolCallStatus, WaitForTerminalExitRequest, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, ByteStreams, Client, Responder};
use anyhow::Context as _;
use async_process::{Child, ChildStderr, ChildStdin, ChildStdout};
use futures::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use futures::{FutureExt, StreamExt};

use crate::protocol::{
    AgentCommand, ChoiceRequest, ConfigOption, Model, PermissionRequest, SessionInfo, SessionState,
    Usage, options_of, session_state, sessions_of,
};
use crate::session::{ANSWER, AgentSpec, Event, HELLO, PermissionPolicy};
use crate::wire::Wire;

/// An agent started for one session, with the transport the SDK drives.
pub struct Started {
    /// Lines to and from the agent, every one of them written into the record as it crosses.
    pub transport: ByteStreams<ReportingWriter<ChildStdin>, ReportingReader<ChildStdout>>,
    /// The agent's own error output. Nothing reads it by itself: whoever drives the connection hands it to
    /// [`note_the_agent_words`], or a full pipe stops the agent mid-sentence.
    pub stderr: ChildStderr,
    /// The process itself: it lives exactly as long as this does.
    pub child: Child,
}

/// Starts `command` and opens the transport the SDK talks over, with the record kept.
pub fn start(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    wire: Arc<dyn Wire>,
) -> anyhow::Result<Started> {
    let mut config = AcpAgentConfig::new(command);
    for arg in args {
        config = config.arg(arg.clone());
    }
    for (name, value) in env {
        config = config.env(name.clone(), value.clone());
    }
    // The low-level escape hatch, and not the SDK's own connected agent: the child's pipes have to pass
    // through us for the record to be kept, which means owning the process too.
    let (stdin, stdout, stderr, child) = AcpAgent::new(config).spawn_process()?;
    wire.note(&format!("{command} {args:?} started"));
    Ok(Started {
        transport: ByteStreams::new(
            ReportingWriter::new(stdin, Arc::clone(&wire)),
            ReportingReader::new(stdout, wire),
        ),
        stderr,
        child,
    })
}

/// Reads the agent's own words until the pipe closes, writing each line into the record.
///
/// Not a frame of the protocol: an agent that says why it is unhappy says it here, and a person reading the
/// record afterwards is the one who needs it. `keep` is for the panel, which says the same words in its own
/// way when something goes wrong — the record is for whoever reads the file, `keep` is for whoever is looking
/// at the window.
pub async fn note_the_agent_words(
    stderr: ChildStderr,
    wire: Arc<dyn Wire>,
    keep: Option<Arc<Mutex<String>>>,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(Ok(line)) = lines.next().await {
        wire.note(&line);
        if let Some(keep) = &keep
            && let Ok(mut kept) = keep.lock()
        {
            if !kept.is_empty() {
                kept.push('\n');
            }
            kept.push_str(&line);
        }
    }
}

/// The agent's input: every whole line is written into the record as the pipe takes it.
pub struct ReportingWriter<W> {
    inner: W,
    wire: Arc<dyn Wire>,
    /// What has been written since the last newline: a line is the record's unit, not a write.
    pending: Vec<u8>,
}

impl<W> ReportingWriter<W> {
    /// Wraps `inner`, writing down what crosses it.
    pub fn new(inner: W, wire: Arc<dyn Wire>) -> Self {
        Self {
            inner,
            wire,
            pending: Vec::new(),
        }
    }

    /// Notes every line among `bytes`, keeping the last one if it is not whole yet.
    ///
    /// Called after the write and not before: a line the pipe refused did not cross the wire. (The session
    /// that owns its own threads notes a line before writing it, which is honest for a write that is one call
    /// and would be a lie for one that arrives in pieces.)
    fn note_what_left(&mut self, bytes: &[u8]) {
        take_lines(&mut self.pending, bytes, &self.wire, true);
    }
}

/// The agent's output: every whole line is written into the record as it arrives.
pub struct ReportingReader<R> {
    inner: R,
    wire: Arc<dyn Wire>,
    /// What has arrived since the last newline, for the same reason as the writer's.
    pending: Vec<u8>,
}

impl<R> ReportingReader<R> {
    /// Wraps `inner`, writing down what crosses it.
    pub fn new(inner: R, wire: Arc<dyn Wire>) -> Self {
        Self {
            inner,
            wire,
            pending: Vec::new(),
        }
    }

    fn note_what_arrived(&mut self, bytes: &[u8]) {
        take_lines(&mut self.pending, bytes, &self.wire, false);
    }
}

/// Splits `bytes` into lines, notes the whole ones, and keeps the rest for the next call.
fn take_lines(pending: &mut Vec<u8>, bytes: &[u8], wire: &Arc<dyn Wire>, sent: bool) {
    for byte in bytes {
        if *byte == b'\n' {
            let line = String::from_utf8_lossy(pending)
                .trim_end_matches('\r')
                .to_owned();
            if !line.is_empty() {
                if sent {
                    wire.sent(&line);
                } else {
                    wire.got(&line);
                }
            }
            pending.clear();
        } else {
            pending.push(*byte);
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ReportingWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(count)) = poll {
            let left = buf[..count].to_vec();
            this.note_what_left(&left);
        }
        poll
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_close(cx);
        if poll.is_ready() && !this.pending.is_empty() {
            // A last line without its newline: the pipe is closed, so it is as whole as it will ever be.
            let left = std::mem::take(&mut this.pending);
            this.note_what_left(&left);
        }
        poll
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ReportingReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(count)) = poll {
            let arrived = buf[..count].to_vec();
            this.note_what_arrived(&arrived);
        }
        poll
    }
}

// ==============================================================================================
// The session: the surface the panel already knows, spoken over the standard's own client.
// ==============================================================================================

/// How long the handshake may take before the session is called a failure.
///
/// A start that hangs is a start that failed: the panel is waiting for a conversation, and an agent that has
/// not answered `initialize` within this is not one this window can use.
const HANDSHAKE: Duration = Duration::from_secs(60);

/// How long a page of conversations may take.
const SESSIONS: Duration = Duration::from_secs(10);

/// How many conversations the panel keeps from a list that goes on forever.
const SESSIONS_KEPT: usize = 200;

/// What the panel says when the agent asks for something the panel does not serve.
const NO_TERMINAL: &str =
    "this window gives the agent no terminal: the one it was started with is its own";

/// The name the standard gives the setting that picks the model.
///
/// The standard has no request of its own for the model: it is a setting of the agent's, so this is what
/// the setting is looked for by when the agent names no models of its own.
const MODEL: &str = "model";

/// A question the agent asked and the person has not answered yet.
///
/// The responder is the protocol's own handle for the request: it carries the id the agent is waiting on, so
/// a question can be answered as long as the person takes, from a thread that has nothing to do with the one
/// the request arrived on.
enum Waiting {
    /// `session/request_permission`.
    Permission(Responder<RequestPermissionResponse>),
    /// `elicitation/create`.
    Choice(Responder<CreateElicitationResponse>),
}

/// What the panel asks the thread that owns the connection to do.
enum Order {
    /// One turn, with the words the person typed.
    Turn(String),
    /// The session is over.
    Stop,
    /// The person's answer to a permission request.
    Permission {
        /// Which question, by the number handed to the panel.
        id: u64,
        /// The option they chose, or nothing for a refusal.
        option: Option<String>,
    },
    /// The person's answer to a question with variants.
    Choice {
        /// Which question, by the number handed to the panel.
        id: u64,
        /// The property the answer goes back under.
        field: String,
        /// Whether several answers were allowed.
        multiple: bool,
        /// What they picked or wrote, or nothing for a refusal.
        chosen: Option<Vec<String>>,
    },
    /// The panel changes a setting; the agent's own settings come back on the channel.
    Change {
        /// What is being changed.
        what: Change,
        /// Where the settings the agent read back go.
        /// Where the settings the agent read back go, when a caller waits for them; `None` when the
        /// change is the connection's own task and its answer comes through the event queue instead.
        done: Option<std::sync::mpsc::Sender<anyhow::Result<Vec<ConfigOption>>>>,
    },
    /// The panel opens a conversation: a fresh one, or the one it names.
    Open {
        /// The conversation to pick up, or nothing for a fresh one.
        session: Option<String>,
        /// Where the id, whether it was resumed, and the settings go.
        done: std::sync::mpsc::Sender<anyhow::Result<(String, bool, SessionState)>>,
    },
    /// The panel asks which conversations the agent knows of.
    Sessions(std::sync::mpsc::Sender<anyhow::Result<Vec<SessionInfo>>>),
    /// The agent is asked to stop the turn in flight, keeping the conversation.
    Cancel,
}

/// A change the panel asked for.
enum Change {
    /// The mode the agent answers in.
    Mode(String),
    /// The model, through the agent's own method — the way back, taken only when the standard has none.
    Model(String),
    /// One of the other settings, by the id the agent gave it.
    Setting {
        /// Which setting.
        config: String,
        /// The choice to put in force.
        value: String,
    },
}

/// What the handshake answered, handed back to whoever started the session.
struct Ready {
    /// Whether the conversation was picked up from an earlier run.
    resumed: bool,
    /// The models, modes and settings the agent opened the conversation with.
    state: SessionState,
}

/// A running agent, spoken to through the standard's own client.
///
/// The same surface the panel already knows (`crate::session::Session`): what changes in the migration is the
/// wire underneath, not the panel above. The connection lives on a thread of its own and is driven by
/// `futures::executor` — the standard brings no runtime, and this project has none.
pub struct Session {
    /// Orders to that thread.
    orders: futures::channel::mpsc::UnboundedSender<Order>,
    /// Events from it.
    events: std::sync::mpsc::Receiver<Event>,
    /// The process, shared so that stopping the session stops the agent.
    child: Arc<Mutex<Child>>,
    /// The conversation's id, once the handshake named it.
    id: Arc<Mutex<String>>,
    /// Whether it was picked up from an earlier run.
    resumed: bool,
    /// The models and modes the agent offered.
    state: SessionState,
    /// The last context figure it gave.
    usage: Option<Usage>,
    /// The agent's own error output, as it arrives.
    complaints: Arc<Mutex<String>>,
    /// Whether the panel has agreed that the agent may write files through this window.
    writes: Arc<AtomicBool>,
    /// Events a waiting caller set aside before the one it was waiting for.
    ///
    /// [`Session::prompt`] waits for the end of a turn and nothing else, and the pieces of the answer arrive
    /// before it: throwing them away would lose the answer, so they are handed back to the queue they came
    /// from, in the order they were taken.
    pending: std::collections::VecDeque<Event>,
    /// Whether the process is gone.
    closed: bool,
}

impl Session {
    /// Starts an agent in `root` and speaks the protocol with it.
    pub fn start(spec: &AgentSpec, root: &Path, policy: PermissionPolicy) -> anyhow::Result<Self> {
        Self::start_with(spec, root, policy, None, None, None)
    }

    /// Starts an agent and asks it to pick up `resume`, if it has such a conversation.
    ///
    /// The handshake happens before this returns, so a `Session` that exists is a session that answered: an
    /// agent that does not answer `initialize` within [`HANDSHAKE`] is a failure to start and nothing else.
    pub fn start_with(
        spec: &AgentSpec,
        root: &Path,
        policy: PermissionPolicy,
        resume: Option<String>,
        progress: Option<Arc<Mutex<String>>>,
        wire: Option<Arc<dyn Wire>>,
    ) -> anyhow::Result<Self> {
        step_said(progress.as_ref(), "Launching\u{2026}");
        let wire: Arc<dyn Wire> = wire.unwrap_or_else(|| Arc::new(crate::wire::Silent));
        // Named in the refusal: a person reading "no such file or directory" has to be told which agent it
        // was, or the panel's own words are the only thing that knows.
        let started = start(&spec.command, &spec.args, &spec.env, Arc::clone(&wire))
            .with_context(|| format!("running {} for agent {}", spec.command, spec.name))?;

        let (orders_tx, orders) = futures::channel::mpsc::unbounded();
        let (events_tx, events) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let id = Arc::new(Mutex::new(String::new()));
        let complaints = Arc::new(Mutex::new(String::new()));
        let waiting: Arc<Mutex<HashMap<u64, Waiting>>> = Arc::new(Mutex::new(HashMap::new()));
        let questions = Arc::new(AtomicU64::new(1));
        let settings: Arc<Mutex<SessionState>> = Arc::new(Mutex::new(SessionState::default()));
        let writes = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(started.child));
        // The thread that speaks to the agent gets a share of the process: it is the one watching for it.
        let driven = Arc::clone(&child);

        let cwd = root.to_path_buf();
        let driver = {
            let id = Arc::clone(&id);
            let complaints = Arc::clone(&complaints);
            let waiting = Arc::clone(&waiting);
            let questions = Arc::clone(&questions);
            let settings = Arc::clone(&settings);
            let steps = progress.clone();
            let writes = Arc::clone(&writes);
            let wire = Arc::clone(&wire);
            let record = Arc::clone(&wire);
            std::thread::Builder::new()
                .name("acp-sdk".to_owned())
                .spawn(move || {
                    let outcome = futures::executor::block_on(converse(
                        started.transport,
                        started.stderr,
                        orders,
                        events_tx,
                        ready_tx,
                        id,
                        complaints,
                        wire,
                        waiting,
                        questions,
                        settings,
                        writes,
                        steps,
                        driven,
                        policy,
                        resume,
                        cwd,
                    ));
                    if let Err(error) = outcome {
                        // Said into the record and not into a log this crate does not have: whoever reads
                        // the record afterwards is the one who needs to know the connection ended.
                        record.note(&format!("the connection to the agent ended: {error}"));
                    }
                })
                .context("starting the thread that speaks to the agent")?
        };

        let ready = match ready_rx.recv_timeout(HANDSHAKE) {
            Ok(ready) => ready,
            Err(_) => {
                let _ = driver;
                anyhow::bail!(
                    "{} did not answer within {}s",
                    spec.name,
                    HANDSHAKE.as_secs()
                );
            }
        };
        let mut state = ready.state;
        fill_models(&mut state);
        Ok(Self {
            orders: orders_tx,
            events,
            child,
            id,
            resumed: ready.resumed,
            state,
            usage: None,
            complaints,
            writes,
            pending: std::collections::VecDeque::new(),
            closed: false,
        })
    }

    /// The conversation's id, as the agent named it.
    pub fn id(&self) -> String {
        self.id.lock().map(|id| id.clone()).unwrap_or_default()
    }

    /// Whether this conversation was picked up from an earlier run.
    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// The models and modes the agent offered when it opened the conversation.
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// The last context figure the agent gave.
    pub fn usage(&self) -> Option<Usage> {
        self.usage.clone()
    }

    /// Says what the panel is doing while the agent comes up.
    pub fn complaints(&self) -> String {
        self.complaints
            .lock()
            .map(|kept| kept.clone())
            .unwrap_or_default()
    }

    /// Sends one turn to the agent.
    pub fn start_turn(&mut self, text: &str) -> anyhow::Result<()> {
        self.orders
            .unbounded_send(Order::Turn(text.to_owned()))
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))
    }

    /// Sends one turn and waits for the agent's own word about how it ended.
    pub fn prompt(&mut self, text: &str, timeout: Duration) -> anyhow::Result<String> {
        self.start_turn(text)?;
        let deadline = std::time::Instant::now() + timeout;
        let mut set_aside: Vec<Event> = Vec::new();
        let outcome = loop {
            let Some(left) = deadline
                .checked_duration_since(std::time::Instant::now())
                .filter(|left| !left.is_zero())
            else {
                break Err(anyhow::anyhow!("the agent did not answer in {timeout:?}"));
            };
            match self.next_event(left) {
                Some(Event::Finished(word)) => break Ok(word),
                Some(Event::Failed(words)) => break Err(anyhow::anyhow!("{words}")),
                // The process is gone: said in as many words, because "no answer" and "nothing to answer
                // from" are different things to a person reading the panel.
                Some(Event::Closed) => break Err(anyhow::anyhow!("the agent closed its output")),
                Some(event) => set_aside.push(event),
                None => {}
            }
        };
        // Handed back rather than thrown away: the answer arrives in pieces *before* the end of the turn, and
        // a caller that asked only for the ending still gets to read them afterwards.
        for event in set_aside.into_iter().rev() {
            self.pending.push_front(event);
        }
        outcome
    }

    /// The next thing the agent said, waiting up to `timeout`.
    pub fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        // What a waiting caller set aside first: it came first, and the panel draws the answer from this same
        // queue.
        if let Some(event) = self.pending.pop_front() {
            self.keep_in_step(&event);
            return Some(event);
        }
        if self.closed {
            return Some(Event::Closed);
        }
        match self.events.recv_timeout(timeout) {
            Ok(event) => {
                self.keep_in_step(&event);
                Some(event)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.closed = true;
                Some(Event::Closed)
            }
        }
    }

    /// Keeps what the panel asks for separately from what it is shown.
    fn keep_in_step(&mut self, event: &Event) {
        match event {
            Event::Usage(usage) => self.usage = Some(usage.clone()),
            Event::Setup(state) => self.state = state.clone(),
            _ => {}
        }
    }

    /// Answers a permission request that was handed over with [`Event::Permission`].
    pub fn answer_permission(&mut self, id: u64, option: Option<&str>) -> anyhow::Result<()> {
        self.orders
            .unbounded_send(Order::Permission {
                id,
                option: option.map(str::to_owned),
            })
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))
    }

    /// Answers a question with variants, handed over with [`Event::Choice`].
    pub fn answer_choice(
        &mut self,
        id: u64,
        field: &str,
        multiple: bool,
        chosen: Option<&[String]>,
    ) -> anyhow::Result<()> {
        self.orders
            .unbounded_send(Order::Choice {
                id,
                field: field.to_owned(),
                multiple,
                chosen: chosen.map(<[String]>::to_vec),
            })
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))
    }

    /// Allows or forbids the agent to write files through this window.
    ///
    /// A write the panel has not agreed to is refused in the protocol's own words. The agreement is the
    /// panel's to give: the mode the agent was switched to is the agent's own word, not this.
    pub fn allow_writes(&mut self, allow: bool) {
        self.writes.store(allow, Ordering::Relaxed);
    }

    /// Opens a fresh conversation with the agent.
    pub fn begin(&mut self) -> anyhow::Result<()> {
        self.state = self.open(None)?;
        Ok(())
    }

    /// Picks up an earlier conversation, if the agent still has it.
    pub fn load_into(&mut self, session: &str) -> anyhow::Result<()> {
        self.state = self.open(Some(session.to_owned()))?;
        Ok(())
    }

    /// The conversations the agent knows of, in pages, up to what the panel keeps.
    pub fn list_sessions(&mut self) -> anyhow::Result<Vec<SessionInfo>> {
        let (done, answer) = std::sync::mpsc::channel();
        self.orders
            .unbounded_send(Order::Sessions(done))
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))?;
        answer
            .recv_timeout(SESSIONS)
            .map_err(|_| anyhow::anyhow!("the agent listed no conversations in time"))?
    }

    /// Asks the agent to stop the turn in flight, keeping the conversation.
    ///
    /// A notification, not a request: the standard has no answer to it — a stopped turn ends in
    /// `cancelled`, and that arrives through [`Session::next_event`] like any other ending.
    pub fn cancel(&mut self) -> anyhow::Result<()> {
        self.orders
            .unbounded_send(Order::Cancel)
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))
    }

    /// Changes the mode the agent answers in.
    pub fn set_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        self.change(Change::Mode(mode.to_owned()))?;
        // Only after the agent agreed: a panel that shows a mode the agent never took is a panel that
        // lies about how hard the agent is thinking.
        self.state.current_mode = Some(mode.to_owned());
        Ok(())
    }

    /// Changes one of the settings the agent offers: how hard it thinks, in this panel.
    pub fn set_option(&mut self, config: &str, value: &str) -> anyhow::Result<()> {
        let options = self.change(Change::Setting {
            config: config.to_owned(),
            value: value.to_owned(),
        })?;
        self.settings_taken(config, value, options);
        Ok(())
    }

    /// Changes the model: through the standard's setting when the agent offers one, and otherwise through
    /// the agent's own method.
    ///
    /// The standard carries the model as one of the agent's settings and has no request of its own for it.
    /// Hermes 0.22.5 offers no such setting and names its models in a field of its own, so with that agent
    /// the way back is the only way the panel keeps a choice the person already had. It is taken only when
    /// there is no standard road, it is written into the record when it is taken, and it retires itself the
    /// day the agent offers the setting.
    pub fn set_model(&mut self, model: &str) -> anyhow::Result<()> {
        match self.model_road(model)? {
            Change::Setting { config, value } => {
                let options = self.change(Change::Setting {
                    config: config.clone(),
                    value,
                })?;
                self.settings_taken(&config, model, options);
            }
            road => {
                self.change(road)?;
            }
        }
        // Only after the agent agreed — and to what it read back, not to what was asked of it. An agent
        // that answers with its settings has said which model it now holds; one that answers without them
        // (the model's own method) has taken the choice, and the one asked for stands.
        let read_back =
            model_setting(&self.state.options).and_then(|setting| setting.current.clone());
        self.state.current_model = read_back.or_else(|| Some(model.to_owned()));
        Ok(())
    }

    /// Asks for a model without waiting: the panel's road.
    ///
    /// The change runs on the connection and its answer comes back through the event queue as a `Setup`, so
    /// the worker stays free to carry what the agent says meanwhile to the panel. That matters for a switch of
    /// model over a large session, which the agent first puts to the person (`session/request_permission`): a
    /// worker that sat waiting could neither show the card nor carry the answer back, and the switch would
    /// never happen.
    pub fn ask_model(&mut self, model: &str) -> anyhow::Result<()> {
        let road = self.model_road(model)?;
        self.ask(road)
    }

    /// The road a model change takes: the standard's setting when the agent offers one, the agent's own
    /// method otherwise.
    fn model_road(&self, model: &str) -> anyhow::Result<Change> {
        if let Some(setting) = model_setting(&self.state.options) {
            Ok(Change::Setting {
                config: setting.id.clone(),
                value: model.to_owned(),
            })
        } else if !self.state.models.is_empty() {
            Ok(Change::Model(model.to_owned()))
        } else {
            anyhow::bail!("this agent offers no way to change the model")
        }
    }

    /// Asks for one of the settings the agent offers, without waiting: the panel's road.
    pub fn ask_option(&mut self, config: &str, value: &str) -> anyhow::Result<()> {
        self.ask(Change::Setting {
            config: config.to_owned(),
            value: value.to_owned(),
        })
    }

    /// Asks for another mode, without waiting: the panel's road.
    pub fn ask_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        self.ask(Change::Mode(mode.to_owned()))
    }

    /// Hands the connection a change to make on a task of its own, and does not wait for it.
    fn ask(&self, what: Change) -> anyhow::Result<()> {
        self.orders
            .unbounded_send(Order::Change { what, done: None })
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))
    }

    /// Asks the connection to change a setting and waits for the agent's own settings back.
    fn change(&mut self, what: Change) -> anyhow::Result<Vec<ConfigOption>> {
        let (done, answer) = std::sync::mpsc::channel();
        self.orders
            .unbounded_send(Order::Change {
                what,
                done: Some(done),
            })
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))?;
        answer
            .recv_timeout(ANSWER)
            .map_err(|_| anyhow::anyhow!("the agent did not answer in time"))?
    }

    /// Opens a conversation and takes what the agent says it is.
    fn open(&mut self, session: Option<String>) -> anyhow::Result<SessionState> {
        let (done, answer) = std::sync::mpsc::channel();
        self.orders
            .unbounded_send(Order::Open { session, done })
            .map_err(|_| anyhow::anyhow!("the agent is no longer listening"))?;
        let (_, resumed, mut state) = answer
            .recv_timeout(HELLO)
            .map_err(|_| anyhow::anyhow!("the agent did not open the conversation in time"))??;
        self.resumed = resumed;
        fill_models(&mut state);
        Ok(state)
    }

    /// Keeps the settings the agent read back, and the one that was asked for when it read none.
    ///
    /// The reply carries the settings read back from the agent, so the panel shows what the agent accepted
    /// rather than what was asked of it. An agent that answers without the list took the choice, and the one
    /// that was asked for stands.
    fn settings_taken(&mut self, config: &str, value: &str, options: Vec<ConfigOption>) {
        if options.is_empty() {
            if let Some(setting) = self
                .state
                .options
                .iter_mut()
                .find(|setting| setting.id == config)
            {
                setting.current = Some(value.to_owned());
            }
        } else {
            self.state.options = options;
        }
        fill_models(&mut self.state);
    }

    /// Ends the conversation: the agent is asked to stop, then killed.
    pub fn stop(&mut self) {
        let _ = self.orders.unbounded_send(Order::Stop);
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            let gone = self
                .child
                .lock()
                .map(|mut child| matches!(child.try_status(), Ok(Some(_))))
                .unwrap_or(false);
            if gone {
                self.closed = true;
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
        self.closed = true;
    }
}

/// Says something in the status line the panel watches while an agent comes up.
fn step_said(progress: Option<&Arc<Mutex<String>>>, text: &str) {
    if let Some(progress) = progress
        && let Ok(mut said) = progress.lock()
    {
        *said = text.to_owned();
    }
}

/// Drives the connection: the handshake, then one order after another.
#[allow(clippy::too_many_arguments)]
async fn converse(
    transport: ByteStreams<ReportingWriter<ChildStdin>, ReportingReader<ChildStdout>>,
    stderr: ChildStderr,
    mut orders: futures::channel::mpsc::UnboundedReceiver<Order>,
    events: std::sync::mpsc::Sender<Event>,
    ready: std::sync::mpsc::Sender<Ready>,
    id: Arc<Mutex<String>>,
    complaints: Arc<Mutex<String>>,
    wire: Arc<dyn Wire>,
    waiting: Arc<Mutex<HashMap<u64, Waiting>>>,
    questions: Arc<AtomicU64>,
    settings: Arc<Mutex<SessionState>>,
    writes: Arc<AtomicBool>,
    steps: Option<Arc<Mutex<String>>>,
    child: Arc<Mutex<Child>>,
    policy: PermissionPolicy,
    resume: Option<String>,
    cwd: PathBuf,
) -> anyhow::Result<()> {
    let watched = Arc::clone(&child);
    let words = note_the_agent_words(stderr, Arc::clone(&wire), Some(complaints));
    let told = events.clone();
    let notes = Arc::clone(&settings);
    let reads_root = cwd.clone();
    let writes_root = cwd.clone();
    let allowed = Arc::clone(&writes);
    // Each handler is a `move` closure of its own, so each gets its own share of the same state.
    let asking_permission = events.clone();
    let waiting_permission = Arc::clone(&waiting);
    let numbered_permission = Arc::clone(&questions);
    let asking_choice = events.clone();
    let waiting_choice = Arc::clone(&waiting);
    let numbered_choice = Arc::clone(&questions);
    Client
        .builder()
        .name("andy-ide")
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                let asked_for = PermissionRequest::from_params(&json_of(&request));
                // The policy first, the person second: an agent told to go ahead is not a question, and a
                // window with nobody at it would otherwise hold up a turn nobody is watching.
                let decided = match policy {
                    PermissionPolicy::Allow => asked_for
                        .option_of("allow_once")
                        .or_else(|| asked_for.option_of("allow_always")),
                    PermissionPolicy::Deny => asked_for
                        .option_of("reject_once")
                        .or_else(|| asked_for.option_of("reject_always")),
                    PermissionPolicy::Ask => {
                        let id = numbered_permission.fetch_add(1, Ordering::Relaxed);
                        let kept = match waiting_permission.lock() {
                            Ok(mut waiting) => {
                                waiting.insert(id, Waiting::Permission(responder));
                                true
                            }
                            Err(_) => false,
                        };
                        if !kept {
                            return Ok(());
                        }
                        let _ = asking_permission.send(Event::Permission {
                            id,
                            request: asked_for,
                        });
                        return Ok(());
                    }
                };
                let _ = responder.respond(RequestPermissionResponse::new(outcome_of(decided)));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CreateElicitationRequest, responder, _cx| {
                // A form is what this window can draw. An agent that opens a URL has left the window, and a
                // panel cannot follow it — said in the protocol's own words rather than left unanswered.
                if !matches!(request.mode, ElicitationMode::Form(_)) {
                    let _ = responder.respond_with_internal_error(
                        "this client asks for a choice in form mode only",
                    );
                    return Ok(());
                }
                let id = numbered_choice.fetch_add(1, Ordering::Relaxed);
                let kept = match waiting_choice.lock() {
                    Ok(mut waiting) => {
                        waiting.insert(id, Waiting::Choice(responder));
                        true
                    }
                    Err(_) => false,
                };
                if !kept {
                    return Ok(());
                }
                let _ = asking_choice.send(Event::Choice {
                    id,
                    request: ChoiceRequest::from_params(&json_of(&request)),
                });
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ReadTextFileRequest, responder, _cx| {
                // A request nobody answers is a turn that never ends, and a hidden line looks exactly like
                // silence: every request the panel is asked is answered, even when the answer is a refusal.
                match serve_read(&reads_root, &request.path) {
                    Ok(text) => {
                        let _ = responder.respond(ReadTextFileResponse::new(text));
                    }
                    Err((code, words)) => {
                        let _ = responder.respond_with_error(refusal(code, &words));
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: WriteTextFileRequest, responder, _cx| {
                if !allowed.load(Ordering::Relaxed) {
                    let _ = responder.respond_with_error(refusal(
                        ErrorCode::MethodNotFound,
                        "the panel has not allowed the agent to write files",
                    ));
                    return Ok(());
                }
                match serve_write(&writes_root, &request.path, &request.content) {
                    Ok(()) => {
                        let _ = responder.respond(WriteTextFileResponse::new());
                    }
                    Err((code, words)) => {
                        let _ = responder.respond_with_error(refusal(code, &words));
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: CreateTerminalRequest, responder, _cx| {
                let _ =
                    responder.respond_with_error(refusal(ErrorCode::MethodNotFound, NO_TERMINAL));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: KillTerminalRequest, responder, _cx| {
                let _ =
                    responder.respond_with_error(refusal(ErrorCode::MethodNotFound, NO_TERMINAL));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: ReleaseTerminalRequest, responder, _cx| {
                let _ =
                    responder.respond_with_error(refusal(ErrorCode::MethodNotFound, NO_TERMINAL));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: WaitForTerminalExitRequest, responder, _cx| {
                let _ =
                    responder.respond_with_error(refusal(ErrorCode::MethodNotFound, NO_TERMINAL));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: TerminalOutputRequest, responder, _cx| {
                let _ =
                    responder.respond_with_error(refusal(ErrorCode::MethodNotFound, NO_TERMINAL));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let Some(event) = event_of(&notification.update, &notes) {
                    let _ = told.send(event);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |cx| {
            // The agent's own words are read for as long as it speaks, on a task of the connection's own.
            cx.spawn(async move {
                words.await;
                Ok(())
            })?;
            // The steps a start is at, in the panel's own words: a real agent can take its time at any of
            // them, and naming the step is what keeps a slow start from looking like a hang.
            step_said(steps.as_ref(), "Getting agent parameters\u{2026}");
            let init = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
                    // Said in the handshake as well as beside the directory: this window reads and writes no
                    // files for the agent, and gives it no terminal. Its own terminal is where its project
                    // is — the machine it runs on is not necessarily this one.
                    agent_client_protocol::schema::v1::ClientCapabilities::new().fs(
                        agent_client_protocol::schema::v1::FileSystemCapabilities::new()
                            .read_text_file(false)
                            .write_text_file(false),
                    )
                    .terminal(false),
                ))
                .block_task()
                .await?;
            wire.note(&format!(
                "{} answered initialize: {:?}",
                said_of(&init.agent_info),
                init.protocol_version
            ));

            // What the agent says the conversation is comes from its own answer, read the way this project
            // already reads it: the standard's shapes and the panel's are the same JSON.
            step_said(steps.as_ref(), "Getting model parameters\u{2026}");
            let (mut opened, resumed, state) = match resume {
                Some(session) => {
                    match cx
                        .send_request(kept_conversation(&cwd, &session))
                        .block_task()
                        .await
                    {
                        Ok(answer) => (session, true, session_state(&answer)),
                        // An id the agent no longer knows is not a failure to report: the person wanted a
                        // conversation here, and the panel is not the place for a rumour about an old one.
                        Err(_) => {
                            let answer = cx
                                .send_request(fresh_conversation(&cwd))
                                .block_task()
                                .await?;
                            (session_of(&answer)?, false, session_state(&answer))
                        }
                    }
                }
                None => {
                    let answer = cx
                        .send_request(fresh_conversation(&cwd))
                        .block_task()
                        .await?;
                    let opened = session_of(&answer)?;
                    (opened, false, session_state(&answer))
                }
            };
            if let Ok(mut named) = id.lock() {
                *named = opened.clone();
            }
            if let Ok(mut known) = settings.lock() {
                *known = state.clone();
                fill_models(&mut known);
            }
            let _ = ready.send(Ready { resumed, state });

            // A turn is a task of its own and not something the loop waits on: the person's answer arrives
            // *while* a turn runs — that is what a permission request is — so a loop that waited for the
            // prompt would be waiting for an answer it is itself holding up. One turn at a time, held by a
            // lock the tasks share, so turns keep the order they were asked in.
            let one_at_a_time = Arc::new(futures::lock::Mutex::new(()));
            // A watch of its own on the process. A transport that has seen EOF does not by itself end the
            // session task, and a session task waiting for an order forever is a panel that never notices
            // the agent is gone — it would sit on a dead agent as if it were thinking.
            let (gone_tx, gone_rx) = futures::channel::oneshot::channel::<()>();
            let mut gone = gone_rx.fuse();
            std::thread::Builder::new()
                .name("acp-watch".to_owned())
                .spawn(move || loop {
                    let ended = watched
                        .lock()
                        .map(|mut child| matches!(child.try_status(), Ok(Some(_))))
                        .unwrap_or(false);
                    if ended {
                        let _ = gone_tx.send(());
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(150));
                })
                .ok();
            loop {
                let order = futures::select! {
                    order = orders.next() => order,
                    _ = gone => None,
                };
                let Some(order) = order else {
                    // The process is gone, or the session was stopped: either way this task is over, and
                    // the connection with it.
                    break;
                };
                match order {
                    Order::Turn(text) => {
                        let turns = Arc::clone(&one_at_a_time);
                        let fresh = cx.clone();
                        let events = events.clone();
                        let session = opened.clone();
                        cx.spawn(async move {
                            let _alone = turns.lock().await;
                            let answer = fresh
                                .send_request(PromptRequest::new(
                                    session,
                                    vec![ContentBlock::Text(TextContent::new(text))],
                                ))
                                .block_task()
                                .await;
                            match answer {
                                Ok(answer) => {
                                    let _ =
                                        events.send(Event::Finished(word_of(&answer.stop_reason)));
                                }
                                Err(error) => {
                                    let _ = events.send(Event::Failed(words_of(&error)));
                                }
                            }
                            Ok(())
                        })?;
                    }
                    Order::Stop => break,
                    Order::Permission { id, option } => {
                        if let Some(Waiting::Permission(responder)) = take(&waiting, id) {
                            let _ = responder.respond(RequestPermissionResponse::new(outcome_of(
                                option.as_deref(),
                            )));
                        }
                    }
                    Order::Choice {
                        id,
                        field,
                        multiple,
                        chosen,
                    } => {
                        if let Some(Waiting::Choice(responder)) = take(&waiting, id) {
                            let action = match chosen {
                                Some(values) if !values.is_empty() => {
                                    let value = if multiple {
                                        ElicitationContentValue::StringArray(values)
                                    } else {
                                        ElicitationContentValue::String(values[0].clone())
                                    };
                                    ElicitationAction::Accept(
                                        ElicitationAcceptAction::new()
                                            .content(BTreeMap::from([(field, value)])),
                                    )
                                }
                                _ => ElicitationAction::Decline,
                            };
                            let _ = responder.respond(CreateElicitationResponse::new(action));
                        }
                    }
                    Order::Change { what, done } => {
                        // The change runs as a task of the loop's own and not on the loop itself: a switch of
                        // model over a large session is put to the person first (`session/request_permission`),
                        // and a loop sitting on the change could not carry their answer back to the agent.
                        let fresh = cx.clone();
                        let target = opened.clone();
                        let events = events.clone();
                        let settings = Arc::clone(&settings);
                        let wire = Arc::clone(&wire);
                        cx.spawn(async move {
                            let mut chosen: Option<String> = None;
                            let outcome = match what {
                                Change::Mode(mode) => fresh
                                    .send_request(SetSessionModeRequest::new(target.clone(), mode))
                                    .block_task()
                                    .await
                                    .map(|_| Vec::new())
                                    .map_err(|error| anyhow::anyhow!("{error}")),
                                Change::Model(model) => {
                                    // Written down as it goes: an agent's own method is not the standard's, and
                                    // whoever reads the record afterwards is the one who needs to know which road
                                    // was taken — and to see the day it stops being taken.
                                    wire.note(&format!(
                                        "the model is being changed with the agent's own method \
                                         (session/set_model), because it offers no setting for it: {model}"
                                    ));
                                    chosen = Some(model.clone());
                                    fresh
                                        .send_request(SetModelRaw {
                                            session_id: target.clone(),
                                            model_id: model,
                                        })
                                        .block_task()
                                        .await
                                        .map(|_| Vec::new())
                                        .map_err(|error| anyhow::anyhow!("{error}"))
                                }
                                Change::Setting { config, value } => fresh
                                    .send_request(SetSessionConfigOptionRequest::new(
                                        target.clone(),
                                        config,
                                        SessionConfigOptionValue::value_id(value),
                                    ))
                                    .block_task()
                                    .await
                                    .map(|answer| options_of(&json_of(&answer)))
                                    .map_err(|error| anyhow::anyhow!("{error}")),
                            };
                            // The record the panel reads between updates follows the change too: an agent that
                            // names its models in a field of its own sends no update for them, and the next
                            // broadcast about some other setting would otherwise rebuild the state from the
                            // model of the session's first breath — read in the panel as a switch back.
                            if let Ok(options) = &outcome
                                && let Ok(mut known) = settings.lock()
                            {
                                if !options.is_empty() {
                                    known.options = options.clone();
                                }
                                if let Some(model) = chosen {
                                    known.current_model = Some(model);
                                }
                                fill_models(&mut known);
                                if let Some(mode) = mode_reported_as_a_setting(&known) {
                                    known.current_mode = Some(mode);
                                }
                            }
                            match done {
                                Some(done) => {
                                    let _ = done.send(outcome);
                                }
                                None => {
                                    let said = match outcome {
                                        Ok(_) => settings
                                            .lock()
                                            .ok()
                                            .map(|known| Event::Setup(known.clone())),
                                        Err(error) => Some(Event::Failed(format!(
                                            "the agent would not take the change: {error}"
                                        ))),
                                    };
                                    if let Some(said) = said {
                                        let _ = events.send(said);
                                    }
                                }
                            }
                            Ok(())
                        })?;
                    }
                    Order::Open { session, done } => {
                        let outcome = match session {
                            Some(session) => {
                                match cx
                                    .send_request(kept_conversation(&cwd, &session))
                                    .block_task()
                                    .await
                                {
                                    Ok(answer) => Ok((session, true, session_state(&answer))),
                                    // Forgotten by the agent: a new conversation, not a rumour about an old
                                    // one — and the panel is told which of the two it got.
                                    Err(_) => cx
                                        .send_request(fresh_conversation(&cwd))
                                        .block_task()
                                        .await
                                        .and_then(|answer| {
                                            Ok((session_of(&answer)?, false, session_state(&answer)))
                                        })
                                        .map_err(|error| anyhow::anyhow!("{error}")),
                                }
                            }
                            None => cx
                                .send_request(fresh_conversation(&cwd))
                                .block_task()
                                .await
                                .and_then(|answer| {
                                    Ok((session_of(&answer)?, false, session_state(&answer)))
                                })
                                .map_err(|error| anyhow::anyhow!("{error}")),
                        };
                        // The conversation in hand is the one that was asked for, from here on.
                        if let Ok((fresh, _, state)) = &outcome {
                            opened = fresh.clone();
                            if let Ok(mut named) = id.lock() {
                                *named = fresh.clone();
                            }
                            if let Ok(mut known) = settings.lock() {
                                *known = state.clone();
                                fill_models(&mut known);
                            }
                        }
                        let _ = done.send(outcome);
                    }
                    Order::Sessions(done) => {
                        let mut all: Vec<SessionInfo> = Vec::new();
                        let mut cursor: Option<String> = None;
                        let mut trouble = None;
                        while all.len() < SESSIONS_KEPT {
                            // No directory filter: this window asks for every conversation the agent
                            // knows of, the same request the session made before the migration.
                            let request = ListSessionsRequest::new().cursor(cursor.clone());
                            match cx.send_request(request).block_task().await {
                                Ok(answer) => {
                                    let (mut page, next) = sessions_of(&json_of(&answer));
                                    if page.is_empty() {
                                        break;
                                    }
                                    all.append(&mut page);
                                    match next {
                                        Some(next) => cursor = Some(next),
                                        None => break,
                                    }
                                }
                                Err(error) => {
                                    trouble = Some(error);
                                    break;
                                }
                            }
                        }
                        let listed = match trouble {
                            Some(error) => Err(anyhow::anyhow!("{error}")),
                            None => Ok(all),
                        };
                        let _ = done.send(listed);
                    }
                    Order::Cancel => {
                        // Nothing is waited for: the answer to a stopped turn is the turn's own ending.
                        let _ = cx.send_notification(CancelNotification::new(opened.clone()));
                    }
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

/// One of the agent's own messages as JSON, for the readers this project already has.
///
/// The shapes of the standard and the shapes of the panel are the same JSON: the parsers in
/// `crate::protocol` were written for the wire, so the typed request is handed to them unchanged rather than
/// translated into a second set of types that could drift.
fn json_of<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// What a failed request is called in this project's own words.
///
/// The standard names its own reasons, and one of them is the one a person reading the panel needs most: the
/// agent's output closed, which means the process is gone. Its stable discriminator says so
/// ([`agent_client_protocol::is_incoming_transport_closed`]), so nothing is guessed from the text of a
/// message here — the words a person sees are this project's, and the test that asks for them
/// (`conversation.rs`: an agent that dies mid turn) is asked for a reason.
fn words_of(error: &agent_client_protocol::Error) -> String {
    if agent_client_protocol::is_incoming_transport_closed(error) {
        "the agent closed its output".to_owned()
    } else {
        format!("{error}")
    }
}

/// A refusal in the protocol's own words.
fn refusal(code: ErrorCode, message: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(code.into(), message.to_owned())
}

/// Reads a file the agent asked for, if it lives in the project.
fn serve_read(root: &Path, path: &Path) -> Result<String, (ErrorCode, String)> {
    let full = inside_project(root, path)?;
    std::fs::read_to_string(&full).map_err(|error| (ErrorCode::InternalError, error.to_string()))
}

/// Writes a file the agent asked for, once the panel has agreed to it.
fn serve_write(root: &Path, path: &Path, content: &str) -> Result<(), (ErrorCode, String)> {
    let full = inside_project(root, path)?;
    std::fs::write(&full, content).map_err(|error| (ErrorCode::InternalError, error.to_string()))
}

/// A path the agent named, if it lives in the project.
///
/// `..` and absolute paths outside the root are refused: an agent may reach the project it was started in,
/// and nothing else.
fn inside_project(root: &Path, path: &Path) -> Result<PathBuf, (ErrorCode, String)> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let full = joined.canonicalize().unwrap_or(joined);
    if !full.starts_with(&root) {
        return Err((
            ErrorCode::InvalidParams,
            format!(
                "{} is outside the project; this editor serves nothing else",
                path.display()
            ),
        ));
    }
    Ok(full)
}

/// The conversation the agent opened, out of its own answer.
fn session_of(answer: &serde_json::Value) -> anyhow::Result<String> {
    answer
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("the agent opened no conversation"))
}

/// Takes the question with that number, if it is still waiting.
fn take(waiting: &Arc<Mutex<HashMap<u64, Waiting>>>, id: u64) -> Option<Waiting> {
    waiting
        .lock()
        .ok()
        .and_then(|mut waiting| waiting.remove(&id))
}

/// The answer to a permission request: the option by its id, or a refusal.
fn outcome_of(option: Option<&str>) -> RequestPermissionOutcome {
    match option {
        Some(option) => {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option.to_owned()))
        }
        None => RequestPermissionOutcome::Cancelled,
    }
}

/// The setting the agent offers for the model, if it offers one.
fn model_setting(options: &[ConfigOption]) -> Option<&ConfigOption> {
    options
        .iter()
        .find(|setting| setting.category.as_deref() == Some(MODEL) || setting.id == MODEL)
}

/// The models the agent offers: the ones it named, or, when it names none, the choices of the setting it
/// offers for the model.
///
/// The standard has no `models` of its own — the model is a setting — while Hermes names them in a field of
/// its own. A panel whose model list is empty because the agent answered by the book is a panel that lost a
/// feature to a difference in spelling.
/// The mode an agent is in, read from the setting it carries modes in.
///
/// ACP has `current_mode_update` for this; Hermes never sends it, and reports the mode as its `edits`
/// setting instead — whose values (`ask`, `workspace_session`, `session`) are not the mode ids
/// (`default`, `accept_edits`, `dont_ask`). Both lists come from one table of names in the agent
/// (`acp_adapter/modes.py`), so the names are what can be matched: the setting says "Accept Edits",
/// and the mode of that name has id `accept_edits`.
fn mode_reported_as_a_setting(state: &SessionState) -> Option<String> {
    let option = state
        .options
        .iter()
        .find(|option| option.category.as_deref() == Some(crate::protocol::MODE))?;
    let chosen = option.current.as_deref()?;
    let named = option
        .choices
        .iter()
        .find(|choice| choice.value == chosen)
        .map(|choice| choice.name.as_str())?;
    state
        .modes
        .iter()
        .find(|mode| mode.name == named)
        .map(|mode| mode.id.clone())
}

fn fill_models(state: &mut SessionState) {
    let Some(setting) = model_setting(&state.options) else {
        return;
    };
    if state.models.is_empty() {
        state.models = setting
            .choices
            .iter()
            .map(|choice| Model {
                id: choice.value.clone(),
                name: choice.name.clone(),
            })
            .collect();
    }
    // The model is one of the agent's settings, and that setting carries the one in use, so the panel
    // follows it instead of freezing the first value it ever saw. A panel that froze it read the agent's
    // own update about its settings — sent again on every switch — as a change back to the model of the
    // session's first breath.
    if setting.current.is_some() {
        state.current_model = setting.current.clone();
    }
}

/// The standard's own `session/new`, asked for with the answer kept as the wire's own words.
///
/// The typed request drops whatever the schema has no room for, and one of those things is what Hermes sends
/// for its models: `models` is not in the standard, where the model is one of the agent's settings. A panel
/// whose model menu is empty because the agent answered with a field the schema does not know is a panel
/// that lost a feature to a spelling difference — so this one answer is read raw and handed to the parsers
/// this project already has.
#[derive(
    Debug, Clone, serde::Deserialize, serde::Serialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "session/new", response = serde_json::Value)]
struct NewSessionRaw {
    /// The directory the conversation is opened in.
    cwd: PathBuf,
    /// Servers this client brings: none — the agent's own tools are the ones it has.
    mcp_servers: Vec<serde_json::Value>,
    /// What this window says about itself when it opens a conversation.
    #[serde(rename = "_meta")]
    meta: serde_json::Value,
}

/// The standard's own `session/load`, raw for the same reason as [`NewSessionRaw`].
#[derive(
    Debug, Clone, serde::Deserialize, serde::Serialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "session/load", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
struct LoadSessionRaw {
    /// The conversation to pick up.
    session_id: String,
    /// The directory it is picked up in.
    cwd: PathBuf,
    /// Servers this client brings: none, as when a conversation is opened.
    mcp_servers: Vec<serde_json::Value>,
}

/// A fresh conversation, opened with the words this window has always used.
///
/// The note is not decoration: this window serves neither `fs/*` nor `terminal/*`, and an agent that is not
/// told works on the wrong machine — its own terminal is where its project is. Both halves of that promise
/// are asked after (`conversation.rs`): the capabilities in the handshake, and this note beside the directory.
fn fresh_conversation(cwd: &Path) -> NewSessionRaw {
    NewSessionRaw {
        cwd: cwd.to_path_buf(),
        mcp_servers: Vec::new(),
        meta: serde_json::json!({
            "client": "andy-ide",
            "fileAccess": "terminal",
            "note": "Files are not read or written for you by this client: use your own \
                     terminal for every file operation, in the cwd above."
        }),
    }
}

/// The conversation named, picked up with the same words.
fn kept_conversation(cwd: &Path, session: &str) -> LoadSessionRaw {
    LoadSessionRaw {
        session_id: session.to_owned(),
        cwd: cwd.to_path_buf(),
        mcp_servers: Vec::new(),
    }
}

/// Hermes' own `session/set_model`: the way back to a model the standard cannot reach.
///
/// The standard has no request of its own for the model — there the model is one of the agent's settings —
/// and Hermes 0.22.5 offers no such setting, naming its models in a field the schema does not know instead.
/// So its own method is asked for raw, and only when there is no standard road: the day the agent offers the
/// setting, this stops being called by itself. Every use is written into the record as it goes, so the day it
/// is no longer needed is a day that can be seen.
#[derive(
    Debug, Clone, serde::Deserialize, serde::Serialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "session/set_model", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
struct SetModelRaw {
    /// The conversation.
    session_id: String,
    /// The model the panel asked for.
    model_id: String,
}

/// How the agent named itself, for the record.
fn said_of(info: &Option<Implementation>) -> String {
    match info {
        Some(info) => format!("{} {}", info.name, info.version),
        None => "an agent that did not name itself".to_owned(),
    }
}

/// The turn's end in the agent's own word, which is what the panel shows.
fn word_of(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn".to_owned(),
        StopReason::Cancelled => "cancelled".to_owned(),
        StopReason::Refusal => "refusal".to_owned(),
        StopReason::MaxTokens => "max_tokens".to_owned(),
        StopReason::MaxTurnRequests => "max_turn_requests".to_owned(),
        other => format!("{other:?}"),
    }
}

/// One update, as the events the panel knows. `None` when it is nothing to show.
///
/// The settings are kept here as well as read: a change the panel was not expecting is still a change it
/// can show, and it hears about settings the way it heard about them when the session opened — one message
/// carrying all of them.
fn event_of(update: &SessionUpdate, settings: &Arc<Mutex<SessionState>>) -> Option<Event> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => text_of(&chunk.content).map(Event::Text),
        SessionUpdate::AgentThoughtChunk(chunk) => text_of(&chunk.content).map(Event::Thinking),
        SessionUpdate::UserMessageChunk(chunk) => text_of(&chunk.content).map(Event::You),
        SessionUpdate::Plan(plan) => Some(Event::Plan(
            plan.entries
                .iter()
                .map(|entry| entry.content.clone())
                .collect(),
        )),
        SessionUpdate::ToolCall(call) => Some(Event::ToolCall {
            id: call.tool_call_id.to_string(),
            title: call.title.clone(),
            status: status_of(&call.status),
        }),
        SessionUpdate::ToolCallUpdate(update) => Some(Event::ToolCall {
            id: update.tool_call_id.to_string(),
            title: update.fields.title.clone().unwrap_or_default(),
            status: update
                .fields
                .status
                .as_ref()
                .map(status_of)
                .unwrap_or_default(),
        }),
        SessionUpdate::AvailableCommandsUpdate(update) => Some(Event::Commands(
            update.available_commands.iter().map(command_of).collect(),
        )),
        SessionUpdate::UsageUpdate(update) => Some(Event::Usage(Usage {
            used: update.used,
            window: Some(update.size),
            account: crate::protocol::account_usage_of(&json_of(update)),
        })),
        SessionUpdate::ConfigOptionUpdate(update) => {
            let mut known = settings.lock().ok()?;
            known.options = options_of(&json_of(update));
            fill_models(&mut known);
            // Hermes reports a changed mode as a change of its `edits` setting and never sends
            // `current_mode_update`, so a panel that listened only for the mode notification kept
            // showing the mode a person had just asked to leave.
            if let Some(mode) = mode_reported_as_a_setting(&known) {
                known.current_mode = Some(mode);
            }
            Some(Event::Setup(known.clone()))
        }
        SessionUpdate::CurrentModeUpdate(update) => {
            let mut known = settings.lock().ok()?;
            known.current_mode = Some(update.current_mode_id.to_string());
            Some(Event::Setup(known.clone()))
        }
        other => Some(Event::Other {
            kind: kind_of(other),
        }),
    }
}

/// The words of a text chunk: anything else the protocol allows is not something the panel draws.
fn text_of(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

/// A tool call's standing, in the words the panel already uses.
fn status_of(status: &ToolCallStatus) -> String {
    match status {
        ToolCallStatus::Pending => "pending".to_owned(),
        ToolCallStatus::InProgress => "in_progress".to_owned(),
        ToolCallStatus::Completed => "completed".to_owned(),
        ToolCallStatus::Failed => "failed".to_owned(),
        other => format!("{other:?}"),
    }
}

/// One of the agent's commands, in the shape the panel offers.
fn command_of(command: &AvailableCommand) -> AgentCommand {
    AgentCommand {
        name: command.name.clone(),
        description: command.description.clone(),
        hint: match &command.input {
            Some(AvailableCommandInput::Unstructured(input)) => Some(input.hint.clone()),
            _ => None,
        },
    }
}

/// The name an update goes by on the wire, for the ones this panel does not show yet.
fn kind_of(update: &SessionUpdate) -> String {
    let word = match update {
        SessionUpdate::CurrentModeUpdate(_) => "current_mode_update",
        SessionUpdate::ConfigOptionUpdate(_) => "config_option_update",
        SessionUpdate::SessionInfoUpdate(_) => "session_info_update",
        SessionUpdate::Notice(_) => "notice",
        SessionUpdate::CompactionUpdate(_) => "compaction_update",
        SessionUpdate::CompactionSummaryChunk(_) => "compaction_summary_chunk",
        _ => "an update of a kind this panel does not know",
    };
    word.to_owned()
}
