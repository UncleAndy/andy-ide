//! The language server as the frame loop uses it: a worker thread with a job
//! queue.
//!
//! Requests block (a definition can take a second while the server is still
//! indexing), so they never run on the drawing thread. The application posts a
//! job, keeps drawing, and picks the answer up from [`LanguageService::poll`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ide_lsp::registry::ServerConfig;
use ide_lsp::{LspSession, uri_to_path};
use lsp_types::{DocumentSymbol, Location, Position, SymbolInformation};

/// How long the window waits for the worker to go on its own before leaving it.
///
/// A server in the middle of a request cannot be interrupted, and waiting for one
/// held the window open for tens of seconds on the way out. The worker is an
/// ordinary thread with a process behind it: the program is leaving anyway, and
/// the server sees end-of-file on its stdin once the pipes close.
const WORKER_GRACE: Duration = Duration::from_millis(400);

/// Something the editor wants from the server.
#[derive(Debug)]
pub enum Job {
    Open {
        path: PathBuf,
        text: String,
    },
    Change {
        path: PathBuf,
        text: String,
    },
    Close {
        path: PathBuf,
    },
    Definition {
        path: PathBuf,
        position: Position,
    },
    References {
        path: PathBuf,
        position: Position,
    },
    /// What the symbol at one position is: signature and documentation.
    Hover {
        path: PathBuf,
        position: Position,
    },
    /// Where the symbol at one position is used in this file.
    Highlights {
        path: PathBuf,
        position: Position,
    },
    /// Rename the symbol at `position` to `new_name`.
    Rename {
        path: PathBuf,
        position: Position,
        new_name: String,
    },
    /// The outline of one file.
    Symbols {
        path: PathBuf,
    },
    /// The symbols of the workspace whose name matches `query`.
    WorkspaceSymbols {
        query: String,
    },
}

/// What came back.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A definition request that found something, with the name of the file it
    /// was asked about for the status line.
    Definitions {
        from: PathBuf,
        locations: Vec<Location>,
    },
    /// A definition request that found nothing.
    NoDefinition { from: PathBuf },
    /// Every place the symbol under the caret is used.
    References {
        from: PathBuf,
        locations: Vec<Location>,
    },
    /// What the server says about the symbol at a position. `text: None` means it
    /// said nothing, which is the usual answer over whitespace.
    Hover {
        path: PathBuf,
        line: u32,
        character: u32,
        text: Option<String>,
    },
    /// The uses of a symbol in one file, as ranges of the protocol's positions.
    Highlights {
        path: PathBuf,
        line: u32,
        character: u32,
        spots: Vec<(u32, u32, u32, u32)>,
    },
    /// The edits that rename a symbol: every file the server would change, and
    /// the ranges in it as protocol positions.
    Rename {
        new_name: String,
        changes: crate::lsp::RenameChanges,
    },
    /// The outline of one file, as the server nested it.
    Symbols {
        from: PathBuf,
        symbols: Vec<DocumentSymbol>,
    },
    /// The symbols of the workspace that answer `query`.
    WorkspaceSymbols {
        query: String,
        symbols: Vec<SymbolInformation>,
    },
    /// Something went wrong; the text is for the status bar, the editor keeps
    /// working.
    Failed(String),
    /// Diagnostics for one file, already flattened to one line each.
    Diagnostics {
        path: PathBuf,
        lines: Vec<DiagnosticLine>,
    },
}

