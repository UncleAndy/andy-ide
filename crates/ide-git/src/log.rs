//! History: the commits behind `HEAD`, and what each one changed.
//!
//! Walked natively, newest first, and each commit's diff is built from the trees it
//! points at — the same reader that answers for the work tree, so a commit and a
//! working change are described in the same words.

use std::path::PathBuf;

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;

use crate::Repo;

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
}

impl Repo {
    /// The last `limit` commits of the checked-out branch, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        let head = match self.inner().head_commit() {
            Ok(head) => head,
            // An unborn `HEAD` has no history yet; that is a state, not a failure.
            Err(_) => return Ok(Vec::new()),
        };

        let mut commits = Vec::new();
        for info in head.ancestors().all()?.take(limit) {
            let info = info?;
            let commit = info.object()?;
            let author = commit.author()?;
            let summary = commit.message()?.summary().to_string();
            commits.push(Commit {
                id: info.id().to_string(),
                short: info.id().shorten()?.to_string(),
                summary,
                author: String::from_utf8_lossy(author.name.trim()).into_owned(),
                when: author.time().unwrap_or_default().seconds,
            });
        }
        Ok(commits)
    }

    /// The diff a commit introduced, against its first parent.
    ///
    /// A root commit is compared against the empty tree, which is what `git show`
    /// does: everything it contains reads as added.
    pub fn commit_diff(&self, rev: &str) -> Result<String> {
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

        let mut out = String::new();
        for change in changes {
            let (path, old, new) = self.sides_of(&change)?;
            out.push_str(&crate::diff::unified(&old, &new, &path));
        }
        Ok(out)
    }

    /// The two sides of one tree change, as bytes.
    fn sides_of(
        &self,
        change: &gix::object::tree::diff::ChangeDetached,
    ) -> Result<(PathBuf, Vec<u8>, Vec<u8>)> {
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
        Ok((path, self.blob(old_id)?, self.blob(new_id)?))
    }

    /// The bytes of a blob, or nothing when it is not there.
    fn blob(&self, id: Option<gix::ObjectId>) -> Result<Vec<u8>> {
        let Some(id) = id else {
            return Ok(Vec::new());
        };
        Ok(self.inner().find_object(id)?.into_blob().data.to_vec())
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
