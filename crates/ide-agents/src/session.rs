//! One agent process, spoken to over its standard input and output.
//!
//! The shape of this module is dictated by one rule: nothing may block the window. All
//! the waiting happens on a thread of its own — one reading the child's output, one
//! writing to its input — and the editor asks for events and for answers, never for a
//! whole turn. A turn can take minutes; a frame has sixteen milliseconds.
//!
//! Reads are not the agent's idea of time either. Every wait here has a deadline, and a
//! process that dies, closes its output or stops answering is reported as such instead
//! of being waited for forever.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::protocol::{
    ClientCapabilities, FsCapabilities, Message, PermissionRequest, Update, initialize_params,
    new_session_params, permission_answer, prompt_params,
};

/// How long the agent gets to start and say hello.
pub const HELLO: Duration = Duration::from_secs(60);

/// How long one request may go unanswered before the caller is told so.
pub const ANSWER: Duration = Duration::from_secs(120);

/// How much of the agent's own error output is kept, for when it fails.
const STDERR_KEPT: usize = 8000;

/// Which agent to run, and how.
///
/// The command is data, not code: a second agent is a line in `agents.toml`, and the
/// editor does not have to learn anything about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    /// The name shown in the panel.
    pub name: String,
    /// The program to run.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Extra environment, as written in the configuration.
    pub env: BTreeMap<String, String>,
}

/// What the client does when the agent asks for permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Allow the first time the agent asks, which is what a test wants.
    Allow,
    /// Refuse everything, which is the safe default.
    #[default]
    Deny,
    /// Ask the person in the panel: the request is handed over and the agent waits.
    Ask,
}

/// Something the agent said or did, for the panel to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A piece of the answer, to append.
    Text(String),
    /// A piece of the agent's reasoning, to be kept apart from the answer and folded
    /// away: a person reading an answer wants the answer, and wants the reasoning to
    /// be there when they ask for it.
    Thinking(String),
    /// A tool call, started or finished.
    ToolCall {
        /// The agent's id for it.
        id: String,
        /// Its title.
        title: String,
        /// `pending`, `in_progress`, `completed`, `failed`.
        status: String,
    },
    /// The plan the agent is following.
    Plan(Vec<String>),
    /// An update this version does not show.
    Other {
        /// The tag it arrived with.
        kind: String,
    },
    /// The agent wants permission; the answer goes back through
    /// [`Session::answer_permission`].
    Permission {
        /// The request id, to answer with.
        id: u64,
        /// What is being asked.
        request: PermissionRequest,
    },
    /// The turn ended, in the agent's own word: `end_turn`, `refusal`, `cancelled`.
    Finished(String),
    /// The agent closed its output: the process is gone.
    Closed,
    /// Its own words, when it failed.
    Failed(String),
}

/// What the reader thread sends to the session.
enum Incoming {
    /// One parsed line.
    Line(Message),
    /// A line that is not JSON-RPC.
    Garbage(String),
    /// The child closed its output.
    Closed,
}

/// A running agent, from the editor's side of the pipe.
pub struct Session {
    child: Child,
    outgoing: Sender<Message>,
    incoming: Receiver<Incoming>,
    /// Events read while waiting for an answer, for the panel to catch up on.
    /// A queue and not a stack: the pieces of an answer are only worth anything in
    /// the order the agent wrote them.
    waiting: std::collections::VecDeque<Event>,
    next_id: u64,
    session_id: String,
    policy: PermissionPolicy,
    root: PathBuf,
    allow_writes: bool,
    stderr: Arc<Mutex<String>>,
    closed: bool,
    /// The request id of the turn in flight, so its answer can end the turn.
    turn: Option<u64>,
}

