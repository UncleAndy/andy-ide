//! The log of the window: a file that does not grow without end.
//!
//! The technical requirements ask for `~/.local/state/andy-ide/andy-ide.log`, rotated at ten
//! megabytes, with the level taken from `ANDY_IDE_LOG`. Rotating by size is not something
//! `tracing-appender` offers — it rotates by the clock — so the writer is our own: it counts the
//! bytes it has written and, when the next line would pass the limit, closes the file, renames it
//! to `andy-ide.log.1` and starts a fresh one. One previous run is kept, so the directory holds at
//! most twenty megabytes however long the window runs.
//!
//! The lines also go to the standard error. The file is what a person reads after the fact, the
//! standard error is what a run from a terminal shows; a window started from a desktop entry has no
//! terminal and writes to the file only, which is the case the requirements are about.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;

/// The size at which the log is rotated, as the requirements ask: ten megabytes.
pub const LIMIT: u64 = 10 * 1024 * 1024;

/// The name of the file the previous run was written to.
fn previous_of(path: &Path) -> PathBuf {
    path.with_extension("log.1")
}

/// Where the log lives: `$XDG_STATE_HOME/andy-ide/andy-ide.log`, falling back to
/// `~/.local/state/andy-ide/andy-ide.log`, as the requirements name it.
pub fn path() -> PathBuf {
    state_dir(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
        .join("andy-ide")
        .join("andy-ide.log")
}

/// The part of the path that comes from the environment, apart so it can be checked without
/// touching the environment of a whole process (which the checks in one binary share).
fn state_dir(
    xdg_state_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> PathBuf {
    xdg_state_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
}

/// The log of this run: one file, rotated by size.
///
/// Cloning is cheap and the clones write into the same file, which is what a subscriber per layer
/// needs; the count of bytes is shared with them.
#[derive(Clone)]
pub struct Log {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    limit: u64,
    /// `None` only while the file is renamed aside, which Windows refuses to do to an open file.
    file: Mutex<Option<File>>,
    written: AtomicU64,
}

impl Log {
    /// Open the log at `path`, making its directory if it is not there. A log an earlier run
    /// already wrote to is added to, and its size counts towards the limit.
    pub fn open(path: PathBuf, limit: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = append(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            inner: Arc::new(Inner {
                path,
                limit,
                file: Mutex::new(Some(file)),
                written: AtomicU64::new(written),
            }),
        })
    }

    /// The size it has grown to since the last rotation.
    pub fn written(&self) -> u64 {
        self.inner.written.load(Ordering::Relaxed)
    }

    /// Put the finished file aside and start a new one.
    fn rotate(&self) -> io::Result<()> {
        let mut file = self
            .inner
            .file
            .lock()
            .expect("the lock over the log file is not poisoned");
        let previous = previous_of(&self.inner.path);
        // One previous run is kept, and replacing it is the point: the log is a window on what just
        // happened, not an archive.
        let _ = std::fs::remove_file(&previous);
        // The handle is let go of before the rename: Windows refuses to rename an open file, and a
        // log that cannot be rotated on one platform is a log that grows without end there.
        *file = None;
        std::fs::rename(&self.inner.path, &previous)?;
        *file = Some(append(&self.inner.path)?);
        self.inner.written.store(0, Ordering::Relaxed);
        Ok(())
    }
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written() + buf.len() as u64 > self.inner.limit {
            self.rotate()?;
        }
        let mut file = self
            .inner
            .file
            .lock()
            .expect("the lock over the log file is not poisoned");
        let file = file
            .as_mut()
            .ok_or_else(|| io::Error::other("the log file is being rotated"))?;
        let written = file.write(buf)?;
        self.inner
            .written
            .fetch_add(written as u64, Ordering::Relaxed);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut file = self
            .inner
            .file
            .lock()
            .expect("the lock over the log file is not poisoned");
        match file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// The log and the standard error together: the file is the contract, the terminal a courtesy.
#[derive(Clone)]
pub struct ToStderrAndFile(pub Log);

/// One line's worth of writer, made for each event by [`ToStderrAndFile`].
pub struct Both {
    log: Log,
    stderr: io::Stderr,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ToStderrAndFile {
    type Writer = Both;

    fn make_writer(&'a self) -> Self::Writer {
        Both {
            log: self.0.clone(),
            stderr: io::stderr(),
        }
    }
}

impl Write for Both {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.log.write(buf)?;
        let _ = self.stderr.write_all(buf);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.stderr.flush();
        self.log.flush()
    }
}

/// The level `ANDY_IDE_LOG` asks for, `info` when it asks for nothing.
fn filter() -> EnvFilter {
    EnvFilter::try_from_env("ANDY_IDE_LOG").unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Start logging: the file the requirements name, and the standard error beside it.
///
/// A log that cannot be opened is not a reason to refuse to open a window: the standard error still
/// carries the lines, and the reason is said there.
pub fn init() {
    let path = path();
    match start(path.clone(), LIMIT, filter()) {
        Ok(()) => {}
        Err(error) => {
            tracing_subscriber::fmt().with_env_filter(filter()).init();
            tracing::warn!(
                %error,
                path = %path.display(),
                "the log file cannot be opened: logging to the standard error only"
            );
        }
    }
}

/// The two halves of [`init`] with the path and the limit given: what a check can call without
/// touching the environment, and what makes it plain that the file has a limit at all.
fn start(
    path: PathBuf,
    limit: u64,
    filter: EnvFilter,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log = Log::open(path, limit)?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(ToStderrAndFile(log))
        .try_init()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of the size given, ending the way a log line does.
    fn line(size: usize) -> String {
        let mut line = "x".repeat(size.saturating_sub(1));
        line.push('\n');
        line
    }

    #[test]
    fn the_limit_is_the_ten_megabytes_of_the_requirements() {
        assert_eq!(LIMIT, 10 * 1024 * 1024);
    }

    #[test]
    fn the_path_is_the_one_the_requirements_name() {
        let named = state_dir(
            Some("/tmp/state-of-this-check".into()),
            Some("/home/whoever".into()),
        )
        .join("andy-ide")
        .join("andy-ide.log");
        assert_eq!(
            named,
            PathBuf::from("/tmp/state-of-this-check/andy-ide/andy-ide.log")
        );

        // Without a state home of its own, the path is the one under the home directory.
        let under_home = state_dir(None, Some("/home/whoever".into()))
            .join("andy-ide")
            .join("andy-ide.log");
        assert_eq!(
            under_home,
            PathBuf::from("/home/whoever/.local/state/andy-ide/andy-ide.log")
        );

        // A relative `XDG_STATE_HOME` is not a state home: it counts only when it is absolute.
        let relative = state_dir(Some("state".into()), Some("/home/whoever".into()));
        assert_eq!(relative, PathBuf::from("/home/whoever/.local/state"));
    }

    #[test]
    fn the_log_is_rotated_when_it_would_pass_the_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("andy-ide.log");
        // The limit holds two lines of the three written, so the file put aside has the two that
        // were finished with and the fresh one has the last.
        let mut log = Log::open(path.clone(), 100).unwrap();

        log.write_all(line(40).as_bytes()).unwrap();
        log.write_all(line(40).as_bytes()).unwrap();
        log.write_all(line(40).as_bytes()).unwrap();
        log.flush().unwrap();

        let current = std::fs::read_to_string(&path).unwrap();
        let previous = std::fs::read_to_string(previous_of(&path)).unwrap();
        assert_eq!(
            current.len(),
            40,
            "the log did not start again after the limit: {current:?}"
        );
        assert_eq!(
            previous.len(),
            80,
            "the file put aside is not the one that was finished: {previous:?}"
        );
        assert_eq!(
            log.written(),
            40,
            "the count of bytes is not the size of the file being written"
        );
    }

    #[test]
    fn a_log_that_was_written_to_before_is_added_to_and_counted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("andy-ide.log");
        std::fs::write(&path, line(70)).unwrap();

        let mut log = Log::open(path.clone(), 64).unwrap();
        assert_eq!(
            log.written(),
            70,
            "the size of the file was taken to be zero"
        );
        log.write_all(line(10).as_bytes()).unwrap();
        log.flush().unwrap();

        assert_eq!(
            std::fs::read_to_string(previous_of(&path)).unwrap().len(),
            70,
            "the file of the previous run was not put aside"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap().len(), 10);
    }

    #[test]
    fn one_previous_run_is_kept_and_no_more() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("andy-ide.log");
        let mut log = Log::open(path.clone(), 32).unwrap();
        for _ in 0..8 {
            log.write_all(line(20).as_bytes()).unwrap();
        }
        log.flush().unwrap();

        let mut names: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["andy-ide.log".to_owned(), "andy-ide.log.1".to_owned()],
            "the directory keeps more than the log and one run before it"
        );
    }

    #[test]
    fn the_directory_of_the_log_is_made_when_it_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("deeper/still/andy-ide.log");
        let mut log = Log::open(path.clone(), LIMIT).unwrap();
        log.write_all(b"a line\n").unwrap();
        log.flush().unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a line\n");
    }

    #[test]
    fn a_line_reaches_the_file_through_the_subscriber() {
        // The subscriber is set once per process and another check in this binary may have got
        // there first; then there is nothing left to check here.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("andy-ide.log");
        let Ok(()) = start(path.clone(), LIMIT, EnvFilter::new("info")) else {
            println!("logging was already started: the check was skipped");
            return;
        };

        tracing::info!(mark = "ANDY-IDE-LOG-MARK", "the line of this check");
        let written = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            written.contains("ANDY-IDE-LOG-MARK"),
            "the line did not reach the file of the log: {written:?}"
        );
        assert!(
            written.contains("the line of this check"),
            "the file holds something other than the line: {written:?}"
        );
    }
}
