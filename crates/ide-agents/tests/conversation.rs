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

#[test]
fn the_session_says_which_models_and_modes_it_offers() {
    let (_dir, session) = start(PermissionPolicy::Deny);
    let state = session.state();
    assert_eq!(state.current_model.as_deref(), Some("small"));
    assert_eq!(
        state
            .models
            .iter()
            .map(|model| model.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Small",
            "Large",
            "Nous \u{b7} Hermes 4",
            "OpenAI \u{b7} GPT-5"
        ]
    );
    // The first two ids name no provider; the other two name one before the slash, which
    // is what the panel groups by. The grouping itself is the panel's business and is
    // checked there.
    assert_eq!(
        state
            .models
            .iter()
            .map(|model| model.id.split(['/', ':']).next().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["small", "large", "nous", "openai"]
    );
    // The mode is the edit policy and never a level of thinking: ACP carries the level as
    // a setting of its own.
    assert_eq!(state.current_mode.as_deref(), Some("default"));
    assert_eq!(
        state
            .modes
            .iter()
            .map(|mode| mode.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Default", "Accept Edits", "Don't Ask"]
    );
    let level = state
        .thought_level()
        .expect("the settings do not carry a level of thinking");
    assert_eq!(level.id, "reasoning_effort");
    assert_eq!(level.current.as_deref(), Some("default"));
    assert_eq!(
        level
            .choices
            .iter()
            .map(|choice| choice.value.as_str())
            .collect::<Vec<_>>(),
        vec![
            "default", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"
        ]
    );
    // The other setting is the edit policy, which the panel must not mistake for a level.
    assert!(
        state
            .options
            .iter()
            .any(|option| option.category.as_deref() == Some("mode")),
        "the edit policy is not among the settings"
    );
}

/// Changing a setting: the agent answers with the settings read back, and the one that was
/// asked for is the one in force.
#[test]
fn a_setting_can_be_changed_and_is_read_back() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    session
        .set_option("reasoning_effort", "high")
        .expect("the agent would not change the level");
    let level = session
        .state()
        .thought_level()
        .expect("the level is gone after the change");
    assert_eq!(level.current.as_deref(), Some("high"));

    // A setting the agent does not know is accepted and ignored, as Hermes accepts an
    // editor's own config writes: a client must not fail where nothing was changed.
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    session
        .set_option("no_such_setting", "high")
        .expect("an unknown setting was refused");
    let level = session
        .state()
        .thought_level()
        .expect("the level is gone after an unknown setting");
    assert_eq!(
        level.current.as_deref(),
        Some("default"),
        "an unknown setting changed the level"
    );
}

#[test]
fn a_switch_of_model_and_mode_reaches_the_agent() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    session.set_model("large").expect("the model was taken");
    session
        .set_mode("accept_edits")
        .expect("the mode was taken");
    assert_eq!(session.state().current_model.as_deref(), Some("large"));
    assert_eq!(
        session.state().current_mode.as_deref(),
        Some("accept_edits")
    );

    // And the agent itself agrees: this word is answered from its own state.
    session.start_turn("model").expect("the turn started");
    let said = loop {
        match session.next_event(Duration::from_secs(10)) {
            Some(Event::Text(text)) => break text,
            Some(Event::Finished(_)) | None => break String::new(),
            Some(_) => {}
        }
    };
    // The mode is the edit policy, and the agent names it the way Hermes does.
    assert_eq!(said, "large / workspace_session");
}

#[test]
fn the_agent_is_told_which_directory_the_project_is_in() {
    let (dir, mut session) = start(PermissionPolicy::Deny);
    session.start_turn("where").expect("the turn started");
    let said = loop {
        match session.next_event(Duration::from_secs(10)) {
            Some(Event::Text(text)) => break text,
            Some(Event::Finished(_)) | None => break String::new(),
            Some(_) => {}
        }
    };
    assert_eq!(said, dir.path().canonicalize().unwrap().to_string_lossy());
}