/// The text of a hover answer, whatever shape the server sent it in.
///
/// The protocol has three shapes for the same thing — a plain string, an array
/// of marked strings (language-tagged code, then prose) and a markup block that
/// says whether it holds Markdown. The window paints one text, so they are all
/// brought to one, separated by blank lines.
pub fn hover_text(hover: &lsp_types::Hover) -> Option<String> {
    let pieces: Vec<String> = match &hover.contents {
        lsp_types::HoverContents::Scalar(marked) => vec![marked_string_text(marked)],
        lsp_types::HoverContents::Array(marked) => marked.iter().map(marked_string_text).collect(),
        lsp_types::HoverContents::Markup(markup) => vec![markup.value.clone()],
    };
    let text = pieces
        .into_iter()
        .map(|piece| piece.trim().to_owned())
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// The files and ranges of a workspace edit, whichever of the two shapes the
/// server chose.
///
/// The protocol lets a server say "edits per file" or "a list of document edits",
/// and rust-analyzer uses the second one. Both are flattened here, and a file
/// whose URI is not a path on this machine is dropped: the editor cannot write it.
pub type RenameChanges = Vec<(PathBuf, Vec<(u32, u32, u32, u32)>)>;

pub fn rename_changes(edit: &lsp_types::WorkspaceEdit) -> RenameChanges {
    let mut per_file: RenameChanges = Vec::new();
    let mut add = |uri: &lsp_types::Uri, edits: &[lsp_types::TextEdit]| {
        let Some(path) = ide_lsp::uri_to_path(uri) else {
            return;
        };
        let ranges = edits
            .iter()
            .map(|edit| {
                (
                    edit.range.start.line,
                    edit.range.start.character,
                    edit.range.end.line,
                    edit.range.end.character,
                )
            })
            .collect();
        match per_file.iter_mut().find(|(file, _)| *file == path) {
            Some((_, found)) => found.extend(ranges),
            None => per_file.push((path, ranges)),
        }
    };
    if let Some(changes) = &edit.changes {
        for (uri, edits) in changes {
            add(uri, edits);
        }
    }
    // The two shapes of `documentChanges` are untagged in the protocol types, and
    // which one a server's answer becomes is up to `serde`: both are read here, so
    // the answer does not depend on that guess.
    match &edit.document_changes {
        Some(lsp_types::DocumentChanges::Edits(edits)) => {
            for edit in edits {
                add_text_document_edit(&mut add, edit);
            }
        }
        Some(lsp_types::DocumentChanges::Operations(operations)) => {
            for operation in operations {
                if let lsp_types::DocumentChangeOperation::Edit(edit) = operation {
                    add_text_document_edit(&mut add, edit);
                }
            }
        }
        None => {}
    }
    per_file
}

fn add_text_document_edit(
    add: &mut impl FnMut(&lsp_types::Uri, &[lsp_types::TextEdit]),
    edit: &lsp_types::TextDocumentEdit,
) {
    let ranges: Vec<lsp_types::TextEdit> = edit
        .edits
        .iter()
        .map(|change| match change {
            lsp_types::OneOf::Left(edit) => edit.clone(),
            lsp_types::OneOf::Right(annotated) => annotated.text_edit.clone(),
        })
        .collect();
    add(&edit.text_document.uri, &ranges);
}

/// One `textDocument/rename`, with an answer of `null` read as "no changes".
fn rename_once(
    session: &mut LspSession,
    path: &Path,
    position: Position,
    new_name: &str,
) -> anyhow::Result<RenameChanges> {
    let edit = session.rename(path, position, new_name)?;
    Ok(edit.map(|edit| rename_changes(&edit)).unwrap_or_default())
}

/// Whether an error is a server that has not loaded the project yet.
///
/// rust-analyzer says `No references found at position` (-32602) about a symbol it
/// would find a moment later, which is the same cold start a definition shows as an
/// empty answer. Anything else is taken at its word.
fn is_cold_refusal(error: &anyhow::Error) -> bool {
    if ide_lsp::is_content_modified(error) {
        return true;
    }
    let text = format!("{error:#}");
    text.contains("code -32602") && text.contains("No references found")
}

fn marked_string_text(marked: &lsp_types::MarkedString) -> String {
    match marked {
        lsp_types::MarkedString::String(text) => text.clone(),
        lsp_types::MarkedString::LanguageString(code) => {
            format!("```{}\n{}\n```", code.language, code.value)
        }
    }
}

/// One diagnostic as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticLine {
    pub line: u32,
    pub message: String,
}

/// A running language server with a queue in front of it.
pub struct LanguageService {
    jobs: Sender<Job>,
    outcomes: Receiver<Outcome>,
    worker: Option<JoinHandle<()>>,
    /// Set when the application is closing: retry pauses stop pausing and long
    /// work gives up, so the thread can be joined instead of waited out.
    stopping: Arc<AtomicBool>,
    /// The worker rings this when it has returned; the exit waits on it, not on
    /// the thread handle, because a join cannot be given a deadline.
    finished: Option<Receiver<()>>,
}

