//! Diffs: what a change would do to a file, in the form a person reads.
//!
//! Both sides are read where they really live: `HEAD`'s tree and the index are objects
//! in the database, the work tree is a file on disk. Nothing is checked out or stashed
//! to produce a diff, so showing one cannot disturb the work in progress.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use similar::TextDiff;

use crate::Repo;

/// Which comparison to make.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// `HEAD` against the index: what the next commit would contain.
    Staged,
    /// The index against the file on disk: what has not been added yet.
    Worktree,
}

/// One file's diff.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileDiff {
    pub path: PathBuf,
    pub side: Side,
    /// `false` when the file is binary: the panel says so rather than print bytes.
    pub text: bool,
    /// The unified diff, ready to be shown line by line.
    pub unified: String,
}

impl FileDiff {
    /// The diff cut into lines, with the `+`/`-`/` ` marker still on each.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.unified.lines()
    }
}

/// What the margin of the editor shows beside a line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkKind {
    /// A line the file has and the index does not.
    Added,
    /// A line both have, with different text.
    Modified,
    /// A line the index has and the file no longer does. The mark sits on the line
    /// that follows the place it used to be, as `git diff` prints it.
    Deleted,
}

/// One mark in the margin.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineMark {
    /// Zero-based, as the editor counts lines.
    pub line: usize,
    pub kind: MarkKind,
}

impl Repo {
    /// The marks for the margin of the editor: what the file has and the index does
    /// not, line by line.
    ///
    /// Compared against the index and not against `HEAD`: the margin answers "what of
    /// this file is not committed yet", and a staged change is not yet committed
    /// either.
    pub fn changed_lines(&self, path: &Path) -> Result<Vec<LineMark>> {
        let old = self.blob_in_index(path)?.unwrap_or_default();
        let new = self.blob_on_disk(path)?.unwrap_or_default();
        if is_binary(&old) || is_binary(&new) {
            return Ok(Vec::new());
        }
        Ok(marks(&old, &new))
    }

    /// The diff of one file, against the index or against `HEAD`.
    pub fn diff(&self, path: &Path, side: Side) -> Result<FileDiff> {
        let old = match side {
            Side::Staged => self.blob_at_head(path)?,
            Side::Worktree => self.blob_in_index(path)?,
        };
        let new = match side {
            Side::Staged => self.blob_in_index(path)?,
            Side::Worktree => self.blob_on_disk(path)?,
        };

        let old = old.unwrap_or_default();
        let new = new.unwrap_or_default();
        if is_binary(&old) || is_binary(&new) {
            return Ok(FileDiff {
                path: path.to_path_buf(),
                side,
                text: false,
                unified: format!("Binary file {}", path.display()),
            });
        }

        Ok(FileDiff {
            path: path.to_path_buf(),
            side,
            text: true,
            unified: unified(&old, &new, path),
        })
    }

    /// The bytes `HEAD`'s tree holds for a path.
    fn blob_at_head(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let Ok(tree) = self.inner().head_tree() else {
            // An unborn `HEAD` has no tree at all: every file in the index is new.
            return Ok(None);
        };
        self.bytes_of(
            tree.lookup_entry_by_path(path)?
                .map(|entry| entry.object_id()),
        )
    }

    /// The bytes the index holds for a path.
    fn blob_in_index(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let index = self.inner().index_or_empty()?;
        let name = path.to_string_lossy().into_owned();
        self.bytes_of(
            index
                .entry_by_path(name.as_str().into())
                .map(|entry| entry.id),
        )
    }

    /// The bytes the file has on disk, right now.
    ///
    /// A file that is gone reads as empty: deleting a file is a change like any
    /// other, and its diff is every line going away.
    fn blob_on_disk(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let Some(root) = self.root() else {
            return Ok(None);
        };
        let full = root.join(path);
        match std::fs::read(&full) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", full.display())),
        }
    }

    fn bytes_of(&self, id: Option<gix::ObjectId>) -> Result<Option<Vec<u8>>> {
        let Some(id) = id else {
            return Ok(None);
        };
        let object = self.inner().find_object(id)?;
        Ok(Some(object.into_blob().data.to_vec()))
    }
}

