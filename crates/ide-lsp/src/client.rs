//! A language server as a child process: the transport, the handshake and the
//! typed requests the editor needs.
//!
//! The reading happens on its own thread, so a server that is slow, silent or
//! dead can never freeze the frame loop: the editor asks, waits with a timeout,
//! and gets an error it can show.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use lsp_types::{
    DocumentSymbol, GotoDefinitionResponse, Hover, InitializeResult, Location, Position,
    PublishDiagnosticsParams, ReferenceParams, SymbolInformation, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentPositionParams, Uri,
};
use serde_json::{Value, json};

use crate::framing::{read_message, write_message};
use crate::registry::ServerConfig;
use crate::rpc::{Incoming, Request, Response, classify};
use crate::uri::path_to_uri;

/// How long a plain request may take before the editor gives up on it.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The handshake gets longer: a cold rust-analyzer indexes the project first.
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(180);

/// A running server and the pipe to it.
pub struct ServerProcess {
    child: Child,
    /// Shared, because the reader thread has to answer requests of the server's own:
    /// a server that asked something and waits forever for the answer is a server
    /// that stops answering everything else behind it.
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: i64,
    pending: Arc<Mutex<HashMap<i64, Sender<Response>>>>,
    notifications: Receiver<(String, Option<Value>)>,
    alive: Arc<AtomicBool>,
    /// Set by the application when it is going away: a request that is waiting for
    /// an answer gives up instead of holding the caller for its whole patience.
    abort: Arc<AtomicBool>,
}

impl ServerProcess {
    /// Starts the server in `root`.
    pub fn spawn(config: &ServerConfig, root: &Path) -> Result<Self> {
        Self::spawn_with_abort(config, root, Arc::new(AtomicBool::new(false)))
    }

    /// The same, with a flag that ends a waiting request early.
    pub fn spawn_with_abort(
        config: &ServerConfig,
        root: &Path,
        abort: Arc<AtomicBool>,
    ) -> Result<Self> {
        let mut child = Command::new(config.command)
            .args(config.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A full stderr pipe would block the server once it fills up, so it
            // is drained on its own thread and turned into log lines.
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("cannot start {}", config.command))?;

        let stdin = Arc::new(Mutex::new(
            child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?,
        ));
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take();

        let pending: Arc<Mutex<HashMap<i64, Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (notification_sender, notifications) = channel();
        let alive = Arc::new(AtomicBool::new(true));

        let reader_pending = Arc::clone(&pending);
        let reader_notifications = notification_sender.clone();
        let reader_alive = Arc::clone(&alive);
        let reader_stdin = Arc::clone(&stdin);
        let name = config.name;
        std::thread::Builder::new()
            .name(format!("{name}-reader"))
            .spawn(move || {
                read_loop(
                    stdout,
                    reader_pending,
                    reader_notifications,
                    reader_alive,
                    reader_stdin,
                )
            })?;

        if let Some(stderr) = stderr {
            std::thread::Builder::new()
                .name(format!("{name}-stderr"))
                .spawn(move || {
                    use std::io::BufRead;
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        tracing::debug!(target: "ide_lsp", "{name}: {line}");
                    }
                })?;
        }