impl LanguageService {
    /// Starts the server in `root` on its own thread.
    ///
    /// The handshake happens on that thread too, so a cold rust-analyzer does
    /// not delay the first frame.
    pub fn start(config: &'static ServerConfig, root: &Path) -> Self {
        let (job_sender, jobs) = channel();
        let (outcomes, outcome_receiver) = channel();
        let (finished, finished_receiver) = channel();
        let root = root.to_path_buf();
        let name = config.name;
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);

        let worker = std::thread::Builder::new()
            .name(format!("{name}-worker"))
            .spawn(move || {
                work(config, &root, jobs, outcomes, worker_stopping);
                let _ = finished.send(());
            })
            .ok();

        Self {
            jobs: job_sender,
            outcomes: outcome_receiver,
            worker,
            stopping,
            finished: Some(finished_receiver),
        }
    }

    /// Posts a job; a full queue or a dead worker is silently ignored, because
    /// nothing about editing depends on it.
    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// Takes every outcome that arrived since the last frame.
    pub fn poll(&self) -> Vec<Outcome> {
        let mut found = Vec::new();
        // A closed queue means the worker is gone; both cases end the drain.
        while let Ok(outcome) = self.outcomes.try_recv() {
            found.push(outcome);
        }
        found
    }

    /// Stops the worker by dropping the job queue; the server is shut down by
    /// the worker itself when the queue closes.
    pub fn stop(&mut self) {
        // Retry pauses wake up and long work gives up, so the usual stop takes
        // milliseconds even in the middle of a rename.
        self.stopping.store(true, Ordering::Relaxed);
        // A channel closes when the last sender goes, so the sender is replaced
        // with one whose receiver is gone.
        let (dead, _) = channel();
        self.jobs = dead;
        if let Some(finished) = self.finished.take() {
            let _ = finished.recv_timeout(WORKER_GRACE);
        }
        // Whatever the worker is still doing (a request the server has not
        // answered yet) is not waited for: no join can be given a deadline, and
        // the answer is worth nothing to a program that is leaving.
        self.worker.take();
    }
}