#[test]
fn how_much_of_the_window_is_used_arrives_with_the_turn() {
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    assert!(session.usage().is_none(), "a figure appeared from nowhere");
    session.start_turn("hello").expect("the turn started");
    let events = loop {
        let events = drain(&mut session);
        if events
            .iter()
            .any(|event| matches!(event, Event::Finished(_)))
        {
            break events;
        }
        if events.is_empty() {
            break Vec::new();
        }
    };
    let usage = events
        .iter()
        .find_map(|event| match event {
            Event::Usage(usage) => Some(*usage),
            _ => None,
        })
        .expect("the figure did not arrive");
    assert_eq!(usage.used, 42_000);
    assert_eq!(usage.window, Some(100_000));
    assert_eq!(session.usage(), Some(usage));
}

#[test]
fn the_client_asks_nothing_of_itself_and_says_files_go_through_the_terminal() {
    // The agent's terminal is where its project is; this client serves neither `fs/*`
    // nor `terminal/*`, and says so twice: in the capabilities and beside the directory.
    let (_dir, mut session) = start(PermissionPolicy::Deny);
    session.start_turn("caps").expect("the turn started");
    let said = loop {
        match session.next_event(Duration::from_secs(10)) {
            Some(Event::Text(text)) => break text,
            Some(Event::Finished(_)) | None => break String::new(),
            Some(_) => {}
        }
    };
    assert!(
        said.contains("\"readTextFile\":false"),
        "the client offered to read files: {said}"
    );
    assert!(
        said.contains("\"writeTextFile\":false"),
        "the client offered to write files: {said}"
    );
    assert!(
        said.contains("\"terminal\":false"),
        "the client offered to run commands: {said}"
    );
    assert!(
        said.contains("\"fileAccess\":\"terminal\""),
        "the agent was not told to use its terminal: {said}"
    );
}

/// A session that was told to pick up `id`.
fn start_resumed(policy: PermissionPolicy, id: &str) -> (tempfile::TempDir, Session) {
    let dir = project();
    let session = Session::start_with(&stand_in(), dir.path(), policy, Some(id.to_owned()), None)
        .expect("the agent started");
    (dir, session)
}

/// What the agent says about the conversation it is in, after one turn.
fn conversation(session: &mut Session) -> String {
    session.start_turn("session").expect("the turn started");
    let mut said = String::new();
    while let Some(event) = session.next_event(Duration::from_secs(5)) {
        match event {
            // A resumed session replays what it kept before it answers; the answer is the
            // last thing said, and the replay is the history, not the report.
            Event::Text(text) => said = text,
            Event::Finished(_) | Event::Closed => break,
            _ => {}
        }
    }
    said
}

#[test]
fn a_start_says_which_step_it_is_at() {
    // Between "Start" and "ready" the panel says which step the start is at, and a real
    // agent can take its time at any of them: naming the step is what keeps a slow start
    // from looking like a hang. The last step of a start is the one about the models,
    // since they arrive with the session's own answer.
    let dir = project();
    let progress = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let _session = Session::start_with(
        &stand_in(),
        dir.path(),
        PermissionPolicy::Deny,
        None,
        Some(std::sync::Arc::clone(&progress)),
    )
    .expect("the agent started");
    let last = progress.lock().unwrap().clone();
    assert_eq!(
        last, "Getting model parameters\u{2026}",
        "the last step of a start is not the one the panel shows: {last:?}"
    );
}

#[test]
fn a_kept_conversation_is_picked_up_by_its_id() {
    let (_dir, mut session) = start_resumed(PermissionPolicy::Deny, "kept-42");
    assert_eq!(session.id(), "kept-42");
    assert!(session.resumed(), "the session was opened anew");
    assert_eq!(conversation(&mut session), "kept-42 loaded");
}

#[test]
fn a_conversation_the_agent_forgot_is_opened_anew() {
    // The project has an id the agent no longer knows — it was wiped, or the agent is a
    // different one. That is not a failure to report: a new conversation is what the
    // person wanted, and the panel is not the place for a rumour about an old one.
    let (_dir, mut session) = start_resumed(PermissionPolicy::Deny, "forgotten-7");
    assert!(
        !session.resumed(),
        "a forgotten session was reported as resumed"
    );
    assert_ne!(session.id(), "forgotten-7");
    assert_eq!(conversation(&mut session), "stand-in-session new");
}
