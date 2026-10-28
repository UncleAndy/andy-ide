//! Watching the project tree: what the interface has to do when a file changes
//! behind its back.

use std::sync::mpsc;
use std::time::Duration;

use ide_core::watch::{ExternalChange, FsEvent, decide_external_change, watch_project};

#[test]
fn a_document_without_edits_is_reloaded() {
    assert!(matches!(
        decide_external_change(false),
        ExternalChange::Reload
    ));
}

#[test]
fn a_document_with_edits_asks_the_user() {
    assert!(matches!(
        decide_external_change(true),
        ExternalChange::Prompt
    ));
}

/// The watcher is a background thread that reports asynchronously: the test
/// waits for the event rather than assuming a delay.
#[test]
fn a_modified_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one").unwrap();

    let (tx, rx) = mpsc::channel();
    let _watcher = watch_project(dir.path(), tx).unwrap();
    // A new watcher needs a moment before it sees anything.
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(&file, "two").unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(FsEvent::Changed(path)) | Ok(FsEvent::Created(path)) => {
                if path.ends_with("a.txt") {
                    seen = true;
                    break;
                }
            }
            Ok(FsEvent::Removed(_)) => {}
            Err(_) => continue,
        }
    }
    assert!(seen, "a modification of a.txt was never reported");
}

#[test]
fn a_new_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let _watcher = watch_project(dir.path(), tx).unwrap();
    std::thread::sleep(Duration::from_millis(200));

    std::fs::write(dir.path().join("fresh.txt"), "x").unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(FsEvent::Created(path)) | Ok(FsEvent::Changed(path)) => {
                if path.ends_with("fresh.txt") {
                    seen = true;
                    break;
                }
            }
            Ok(FsEvent::Removed(_)) => {}
            Err(_) => continue,
        }
    }
    assert!(seen, "a new file was never reported");
}

/// The editor writes its own state under `.andy-ide` every half second and
/// builds land in `target`; both would flood the queue.
#[test]
fn build_and_state_directories_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".andy-ide/cache")).unwrap();

    let (tx, rx) = mpsc::channel();
    let _watcher = watch_project(dir.path(), tx).unwrap();
    std::thread::sleep(Duration::from_millis(200));

    std::fs::write(dir.path().join("target/debug/artifact"), "x").unwrap();
    std::fs::write(dir.path().join(".git/HEAD"), "x").unwrap();
    std::fs::write(dir.path().join(".andy-ide/workspace.json"), "x").unwrap();
    // Something legitimate after the noise, to know the queue has been read.
    std::fs::write(dir.path().join("real.txt"), "x").unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut noise = Vec::new();
    let mut seen_real = false;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                let path = match &event {
                    FsEvent::Created(path) | FsEvent::Changed(path) | FsEvent::Removed(path) => {
                        path
                    }
                };
                if path.ends_with("real.txt") {
                    seen_real = true;
                    break;
                }
                noise.push(path.clone());
            }
            Err(_) => continue,
        }
    }
    assert!(seen_real, "the real file was never reported");
    assert!(noise.is_empty(), "ignored directories leaked: {noise:?}");
}