impl Drop for LanguageService {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The worker: start the server, then answer jobs until the queue closes.
fn work(
    config: &'static ServerConfig,
    root: &Path,
    jobs: Receiver<Job>,
    outcomes: Sender<Outcome>,
    stopping: Arc<AtomicBool>,
) {
    // The same flag ends a waiting request, so a stop is felt inside the session too
    // and the worker returns without the server having to answer anything.
    let mut session = match LspSession::start_with_abort(config, root, Arc::clone(&stopping)) {
        Ok(session) => session,
        Err(error) => {
            let _ = outcomes.send(Outcome::Failed(format!(
                "{} did not start: {error:#}",
                config.name
            )));
            return;
        }
    };

    // Whether the server has answered a navigation request yet: until it has, an
    // empty answer may only mean it is still indexing, not that the symbol has no
    // definition.
    let mut warm = false;

    loop {
        if stopping.load(Ordering::Relaxed) {
            break;
        }
        match jobs.recv_timeout(Duration::from_millis(200)) {
            Ok(job) => {
                if let Some(outcome) = apply(&mut session, job, &mut warm, &stopping)
                    && outcomes.send(outcome).is_err()
                {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // Diagnostics arrive as notifications; they have no job of their own.
        for published in session.drain_diagnostics() {
            let Some(path) = uri_to_path(&published.uri) else {
                continue;
            };
            let lines = published
                .diagnostics
                .iter()
                .map(|diagnostic| DiagnosticLine {
                    line: diagnostic.range.start.line,
                    message: diagnostic.message.clone(),
                })
                .collect();
            let outcome = Outcome::Diagnostics { path, lines };
            if outcomes.send(outcome).is_err() {
                session.shutdown();
                return;
            }
        }
    }

    if stopping.load(Ordering::Relaxed) {
        // Leaving: closing the pipes is enough. A polite `shutdown` asks a server
        // that may be busy indexing, and the window is waiting on this thread.
        return;
    }
    session.shutdown();
}

/// Waits before a retry, in steps, so that leaving does not have to wait too.
///
/// Returns `false` when the application is closing: the caller should stop asking.
fn pause(delay_ms: u64, stopping: &AtomicBool) -> bool {
    const STEP: Duration = Duration::from_millis(20);
    let deadline = Instant::now() + Duration::from_millis(delay_ms);
    loop {
        if stopping.load(Ordering::Relaxed) {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(STEP.min(left));
    }
}

/// Carries out one job, if it produces an answer worth reporting.
fn apply(
    session: &mut LspSession,
    job: Job,
    warm: &mut bool,
    stopping: &AtomicBool,
) -> Option<Outcome> {
    match job {
        Job::Open { path, text } => {
            if let Err(error) = session.did_open(&path, &text) {
                return Some(Outcome::Failed(format!("{error:#}")));
            }
            None
        }
        Job::Change { path, text } => {
            if let Err(error) = session.did_change(&path, &text) {
                return Some(Outcome::Failed(format!("{error:#}")));
            }
            None
        }
        Job::Close { path } => {
            let _ = session.did_close(&path);
            None
        }
        Job::References { path, position } => Some(match session.references(&path, position) {
            Ok(locations) => Outcome::References {
                from: path,
                locations,
            },
            Err(error) => Outcome::Failed(format!("{error:#}")),
        }),
        Job::Symbols { path } => {
            let mut found = session.document_symbols(&path);
            // Same cold start as a definition: an empty outline before the
            // server has indexed the file means "not yet", not "nothing here".
            if !*warm && matches!(&found, Ok(symbols) if symbols.is_empty()) {
                for delay in [250u64, 500, 1000, 2000] {
                    if !pause(delay, stopping) {
                        break;
                    }
                    found = session.document_symbols(&path);
                    if !matches!(&found, Ok(symbols) if symbols.is_empty()) {
                        break;
                    }
                }
            }
            if matches!(&found, Ok(symbols) if !symbols.is_empty()) {
                *warm = true;
            }
            Some(match found {
                Ok(symbols) => Outcome::Symbols {
                    from: path,
                    symbols,
                },
                Err(error) => Outcome::Failed(format!("{error:#}")),
            })
        }
        Job::Hover { path, position } => {
            // A cold server answers "nothing" — or refuses with `ContentModified`
            // while it is still loading the project — and the pointer is still on
            // the symbol, so the question is asked again. After the first real
            // answer, "nothing" is the truth and is taken as one.
            let ask = |session: &mut LspSession| -> Result<Option<String>, anyhow::Error> {
                match session.hover(&path, position) {
                    Ok(hover) => Ok(hover.as_ref().and_then(hover_text)),
                    Err(error) if ide_lsp::is_content_modified(&error) => Ok(None),
                    Err(error) => Err(error),
                }
            };
            let mut found = ask(session);
            if !*warm && matches!(&found, Ok(None)) {
                for delay in [250u64, 500, 1000, 2000] {
                    if !pause(delay, stopping) {
                        break;
                    }
                    found = ask(session);
                    if !matches!(&found, Ok(None)) {
                        break;
                    }
                }
            }
            if matches!(&found, Ok(Some(_))) {
                *warm = true;
            }
            let text = match found {
                Ok(text) => text,
                Err(error) => return Some(Outcome::Failed(format!("{error:#}"))),
            };
            Some(Outcome::Hover {
                path,
                line: position.line,
                character: position.character,
                text,
            })
        }
        Job::Rename {
            path,
            position,
            new_name,
        } => {
            // rust-analyzer has two ways of saying "not yet": it refuses with "No
            // references found at position" (-32602), or it answers `null` because the
            // symbol is not in the index it is still building. It loads the crate on
            // the first rename rather than on the outline, so even a server that has
            // answered a dozen questions can refuse the first one. A refusal that
            // survives the retries is taken at its word.
            let mut asked = rename_once(session, &path, position, &new_name);
            for delay in [250u64, 500, 1000, 2000, 4000, 8000, 8000, 8000] {
                let not_yet = match &asked {
                    Ok(changes) => changes.is_empty(),
                    Err(error) => is_cold_refusal(error),
                };
                if !not_yet {
                    break;
                }
                if !pause(delay, stopping) {
                    break;
                }
                asked = rename_once(session, &path, position, &new_name);
            }
            let changes = match asked {
                Ok(changes) => {
                    if !changes.is_empty() {
                        *warm = true;
                    }
                    changes
                }
                Err(error) => return Some(Outcome::Failed(format!("{error:#}"))),
            };
            Some(Outcome::Rename { new_name, changes })
        }
        Job::Highlights { path, position } => {
            let spots = match session.document_highlights(&path, position) {
                Ok(highlights) => highlights
                    .iter()
                    .map(|highlight| {
                        (
                            highlight.range.start.line,
                            highlight.range.start.character,
                            highlight.range.end.line,
                            highlight.range.end.character,
                        )
                    })
                    .collect(),
                // A highlight is decoration: a server that cannot answer must not
                // put an error in the status bar on every caret move.
                Err(_) => Vec::new(),
            };
            Some(Outcome::Highlights {
                path,
                line: position.line,
                character: position.character,
                spots,
            })
        }
        Job::WorkspaceSymbols { query } => Some(match session.workspace_symbols(&query) {
            Ok(symbols) => Outcome::WorkspaceSymbols { query, symbols },
            Err(error) => Outcome::Failed(format!("{error:#}")),
        }),
        Job::Definition { path, position } => {
            let mut found = session.definition(&path, position);
            // A cold server answers "nothing" until it has loaded the project, so
            // an empty answer before the first success is asked again. The worker
            // is what waits, never the frame loop.
            if !*warm && matches!(&found, Ok(locations) if locations.is_empty()) {
                for delay in [250u64, 500, 1000, 2000] {
                    if !pause(delay, stopping) {
                        break;
                    }
                    found = session.definition(&path, position);
                    if !matches!(&found, Ok(locations) if locations.is_empty()) {
                        break;
                    }
                }
            }
            if matches!(&found, Ok(locations) if !locations.is_empty()) {
                *warm = true;
            }
            Some(match found {
                Ok(locations) if locations.is_empty() => Outcome::NoDefinition { from: path },
                Ok(locations) => Outcome::Definitions {
                    from: path,
                    locations,
                },
                Err(error) => Outcome::Failed(format!("{error:#}")),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::hover_text;
    use lsp_types::{
        Hover, HoverContents, LanguageString, MarkedString, MarkupContent, MarkupKind, Range,
    };

    fn at() -> Range {
        Range::new(
            lsp_types::Position::new(0, 0),
            lsp_types::Position::new(0, 1),
        )
    }

    #[test]
    fn a_markup_answer_keeps_its_text() {
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "```rust\nfn a()\n```".to_owned(),
            }),
            range: Some(at()),
        };
        assert_eq!(hover_text(&hover).as_deref(), Some("```rust\nfn a()\n```"));
    }

    #[test]
    fn an_array_of_marked_strings_becomes_one_text() {
        let hover = Hover {
            contents: HoverContents::Array(vec![
                MarkedString::LanguageString(LanguageString {
                    language: "rust".to_owned(),
                    value: "fn a()".to_owned(),
                }),
                MarkedString::String("Does a thing.".to_owned()),
            ]),
            range: None,
        };
        assert_eq!(
            hover_text(&hover).as_deref(),
            Some("```rust\nfn a()\n```\n\nDoes a thing.")
        );
    }

    #[test]
    fn a_plain_string_is_a_plain_answer() {
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String("Point".to_owned())),
            range: None,
        };
        assert_eq!(hover_text(&hover).as_deref(), Some("Point"));
    }

    #[test]
    fn an_empty_answer_is_no_answer() {
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String("   ".to_owned())),
            range: None,
        };
        assert!(
            hover_text(&hover).is_none(),
            "whitespace was taken for a hint"
        );
    }
}

#[cfg(test)]
mod workspace_edits {
    use super::rename_changes;

    /// The answer rust-analyzer gave when a function was renamed in a measured
    /// run: `documentChanges`, one file, two edits.
    const REAL_ANSWER: &str = r#"{"documentChanges":[{"textDocument":{"uri":"file:///tmp/x/src/lib.rs","version":1},"edits":[{"range":{"start":{"line":0,"character":7},"end":{"line":0,"character":12}},"newText":"beta"},{"range":{"start":{"line":5,"character":4},"end":{"line":5,"character":9}},"newText":"beta"}]}]}"#;

    #[test]
    fn a_workspace_edit_becomes_files_and_ranges() {
        let edit: lsp_types::WorkspaceEdit = serde_json::from_str(REAL_ANSWER).unwrap();
        let changes = rename_changes(&edit);
        assert_eq!(changes.len(), 1, "the file was lost: {changes:?}");
        let (path, ranges) = &changes[0];
        assert!(path.ends_with("src/lib.rs"), "{}", path.display());
        assert_eq!(ranges, &vec![(0, 7, 0, 12), (5, 4, 5, 9)]);
    }

    #[test]
    fn the_edits_per_file_shape_is_read_too() {
        let text = r#"{"changes":{"file:///tmp/x/a.rs":[{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}},"newText":"z"}]}}"#;
        let edit: lsp_types::WorkspaceEdit = serde_json::from_str(text).unwrap();
        let changes = rename_changes(&edit);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].1, vec![(1, 0, 1, 3)]);
    }