        Ok(Self {
            child,
            stdin,
            next_id: 0,
            pending,
            notifications,
            alive,
            abort,
        })
    }

    /// Whether the server is still running and talking to us.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Sends a request and waits for its answer.
    pub fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
    }

    /// The same, with an explicit patience.
    pub fn request_with_timeout(
        &mut self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let (sender, receiver) = channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("the pending table is poisoned"))?
            .insert(id, sender);

        let message = serde_json::to_value(Request::new(id, method, params))?;
        // The lock covers the write and nothing else: held across the wait for the answer
        // it would keep the reader thread from answering the server, and a server that
        // asked us something waits before it answers anything else — a deadlock of our
        // own making, which is how a request that works alone times out under load.
        {
            let mut stdin = self
                .stdin
                .lock()
                .map_err(|_| anyhow!("the server's input is gone"))?;
            write_message(&mut *stdin, &message)
                .with_context(|| format!("cannot send {method}"))?;
        }

        // The wait is taken in short steps so that a request cannot keep the caller
        // (and the window it is closing) for the whole patience: a server that is
        // busy indexing answers when it answers, and leaving must not wait for it.
        const STEP: Duration = Duration::from_millis(25);
        let deadline = Instant::now() + timeout;
        let response = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(left.min(STEP)) {
                Ok(response) => break response,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if let Ok(mut pending) = self.pending.lock() {
                        pending.remove(&id);
                    }
                    bail!("{method} got no answer: the server is gone");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.abort.load(Ordering::Relaxed) {
                        if let Ok(mut pending) = self.pending.lock() {
                            pending.remove(&id);
                        }
                        bail!("{method} was abandoned: the application is closing");
                    }
                    if Instant::now() >= deadline {
                        if let Ok(mut pending) = self.pending.lock() {
                            pending.remove(&id);
                        }
                        bail!("{method} got no answer within {timeout:?}");
                    }
                }
            }
        };

        if let Some(error) = response.error {
            bail!("{method} failed: {error}");
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    /// Sends a message that expects no answer.
    pub fn notify(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let message = serde_json::to_value(crate::rpc::Notification::new(method, params))?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| anyhow!("the server's input is gone"))?;
        write_message(&mut *stdin, &message).with_context(|| format!("cannot send {method}"))
    }

    /// Takes one notification the server sent, if any.
    pub fn try_notification(&self) -> Option<(String, Option<Value>)> {
        self.notifications.try_recv().ok()
    }

    /// Sends every notification received so far.
    pub fn drain_notifications(&self) -> Vec<(String, Option<Value>)> {
        self.notifications.try_iter().collect()
    }

    /// Asks the server to shut down and waits for the process to go.
    pub fn shutdown(&mut self) {
        // Leaving: a polite goodbye is a request, and a server that is busy indexing
        // would make the window wait five more seconds for nothing.
        if !self.abort.load(Ordering::Relaxed) {
            let _ = self.request_with_timeout("shutdown", None, Duration::from_secs(5));
        }
        let _ = self.notify("exit", None);
        // A server that ignores both is not worth waiting for.
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        // Dropping the stdin closes the pipe, which most servers treat as exit.
        if let Ok(mut stdin) = self.stdin.lock() {
            let _ = stdin.flush();
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Reads messages until the pipe closes, handing answers and notifications on.
fn read_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<i64, Sender<Response>>>>,
    notifications: Sender<(String, Option<Value>)>,
    alive: Arc<AtomicBool>,
    stdin: Arc<Mutex<ChildStdin>>,
) {
    // The flag goes down however the loop ends, including a panic in a parser.
    struct Lower(Arc<AtomicBool>);
    impl Drop for Lower {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Relaxed);
        }
    }
    let _lower = Lower(alive);

    let mut reader = BufReader::new(stdout);
    loop {
        let value = match read_message(&mut reader) {
            Ok(Some(value)) => value,
            Ok(None) => return,
            Err(error) => {
                tracing::debug!(target: "ide_lsp", %error, "the server pipe stopped");
                return;
            }
        };
        if !forward(value, &pending, &notifications, &stdin) {
            return;
        }
    }
}

/// Hands one message to whoever waits for it; `false` when nobody is left.
fn forward(
    value: Value,
    pending: &Arc<Mutex<HashMap<i64, Sender<Response>>>>,
    notifications: &Sender<(String, Option<Value>)>,
    stdin: &Arc<Mutex<ChildStdin>>,
) -> bool {
    match classify(value) {
        Some(Incoming::Response(response)) => {
            if let Some(id) = response.numeric_id()
                && let Ok(mut pending) = pending.lock()
                && let Some(sender) = pending.remove(&id)
            {
                let _ = sender.send(response);
            }
            true
        }
        Some(Incoming::Notification { method, params }) => {
            notifications.send((method, params)).is_ok()
        }
        Some(Incoming::ServerRequest { id, method, params }) => {
            // A request of the server's has to be answered, and answered *on the wire*.
            // Building the reply and dropping it was the bug that made a server stall:
            // rust-analyzer asks `workspace/configuration` the first time it needs a
            // setting, and waits for the answer before it answers anything else — so
            // every later request, highlights among them, was never served.
            let reply = match method.as_str() {
                // The protocol wants one entry per item asked about; `null` for each is
                // "no configuration for this one", which is the truth here.
                "workspace/configuration" => {
                    let count = params
                        .as_ref()
                        .and_then(|params| params.get("items"))
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or_default();
                    json!({"jsonrpc": "2.0", "id": id, "result": vec![Value::Null; count]})
                }
                _ => json!({"jsonrpc": "2.0", "id": id, "result": null}),
            };
            match stdin.lock() {
                Ok(mut stdin) => {
                    if let Err(error) = write_message(&mut *stdin, &reply) {
                        tracing::debug!(target: "ide_lsp", %error, "cannot answer {method}");
                        return false;
                    }
                }
                Err(_) => return false,
            }
            tracing::debug!(target: "ide_lsp", %method, "answered a server request");
            true
        }
        None => true,
    }
}

