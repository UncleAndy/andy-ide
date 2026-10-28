//! Who last touched a line, and when — asked for, not shown all the time.
//!
//! Walked natively like the rest of the reading: `gix` blames the file against `HEAD`
//! without starting a process. The whole file is blamed in one go and the answer is
//! kept by the caller, because a line-by-line query would walk the same history once
//! per line; asking about another line of the same file then costs nothing.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use gix::bstr::ByteSlice;

use crate::Repo;

/// One line of the file, and the commit that last changed it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlameLine {
    /// Zero-based, as the editor counts lines.
    pub line: usize,
    /// Seven characters of the commit id.
    pub short: String,
    /// Who wrote that commit.
    pub author: String,
    /// When, in seconds since the epoch.
    pub when: i64,
    /// The first line of its message.
    pub summary: String,
}

/// Seven characters of an id, as `git log --oneline` prints them.
fn short_of(id: gix::ObjectId) -> String {
    let full = id.to_string();
    full.chars().take(7).collect()
}

impl Repo {
    /// Blames every line of a file that exists in `HEAD`.
    ///
    /// A file that is not committed yet has no blame to give: there is no commit that
    /// introduced its lines, and an empty answer says so without inventing an author.
    pub fn blame(&self, path: &Path) -> Result<Vec<BlameLine>> {
        let head = match self.inner().head_commit() {
            Ok(head) => head,
            Err(_) => return Ok(Vec::new()),
        };

        let name = path.to_string_lossy().into_owned();
        // A file that `HEAD` does not have has no line that any commit introduced: the
        // walk would fail on it, and an empty answer is the truth.
        if head.tree()?.lookup_entry_by_path(name.as_str())?.is_none() {
            return Ok(Vec::new());
        }

        let outcome = self.inner().blame_file(
            name.as_str().into(),
            head.id,
            gix::repository::blame_file::Options::default(),
        )?;

        let mut seen: HashMap<gix::ObjectId, (String, String, i64, String)> = HashMap::new();
        let mut lines = Vec::new();
        for entry in outcome.entries {
            let (short, author, when, summary) = match seen.get(&entry.commit_id) {
                Some(known) => known.clone(),
                None => {
                    let commit = self.inner().find_commit(entry.commit_id)?;
                    let signature = commit.author()?;
                    let known = (
                        short_of(entry.commit_id),
                        String::from_utf8_lossy(signature.name.trim()).into_owned(),
                        signature.time().unwrap_or_default().seconds,
                        commit.message()?.summary().to_string(),
                    );
                    seen.insert(entry.commit_id, known.clone());
                    known
                }
            };
            for line in entry.range_in_blamed_file() {
                lines.push(BlameLine {
                    line,
                    short: short.clone(),
                    author: author.clone(),
                    when,
                    summary: summary.clone(),
                });
            }
        }

        // The entries come in file order already; sorting keeps that true if the walk
        // ever yields them in another order.
        lines.sort_by_key(|line| line.line);
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repo;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "First")
            .env("GIT_AUTHOR_EMAIL", "first@example.com")
            .env("GIT_COMMITTER_NAME", "First")
            .env("GIT_COMMITTER_EMAIL", "first@example.com")
            .output()
            .expect("git ran");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn repository() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "First"]);
        git(root, &["config", "user.email", "first@example.com"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-q", "-m", "the beginning"]);

        // A second commit that only touches the second line, by another author.
        std::fs::write(root.join("a.txt"), "one\nsecond\n").unwrap();
        Command::new("git")
            .args(["commit", "-q", "-am", "a later hand"])
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "Second")
            .env("GIT_AUTHOR_EMAIL", "second@example.com")
            .env("GIT_COMMITTER_NAME", "Second")
            .env("GIT_COMMITTER_EMAIL", "second@example.com")
            .output()
            .expect("git ran");

        let repo = Repo::discover(root).expect("the repository");
        (dir, repo)
    }

    #[test]
    fn each_line_names_the_commit_that_last_touched_it() {
        let (_dir, repo) = repository();
        let lines = repo.blame(Path::new("a.txt")).expect("the blame");

        assert_eq!(lines.len(), 2, "the file has two lines");
        assert_eq!(lines[0].line, 0);
        assert_eq!(lines[0].author, "First");
        assert_eq!(lines[0].summary, "the beginning");
        assert_eq!(lines[1].line, 1);
        assert_eq!(lines[1].author, "Second");
        assert_eq!(lines[1].summary, "a later hand");
        assert!(
            !lines[1].short.is_empty() && lines[1].short.len() >= 7,
            "the commit is not named: {:?}",
            lines[1]
        );
    }

    #[test]
    fn lines_from_one_commit_agree_with_each_other_and_with_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "First"]);
        git(root, &["config", "user.email", "first@example.com"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-q", "-m", "one hand, three lines"]);

        let repo = Repo::discover(root).expect("the repository");
        let lines = repo.blame(Path::new("a.txt")).expect("the blame");
        assert_eq!(lines.len(), 3);
        assert!(
            lines.iter().all(|line| line.short == lines[0].short),
            "three lines of one commit carry different ids: {lines:?}"
        );
        assert_eq!(
            lines[0].short,
            repo.log(1).expect("the history")[0].short,
            "the blame and the history disagree about the same commit"
        );
    }

    #[test]
    fn a_file_that_is_not_committed_has_no_blame() {
        let (dir, repo) = repository();
        std::fs::write(dir.path().join("new.txt"), "fresh\n").unwrap();
        assert!(
            repo.blame(Path::new("new.txt"))
                .expect("the blame")
                .is_empty(),
            "an uncommitted file was given a blame"
        );
    }

    #[test]
    fn a_repository_without_commits_has_no_blame() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let repo = Repo::discover(dir.path()).expect("the repository");
        assert!(
            repo.blame(Path::new("a.txt"))
                .expect("the blame")
                .is_empty(),
            "an unborn HEAD produced a blame"
        );
    }
}
