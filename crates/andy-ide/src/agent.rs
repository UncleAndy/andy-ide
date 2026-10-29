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
    Event as AgentEvent, Mode, Model, PermissionPolicy, PermissionRequest, Registry, Session, Usage,
};

/// How long the worker waits for the next thing before checking whether it was told to go.
const PUMP: std::time::Duration = std::time::Duration::from_millis(20);

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
    /// Switch models, and say so through the status line.
    Model(String),
    /// Switch mode: how hard the agent thinks.
    Mode(String),
}

/// What the worker tells the panel.
enum Reply {
    /// The session is up, with this id.
    Ready(String),
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
#[derive(Default)]
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
    /// When the turn in flight began, and how long the last one took.
    started: Option<Instant>,
    pub took: Option<Duration>,
    /// Set while a turn is in flight: the panel disables the input rather than queueing.
    pub busy: bool,
    /// The question waiting for a person's answer.
    pub asking: Option<(u64, PermissionRequest)>,
    worker: Option<Worker>,
    root: Option<PathBuf>,
    /// Where the agents are kept for this project, once a project is known.
    place: Option<PathBuf>,
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
        self.busy = false;
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
        let (commands, jobs) = channel();
        let (answers, replies) = channel();
        let started = std::thread::Builder::new()
            .name(format!("agent-{name}"))
            .spawn(move || work(spec, root, jobs, answers));
        match started {
            Ok(_) => {
                self.worker = Some(Worker { commands, replies });
                self.chosen = Some(name.clone());
                self.status = format!("{name}: starting…");
            }
            Err(error) => self.status = format!("{name} did not start: {error}"),
        }
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
                    self.status = "the agent stopped".to_owned();
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
    }

    /// Files one reply from the worker.
    fn take(&mut self, reply: Reply) {
        match reply {
            Reply::Ready(id) => {
                self.status = format!("ready, session {id}");
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
                    self.status = "the agent closed its output".to_owned();
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
    commands: Receiver<Command>,
    replies: Sender<Reply>,
) {
    // `Ask` and not `Allow`: an agent that can write files without a question can undo
    // an afternoon, and the panel is where that question is asked.
    let mut session = match Session::start(&spec, &root, PermissionPolicy::Ask) {
        Ok(session) => session,
        Err(error) => {
            let _ = replies.send(Reply::Failed(format!("{error}")));
            return;
        }
    };
    if replies.send(Reply::Ready(session.id().to_owned())).is_err() {
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
