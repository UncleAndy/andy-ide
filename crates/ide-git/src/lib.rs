//! Repository state, diffs and history for andy-ide.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.
//!
//! Reading is native, through `gix`: the index, the objects and the work tree are
//! read without starting a process, which is what keeps the panel honest when `git`
//! is not installed or is a wrapper. Operations that need authentication — `fetch`,
//! `pull`, `push` — are left to the system `git`, and they will live here too, so
//! the window never builds a command line of its own.

use std::path::{Path, PathBuf};

use gix::status::index_worktree::Item as WorktreeItem;

pub mod diff;
pub mod log;
pub mod ops;
pub use diff::{FileDiff, Side};
use gix_status::index_as_worktree::{Change as WorktreeChange, EntryStatus};
pub use log::Commit;

/// Version of the crate, taken from its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// An open repository.
pub struct Repo {
    inner: gix::Repository,
}

/// What happened to a file, in the vocabulary a person uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Conflicted,
    Untracked,
}

impl Kind {
    /// The letter `git status` prints for this change.
    pub fn letter(self) -> char {
        match self {
            Kind::Added => 'A',
            Kind::Modified => 'M',
            Kind::Deleted => 'D',
            Kind::Renamed => 'R',
            Kind::TypeChanged => 'T',
            Kind::Conflicted => 'U',
            Kind::Untracked => '?',
        }
    }
}

/// Which of the two halves of `git status` a change belongs to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Area {
    /// Ready to go into the next commit: in the index, differing from `HEAD`.
    Staged,
    /// On disk, differing from the index.
    Worktree,
}

/// One changed file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Change {
    /// Repository-relative, with `/` on every platform.
    pub path: PathBuf,
    pub kind: Kind,
    pub area: Area,
}

impl Change {
    /// Which comparison shows this change.
    pub fn side(&self) -> Side {
        match self.area {
            Area::Staged => Side::Staged,
            Area::Worktree => Side::Worktree,
        }
    }
}

impl Repo {
    /// Finds the repository a path belongs to, walking up as `git` does.
    ///
    /// A directory that is not in a repository is not an error: the interface says
    /// so and offers to create one, which is what `None` means here.
    pub fn discover(path: impl AsRef<Path>) -> Option<Self> {
        let inner = gix::discover(path.as_ref()).ok()?;
        Some(Self { inner })
    }

    /// The repository underneath, for the rest of the crate.
    pub(crate) fn inner(&self) -> &gix::Repository {
        &self.inner
    }

    /// The top of the work tree: the directory the project tree shows.
    pub fn root(&self) -> Option<PathBuf> {
        self.inner.workdir().map(Path::to_path_buf)
    }

    /// The branch checked out, or `None` when `HEAD` is detached.
    pub fn branch(&self) -> Option<String> {
        let name = self.inner.head_name().ok()??;
        Some(name.shorten().to_string())
    }

    /// What to show in the title of the panel.
    pub fn head_label(&self) -> String {
        match self.branch() {
            Some(name) => name,
            None => "(detached)".to_owned(),
        }
    }

    /// Whether the work tree or the index differs from the last commit.
    ///
    /// Untracked files do not count here — `git` itself does not count them.
    pub fn is_dirty(&self) -> bool {
        self.inner.is_dirty().unwrap_or(false)
    }

    /// Everything `git status` would print, in a stable order.
    ///
    /// The list is sorted by path and then by area, so two runs over an unchanged
    /// work tree hand the panel the same lines in the same order; the panel moves
    /// the selection by index, and a list that reshuffles under the user is a bug.
    pub fn changes(&self) -> anyhow::Result<Vec<Change>> {
        let mut changes = Vec::new();

        for item in self
            .inner
            .status(gix::progress::Discard)?
            .into_iter(Vec::new())?
        {
            match item? {
                gix::status::Item::IndexWorktree(WorktreeItem::Modification {
                    rela_path,
                    status,
                    ..
                }) => {
                    if let Some(kind) = worktree_kind(&status) {
                        changes.push(Change {
                            path: to_path(&rela_path),
                            kind,
                            area: Area::Worktree,
                        });
                    }
                }
                gix::status::Item::IndexWorktree(WorktreeItem::DirectoryContents {
                    entry, ..
                }) => {
                    if matches!(entry.status, gix::dir::entry::Status::Untracked) {
                        changes.push(Change {
                            path: to_path(&entry.rela_path),
                            kind: Kind::Untracked,
                            area: Area::Worktree,
                        });
                    }
                }
                // Rename tracking is off, so a rewritten entry arrives as its two
                // halves; the rest — pruned directories, ignored files — is not
                // the panel's business.
                gix::status::Item::IndexWorktree(_) => {}
                gix::status::Item::TreeIndex(change) => {
                    changes.push(staged_change(&change));
                }
            }
        }

        changes.sort_by(|a, b| a.path.cmp(&b.path).then(a.area.cmp(&b.area)));
        changes.dedup();
        Ok(changes)
    }
}

