//! What the Git panel shows, kept apart from how it is drawn.
//!
//! The repository is opened once per project and its state is re-read on a timer:
//! a status walk is not free, and a panel that runs it on every frame would eat the
//! idle of an application whose whole point is to stay responsive.

use std::path::Path;
use std::time::{Duration, Instant};

use ide_git::{Area, Change, Repo};

/// How often the panel re-reads the repository while it is on screen.
pub const REFRESH: Duration = Duration::from_millis(1200);

/// The repository behind the current project, and what it last said.
#[derive(Default)]
pub struct GitState {
    /// `None` when the project is not inside a repository.
    pub repo: Option<Repo>,
    /// The last read of the work tree, in the order the panel shows it.
    pub changes: Vec<Change>,
    /// What went wrong on the last read, if anything.
    pub error: Option<String>,
    /// The root this state belongs to, so a second project re-opens it.
    root: Option<std::path::PathBuf>,
    last_read: Option<Instant>,
}

impl GitState {
    /// Points the state at a project, opening its repository if it has one.
    ///
    /// Called every frame; only a change of root does any work.
    pub fn follow(&mut self, root: Option<&Path>) {
        let root = root.map(Path::to_path_buf);
        if root == self.root {
            return;
        }
        self.root = root.clone();
        self.repo = root.and_then(Repo::discover);
        self.last_read = None;
        self.changes.clear();
        self.error = None;
        if self.repo.is_none() {
            self.error = Some("This project is not a repository.".to_owned());
        }
    }

    /// Re-reads the repository if enough time has passed, or if `force`d.
    pub fn refresh(&mut self, force: bool) {
        let Some(repo) = &self.repo else {
            return;
        };
        let due = match self.last_read {
            None => true,
            Some(at) => force || at.elapsed() >= REFRESH,
        };
        if !due {
            return;
        }
        self.last_read = Some(Instant::now());
        match repo.changes() {
            Ok(changes) => {
                self.changes = changes;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error}")),
        }
    }

    /// The staged half of the list.
    pub fn staged(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.area == Area::Staged)
    }

    /// The half that is only on disk.
    pub fn worktree(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.area == Area::Worktree)
    }

    /// The branch name to put in the title, when there is one.
    pub fn branch(&self) -> Option<String> {
        self.repo.as_ref().map(Repo::head_label)
    }
}
