//! A whole conversation with the stand-in agent: handshake, streaming, permissions.

use std::time::Duration;

use ide_agents::{AgentSpec, Event, PermissionPolicy, Session};

/// The stand-in agent, built with the crate and spawned like any other program.
fn stand_in() -> AgentSpec {
    AgentSpec {
        name: "stand-in".to_owned(),
        command: env!("CARGO_BIN_EXE_acp_test_agent").to_owned(),
        args: Vec::new(),
        env: Default::default(),
    }
}

/// A project directory with one file the agent may read.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "what the file says\n").unwrap();
    dir
}

fn start(policy: PermissionPolicy) -> (tempfile::TempDir, Session) {
    let dir = project();
    let session = Session::start(&stand_in(), dir.path(), policy).expect("the agent started");
    (dir, session)
}

/// Every event that has already arrived, without waiting for more.
fn drain(session: &mut Session) -> Vec<Event> {
    let mut events = Vec::new();
    while let Some(event) = session.next_event(Duration::from_millis(50)) {
        events.push(event);
    }
    events
}

fn text_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_session_says_hello_and_gets_an_id() {
    let (_dir, session) = start(PermissionPolicy::Deny);
    assert_eq!(session.id(), "stand-in-session");
}

#[test]
fn the_answer_streams_in_pieces() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    let reason = session
        .prompt("hello", Duration::from_secs(10))
        .expect("the turn");
    assert_eq!(reason, "end_turn");

    let events = drain(&mut session);
    assert_eq!(
        text_of(&events),
        "Hello",
        "the pieces did not add up to the answer: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Text(text) if text == "Hel")),
        "the answer arrived whole instead of in pieces: {events:?}"
    );
}

#[test]
fn the_agent_reads_a_file_through_the_client() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    session
        .prompt("read", Duration::from_secs(10))
        .expect("the turn");
    assert_eq!(text_of(&drain(&mut session)), "what the file says\n");
}

#[test]
fn a_permission_that_is_allowed_is_taken() {
    let (_dir, mut session) = start(PermissionPolicy::Allow);
    session
        .prompt("ask", Duration::from_secs(10))
        .expect("the turn");
    assert_eq!(text_of(&drain(&mut session)), "allowed");
}

#[test]
fn a_permission_that_is_refused_is_refused_in_the_agents_own_option() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    session
        .prompt("ask", Duration::from_secs(10))
        .expect("the turn");
    assert_eq!(text_of(&drain(&mut session)), "refused");
}

#[test]
fn a_permission_that_is_asked_reaches_the_panel_and_waits_for_the_answer() {
    let (_dir, mut session) = start(PermissionPolicy::Ask);

    // The agent cannot finish while it waits for the answer, so the turn times out —
    // which is exactly the state the panel is in: a question on screen, a turn in flight.
    let refused = session.prompt("ask", Duration::from_millis(600));
    assert!(refused.is_err(), "the turn ended before it was answered");

    let question = drain(&mut session)
        .into_iter()
        .find(|event| matches!(event, Event::Permission { .. }))
        .expect("the question never reached the caller");
    let Event::Permission { id, request } = question else {
        unreachable!()
    };
    assert_eq!(request.title(), "Run the tests");
    assert_eq!(request.option_of("allow_once"), Some("yes"));

    session
        .answer_permission(id, Some("yes"))
        .expect("the answer went back");
    assert_eq!(text_of(&drain(&mut session)), "allowed");
}

#[test]
fn writing_a_file_is_refused_until_the_editor_allows_it() {
    let (dir, mut session) = start(PermissionPolicy::Deny);
    session
        .prompt("write", Duration::from_secs(10))
        .expect("the turn");
    assert_eq!(text_of(&drain(&mut session)), "write refused");
    assert!(
        !dir.path().join("written.txt").exists(),
        "the file was written without permission"
    );

    session.allow_writes(true);
    session
        .prompt("write", Duration::from_secs(10))
        .expect("the second turn");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("written.txt")).expect("the file"),
        "from the agent\n"
    );
}

#[test]
fn a_path_outside_the_project_is_refused() {
    let dir = project();
    // The agent is told to read a file that is not in the project by a small detour:
    // the stand-in reads `a.txt`, so `a.txt` is removed and a link to outside is put in
    // its place — the canonical path leaves the project, and the client must say so.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "not yours\n").unwrap();
    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.path().join("a.txt"))
        .unwrap();

    let mut session =
        Session::start(&stand_in(), dir.path(), PermissionPolicy::Deny).expect("the agent started");
    session
        .prompt("read", Duration::from_secs(10))
        .expect("the turn");
    let said = text_of(&drain(&mut session));
    assert_eq!(
        said, "nothing",
        "the client served a file from outside the project: {said:?}"
    );
}

#[test]
fn an_agent_that_dies_mid_turn_is_reported_and_not_waited_for() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    let error = session
        .prompt("die", Duration::from_secs(10))
        .expect_err("a dead agent answered");
    let said = format!("{error}");
    assert!(
        said.contains("closed its output"),
        "the failure does not say what happened: {said}"
    );
}

#[test]
fn an_agent_that_says_nothing_is_given_up_on() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    let started = std::time::Instant::now();
    let error = session
        .prompt("slow", Duration::from_millis(400))
        .expect_err("a silent agent answered");
    assert!(
        format!("{error}").contains("did not answer"),
        "the failure does not say what happened: {error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the deadline was not the one asked for"
    );
}