/// Whether these bytes are a file a person reads.
fn is_binary(bytes: &[u8]) -> bool {
    // The same rule `git` uses: a NUL in the first few kilobytes.
    bytes.iter().take(8000).any(|byte| *byte == 0)
}

/// A unified diff of two texts, with the names `git` itself would use.
pub(crate) fn unified(old: &[u8], new: &[u8], path: &Path) -> String {
    let old = String::from_utf8_lossy(old);
    let new = String::from_utf8_lossy(new);
    let name = path.display();
    TextDiff::from_lines(&*old, &*new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{FileDiff, Side};
    use crate::Repo;
    use std::path::Path;

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(&root, &["add", "-A"]);
        commit(&root, "first");
        (dir, root)
    }

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
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

    fn commit(root: &Path, message: &str) {
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
    }

    fn changed(diff: &FileDiff) -> Vec<String> {
        diff.lines()
            .filter(|line| line.starts_with('+') || line.starts_with('-'))
            .filter(|line| !line.starts_with("+++") && !line.starts_with("---"))
            .map(|line| line.to_owned())
            .collect()
    }

    #[test]
    fn an_edit_is_shown_against_the_index() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        std::fs::write(root.join("a.txt"), "one\nTWO\nthree\n").unwrap();

        let diff = repo
            .diff(Path::new("a.txt"), Side::Worktree)
            .expect("a diff");
        assert!(diff.text, "a text file was taken for a binary one");
        assert_eq!(
            changed(&diff),
            vec!["-two".to_owned(), "+TWO".to_owned()],
            "the wrong lines were reported"
        );
        assert!(
            diff.unified.starts_with("--- a/a.txt"),
            "the diff has no header: {}",
            diff.unified
        );
    }

    #[test]
    fn a_staged_change_is_shown_against_head() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        git(&root, &["add", "a.txt"]);

        let diff = repo.diff(Path::new("a.txt"), Side::Staged).expect("a diff");
        assert_eq!(changed(&diff), vec!["+four".to_owned()]);
        // Nothing has been said about the work tree, so there is nothing there.
        let unstaged = repo
            .diff(Path::new("a.txt"), Side::Worktree)
            .expect("a diff");
        assert!(changed(&unstaged).is_empty(), "the index was not used");
    }

    #[test]
    fn a_new_file_is_all_additions() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        std::fs::write(root.join("b.txt"), "hello\n").unwrap();

        let diff = repo
            .diff(Path::new("b.txt"), Side::Worktree)
            .expect("a diff");
        assert_eq!(changed(&diff), vec!["+hello".to_owned()]);
    }

    #[test]
    fn a_deleted_file_is_all_removals() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        std::fs::remove_file(root.join("a.txt")).unwrap();

        let diff = repo
            .diff(Path::new("a.txt"), Side::Worktree)
            .expect("a diff");
        assert_eq!(
            changed(&diff),
            vec!["-one".to_owned(), "-two".to_owned(), "-three".to_owned()]
        );
    }

    #[test]
    fn a_binary_file_is_named_and_not_printed() {
        let (_dir, root) = scratch();
        let repo = Repo::discover(&root).expect("the repository");
        git(&root, &["add", "-A"]);
        std::fs::write(root.join("blob.bin"), [0u8, 1, 2, 3, 255]).unwrap();

        let diff = repo
            .diff(Path::new("blob.bin"), Side::Worktree)
            .expect("a diff");
        assert!(!diff.text, "a binary file was taken for text");
        assert_eq!(diff.unified, "Binary file blob.bin");
    }
}