/// The kind of a work-tree change, or `None` when nothing really changed.
fn worktree_kind(status: &EntryStatus<(), gix::submodule::Status>) -> Option<Kind> {
    match status {
        EntryStatus::Conflict { .. } => Some(Kind::Conflicted),
        EntryStatus::Change(change) => Some(match change {
            WorktreeChange::Removed => Kind::Deleted,
            WorktreeChange::Type { .. } => Kind::TypeChanged,
            WorktreeChange::Modification { .. } => Kind::Modified,
            WorktreeChange::SubmoduleModification(_) => Kind::Modified,
        }),
        // A stat that only needs writing back: the file is the same file.
        EntryStatus::NeedsUpdate(_) | EntryStatus::IntentToAdd => None,
    }
}

/// A change between `HEAD` and the index.
fn staged_change(change: &gix::diff::index::Change) -> Change {
    use gix::diff::index::Change as Staged;
    let (path, kind) = match change {
        Staged::Addition { location, .. } => (location.to_string(), Kind::Added),
        Staged::Deletion { location, .. } => (location.to_string(), Kind::Deleted),
        Staged::Modification { location, .. } => (location.to_string(), Kind::Modified),
        Staged::Rewrite { location, .. } => (location.to_string(), Kind::Renamed),
    };
    Change {
        path: PathBuf::from(path),
        kind,
        area: Area::Staged,
    }
}

/// A repository-relative path, as a path the interface can show.
fn to_path(bytes: impl AsRef<[u8]>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes.as_ref()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{Area, Kind, Repo};

    /// A repository of this test's own, made with the system `git`.
    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("b.txt"), "two\n").unwrap();
        git(&root, &["add", "a.txt", "b.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "first",
            ],
        );
        (dir, root)
    }

    fn git(root: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git ran");
        assert!(
            status.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn a_repository_is_found_from_a_directory_inside_it() {
        let (_dir, root) = scratch();
        let deep = root.join("src/deep");
        std::fs::create_dir_all(&deep).unwrap();

        let repo = Repo::discover(&deep).expect("the repository was not found");
        assert_eq!(
            repo.root().map(|path| path.canonicalize().unwrap()),
            Some(root.canonicalize().unwrap())
        );
        assert_eq!(repo.branch().as_deref(), Some("main"));
        assert_eq!(repo.head_label(), "main");
    }

    #[test]
    fn a_directory_outside_a_repository_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Repo::discover(dir.path()).is_none(),
            "a plain directory was taken for a repository"
        );
    }

    #[test]
    fn an_edit_makes_the_repository_dirty_and_a_commit_clears_it() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        assert!(!repo.is_dirty(), "a fresh repository is dirty");

        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        assert!(repo.is_dirty(), "an edited file left the repository clean");

        git(&root, &["add", "a.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "second",
            ],
        );
        assert!(!repo.is_dirty(), "a commit left the repository dirty");
    }

    #[test]
    fn the_work_tree_changes_are_listed_with_their_kind() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");

        std::fs::write(root.join("a.txt"), "one\nedited\n").unwrap();
        std::fs::remove_file(root.join("b.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "fresh\n").unwrap();

        let changes = repo.changes().expect("status");
        let seen: Vec<(String, char)> = changes
            .iter()
            .map(|change| (change.path.display().to_string(), change.kind.letter()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("a.txt".to_owned(), 'M'),
                ("b.txt".to_owned(), 'D'),
                ("new.txt".to_owned(), '?'),
            ],
            "the work tree changes are wrong"
        );
        assert!(changes.iter().all(|change| change.area == Area::Worktree));
        assert_eq!(changes[0].kind, Kind::Modified);
    }

    #[test]
    fn a_staged_change_is_told_apart_from_a_work_tree_change() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");

        std::fs::write(root.join("a.txt"), "one\nstaged\n").unwrap();
        git(&root, &["add", "a.txt"]);
        std::fs::write(root.join("a.txt"), "one\nstaged\nthen edited\n").unwrap();

        let changes = repo.changes().expect("status");
        let areas: Vec<(String, char, &str)> = changes
            .iter()
            .map(|change| {
                (
                    change.path.display().to_string(),
                    change.kind.letter(),
                    match change.area {
                        Area::Staged => "index",
                        Area::Worktree => "worktree",
                    },
                )
            })
            .collect();
        assert_eq!(
            areas,
            vec![
                ("a.txt".to_owned(), 'M', "index"),
                ("a.txt".to_owned(), 'M', "worktree"),
            ],
            "a half-staged file was not split in two"
        );
    }

    #[test]
    fn a_clean_repository_has_nothing_to_show() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        assert_eq!(repo.changes().expect("status"), vec![]);
    }
}
