//! One window per project.
//!
//! A person who presses the launcher twice, or asks for a project that is already in front of them,
//! gets two windows editing one tree: both write, both watch, and neither knows about the other's
//! saves. The guard is an advisory lock on a file inside the project — `<project>/.andy-ide/
//! running.lock` — held for as long as the window lives.
//!
//! It is a lock and not a file that says "running" on purpose: the operating system releases it when
//! the process ends, so a crash leaves nothing to clean up and nothing that lies about who is there.
//! Locking is also the one mechanism that behaves the same on every desk we ship to — `flock` on
//! Unix, `LockFileEx` on Windows. D-Bus would be the native way on Linux and does not exist on the
//! other two.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::workspace::WORKSPACE_DIR;

/// The file a running window locks, inside the project's own directory.
pub const LOCK_FILE: &str = "running.lock";

/// Why a project could not be locked.
#[derive(Debug)]
pub enum Locked {
    /// Another window has the project open.
    AnotherWindow,
    /// The lock file could not be made or written — a read-only project, a directory where a file
    /// belongs, a full disk. The window opens anyway: a guard nobody can take must not stop work.
    Cannot(io::Error),
}

/// The lock a window holds on the project it has open.
#[derive(Debug)]
pub struct ProjectLock {
    path: PathBuf,
    /// Closed when the window goes, which is what lets the next one in.
    _file: File,
}

impl ProjectLock {
    /// Takes the project's lock, or says who holds it.
    ///
    /// The project's own directory (`.andy-ide`) is made if it is not there yet: a project this
    /// window is the first to open has nothing in it.
    pub fn take(root: &Path) -> Result<ProjectLock, Locked> {
        let dir = root.join(WORKSPACE_DIR);
        std::fs::create_dir_all(&dir).map_err(Locked::Cannot)?;
        let path = dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(Locked::Cannot)?;
        match file.try_lock() {
            Ok(()) => Ok(ProjectLock { path, _file: file }),
            Err(TryLockError::WouldBlock) => Err(Locked::AnotherWindow),
            Err(TryLockError::Error(error)) => Err(Locked::Cannot(error)),
        }
    }

    /// Whether another window holds the project — asked and let go in the same breath.
    pub fn is_held(root: &Path) -> bool {
        match Self::take(root) {
            Ok(_) => false,
            Err(Locked::AnotherWindow) => true,
            // A project we cannot lock is not a project we may quietly refuse.
            Err(Locked::Cannot(_)) => false,
        }
    }

    /// The file the lock is kept in.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::{Locked, ProjectLock};

    #[test]
    fn two_windows_cannot_hold_one_project() {
        // The heart of the rule: the second window is told, not silently allowed in.
        let project = tempfile::tempdir().unwrap();
        let first = ProjectLock::take(project.path()).expect("the first window takes the project");
        assert_eq!(
            first.path(),
            project.path().join(".andy-ide").join("running.lock"),
            "the lock does not stand in the project's own directory"
        );
        assert!(
            matches!(
                ProjectLock::take(project.path()),
                Err(Locked::AnotherWindow)
            ),
            "a second window took a project that is already open"
        );
        assert!(
            ProjectLock::is_held(project.path()),
            "a held project does not read as held"
        );
    }

    #[test]
    fn the_project_is_free_again_once_the_window_is_gone() {
        // A crash is the case this shape exists for: the process ends, the operating system drops
        // the lock, and nothing a person has to clean up is left behind.
        let project = tempfile::tempdir().unwrap();
        let first = ProjectLock::take(project.path()).unwrap();
        drop(first);
        assert!(
            !ProjectLock::is_held(project.path()),
            "the project stayed taken after the window went"
        );
        assert!(
            ProjectLock::take(project.path()).is_ok(),
            "the next window was refused a project nobody holds"
        );
    }

    #[test]
    fn the_lock_does_not_need_a_project_that_has_been_opened_before() {
        // `.andy-ide` is made on the way: a project the window is the first to open has nothing in it.
        let project = tempfile::tempdir().unwrap();
        assert!(!project.path().join(".andy-ide").exists());
        ProjectLock::take(project.path()).expect("the first window makes the directory it needs");
        assert!(
            project
                .path()
                .join(".andy-ide")
                .join("running.lock")
                .is_file()
        );
    }

    #[test]
    fn a_project_that_cannot_be_locked_does_not_read_as_taken() {
        // A read-only project is not a project another window has; refusing to open it would be a
        // guard that stops work instead of one that prevents a mistake.
        let project = tempfile::tempdir().unwrap();
        let blocked = project.path().join(".andy-ide");
        std::fs::write(&blocked, "not a directory").unwrap();
        assert!(matches!(
            ProjectLock::take(project.path()),
            Err(Locked::Cannot(_))
        ));
        assert!(!ProjectLock::is_held(project.path()));
    }
}