/// Where the marks go, from a line diff of the two texts.
///
/// `similar` hands back one change per run of lines, and each run is turned into marks
/// on the new numbering: a line that only the new text has is `Added`, a run that
/// replaced an old one is `Modified` line by line, and a deletion is marked on the line
/// that took its place — or on the last line, when the file ended there.
fn marks(old: &[u8], new: &[u8]) -> Vec<LineMark> {
    let old = String::from_utf8_lossy(old);
    let new = String::from_utf8_lossy(new);
    let diff = TextDiff::from_lines(&*old, &*new);
    let new_len = new.lines().count();
    let mut marks = Vec::new();

    for op in diff.ops() {
        let old_range = op.old_range();
        let new_range = op.new_range();
        match op.tag() {
            similar::DiffTag::Equal => {}
            similar::DiffTag::Insert => {
                for line in new_range {
                    marks.push(LineMark {
                        line,
                        kind: MarkKind::Added,
                    });
                }
            }
            similar::DiffTag::Delete => marks.push(LineMark {
                line: new_range.start.min(new_len.saturating_sub(1)),
                kind: MarkKind::Deleted,
            }),
            similar::DiffTag::Replace => {
                for line in new_range.clone() {
                    marks.push(LineMark {
                        line,
                        kind: MarkKind::Modified,
                    });
                }
                // A replacement that removes more lines than it adds leaves the extra
                // removals with nowhere to show; the last replaced line carries them.
                if old_range.len() > new_range.len() {
                    marks.push(LineMark {
                        line: new_range.start.min(new_len.saturating_sub(1)),
                        kind: MarkKind::Deleted,
                    });
                }
            }
        }
    }

    marks.sort_by_key(|mark| mark.line);
    marks.dedup_by_key(|mark| mark.line);
    marks
}

#[cfg(test)]
mod mark_tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(usize, MarkKind)> {
        marks(source.as_bytes(), source.as_bytes())
            .into_iter()
            .map(|mark| (mark.line, mark.kind))
            .collect()
    }

    fn change(old: &str, new: &str) -> Vec<(usize, MarkKind)> {
        marks(old.as_bytes(), new.as_bytes())
            .into_iter()
            .map(|mark| (mark.line, mark.kind))
            .collect()
    }

    #[test]
    fn an_unchanged_file_has_no_marks() {
        assert_eq!(kinds("one\ntwo\n"), vec![]);
    }

    #[test]
    fn a_changed_line_is_marked_where_it_is() {
        assert_eq!(
            change("one\ntwo\nthree\n", "one\nTWO\nthree\n"),
            vec![(1, MarkKind::Modified)]
        );
    }

    #[test]
    fn an_added_line_is_marked() {
        assert_eq!(
            change("one\nthree\n", "one\ntwo\nthree\n"),
            vec![(1, MarkKind::Added)]
        );
    }

    #[test]
    fn a_removed_line_is_marked_on_the_line_that_took_its_place() {
        assert_eq!(
            change("one\ntwo\nthree\n", "one\nthree\n"),
            vec![(1, MarkKind::Deleted)]
        );
    }

    #[test]
    fn a_removed_last_line_is_marked_on_the_new_last_line() {
        assert_eq!(change("one\ntwo\n", "one\n"), vec![(0, MarkKind::Deleted)]);
    }

    #[test]
    fn marks_hold_the_work_tree_against_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .expect("git ran");
            assert!(output.status.success(), "{args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "a.txt"]);

        let repo = Repo::discover(root).expect("the repository");
        assert_eq!(
            repo.changed_lines(Path::new("a.txt")).expect("marks"),
            vec![]
        );

        std::fs::write(root.join("a.txt"), "one\nTWO\n").unwrap();
        assert_eq!(
            repo.changed_lines(Path::new("a.txt")).expect("marks"),
            vec![LineMark {
                line: 1,
                kind: MarkKind::Modified
            }]
        );
    }
}
