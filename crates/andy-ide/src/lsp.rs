//! The language server as the frame loop uses it: a worker thread with a job
//! queue.
//!
//! Requests block (a definition can take a second while the server is still
//! indexing), so they never run on the drawing thread. The application posts a
//! job, keeps drawing, and picks the answer up from [`LanguageService::poll`].

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use ide_lsp::registry::ServerConfig;
use ide_lsp::{LspSession, uri_to_path};
use lsp_types::{DocumentSymbol, Location, Position, SymbolInformation};

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
}

impl LanguageService {
    /// Starts the server in `root` on its own thread.
    ///
    /// The handshake happens on that thread too, so a cold rust-analyzer does
    /// not delay the first frame.
    pub fn start(config: &'static ServerConfig, root: &Path) -> Self {
        let (job_sender, jobs) = channel();
        let (outcomes, outcome_receiver) = channel();
        let root = root.to_path_buf();
        let name = config.name;

        let worker = std::thread::Builder::new()
            .name(format!("{name}-worker"))
            .spawn(move || work(config, &root, jobs, outcomes))
            .ok();

        Self {
            jobs: job_sender,
            outcomes: outcome_receiver,
            worker,
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
        // A channel closes when the last sender goes, so the sender is replaced
        // with one whose receiver is gone.
        let (dead, _) = channel();
        self.jobs = dead;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
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
) {
    let mut session = match LspSession::start(config, root) {
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
        match jobs.recv_timeout(Duration::from_millis(200)) {
            Ok(job) => {
                if let Some(outcome) = apply(&mut session, job, &mut warm)
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

    session.shutdown();
}

/// Carries out one job, if it produces an answer worth reporting.
fn apply(session: &mut LspSession, job: Job, warm: &mut bool) -> Option<Outcome> {
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
                    std::thread::sleep(Duration::from_millis(delay));
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
                    std::thread::sleep(Duration::from_millis(delay));
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
                    std::thread::sleep(Duration::from_millis(delay));
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
