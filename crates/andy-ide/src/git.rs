//! What the Git panel shows, kept apart from how it is drawn.
//!
//! The repository is opened once per project and its state is re-read on a timer:
//! a status walk is not free, and a panel that runs it on every frame would eat the
//! idle of an application whose whole point is to stay responsive.

use std::path::Path;
use std::time::{Duration, Instant};

use ide_git::{Area, Change, FileDiff, Repo};

/// How often the panel re-reads the repository while it is on screen.
pub const REFRESH: Duration = Duration::from_millis(1200);

/// The repository behind the current project, and what it last said.
#[derive(Default)]
pub struct GitState {
    /// `None` when the project is not inside a repository.
    pub repo: Option<Repo>,
    /// The last read of the work tree, in the order the panel shows it.
    pub changes: Vec<Change>,
    /// The change the panel has selected, by index into `changes`.
    pub selected: Option<usize>,
    /// The diff of the selected change, once asked for.
    pub diff: Option<FileDiff>,
    /// Why the diff could not be made, if it could not.
    pub diff_error: Option<String>,
    /// Set when a fresh diff should be brought to the front.
    pub show_diff: bool,
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
                // A row that is gone takes the selection with it: an index that points
                // past the end would open whatever slid into its place.
                if self
                    .selected
                    .is_some_and(|index| index >= self.changes.len())
                {
                    self.selected = None;
                    self.diff = None;
                }
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

    /// The change under the selection, if it is still there.
    pub fn selected_change(&self) -> Option<&Change> {
        self.selected.and_then(|index| self.changes.get(index))
    }

    /// Reads the diff of the selected change into `diff`.
    ///
    /// Kept apart from the panel so the window can ask for it from a key press and
    /// the panel only draws what arrived.
    pub fn open_selected_diff(&mut self) {
        let Some(change) = self.selected_change().cloned() else {
            return;
        };
        let Some(repo) = &self.repo else {
            return;
        };
        let side = change.side();
        match repo.diff(&change.path, side) {
            Ok(diff) => {
                self.diff = Some(diff);
                self.diff_error = None;
                self.show_diff = true;
            }
            Err(error) => {
                self.diff = None;
                self.diff_error = Some(format!("{error}"));
            }
        }
    }

    /// The branch name to put in the title, when there is one.
    pub fn branch(&self) -> Option<String> {
        self.repo.as_ref().map(Repo::head_label)
    }
}
