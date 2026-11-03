//! History: the commits behind `HEAD`, and what each one changed.
//!
//! Walked natively, newest first, and each commit's diff is built from the trees it
//! points at — the same reader that answers for the work tree, so a commit and a
//! working change are described in the same words.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;

use crate::{Kind, Repo};

/// Which kind of name points at a commit.
///
/// The order is the order the panel shows the names in: `HEAD` first, because it is where the
/// person is, then the branches of this repository, then the remote ones, then the tags.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RefKind {
    /// `HEAD` itself.
    Head,
    /// A branch of this repository.
    Local,
    /// A branch of a remote: `origin/main`.
    Remote,
    /// A tag.
    Tag,
}

/// One name that points at a commit: `HEAD`, `main`, `origin/main`, `tag: v1.0`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RefMark {
    /// The name as `git log --decorate` prints it.
    pub name: String,
    pub kind: RefKind,
}

/// One commit, as the history panel shows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Commit {
    /// The full id, for asking about it later.
    pub id: String,
    /// Seven characters of it, as `git log --oneline` prints.
    pub short: String,
    /// The first line of the message.
    pub summary: String,
    /// Who wrote it.
    pub author: String,
    /// When, in seconds since the epoch.
    pub when: i64,
    /// The names that point at this commit, in the order they are shown.
    pub refs: Vec<RefMark>,
}

/// One file a commit touched, and what the commit did to it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitFile {
    /// Repository-relative path.
    pub path: PathBuf,
    /// What happened to it.
    pub kind: Kind,
    /// How many lines the commit added.
    pub added: usize,
    /// How many it removed.
    pub removed: usize,
    /// The unified diff of this file alone.
    pub text: String,
}

/// One side of a tree change: the bytes of a file, or nothing when the entry is not a file at all.
type Content = Option<Vec<u8>>;

/// A tree change as this reader sees it: the path it touched and its two sides.
type Sides = (PathBuf, Content, Content);