    #[test]
    fn an_empty_answer_is_no_changes() {
        let edit: lsp_types::WorkspaceEdit = serde_json::from_str("{}").unwrap();
        assert!(rename_changes(&edit).is_empty());
    }
}

#[cfg(test)]
mod leaving {
    use super::*;

    #[test]
    fn a_retry_pause_ends_as_soon_as_the_application_leaves() {
        let stopping = AtomicBool::new(false);
        let started = Instant::now();
        assert!(pause(40, &stopping), "a pause that was not interrupted");
        assert!(started.elapsed() >= Duration::from_millis(35));

        stopping.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(
            !pause(8000, &stopping),
            "the pause outlived the request to go"
        );
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "the pause lasted {:?} after the stop",
            started.elapsed()
        );
    }

    /// The real thing: a rename that keeps retrying, and a window closing on it.
    #[test]
    fn a_busy_worker_does_not_hold_the_window_shut() {
        let _queue = crate::app::REAL_SERVER
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(server) = ide_lsp::server_for(Path::new("src/main.rs")) else {
            println!("no server is known for Rust: the check was skipped");
            return;
        };
        if !ide_lsp::is_installed(server) {
            println!("{} is not installed: the check was skipped", server.name);
            return;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let path = root.join("crates/andy-ide/src/state.rs");
        let text = std::fs::read_to_string(&path).unwrap();

        let mut service = LanguageService::start(server, &root);
        service.send(Job::Open {
            path: path.clone(),
            text,
        });
        // A position with no symbol on it: rust-analyzer refuses, and the job spends
        // the next half a minute retrying. That is what the old exit waited out.
        service.send(Job::Rename {
            path,
            position: Position::new(0, 1),
            new_name: "zzz".to_owned(),
        });
        std::thread::sleep(Duration::from_millis(400));

        let before = servers_of_this_process();
        assert!(before > 0, "no server was running to leave behind");

        let started = Instant::now();
        service.stop();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "leaving took {:?}",
            started.elapsed()
        );

        // Letting the thread go is only honest if the worker gave up on its own:
        // a detached worker would keep a rust-analyzer indexing for minutes.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && servers_of_this_process() > 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            servers_of_this_process(),
            0,
            "the server outlived the application"
        );
    }

    /// Language servers running as children of this test process.
    fn servers_of_this_process() -> usize {
        let me = std::process::id();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return 0;
        };
        let mut found = 0;
        for entry in entries.flatten() {
            let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
                continue;
            };
            let parent = status
                .lines()
                .find_map(|line| line.strip_prefix("PPid:"))
                .and_then(|value| value.trim().parse::<u32>().ok());
            let name = status
                .lines()
                .find_map(|line| line.strip_prefix("Name:"))
                .unwrap_or_default()
                .trim()
                .to_owned();
            if parent == Some(me) && name.starts_with("rust-analyzer") {
                found += 1;
            }
        }
        found
    }
}
