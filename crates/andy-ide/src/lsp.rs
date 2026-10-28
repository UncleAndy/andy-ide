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
use lsp_types::{Location, Position};

/// Something the editor wants from the server.
#[derive(Debug)]
pub enum Job {
    Open { path: PathBuf, text: String },
    Change { path: PathBuf, text: String },
    Close { path: PathBuf },
    Definition { path: PathBuf, position: Position },
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
    /// Something went wrong; the text is for the status bar, the editor keeps
    /// working.
    Failed(String),
    /// Diagnostics for one file, already flattened to one line each.
    Diagnostics {
        path: PathBuf,
        lines: Vec<DiagnosticLine>,
    },
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