/// How many lines a unified diff adds and removes: the hunk lines, not the `+++`/`---` headings.
fn count_lines(diff: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

impl Repo {
    /// The last `limit` commits of the checked-out branch, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        let head = match self.inner().head_commit() {
            Ok(head) => head,
            // An unborn `HEAD` has no history yet; that is a state, not a failure.
            Err(_) => return Ok(Vec::new()),
        };
        // Where the names point is asked once for the whole walk: a ref is a file, and reading
        // them per commit would read the same handful of files two hundred times.
        let marks = self.marks()?;

        let mut commits = Vec::new();
        for info in head.ancestors().all()?.take(limit) {
            let info = info?;
            let commit = info.object()?;
            let author = commit.author()?;
            let summary = commit.message()?.summary().to_string();
            let id = info.id();
            commits.push(Commit {
                id: id.to_string(),
                short: id.shorten()?.to_string(),
                summary,
                author: String::from_utf8_lossy(author.name.trim()).into_owned(),
                when: author.time().unwrap_or_default().seconds,
                refs: marks.get(&id.detach()).cloned().unwrap_or_default(),
            });
        }
        Ok(commits)
    }

    /// Every name that points at a commit, by commit.
    ///
    /// The names a person sees in `git log --decorate`: `HEAD`, the branches, the remote branches
    /// and the tags. A symbolic reference has no commit of its own — `origin/HEAD` is one — so it
    /// is peeled to the commit it names and stands beside the name it points at.
    fn marks(&self) -> Result<HashMap<gix::ObjectId, Vec<RefMark>>> {
        let mut marks: HashMap<gix::ObjectId, Vec<RefMark>> = HashMap::new();
        if let Ok(head) = self.inner().head_commit() {
            marks.entry(head.id().detach()).or_default().push(RefMark {
                name: "HEAD".to_owned(),
                kind: RefKind::Head,
            });
        }

        for reference in self.inner().references()?.all()? {
            let mut reference = reference?;
            let kind = match reference.name().category() {
                Some(gix::reference::Category::LocalBranch) => RefKind::Local,
                Some(gix::reference::Category::RemoteBranch) => RefKind::Remote,
                Some(gix::reference::Category::Tag) => RefKind::Tag,
                // Notes, the stash and the pseudo-refs of a worktree are not names a person
                // reads beside a commit.
                _ => continue,
            };
            let short = reference.name().shorten().to_string();
            let Ok(id) = reference.peel_to_id() else {
                // A broken reference is the repository's problem, not the panel's: the rest of
                // the history is still worth showing.
                continue;
            };
            marks.entry(id.detach()).or_default().push(RefMark {
                name: if kind == RefKind::Tag {
                    format!("tag: {short}")
                } else {
                    short
                },
                kind,
            });
        }

        for names in marks.values_mut() {
            names.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
        }
        Ok(marks)
    }

    /// The diff a commit introduced, against its first parent.
    ///
    /// A root commit is compared against the empty tree, which is what `git show`
    /// does: everything it contains reads as added. The whole is the parts of `commit_files`
    /// joined, so the whole and its parts cannot tell different stories.
    pub fn commit_diff(&self, rev: &str) -> Result<String> {
        Ok(self
            .commit_files(rev)?
            .into_iter()
            .map(|file| file.text)
            .collect())
    }

    /// The files a commit touched, each with its own diff and the lines it added and removed.
    ///
    /// One walk of the tree diff against the first parent: the list, the counts and the diffs come
    /// from the same comparison, and a commit is read by walking what it changed.
    pub fn commit_files(&self, rev: &str) -> Result<Vec<CommitFile>> {
        let commit = self
            .inner()
            .rev_parse_single(rev)
            .with_context(|| format!("no such commit: {rev}"))?
            .object()?
            .into_commit();
        let new_tree = commit.tree()?;

        let old_tree = match commit.parent_ids().next() {
            Some(parent) => Some(self.inner().find_object(parent)?.into_commit().tree()?),
            None => None,
        };

        let changes = self
            .inner()
            .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)?;

        let mut files = Vec::new();
        for change in changes {
            let kind = match &change {
                gix::object::tree::diff::ChangeDetached::Addition { .. } => Kind::Added,
                gix::object::tree::diff::ChangeDetached::Deletion { .. } => Kind::Deleted,
                gix::object::tree::diff::ChangeDetached::Modification { .. } => Kind::Modified,
                gix::object::tree::diff::ChangeDetached::Rewrite { .. } => Kind::Renamed,
            };
            let (path, old, new) = self.sides_of(&change)?;
            let (Some(old), Some(new)) = (old, new) else {
                // A path that turned into a directory arrives as its tree, and a tree has no lines to
                // read: the files inside it are entries of the diff already, and a row of two headers
                // with nothing under them says less than no row at all.
                continue;
            };
            let text = crate::diff::unified(&old, &new, &path);
            let (added, removed) = count_lines(&text);
            files.push(CommitFile {
                path,
                kind,
                added,
                removed,
                text,
            });
        }
        // The order `git show --stat` prints: by path, so two commits that touch the same files
        // read the same way.
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// The two sides of one tree change, as bytes — `None` for a side that is not a file at all.
    fn sides_of(&self, change: &gix::object::tree::diff::ChangeDetached) -> Result<Sides> {
        use gix::object::tree::diff::ChangeDetached as Changed;
        let (path, old_id, new_id) = match change {
            Changed::Addition { id, location, .. } => (location, None, Some(*id)),
            Changed::Deletion { id, location, .. } => (location, Some(*id), None),
            Changed::Modification {
                previous_id,
                id,
                location,
                ..
            } => (location, Some(*previous_id), Some(*id)),
            Changed::Rewrite { id, location, .. } => (location, None, Some(*id)),
        };
        let path = PathBuf::from(String::from_utf8_lossy(path.as_ref()).into_owned());
        Ok((path, self.content(old_id)?, self.content(new_id)?))
    }

    /// The content of one side of a change: the bytes of a file, `None` when the entry is not a file.
    ///
    /// A tree diff reports entries that are not files at all. A submodule is a commit and reads the way
    /// `git diff` writes it (`Subproject commit <id>`); a path that turned from a file into a directory
    /// arrives as a tree, which has no lines of its own. Reading either through `into_blob` panics —
    /// "Object must be a blob: Tree(…)" — which is what pressing Enter on such a commit used to do.
    fn content(&self, id: Option<gix::ObjectId>) -> Result<Content> {
        let Some(id) = id else {
            return Ok(Some(Vec::new()));
        };
        let object = self.inner().find_object(id)?;
        Ok(match object.kind {
            gix::object::Kind::Blob => Some(object.into_blob().data.to_vec()),
            gix::object::Kind::Commit => Some(format!("Subproject commit {id}\n").into_bytes()),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        git(&root, &["commit", "-q", "-am", "second"]);
        (dir, root)
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git ran");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn the_history_is_newest_first_with_its_authors() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");

        let log = repo.log(10).expect("a history");
        let summaries: Vec<&str> = log.iter().map(|commit| commit.summary.as_str()).collect();
        assert_eq!(summaries, vec!["second", "first"]);
        assert_eq!(log[0].author, "Test");
        assert_eq!(log[0].short.len(), 7, "the short id is not a short id");
        assert_eq!(log[0].id.len(), 40);
        assert!(log[0].when > 0, "the commit has no time");
    }

    #[test]
    fn a_limit_stops_the_walk() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        assert_eq!(repo.log(1).expect("a history").len(), 1);
    }

    #[test]
    fn the_names_of_the_repository_stand_beside_the_commits_they_point_at() {
        let (_dir, root) = scratch();
        // What a clone leaves behind: a remote branch, and the symbolic `origin/HEAD` that names
        // the remote's default branch. A local branch of the person's own is there too.
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        git(&root, &["branch", "feature"]);
        let repo = Repo::discover(&root).expect("the repository");

        let log = repo.log(10).expect("a history");
        let names = |commit: &Commit| -> Vec<String> {
            commit
                .refs
                .iter()
                .map(|mark| mark.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&log[0]),
            vec!["HEAD", "feature", "main", "origin/HEAD", "origin/main"],
            "the names of the newest commit are wrong or out of order"
        );
        assert!(
            log[1].refs.is_empty(),
            "a name was hung on a commit it does not point at: {:?}",
            log[1].refs
        );
    }

    #[test]
    fn a_commit_is_listed_by_the_files_it_touched_with_what_it_did_to_them() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        let head = repo.log(1).expect("a history")[0].id.clone();

        let files = repo.commit_files(&head).expect("the files of a commit");
        assert_eq!(files.len(), 1, "the commit touched {} files", files.len());
        assert_eq!(files[0].path, PathBuf::from("a.txt"));
        assert_eq!(files[0].kind, Kind::Modified);
        assert_eq!(
            (files[0].added, files[0].removed),
            (1, 0),
            "the counts of the change are wrong: {:?}",
            files[0]
        );
        assert!(
            files[0].text.contains("+two"),
            "the diff of the file is missing: {}",
            files[0].text
        );
    }

    #[test]
    fn a_file_a_commit_created_reads_as_added() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        let first = repo.log(10).expect("a history")[1].id.clone();

        let files = repo.commit_files(&first).expect("the files of a commit");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].kind, Kind::Added);
        assert_eq!((files[0].added, files[0].removed), (1, 0));
    }

    #[test]
    fn a_commit_that_turns_a_file_into_a_directory_is_read_without_a_panic() {
        // The tree diff reports the path that became a directory as its tree, and reading a tree
        // through `into_blob` panics ("Object must be a blob: Tree(…)") — which is what pressing
        // Enter on such a commit did. The tree has no lines of its own; the files inside it do.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("x"), "one\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "a file"]);
        std::fs::remove_file(root.join("x")).unwrap();
        std::fs::create_dir(root.join("x")).unwrap();
        std::fs::write(root.join("x/y"), "two\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "a directory"]);

        let repo = Repo::discover(&root).expect("the repository");
        let head = repo.log(1).expect("a history")[0].id.clone();
        let files = repo.commit_files(&head).expect("the files of a commit");
        let seen: Vec<(String, Kind)> = files
            .iter()
            .map(|file| (file.path.display().to_string(), file.kind))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("x".to_owned(), Kind::Deleted),
                ("x/y".to_owned(), Kind::Added),
            ],
            "the change reads as something other than the file going and the file arriving"
        );
        assert!(
            files[0].text.contains("-one"),
            "the file that went away left no lines: {}",
            files[0].text
        );
    }

    #[test]
    fn a_submodule_reads_as_the_commit_it_points_at() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        let first = String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&root)
                .output()
                .expect("git ran")
                .stdout,
        )
        .trim()
        .to_owned();
        // A gitlink entry: what a submodule is in the tree, and an object that is not a blob.
        git(
            &root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{first},sub"),
            ],
        );
        git(&root, &["commit", "-q", "-m", "a submodule"]);

        let repo = Repo::discover(&root).expect("the repository");
        let head = repo.log(1).expect("a history")[0].id.clone();
        let files = repo.commit_files(&head).expect("the files of a commit");
        assert_eq!(files.len(), 1, "the submodule is not the one file changed");
        assert_eq!(files[0].path, PathBuf::from("sub"));
        assert!(
            files[0].text.contains("Subproject commit "),
            "the submodule does not read as its commit: {}",
            files[0].text
        );
    }

    #[test]
    fn a_commit_diff_is_the_change_it_made() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        let head = repo.log(1).expect("a history")[0].id.clone();

        let diff = repo.commit_diff(&head).expect("a diff");
        assert!(diff.contains("+two"), "the added line is missing: {diff}");
        assert!(
            !diff.contains("-one"),
            "the diff reaches past its own commit: {diff}"
        );
    }

    #[test]
    fn the_first_commit_reads_as_all_additions() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        let root_commit = repo.log(10).expect("a history")[1].id.clone();

        let diff = repo.commit_diff(&root_commit).expect("a diff");
        assert!(
            diff.contains("+one"),
            "the first commit added nothing: {diff}"
        );
        let removed: Vec<&str> = diff
            .lines()
            .filter(|line| line.starts_with('-') && !line.starts_with("---"))
            .collect();
        assert!(
            removed.is_empty(),
            "the first commit removed something: {removed:?}"
        );
    }
}
