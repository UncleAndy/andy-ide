//! A session on a real pseudo-terminal.
//!
//! These tests start real processes: the point of the crate is that escape sequences
//! and ioctls behave the way a real program expects, and a mock would test nothing.

use std::time::{Duration, Instant};

use ide_term::Session;

/// Waits until the screen holds `needle`, or gives up.
fn wait_for(session: &Session, needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if session.contents().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn what_the_program_prints_appears_on_the_screen() {
    let session = Session::spawn("bash", &["-c".to_owned(), "echo hello".to_owned()], 80, 24)
        .expect("a session starts");
    assert!(
        wait_for(&session, "hello"),
        "nothing on the screen: {:?}",
        session.contents()
    );
}

#[test]
fn typing_reaches_the_program_and_its_answer_comes_back() {
    let mut session = Session::shell(80, 24).expect("a shell starts");
    session
        .write(b"echo typed-ok\n")
        .expect("the write goes through");
    assert!(
        wait_for(&session, "typed-ok"),
        "no answer from the shell: {:?}",
        session.contents()
    );
    session.kill();
}

#[test]
fn the_child_is_told_about_the_new_size() {
    let mut session = Session::spawn("bash", &["-c".to_owned(), "sleep 5".to_owned()], 80, 24)
        .expect("a session starts");
    assert_eq!(session.size(), (80, 24));
    session.resize(120, 40).expect("the resize goes through");
    assert_eq!(session.size(), (120, 40));
    let (rows, cols) = session.with_screen(|screen| screen.size());
    assert_eq!(
        (rows.get(), cols.get()),
        (40, 120),
        "the screen was not resized with the pty"
    );
    session.kill();
}

#[test]
fn a_program_that_exits_is_seen_as_gone() {
    let session = Session::spawn("bash", &["-c".to_owned(), "exit 0".to_owned()], 80, 24)
        .expect("a session starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && session.is_alive() {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !session.is_alive(),
        "the finished child still counts as alive"
    );
}

#[test]
fn a_program_that_does_not_exist_is_an_error_not_a_panic() {
    let session = Session::spawn("/definitely/not/here", &[], 80, 24);
    assert!(session.is_err(), "a missing program started a session");
}
