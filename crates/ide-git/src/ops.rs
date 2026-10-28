//! The operations that change the repository: staging, unstaging, committing.
//!
//! Reading is native (`gix`), writing is the system `git`. That is not laziness: a
//! commit runs hooks, may be signed, may rewrite the message through `commit.template`
//! and `prepare-commit-msg`, and none of that can be reproduced faithfully without
//! becoming `git`. A panel that silently bypassed a pre-commit hook would be worse
//! than one that is a second slow.
//!
//! Everything here returns what `git` said, because a hook's complaint is the answer
//! the user needs to see, word for word.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::Branch;

use crate::Repo;

impl Repo {
    /// Puts one file into the index, all of it.
    pub fn stage(&self, path: &Path) -> Result<String> {
        self.git(&["add", "--", &rel(path)])
    }

    /// Puts every change into the index, deletions included.
    pub fn stage_all(&self) -> Result<String> {
        self.git(&["add", "-A"])
    }

    /// Takes one file out of the index again, leaving the file on disk as it is.
    ///
    /// `git restore --staged` is the modern spelling and the one that also works for a
    /// file that is not in `HEAD`; on an unborn `HEAD` it cannot resolve the commit it
    /// should restore from, so `git rm --cached` — which is what "unstage a new file"
    /// means there — is the fallback.
    pub fn unstage(&self, path: &Path) -> Result<String> {
        let file = rel(path);
        match self.git(&["restore", "--staged", "--", &file]) {
            Ok(output) => Ok(output),
            Err(_) => self.git(&["rm", "--cached", "-q", "--", &file]),
        }
    }

    /// The local branches, the one checked out first.
    pub fn branches(&self) -> Result<Vec<Branch>> {
        let head = self.branch();
        let mut branches = Vec::new();
        for reference in self.inner().references()?.local_branches()? {
            let reference = reference?;
            let shortened = reference.name().shorten();
            let name = String::from_utf8_lossy(shortened.as_ref()).into_owned();
            branches.push(Branch {
                is_head: head.as_deref() == Some(name.as_str()),
                name,
            });
        }
        branches.sort_by(|a, b| b.is_head.cmp(&a.is_head).then(a.name.cmp(&b.name)));
        Ok(branches)
    }

    /// Moves `HEAD` to another branch.
    ///
    /// `git switch` refuses when the move would throw away local changes, and that
    /// refusal is the answer: it is shown, not worked around.
    pub fn switch_to(&self, name: &str) -> Result<String> {
        self.git(&["switch", name])
    }

    /// Starts a branch at `HEAD` and moves to it.
    pub fn create_branch(&self, name: &str) -> Result<String> {
        if name.trim().is_empty() {
            bail!("A branch needs a name.");
        }
        self.git(&["switch", "-c", name])
    }

    /// Commits what is in the index, and returns what `git` printed about it.
    ///
    /// An empty message is refused here, before anything runs: `git` would either
    /// abort or, with an editor configured, open one — and there is no editor to open.
    pub fn commit(&self, message: &str) -> Result<String> {
        if message.trim().is_empty() {
            bail!("A commit needs a message.");
        }
        self.git(&["commit", "-m", message])
    }

    /// Runs `git` in the work tree and hands back its output.
    fn git(&self, args: &[&str]) -> Result<String> {
        let root = self.root().context("the repository has no work tree")?;
        let output = Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .with_context(|| format!("running git {}", args.join(" ")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !output.status.success() {
            // Hooks and `git` itself explain refusals on stderr; the panel shows it.
            let said = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            bail!("{}", said.trim().to_owned());
        }
        Ok(if stdout.trim().is_empty() {
            stderr
        } else {
            stdout
        })
    }
}

/// A path as `git` wants it on the command line.
fn rel(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Area, Kind};

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&root)
            .status()
            .expect("git ran");
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        (dir, root)
    }

    /// A repository that has an identity, so commits can be made in a test.
    fn with_identity(root: &Path) -> Repo {
        let repo = Repo::discover(root).expect("the repository");
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .expect("git ran");
        }
        repo
    }

    #[test]
    fn the_branch_in_use_is_first_in_the_list() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);
        repo.stage_all().expect("staged");
        repo.commit("first").expect("committed");
        repo.create_branch("feature").expect("a branch");

        let branches = repo.branches().expect("the branches");
        let names: Vec<(&str, bool)> = branches
            .iter()
            .map(|branch| (branch.name.as_str(), branch.is_head))
            .collect();
        assert_eq!(names, vec![("feature", true), ("main", false)]);
        assert_eq!(repo.branch().as_deref(), Some("feature"));
    }

    #[test]
    fn switching_moves_head_to_another_branch() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);
        repo.stage_all().expect("staged");
        repo.commit("first").expect("committed");
        repo.create_branch("feature").expect("a branch");

        repo.switch_to("main").expect("switched");

        assert_eq!(repo.branch().as_deref(), Some("main"));
    }

    #[test]
    fn a_branch_that_does_not_exist_is_refused_by_git() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);
        repo.stage_all().expect("staged");
        repo.commit("first").expect("committed");

        let error = repo.switch_to("nope").expect_err("a branch appeared");
        assert!(
            !format!("{error}").trim().is_empty(),
            "git said nothing about the refusal"
        );
        assert_eq!(repo.branch().as_deref(), Some("main"), "HEAD moved anyway");
    }

    #[test]
    fn an_empty_branch_name_is_refused_before_git_runs() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);
        let error = repo.create_branch("   ").expect_err("a branch appeared");
        assert!(format!("{error}").contains("needs a name"));
    }

    #[test]
    fn staging_a_new_file_moves_it_into_the_index() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);

        repo.stage(Path::new("a.txt")).expect("staged");
        let changes = repo.changes().expect("status");
        assert_eq!(changes.len(), 1, "one change: {changes:?}");
        assert_eq!(changes[0].area, Area::Staged);
        assert_eq!(changes[0].kind, Kind::Added);
    }

    #[test]
    fn unstaging_leaves_the_file_where_it_is() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);
        repo.stage(Path::new("a.txt")).expect("staged");

        repo.unstage(Path::new("a.txt")).expect("unstaged");

        let changes = repo.changes().expect("status");
        assert_eq!(changes.len(), 1, "one change: {changes:?}");
        assert_eq!(changes[0].area, Area::Worktree);
        assert_eq!(changes[0].kind, Kind::Untracked);
        assert!(
            root.join("a.txt").exists(),
            "unstaging removed the file from disk"
        );
    }

    #[test]
    fn a_commit_empties_the_index_and_leaves_a_message() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);
        repo.stage_all().expect("staged all");

        repo.commit("first").expect("committed");

        assert!(repo.changes().expect("status").is_empty(), "still dirty");
        assert_eq!(repo.branch().as_deref(), Some("main"));
        let log = Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(&root)
            .output()
            .expect("git ran");
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "first");
    }

    #[test]
    fn an_empty_message_is_refused_before_git_runs() {
        let (_dir, root) = scratch();
        let repo = with_identity(&root);
        repo.stage_all().expect("staged all");

        let error = repo
            .commit("  ")
            .expect_err("an empty message was accepted");
        assert!(
            format!("{error}").contains("needs a message"),
            "the wrong complaint: {error}"
        );
        // Nothing was committed: the change is still there.
        assert!(!repo.changes().expect("status").is_empty());
    }
}
