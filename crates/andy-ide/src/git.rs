//! What the Git panels show, kept apart from how they are drawn.
//!
//! The repository is opened once per project and its state is re-read on a timer:
//! a status walk is not free, and a panel that runs it on every frame would eat the
//! idle of an application whose whole point is to stay responsive.

use std::path::Path;
use std::time::{Duration, Instant};

use ide_git::{Area, Change, Commit, Repo};

/// How often the panels re-read the repository while they are on screen.
pub const REFRESH: Duration = Duration::from_millis(1200);

/// How much history the panel keeps: enough to find yesterday's commit, not the
/// whole of a decade-old repository.
pub const HISTORY: usize = 200;

/// One diff, ready to be drawn — a working change or a commit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffView {
    /// What the diff is of.
    pub title: String,
    /// What it is compared against.
    pub note: String,
    /// The unified diff.
    pub text: String,
}

/// The repository behind the current project, and what it last said.
#[derive(Default)]
pub struct GitState {
    /// `None` when the project is not inside a repository.
    pub repo: Option<Repo>,
    /// The last read of the work tree, in the order the panel shows it.
    pub changes: Vec<Change>,
    /// The change the panel has selected, by index into `changes`.
    pub selected: Option<usize>,
    /// The last read of the history, newest first.
    pub commits: Vec<Commit>,
    /// The commit the history panel has selected, by index into `commits`.
    pub commit_selected: Option<usize>,
    /// The diff on screen, once asked for.
    pub diff: Option<DiffView>,
    /// Why the diff could not be made, if it could not.
    pub diff_error: Option<String>,
    /// Set when a fresh diff should be brought to the front.
    pub show_diff: bool,
    /// Set when the history panel should be brought to the front.
    pub show_history: bool,
    /// What went wrong on the last read, if anything.
    pub error: Option<String>,
    /// The message being written for the next commit.
    pub message: String,
    /// What `git` said about the last change it was asked to make.
    pub outcome: Option<String>,
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
        self.commits.clear();
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
        match repo.log(HISTORY) {
            Ok(commits) => {
                self.commits = commits;
                if self
                    .commit_selected
                    .is_some_and(|index| index >= self.commits.len())
                {
                    self.commit_selected = None;
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
            Ok(file) => {
                self.diff = Some(DiffView {
                    title: file.path.display().to_string(),
                    note: match file.side {
                        ide_git::Side::Staged => "against HEAD".to_owned(),
                        ide_git::Side::Worktree => "against the index".to_owned(),
                    },
                    text: file.unified,
                });
                self.diff_error = None;
                self.show_diff = true;
            }
            Err(error) => {
                self.diff = None;
                self.diff_error = Some(format!("{error}"));
            }
        }
    }

    /// Reads the diff of the selected commit into `diff`.
    pub fn open_selected_commit(&mut self) {
        let Some(commit) = self
            .commit_selected
            .and_then(|index| self.commits.get(index))
            .cloned()
        else {
            return;
        };
        let Some(repo) = &self.repo else {
            return;
        };
        match repo.commit_diff(&commit.id) {
            Ok(text) => {
                self.diff = Some(DiffView {
                    title: format!("{} {}", commit.short, commit.summary),
                    note: commit.author,
                    text,
                });
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

    /// Puts the selected change into the index.
    pub fn stage_selected(&mut self) {
        let Some(path) = self.selected_change().map(|change| change.path.clone()) else {
            return;
        };
        self.apply(|repo| repo.stage(&path), "staged");
    }

    /// Takes the selected change out of the index again.
    pub fn unstage_selected(&mut self) {
        let Some(path) = self.selected_change().map(|change| change.path.clone()) else {
            return;
        };
        self.apply(|repo| repo.unstage(&path), "unstaged");
    }

    /// Puts every change into the index.
    pub fn stage_all(&mut self) {
        self.apply(|repo| repo.stage_all(), "staged everything");
    }

    /// Commits the index with the message in the field.
    pub fn commit(&mut self) {
        let message = self.message.clone();
        let outcome = self.apply(|repo| repo.commit(&message), "committed");
        if outcome {
            self.message.clear();
            self.diff = None;
            self.selected = None;
        }
    }

    /// Runs one operation, re-reads the repository whatever happened, and keeps what
    /// `git` said.
    ///
    /// The re-read is unconditional on purpose: a refused commit still may have
    /// changed the index (a hook that formats files, for instance), and a panel that
    /// showed the state from before would be lying about what is staged.
    fn apply(
        &mut self,
        operation: impl FnOnce(&Repo) -> anyhow::Result<String>,
        done: &str,
    ) -> bool {
        let Some(repo) = &self.repo else {
            return false;
        };
        let result = operation(repo);
        self.refresh(true);
        match result {
            Ok(said) => {
                self.outcome = Some(if said.trim().is_empty() {
                    format!("{done}.")
                } else {
                    said.trim().to_owned()
                });
                true
            }
            Err(error) => {
                self.outcome = Some(format!("{error}"));
                false
            }
        }
    }
}