/// A started server with the handshake done, ready for document questions.
pub struct LspSession {
    process: ServerProcess,
    root: PathBuf,
    name: &'static str,
    language_id: &'static str,
    versions: HashMap<PathBuf, i32>,
}

impl LspSession {
    /// Starts the server and performs `initialize` / `initialized`.
    pub fn start(config: &'static ServerConfig, root: &Path) -> Result<Self> {
        Self::start_with_abort(config, root, Arc::new(AtomicBool::new(false)))
    }

    /// The same, with a flag that ends a waiting request when the application leaves.
    pub fn start_with_abort(
        config: &'static ServerConfig,
        root: &Path,
        abort: Arc<AtomicBool>,
    ) -> Result<Self> {
        let mut process = ServerProcess::spawn_with_abort(config, root, abort)?;
        // The handshake is built as JSON rather than through the struct: the
        // struct still carries the deprecated root_uri, and servers want the
        // workspace folders too.
        let root_uri = path_to_uri(root);
        let params = json!({
            "processId": std::process::id() as i64,
            "rootUri": root_uri,
            "workspaceFolders": root_uri
                .map(|uri| json!([{ "uri": uri, "name": config.name }]))
                .unwrap_or(Value::Null),
            "capabilities": {
                "textDocument": {
                    "definition": { "linkSupport": true },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "references": {}
                },
                "workspace": { "workspaceFolders": true }
            }
        });
        let result = process
            .request_with_timeout("initialize", Some(params), INITIALIZE_TIMEOUT)
            .context("initialize failed")?;
        let _: InitializeResult =
            serde_json::from_value(result).context("initialize returned something unexpected")?;
        process.notify("initialized", Some(json!({})))?;

        Ok(Self {
            process,
            root: root.to_path_buf(),
            name: config.name,
            language_id: config.language_id,
            versions: HashMap::new(),
        })
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn is_alive(&self) -> bool {
        self.process.is_alive()
    }

    /// Tells the server about a document the editor just opened.
    pub fn did_open(&mut self, path: &Path, text: &str) -> Result<()> {
        let uri = self.uri(path)?;
        let version = self.bump(path);
        let params = json!({
            "textDocument": TextDocumentItem {
                uri,
                language_id: self.language_id.to_owned(),
                version,
                text: text.to_owned(),
            }
        });
        self.process.notify("textDocument/didOpen", Some(params))
    }

    /// Sends the whole text again. A change without a range replaces the
    /// document, which every server understands whatever sync kind it asked for.
    pub fn did_change(&mut self, path: &Path, text: &str) -> Result<()> {
        let uri = self.uri(path)?;
        let version = self.bump(path);
        let params = json!({
            "textDocument": TextDocumentIdentifier { uri },
            "contentChanges": [{ "text": text }],
            "version": version,
        });
        self.process.notify("textDocument/didChange", Some(params))
    }

    pub fn did_close(&mut self, path: &Path) -> Result<()> {
        let uri = self.uri(path)?;
        self.versions.remove(path);
        let params = json!({ "textDocument": TextDocumentIdentifier { uri } });
        self.process.notify("textDocument/didClose", Some(params))
    }

    /// Where the symbol under the cursor is defined.
    pub fn definition(&mut self, path: &Path, position: Position) -> Result<Vec<Location>> {
        let result = self.positioned_request("textDocument/definition", path, position)?;
        Ok(definition_locations(result))
    }

    /// Every place the symbol under the cursor is used, the definition included.
    pub fn references(&mut self, path: &Path, position: Position) -> Result<Vec<Location>> {
        let uri = self.uri(path)?;
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: lsp_types::ReferenceContext {
                include_declaration: true,
            },
        };
        let result = self.process.request(
            "textDocument/references",
            Some(serde_json::to_value(params)?),
        )?;
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    /// The symbols of one file, for the structure panel.
    pub fn document_symbols(&mut self, path: &Path) -> Result<Vec<DocumentSymbol>> {
        let uri = self.uri(path)?;
        let params = json!({ "textDocument": TextDocumentIdentifier { uri } });
        let result = self
            .process
            .request("textDocument/documentSymbol", Some(params))?;
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    /// The symbols of the whole workspace whose name matches `query`.
    ///
    /// An empty query means "everything", which is what `Ctrl+Shift+F12` asks
    /// for. Servers that do not implement `workspace/symbol` return an error;
    /// the caller shows it in the status bar rather than failing silently.
    pub fn workspace_symbols(&mut self, query: &str) -> Result<Vec<SymbolInformation>> {
        let params = json!({ "query": query });
        let result = self.process.request("workspace/symbol", Some(params))?;
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    /// The type and documentation under the cursor.
    pub fn hover(&mut self, path: &Path, position: Position) -> Result<Option<Hover>> {
        let result = self.positioned_request("textDocument/hover", path, position)?;
        Ok(serde_json::from_value(result).ok().flatten())
    }

    /// The edits that rename the symbol at `position` to `new_name`.
    ///
    /// A server answers with a workspace edit: every file it touches and every
    /// range in it. The ranges are positions of the protocol, not bytes, and the
    /// caller turns them into bytes of the text it is about to change.
    pub fn rename(
        &mut self,
        path: &Path,
        position: Position,
        new_name: &str,
    ) -> Result<Option<lsp_types::WorkspaceEdit>> {
        let uri = self.uri(path)?;
        let params = lsp_types::RenameParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            new_name: new_name.to_owned(),
            work_done_progress_params: Default::default(),
        };
        let result = self
            .process
            .request("textDocument/rename", Some(serde_json::to_value(params)?))?;
        Ok(serde_json::from_value(result).ok().flatten())
    }

    /// Every place the symbol at `position` is used in this file.
    ///
    /// A server may answer `null` instead of an empty list, so both are one.
    pub fn document_highlights(
        &mut self,
        path: &Path,
        position: Position,
    ) -> Result<Vec<lsp_types::DocumentHighlight>> {
        let result = self.positioned_request("textDocument/documentHighlight", path, position)?;
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    /// Diagnostics the server published since the last call.
    pub fn drain_diagnostics(&self) -> Vec<PublishDiagnosticsParams> {
        self.process
            .drain_notifications()
            .into_iter()
            .filter(|(method, _)| method == "textDocument/publishDiagnostics")
            .filter_map(|(_, params)| serde_json::from_value(params?).ok())
            .collect()
    }

    /// Asks the server to stop; safe to call twice.
    pub fn shutdown(&mut self) {
        self.process.shutdown();
    }

    fn positioned_request(
        &mut self,
        method: &str,
        path: &Path,
        position: Position,
    ) -> Result<Value> {
        let uri = self.uri(path)?;
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        };
        self.process
            .request(method, Some(serde_json::to_value(params)?))
    }

    fn uri(&self, path: &Path) -> Result<Uri> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        path_to_uri(&absolute).ok_or_else(|| anyhow!("cannot make a uri of {}", absolute.display()))
    }

    fn bump(&mut self, path: &Path) -> i32 {
        let version = self.versions.entry(path.to_path_buf()).or_insert(0);
        *version += 1;
        *version
    }
}

/// `textDocument/definition` may answer with one location, a list, or links.
fn definition_locations(result: Value) -> Vec<Location> {
    if let Ok(response) = serde_json::from_value::<GotoDefinitionResponse>(result.clone()) {
        return match response {
            GotoDefinitionResponse::Scalar(location) => vec![location],
            GotoDefinitionResponse::Array(locations) => locations,
            GotoDefinitionResponse::Link(links) => links
                .into_iter()
                .map(|link| Location {
                    uri: link.target_uri,
                    range: link.target_selection_range,
                })
                .collect(),
        };
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Range, Uri};

    fn location(line: u32) -> Value {
        json!({
            "uri": "file:///a.rs",
            "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 5}}
        })
    }

    #[test]
    fn one_location_is_a_list_of_one() {
        let locations = definition_locations(location(3));
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start.line, 3);
    }

    #[test]
    fn a_list_stays_a_list() {
        let locations = definition_locations(json!([location(1), location(2)]));
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[1].range.start.line, 2);
    }

    #[test]
    fn links_become_locations() {
        let locations = definition_locations(json!([{
            "targetUri": "file:///b.rs",
            "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
            "targetSelectionRange": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 8}}
        }]));
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start.line, 2);
        assert_eq!(locations[0].uri.as_str(), "file:///b.rs");
    }

    #[test]
    fn a_null_answer_is_no_locations() {
        assert!(definition_locations(Value::Null).is_empty());
    }

    #[test]
    fn a_range_round_trips_through_json() {
        // The shape the editor will hand back when it jumps to an answer.
        let text =
            json!({"start": {"line": 1, "character": 2}, "end": {"line": 3, "character": 4}});
        let range: Range = serde_json::from_value(text).unwrap();
        assert_eq!(range.start.character, 2);
        let uri: Uri = "file:///x.rs".parse().unwrap();
        assert_eq!(uri.as_str(), "file:///x.rs");
    }
}
