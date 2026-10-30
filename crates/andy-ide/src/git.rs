//! What the Git panels show, kept apart from how they are drawn.
//!
//! The repository is opened once per project and its state is re-read on a timer:
//! a status walk is not free, and a panel that runs it on every frame would eat the
//! idle of an application whose whole point is to stay responsive.

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::time::{Duration, Instant};

use ide_git::{Area, Branch, Change, Commit, LineMark, Repo};

/// How often the panels re-read the repository while they are on screen.
pub const REFRESH: Duration = Duration::from_millis(1200);

/// How much history the panel keeps: enough to find yesterday's commit, not the
/// whole of a decade-old repository.
pub const HISTORY: usize = 200;

/// The marks of one file, and the timestamps they were computed against.
///
/// Two stamps, because a file's margin changes when the file is saved and again when
/// it is staged: the index has a timestamp of its own, and either one moving makes the
/// marks stale.
struct MarksEntry {
    file: Option<SystemTime>,
    index: Option<SystemTime>,
    marks: Vec<LineMark>,
}

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
    /// The local branches, the one checked out first.
    pub branches: Vec<Branch>,
    /// The name being typed for a new branch.
    pub branch_name: String,
    /// The last read of the history, newest first.
    pub commits: Vec<Commit>,
    /// The commit the history panel has selected, by index into `commits`.
    pub commit_selected: Option<usize>,
    /// The diff on screen, once asked for.
    pub diff: Option<DiffView>,
    /// Why the diff could not be made, if it could not.
    pub diff_error: Option<String>,
    /// Whether the diff of what is picked is on screen, under the list. It is a pane of the
    /// panel, not a tab of its own: a diff belongs to the row it was asked from.
    pub diff_open: bool,
    /// Whether the panel shows the history instead of the changes. One tab, two windows onto the
    /// same repository.
    pub history: bool,
    /// What went wrong on the last read, if anything.
    pub error: Option<String>,
    /// The message being written for the next commit.
    pub message: String,
    /// What `git` said about the last change it was asked to make.
    pub outcome: Option<String>,
    /// Whether the repository has a remote to fetch from and push to.
    pub has_remote: bool,
    /// The command talking to the remote, if one is running.
    ///
    /// A network command is the one thing here that can take minutes, so it runs on a
    /// thread of its own and the panel stays usable while it does.
    network: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// What that command is doing, in one word, for the panel to show.
    pub network_busy: Option<&'static str>,
    /// The root this state belongs to, so a second project re-opens it.
    root: Option<std::path::PathBuf>,
    /// Margin marks per file, with the timestamps they were computed at.
    marks: std::collections::HashMap<PathBuf, MarksEntry>,
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
        self.has_remote = self
            .repo
            .as_ref()
            .is_some_and(|repo| !repo.remotes().is_empty());
        self.last_read = None;
        self.changes.clear();
        self.commits.clear();
        self.error = None;
        if self.repo.is_none() {
            self.error = Some("This project is not a repository.".to_owned());
        }
    }

    /// Re-reads the repository if enough time has passed, or if `force`d.
    ///
    /// A finished network command is picked up here first: the panel calls this every
    /// frame, so the answer lands as soon as there is one.
    pub fn refresh(&mut self, force: bool) {
        self.poll_network();
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
        self.branches = repo.branches().unwrap_or_default();
        // Asked until the answer is yes: a project cloned in a terminal while the
        // window was open gets its buttons without a restart.
        if !self.has_remote {
            self.has_remote = !repo.remotes().is_empty();
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
                self.diff_open = true;
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
                self.diff_open = true;
            }
            Err(error) => {
                self.diff = None;
                self.diff_error = Some(format!("{error}"));
            }
        }
    }

    /// The margin marks of a file the editor has open, computed once per save.
    ///
    /// The path is the one a tab carries — relative to the project — and the repository
    /// may sit above the project, so the two are joined before asking.
    pub fn marks(&mut self, path: &Path) -> &[LineMark] {
        let (file, index) = self.stamps(path);
        let fresh = self
            .marks
            .get(path)
            .is_some_and(|entry| entry.file == file && entry.index == index);
        if !fresh {
            let marks = match (&self.repo, self.repo_relative(path)) {
                (Some(repo), Some(relative)) => repo.changed_lines(&relative).unwrap_or_default(),
                _ => Vec::new(),
            };
            self.marks
                .insert(path.to_path_buf(), MarksEntry { file, index, marks });
        }
        self.marks
            .get(path)
            .map(|entry| entry.marks.as_slice())
            .unwrap_or(&[])
    }

    /// The stamps the marks are computed against: the file's and the index's.
    fn stamps(&self, path: &Path) -> (Option<SystemTime>, Option<SystemTime>) {
        let Some(root) = &self.root else {
            return (None, None);
        };
        let modified = |path: &Path| {
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
        };
        let file = modified(&root.join(path));
        let index = self
            .repo
            .as_ref()
            .and_then(Repo::root)
            .map(|repo_root| modified(&repo_root.join(".git/index")))
            .unwrap_or(None);
        (file, index)
    }

    /// A tab's path, as the repository spells it.
    pub(crate) fn repo_relative(&self, path: &Path) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let repo_root = self.repo.as_ref()?.root()?;
        let full = root.join(path);
        let full = full.canonicalize().unwrap_or(full);
        let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
        full.strip_prefix(&repo_root).ok().map(Path::to_path_buf)
    }

    /// The branch name to put in the title, when there is one.
    pub fn branch(&self) -> Option<String> {
        self.repo.as_ref().map(Repo::head_label)
    }

    /// Starts a repository in the project root, when the project has none.
    ///
    /// The root is dropped first so that `follow` does real work: it exists to notice a
    /// change of project, and the project has not changed — only become a repository.
    pub fn init_repository(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let result = ide_git::init(&root);
        self.root = None;
        self.follow(Some(&root));
        self.outcome = Some(match result {
            Ok(said) => said,
            Err(error) => format!("{error}"),
        });
        self.refresh(true);
    }

    /// Asks the remote what it has, without touching the work tree.
    pub fn fetch(&mut self) {
        self.request_network("fetching", |repo| repo.fetch());
    }

    /// Moves the current branch forward to its remote, and only forward.
    pub fn pull(&mut self) {
        self.request_network("pulling", |repo| repo.pull());
    }

    /// Sends the current branch to its remote.
    pub fn push(&mut self) {
        self.request_network("pushing", |repo| repo.push());
    }

    /// Runs one command against the remote on a thread, keeping the panel alive.
    ///
    /// One at a time: a second fetch started while the first is in flight would ask the
    /// remote the same question twice and race with it for the answer.
    fn request_network(
        &mut self,
        doing: &'static str,
        operation: fn(&Repo) -> anyhow::Result<String>,
    ) {
        if self.network.is_some() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        self.network = Some(receiver);
        self.network_busy = Some(doing);
        self.outcome = None;
        std::thread::spawn(move || {
            let result = Repo::discover(&root)
                .ok_or_else(|| anyhow::anyhow!("This project is not a repository."))
                .and_then(|repo| operation(&repo))
                .map_err(|error| format!("{error}"));
            // The receiver may be gone because the project was closed; the answer is
            // then of no use to anybody.
            let _ = sender.send(result);
        });
    }

    /// Takes the answer of a finished network command, if there is one.
    fn poll_network(&mut self) {
        let Some(receiver) = &self.network else {
            return;
        };
        let answer = match receiver.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the git command went away without an answer".to_owned())
            }
        };
        self.network = None;
        self.network_busy = None;
        match answer {
            Ok(said) => self.outcome = Some(said),
            Err(error) => self.outcome = Some(error),
        }
        // Whatever happened, the remote may have moved `HEAD`, the index or the refs.
        self.refresh(true);
    }

    /// Moves `HEAD` to another branch, and says what `git` answered.
    pub fn switch_to(&mut self, name: &str) {
        let name = name.to_owned();
        self.apply(|repo| repo.switch_to(&name), "switched");
    }

    /// Starts a branch at `HEAD` with the name in the field.
    pub fn create_branch(&mut self) {
        let name = self.branch_name.clone();
        if self.apply(|repo| repo.create_branch(&name), "created") {
            self.branch_name.clear();
        }
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
