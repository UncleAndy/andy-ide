//! Watching a project directory.
//!
//! The editor has to notice files that change behind its back: a formatter, a
//! git checkout, another editor. The watcher runs in its own thread and reports
//! through a channel, so the drawing code never blocks on the file system.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Directories whose changes are never interesting: build output, the git
/// database, and the editor's own state under `.andy-ide`, which is written
/// several times a second and would otherwise flood the queue.
const IGNORED: &[&str] = &["target", ".git", ".andy-ide"];

/// The watcher of one project; keep it alive while the events are wanted.
pub type ProjectWatcher = RecommendedWatcher;

/// A change on disk, with an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEvent {
    Created(PathBuf),
    Changed(PathBuf),
    Removed(PathBuf),
}

impl FsEvent {
    pub fn path(&self) -> &Path {
        match self {
            Self::Created(path) | Self::Changed(path) | Self::Removed(path) => path,
        }
    }
}

/// What to do with an open file that changed on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalChange {
    /// Nothing was edited here, so the new text can simply replace the old one.
    Reload,
    /// The buffer holds edits the file on disk does not have: ask the user.
    Prompt,
}

/// A document with unsaved edits is never overwritten silently.
pub fn decide_external_change(is_dirty: bool) -> ExternalChange {
    if is_dirty {
        ExternalChange::Prompt
    } else {
        ExternalChange::Reload
    }
}

/// Starts watching `root` and reports every change through `tx`.
///
/// The returned watcher must stay alive as long as the events are wanted:
/// dropping it stops the watch.
pub fn watch_project(root: &Path, tx: Sender<FsEvent>) -> Result<ProjectWatcher> {
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let Ok(event) = result else {
            return;
        };
        for path in event.paths {
            if is_ignored(&path) {
                continue;
            }
            let event = match event.kind {
                EventKind::Create(_) => FsEvent::Created(path),
                EventKind::Modify(_) => FsEvent::Changed(path),
                EventKind::Remove(_) => FsEvent::Removed(path),
                // Access events and the like say nothing about content.
                _ => continue,
            };
            if tx.send(event).is_err() {
                // The receiver is gone; the editor is shutting down.
                return;
            }
        }
    })
    .context("cannot create a file watcher")?;

    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("cannot watch {}", root.display()))?;
    Ok(watcher)
}

/// Whether a path under an ignored directory should be dropped.
fn is_ignored(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        IGNORED.contains(&name.as_ref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_output_is_ignored() {
        assert!(is_ignored(Path::new("/p/target/debug/x")));
        assert!(is_ignored(Path::new("/p/.git/index")));
        assert!(is_ignored(Path::new("/p/.andy-ide/workspace.json")));
        assert!(!is_ignored(Path::new("/p/src/main.rs")));
        // A file that merely starts with the name is not the directory.
        assert!(!is_ignored(Path::new("/p/targeted/main.rs")));
    }

    #[test]
    fn the_event_carries_its_path() {
        let event = FsEvent::Changed(PathBuf::from("/p/a.rs"));
        assert_eq!(event.path(), Path::new("/p/a.rs"));
    }
}
