//! A file outside the project gets its directory watched.

use std::sync::mpsc;
use std::time::Duration;

use ide_core::{watch_extra_file, watch_project};

#[test]
fn a_change_to_a_file_outside_the_root_is_reported() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("dep.rs");
    std::fs::write(&file, "one\n").unwrap();

    let (tx, rx) = mpsc::channel();
    let mut watcher = watch_project(project.path(), tx).unwrap();
    watch_extra_file(&mut watcher, &file).unwrap();

    // The watch is registered asynchronously; the write comes after it settles.
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(&file, "two\n").unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                let hit = event.path() == file;
                seen.push(event.path().to_path_buf());
                if hit {
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    assert!(
        seen.iter().any(|path| path == &file),
        "no event for the outside file; got {seen:?}"
    );
}

#[test]
fn a_file_without_a_parent_is_not_an_error() {
    let project = tempfile::tempdir().unwrap();
    let (tx, _rx) = mpsc::channel();
    let mut watcher = watch_project(project.path(), tx).unwrap();
    assert!(watch_extra_file(&mut watcher, std::path::Path::new("/")).is_ok());
}

#[test]
fn a_path_that_does_not_exist_is_reported_and_not_panicked_on() {
    let project = tempfile::tempdir().unwrap();
    let (tx, _rx) = mpsc::channel();
    let mut watcher = watch_project(project.path(), tx).unwrap();
    let missing = std::path::Path::new("/definitely/not/here/dep.rs");
    assert!(
        watch_extra_file(&mut watcher, missing).is_err(),
        "a missing directory was watched silently"
    );
}