impl Session {
    /// Starts an agent in `root` and speaks the protocol with it.
    ///
    /// `initialize` and `session/new` are done here, so a `Session` that exists is a
    /// session that has answered: every failure to start is a failure to build one.
    pub fn start(spec: &AgentSpec, root: &Path, policy: PermissionPolicy) -> Result<Self> {
        let mut command = Command::new(&spec.command);
        command
            .args(&spec.args)
            .envs(&spec.env)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .with_context(|| format!("running {} for agent {}", spec.command, spec.name))?;

        let stdin = child.stdin.take().context("the agent has no input")?;
        let stdout = child.stdout.take().context("the agent has no output")?;
        let stderr = child
            .stderr
            .take()
            .context("the agent has no error output")?;

        let (outgoing, jobs) = channel::<Message>();
        let (lines, incoming) = channel::<Incoming>();

        // Writing: one thread owns the child's input, so a slow agent slows nobody else.
        std::thread::Builder::new()
            .name(format!("{}-in", spec.name))
            .spawn(move || {
                let mut stdin = stdin;
                for message in jobs {
                    let line = message.to_line();
                    if writeln!(stdin, "{line}").is_err() || stdin.flush().is_err() {
                        break;
                    }
                }
            })
            .ok();

        // Reading: one message per line, and the child's broken lines are reported
        // rather than skipped — a protocol error the editor cannot see is one it cannot
        // blame on anybody.
        std::thread::Builder::new()
            .name(format!("{}-out", spec.name))
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Message>(&line) {
                        Ok(message) => {
                            if lines.send(Incoming::Line(message)).is_err() {
                                return;
                            }
                        }
                        Err(_) => {
                            if lines.send(Incoming::Garbage(line)).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = lines.send(Incoming::Closed);
            })
            .ok();

        // The agent's own complaints: kept, so a process that dies at once can say why.
        let kept = Arc::new(Mutex::new(String::new()));
        let written = Arc::clone(&kept);
        std::thread::Builder::new()
            .name(format!("{}-err", spec.name))
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(mut text) = written.lock() {
                        text.push_str(&line);
                        text.push('\n');
                        if text.len() > STDERR_KEPT {
                            let cut = text.len() - STDERR_KEPT;
                            text.drain(..cut);
                        }
                    }
                }
            })
            .ok();

        let mut session = Self {
            child,
            outgoing,
            incoming,
            waiting: std::collections::VecDeque::new(),
            next_id: 1,
            session_id: String::new(),
            policy,
            root: root.to_path_buf(),
            allow_writes: false,
            stderr: kept,
            closed: false,
            turn: None,
        };

        session.hello()?;
        Ok(session)
    }

    /// The id ACP gave this conversation.
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// Allows `fs/write_text_file` to be served.
    ///
    /// Off until the panel has asked: an agent that can write files without the editor's
    /// consent is an agent that can undo a person's afternoon.
    pub fn allow_writes(&mut self, allow: bool) {
        self.allow_writes = allow;
    }

    /// `initialize`, then `session/new` — the two requests that make a conversation.
    fn hello(&mut self) -> Result<()> {
        let capabilities = ClientCapabilities {
            fs: FsCapabilities {
                read_text_file: true,
                write_text_file: true,
            },
            terminal: false,
        };
        let answer = self.ask("initialize", initialize_params(&capabilities), HELLO)?;
        let _version = answer
            .get("protocolVersion")
            .cloned()
            .unwrap_or(Value::Null);

        let answer = self.ask("session/new", new_session_params(&self.root), HELLO)?;
        let id = answer
            .get("sessionId")
            .and_then(Value::as_str)
            .context("the agent opened no session")?;
        self.session_id = id.to_owned();
        Ok(())
    }

    /// Starts a turn without waiting for it: the events arrive through
    /// [`Session::next_event`], and `Event::Finished` ends it.
    ///
    /// A chat panel cannot spend a turn blocking: the answer has to be drawn while it
    /// is being written, so the request is sent here and the rest is the event loop.
    pub fn start_turn(&mut self, text: &str) -> Result<()> {
        let id = self.session_id.clone();
        let request = self.next_id;
        self.next_id += 1;
        self.send(Message::request(
            request,
            "session/prompt",
            prompt_params(&id, text),
        ))?;
        self.turn = Some(request);
        Ok(())
    }

    /// Sends a prompt and waits for the turn to end, keeping every update on the way.
    ///
    /// The stop reason is the agent's own word: `end_turn`, `max_tokens`,
    /// `refusal`, `cancelled`. A turn that ends in refusal is not an error of the
    /// protocol, and the panel shows it as what the agent said.
    pub fn prompt(&mut self, text: &str, timeout: Duration) -> Result<String> {
        let id = self.session_id.clone();
        let answer = self.ask("session/prompt", prompt_params(&id, text), timeout)?;
        Ok(answer
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or("end_turn")
            .to_owned())
    }

    /// The next thing the agent said, waiting up to `timeout`.
    pub fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        if let Some(event) = self.waiting.pop_front() {
            return Some(event);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match self.incoming.recv_timeout(left) {
                Ok(incoming) => {
                    if let Some(event) = self.handle(incoming) {
                        return Some(event);
                    }
                    if let Some(event) = self.waiting.pop_front() {
                        return Some(event);
                    }
                    if self.closed {
                        return Some(Event::Closed);
                    }
                }
                Err(_) => return None,
            }
        }
    }

    /// Answers a permission request that was handed over with [`Event::Permission`].
    pub fn answer_permission(&mut self, id: u64, option: Option<&str>) -> Result<()> {
        self.send(Message::result(id, permission_answer(option)))
    }

    /// Ends the conversation: the input is closed, then the process is killed.
    ///
    /// Closing the input first gives a well-behaved agent the chance to leave on its own
    /// terms; the kill is for the ones that do not.
    pub fn stop(&mut self) {
        // Replacing the sender with one whose receiver is gone closes the queue, and the
        // writer thread then drops the child's input.
        let (dead, _) = channel();
        self.outgoing = dead;
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// The agent's own error output, when something went wrong.
    pub fn complaints(&self) -> String {
        self.stderr
            .lock()
            .map(|text| text.trim().to_owned())
            .unwrap_or_default()
    }

    /// Sends one message to the writer thread.
    fn send(&mut self, message: Message) -> Result<()> {
        if self.closed {
            bail!("the agent has closed its output");
        }
        self.outgoing
            .send(message)
            .context("the agent is no longer reading its input")
    }

    /// A request, with a deadline, handling whatever arrives in the meantime.
    fn ask(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(Message::request(id, method, params))?;

        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let complaints = self.complaints();
                bail!(
                    "the agent did not answer {method} in {} s{}",
                    timeout.as_secs(),
                    if complaints.is_empty() {
                        String::new()
                    } else {
                        format!(": {complaints}")
                    }
                );
            }
            match self.incoming.recv_timeout(left) {
                Ok(incoming) => match incoming {
                    Incoming::Line(Message::Response {
                        id: answer_id,
                        result,
                        error,
                        ..
                    }) if answer_id == id => {
                        return match (result, error) {
                            (_, Some(error)) => {
                                bail!("the agent refused {method}: {}", error.message)
                            }
                            (Some(result), None) => Ok(result),
                            (None, None) => Ok(Value::Null),
                        };
                    }
                    Incoming::Line(other) => {
                        if let Some(event) = self.handle(Incoming::Line(other)) {
                            self.waiting.push_back(event);
                        }
                        if self.closed {
                            let complaints = self.complaints();
                            bail!(
                                "the agent closed its output while {method} was in flight{}",
                                if complaints.is_empty() {
                                    String::new()
                                } else {
                                    format!(": {complaints}")
                                }
                            );
                        }
                    }
                    Incoming::Garbage(line) => {
                        self.waiting.push_back(Event::Failed(format!(
                            "the agent wrote something that is not a message: {line}"
                        )));
                    }
                    Incoming::Closed => {
                        self.closed = true;
                        let complaints = self.complaints();
                        bail!(
                            "the agent closed its output while {method} was in flight{}",
                            if complaints.is_empty() {
                                String::new()
                            } else {
                                format!(": {complaints}")
                            }
                        );
                    }
                },
                Err(_) => {
                    bail!(
                        "the agent did not answer {method} in {} s",
                        timeout.as_secs()
                    )
                }
            }
        }
    }

    /// Deals with one line from the agent: a request to answer, or an update to keep.
    fn handle(&mut self, incoming: Incoming) -> Option<Event> {
        let message = match incoming {
            Incoming::Line(message) => message,
            Incoming::Garbage(line) => {
                return Some(Event::Failed(format!(
                    "the agent wrote something that is not a message: {line}"
                )));
            }
            Incoming::Closed => {
                self.closed = true;
                return Some(Event::Closed);
            }
        };

        match message {
            Message::Request {
                id,
                method,
                params: Some(params),
                ..
            } => {
                self.answer_request(id, &method, &params);
                if method == "session/request_permission" {
                    // The request is now in `waiting`, either as a question for the panel
                    // or answered already; the caller may want to see it.
                    return self.waiting.pop_front();
                }
                None
            }
            Message::Request { id, method, .. } => {
                self.fail(id, -32602, &format!("{method} without parameters"));
                None
            }
            Message::Notification { method, params, .. } => {
                if method != "session/update" {
                    return None;
                }
                let params = params.unwrap_or(Value::Null);
                let update = params.get("update")?;
                match Update::from_value(update) {
                    Update::Text(text) => Some(Event::Text(text)),
                    Update::Thinking(thought) => Some(Event::Thinking(thought)),
                    Update::ToolCall { id, title, status } => {
                        Some(Event::ToolCall { id, title, status })
                    }
                    Update::Plan(steps) => Some(Event::Plan(steps)),
                    Update::Other { kind, .. } => Some(Event::Other { kind }),
                }
            }
            // The answer to the turn in flight ends it; any other answer belongs to a
            // wait that has already given up, and is of no use to anybody.
            Message::Response {
                id, result, error, ..
            } => {
                if self.turn != Some(id) {
                    return None;
                }
                self.turn = None;
                match (result, error) {
                    (_, Some(error)) => Some(Event::Failed(format!(
                        "the agent refused the turn: {}",
                        error.message
                    ))),
                    (Some(result), None) => Some(Event::Finished(
                        result
                            .get("stopReason")
                            .and_then(Value::as_str)
                            .unwrap_or("end_turn")
                            .to_owned(),
                    )),
                    (None, None) => Some(Event::Finished("end_turn".to_owned())),
                }
            }
        }
    }

    /// Answers a request the agent made of the client.
    fn answer_request(&mut self, id: u64, method: &str, params: &Value) {
        match method {
            "fs/read_text_file" => self.read_file(id, params),
            "fs/write_text_file" => self.write_file(id, params),
            "session/request_permission" => self.ask_permission(id, params),
            "terminal/create"
            | "terminal/output"
            | "terminal/wait_for_exit"
            | "terminal/kill"
            | "terminal/release" => {
                // Claiming no terminal capability is honest; answering as if there were
                // one would be a promise this client cannot keep yet.
                self.fail(id, -32601, "this editor offers no terminal to agents yet");
            }
            other => {
                self.fail(id, -32601, &format!("this client does not know {other}"));
            }
        }
    }

    /// Serves `fs/read_text_file`, for paths inside the project and no others.
    fn read_file(&mut self, id: u64, params: &Value) {
        let Some(path) = params.get("path").and_then(Value::as_str) else {
            self.fail(id, -32602, "a path is required");
            return;
        };
        match self.inside_project(path) {
            Ok(full) => match std::fs::read_to_string(&full) {
                Ok(text) => {
                    let _ = self.send(Message::result(id, json!({"content": text})));
                }
                Err(error) => self.fail(id, -32603, &format!("{error}")),
            },
            Err(refusal) => self.fail(id, -32602, &refusal),
        }
    }

    /// Serves `fs/write_text_file`, once the editor has agreed to it.
    fn write_file(&mut self, id: u64, params: &Value) {
        if !self.allow_writes {
            self.fail(
                id,
                -32601,
                "the editor has not allowed the agent to write files",
            );
            return;
        }
        let Some(path) = params.get("path").and_then(Value::as_str) else {
            self.fail(id, -32602, "a path is required");
            return;
        };
        let Some(content) = params.get("content").and_then(Value::as_str) else {
            self.fail(id, -32602, "the write carries no content");
            return;
        };
        match self.inside_project(path) {
            Ok(full) => {
                if let Some(parent) = full.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&full, content) {
                    Ok(()) => {
                        let _ = self.send(Message::result(id, Value::Null));
                    }
                    Err(error) => self.fail(id, -32603, &format!("{error}")),
                }
            }
            Err(refusal) => self.fail(id, -32602, &refusal),
        }
    }

    /// Puts a permission request to the policy, or to the panel.
    fn ask_permission(&mut self, id: u64, params: &Value) {
        let request = PermissionRequest::from_params(params);
        let choice = match self.policy {
            PermissionPolicy::Allow => request
                .option_of("allow_once")
                .or_else(|| request.option_of("allow_always")),
            PermissionPolicy::Deny => request
                .option_of("reject_once")
                .or_else(|| request.option_of("reject_always")),
            PermissionPolicy::Ask => {
                self.waiting.push_back(Event::Permission { id, request });
                return;
            }
        };
        let _ = self.send(Message::result(id, permission_answer(choice)));
    }

    /// A path the agent named, if it lives in the project.
    ///
    /// `..` and absolute paths outside the root are refused: an agent may read the
    /// project it was started in, and nothing else.
    fn inside_project(&self, path: &str) -> std::result::Result<PathBuf, String> {
        let named = Path::new(path);
        let joined = if named.is_absolute() {
            named.to_path_buf()
        } else {
            self.root.join(named)
        };
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let full = joined.canonicalize().unwrap_or(joined);
        if !full.starts_with(&root) {
            return Err(format!(
                "{path} is outside the project; this editor serves nothing else"
            ));
        }
        Ok(full)
    }

    /// Refuses a request, in the protocol's own words.
    fn fail(&mut self, id: u64, code: i64, message: &str) {
        let _ = self.send(Message::failure(id, code, message));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}
