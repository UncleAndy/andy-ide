//! A conversation held over the standard's own client.
//!
//! The first slice of the migration: the handshake, the updates the agent sends being read into the events the
//! panel already knows, and a turn ending in the agent's own word.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ide_agents::ChoiceRequest;
use ide_agents::acp;
use ide_agents::session::{AgentSpec, Event, PermissionPolicy};
use ide_agents::wire::{Kept, Wire};

/// The stand-in agent, built with the crate and spawned like any other program.
fn stand_in() -> AgentSpec {
    AgentSpec {
        name: "stand-in".to_owned(),
        command: env!("CARGO_BIN_EXE_acp_test_agent").to_owned(),
        args: Vec::new(),
        env: Default::default(),
    }
}

#[test]
fn a_turn_on_the_sdk_arrives_as_the_events_the_panel_knows() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    assert!(
        !session.id().is_empty(),
        "the handshake named no conversation"
    );
    assert!(
        !session.resumed(),
        "a fresh conversation is not a resumed one"
    );

    session.start_turn("hello").expect("the turn goes out");

    let mut thinking = String::new();
    let mut said = String::new();
    let mut finished = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && finished.is_none() {
        match session.next_event(Duration::from_millis(200)) {
            Some(Event::Thinking(text)) => thinking.push_str(&text),
            Some(Event::Text(text)) => said.push_str(&text),
            Some(Event::Finished(word)) => finished = Some(word),
            Some(_) => {}
            None => {}
        }
    }

    assert_eq!(
        thinking, "pondering the greeting",
        "the agent's reasoning did not arrive as reasoning"
    );
    assert_eq!(
        said, "Hello",
        "the two chunks of the answer did not arrive in order"
    );
    assert_eq!(
        finished.as_deref(),
        Some("end_turn"),
        "the turn did not end in the agent's own word"
    );
    session.stop();
}

#[test]
fn the_commands_the_agent_offers_arrive_with_their_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");
    session.start_turn("hello").expect("the turn goes out");

    let mut commands = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && commands.is_none() {
        if let Some(Event::Commands(offered)) = session.next_event(Duration::from_millis(200)) {
            commands = Some(offered);
        }
    }

    let commands = commands.expect("the agent's commands never arrived");
    let model = commands
        .iter()
        .find(|command| command.name == "model")
        .expect("the stand-in offers a model command");
    assert!(
        !model.description.is_empty(),
        "a command arrived without its words: {model:?}"
    );
    assert!(
        model.hint.is_some(),
        "the command that takes an argument arrived without one: {model:?}"
    );
    session.stop();
}

#[test]
fn a_permission_request_is_answered_by_the_person() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");
    let opened = session.id();
    session.start_turn("ask").expect("the turn goes out");

    let mut asked = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && asked.is_none() {
        if let Some(Event::Permission { id, request }) =
            session.next_event(Duration::from_millis(200))
        {
            asked = Some((id, request));
        }
    }
    let (id, request) = asked.expect("the agent never asked for permission");
    assert_eq!(
        request.session, opened,
        "the question arrived under another conversation"
    );
    assert!(
        request.tool_call.is_object(),
        "the tool call did not arrive: {:?}",
        request.tool_call
    );
    let yes = request
        .option_of("allow_once")
        .expect("the ways to answer did not arrive")
        .to_owned();

    session
        .answer_permission(id, Some(&yes))
        .expect("the answer goes out");

    let mut said = String::new();
    let mut finished = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && finished.is_none() {
        match session.next_event(Duration::from_millis(200)) {
            Some(Event::Text(text)) => said.push_str(&text),
            Some(Event::Finished(word)) => finished = Some(word),
            _ => {}
        }
    }
    assert_eq!(
        said, "allowed",
        "the agent did not hear the answer the person gave"
    );
    assert_eq!(finished.as_deref(), Some("end_turn"));
    session.stop();
}

#[test]
fn a_question_with_variants_is_answered_with_what_was_picked() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");
    session.start_turn("choose").expect("the turn goes out");

    let mut asked = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && asked.is_none() {
        if let Some(Event::Choice { id, request }) = session.next_event(Duration::from_millis(200))
        {
            asked = Some((id, request));
        }
    }
    let (id, request) = asked.expect("the agent never asked the person to choose");
    assert_eq!(
        request.field, "variant",
        "the answer goes back under another name"
    );
    assert!(
        request.options.len() >= 3,
        "the variants did not arrive: {:?}",
        request.options
    );
    assert!(
        request.options[1].title.contains("Readable"),
        "the variants arrived without the agent's words: {:?}",
        request.options
    );
    let picked = request.options[1].value.clone();

    session
        .answer_choice(
            id,
            &request.field,
            false,
            Some(std::slice::from_ref(&picked)),
        )
        .expect("the answer goes out");

    let mut said = String::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !said.contains(&picked) {
        if let Some(Event::Text(text)) = session.next_event(Duration::from_millis(200)) {
            said.push_str(&text);
        }
    }
    assert_eq!(
        said, picked,
        "the agent did not hear which variant was picked"
    );
    session.stop();
}

#[test]
fn the_mode_and_the_settings_are_changed_and_the_agent_says_what_it_took() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    assert_eq!(
        session.state().current_mode.as_deref(),
        Some("default"),
        "the modes the agent offers did not arrive with the conversation"
    );
    session.set_mode("dont_ask").expect("the mode goes out");
    assert_eq!(
        session.state().current_mode.as_deref(),
        Some("dont_ask"),
        "the panel shows a mode the agent was never asked for"
    );

    fn effort(session: &acp::Session) -> Option<String> {
        session
            .state()
            .options
            .iter()
            .find(|setting| setting.id == "reasoning_effort")
            .map(|setting| setting.current.clone().unwrap_or_default())
    }
    assert_eq!(
        effort(&session).as_deref(),
        Some("default"),
        "the settings the agent offers did not arrive with the conversation"
    );
    session
        .set_option("reasoning_effort", "low")
        .expect("the setting goes out");
    assert_eq!(
        effort(&session).as_deref(),
        Some("low"),
        "the panel shows a setting the agent did not take"
    );
    session.stop();
}

#[test]
fn the_turn_in_flight_is_stopped_and_ends_in_the_agents_own_word() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");
    session.start_turn("slow").expect("the turn goes out");
    // Let it be well in flight before it is stopped.
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(300) {
        std::thread::sleep(Duration::from_millis(50));
    }
    session.cancel().expect("the stop goes out");

    let mut finished = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && finished.is_none() {
        if let Some(Event::Finished(word)) = session.next_event(Duration::from_millis(200)) {
            finished = Some(word);
        }
    }
    assert_eq!(
        finished.as_deref(),
        Some("cancelled"),
        "the stopped turn did not end in the agent's own word"
    );
    session.stop();
}

#[test]
fn the_conversations_the_agent_knows_of_arrive_with_their_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    let conversations = session
        .list_sessions()
        .expect("the agent lists its conversations");
    assert!(!conversations.is_empty(), "the agent listed nothing at all");
    assert!(
        conversations.iter().all(|row| !row.id.is_empty()),
        "a conversation arrived without an id: {conversations:?}"
    );
    assert!(
        conversations.iter().any(|row| row.title.is_some()),
        "no conversation arrived with a name: {conversations:?}"
    );
    session.stop();
}

#[test]
fn a_conversation_is_picked_up_again_and_one_the_agent_forgot_is_opened_anew() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    session.begin().expect("a fresh conversation opens");
    assert!(
        !session.resumed(),
        "a fresh conversation was reported as resumed"
    );
    assert_eq!(session.id(), "stand-in-session");

    session
        .load_into("stand-in-session")
        .expect("an earlier conversation is picked up");
    assert!(
        session.resumed(),
        "a picked-up conversation was reported as fresh"
    );
    assert_eq!(session.id(), "stand-in-session");

    // An id the agent no longer knows is not a failure to report: the person wanted a conversation here, and
    // the panel is not the place for a rumour about an old one (`conversation.rs` asks for the same thing).
    session
        .load_into("forgotten-one")
        .expect("a conversation the agent forgot is opened anew");
    assert!(
        !session.resumed(),
        "a conversation opened anew was reported as picked up"
    );
    assert_ne!(
        session.id(),
        "forgotten-one",
        "the panel was left holding an id the agent does not have"
    );
    session.stop();
}

/// The stand-in, started with the flags a test needs.
fn stand_in_with(flags: &[&str]) -> AgentSpec {
    let mut spec = stand_in();
    spec.args = flags.iter().map(|flag| (*flag).to_owned()).collect();
    spec
}

/// The stand-in, told to ask the client for a path of the test's choosing.
fn stand_in_asking_for(path: &std::path::Path) -> AgentSpec {
    let mut spec = stand_in();
    spec.env.insert(
        "ACP_TEST_ASK_PATH".to_owned(),
        path.to_string_lossy().into_owned(),
    );
    spec
}

/// What the agent says during one turn, and how it ends.
fn one_turn(session: &mut acp::Session, words: &str) -> (String, Option<String>) {
    session.start_turn(words).expect("the turn goes out");
    let mut said = String::new();
    let mut finished = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && finished.is_none() {
        match session.next_event(Duration::from_millis(200)) {
            Some(Event::Text(text)) => said.push_str(&text),
            Some(Event::Finished(word)) => finished = Some(word),
            _ => {}
        }
    }
    (said, finished)
}

#[test]
fn the_model_menu_comes_from_the_setting_the_standard_gives_it() {
    // An agent that answers by the book: no `models` field (the standard has none — the model is one of the
    // agent's settings), the model offered as a `select` setting with the `model` category.
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(
        &stand_in_with(&["--no-models", "--model-as-setting"]),
        dir.path(),
        PermissionPolicy::Ask,
    )
    .expect("the agent starts and answers the handshake");

    let models = session.state().models.clone();
    assert!(
        models.iter().any(|model| model.id == "large"),
        "the model menu did not arrive at all: {models:?}"
    );
    assert_eq!(
        session.state().current_model.as_deref(),
        Some("small"),
        "the model in force did not arrive with the conversation"
    );

    session.set_model("large").expect("the model goes out");
    assert_eq!(
        session.state().current_model.as_deref(),
        Some("large"),
        "the panel shows a model the agent never took"
    );
    let (said, _) = one_turn(&mut session, "model");
    assert_eq!(
        said, "large / ask",
        "the agent did not take the model it was given"
    );
    session.stop();
}

#[test]
fn the_model_menu_of_an_agent_that_names_its_models_hermes_way_still_arrives() {
    // What Hermes 0.22.5 really sends: a `models` field of its own, which the standard's types have no room
    // for. Measured against the agent itself (`examples/acp_probe.rs`), and the reason the two session
    // answers are read raw.
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    let models = session.state().models.clone();
    assert!(
        models.iter().any(|model| model.id == "large"),
        "the model menu was lost to a field the schema does not know: {models:?}"
    );
    assert_eq!(
        session.state().current_model.as_deref(),
        Some("small"),
        "the model in force did not arrive with the conversation"
    );

    // No setting for the model is offered, so the way back is taken — the agent's own method — and the
    // agent's own word is what says it took the choice.
    session.set_model("large").expect("the model goes out");
    assert_eq!(
        session.state().current_model.as_deref(),
        Some("large"),
        "the panel shows a model the agent never took"
    );
    let (said, _) = one_turn(&mut session, "model");
    assert_eq!(
        said, "large / ask",
        "the agent did not take the model it was given"
    );
    session.stop();
}

#[test]
fn the_way_back_is_written_into_the_record() {
    // The rule: an agent's own method is not the standard's, so whoever reads the record afterwards has to be
    // able to see that it was taken — and to see the day it stops being taken.
    let dir = tempfile::tempdir().unwrap();
    let kept = Arc::new(Kept::default());
    let mut session = acp::Session::start_with(
        &stand_in(),
        dir.path(),
        PermissionPolicy::Ask,
        None,
        None,
        Some(Arc::clone(&kept) as Arc<dyn Wire>),
    )
    .expect("the agent starts and answers the handshake");

    session.set_model("large").expect("the model goes out");

    let lines = kept.lines().join("\n");
    // The line the request itself puts on the wire is one thing, and this is another: the note a person
    // reading the record afterwards needs, saying plainly that an agent's own method was taken and why.
    assert!(
        lines.contains("because it offers no setting"),
        "the way back was taken without a word about it in the record: {lines}"
    );
    assert!(
        lines.contains("\"method\":\"session/set_model\""),
        "the way back does not show on the wire: {lines}"
    );
    assert!(
        lines.contains("large"),
        "the record does not say which model was asked for: {lines}"
    );
    session.stop();
}

#[test]
fn the_standard_road_is_taken_when_the_agent_offers_both() {
    // Both spellings at once: the agent names models in its own field *and* offers the standard's setting.
    // The setting is what the standard road goes through, so the settings the agent reads back come changed —
    // which is how a test can tell the two roads apart, and why the way back is left for agents that have
    // none.
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(
        &stand_in_with(&["--model-as-setting"]),
        dir.path(),
        PermissionPolicy::Ask,
    )
    .expect("the agent starts and answers the handshake");

    session.set_model("large").expect("the model goes out");

    let setting = session
        .state()
        .options
        .iter()
        .find(|setting| setting.id == "model")
        .expect("the stand-in offers a setting for the model");
    assert_eq!(
        setting.current.as_deref(),
        Some("large"),
        "the standard road was not taken although the agent offers it"
    );
    assert_eq!(session.state().current_model.as_deref(), Some("large"));
    session.stop();
}

#[test]
fn a_file_the_agent_asks_for_inside_the_project_is_served() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "the project's own words").unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    let (said, finished) = one_turn(&mut session, "read");
    assert_eq!(
        said, "the project's own words",
        "the file the agent asked for was not served"
    );
    assert_eq!(
        finished.as_deref(),
        Some("end_turn"),
        "the turn did not end: an unanswered request is a turn that never ends"
    );
    session.stop();
}

#[test]
fn a_file_outside_the_project_is_refused_and_the_turn_goes_on() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join("secret.txt"),
        "not the project's business",
    )
    .unwrap();
    let project = tempfile::tempdir().unwrap();
    let spec = stand_in_asking_for(&outside.path().join("secret.txt"));
    let mut session = acp::Session::start(&spec, project.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    let (said, finished) = one_turn(&mut session, "read");
    assert_ne!(
        said, "not the project's business",
        "a file outside the project was served"
    );
    assert_eq!(
        finished.as_deref(),
        Some("end_turn"),
        "the turn did not end after the refusal"
    );
    session.stop();
}

#[test]
fn a_write_the_panel_has_not_allowed_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    let (said, _) = one_turn(&mut session, "write");
    assert_eq!(said, "write refused", "a write went through unallowed");
    assert!(
        !dir.path().join("written.txt").exists(),
        "the file was written although the panel had not allowed it"
    );
    session.stop();
}

#[test]
fn a_write_the_panel_has_allowed_goes_through() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");
    session.allow_writes(true);

    let (said, _) = one_turn(&mut session, "write");
    assert_eq!(said, "written", "an allowed write did not go through");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("written.txt")).unwrap_or_default(),
        "from the agent\n",
        "the file the agent wrote does not hold what it wrote"
    );
    session.stop();
}

/// Waits for the first question of the kind asked, and the turn's own ending afterwards.
fn asked(session: &mut acp::Session, wanted: &str) -> (u64, ChoiceRequest) {
    session.start_turn(wanted).expect("the turn goes out");
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(Event::Choice { id, request }) = session.next_event(Duration::from_millis(200))
        {
            return (id, request);
        }
    }
    panic!("the agent never asked the person anything");
}

#[test]
fn a_question_to_be_written_out_is_answered_with_the_words_themselves() {
    // Hermes asks for free text with a bare string and nothing to choose between: there is nothing to click,
    // so the card has to offer somewhere to write — and the words go back as the words.
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    let (id, request) = asked(&mut session, "ask-text");
    assert!(
        request.free_text,
        "a question to be written out was read as one to pick: {request:?}"
    );
    assert!(request.options.is_empty(), "there is nothing to pick");
    assert!(!request.multiple, "one answer was asked for, not several");
    assert_eq!(request.field, "answer");

    session
        .answer_choice(
            id,
            &request.field,
            request.multiple,
            Some(&["a name of my own".to_owned()]),
        )
        .expect("the answer goes out");

    let (said, finished) = one_turn(&mut session, "hello");
    assert_eq!(
        said, "wrote: a name of my own",
        "the agent did not get the words that were written"
    );
    assert_eq!(finished.as_deref(), Some("end_turn"));
    session.stop();
}

#[test]
fn a_question_with_several_answers_goes_back_as_a_list() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");

    let (id, request) = asked(&mut session, "choose-many");
    assert!(
        request.multiple,
        "a question that allows several answers was read as one: {request:?}"
    );
    assert!(request.options.len() >= 2, "the variants did not arrive");

    session
        .answer_choice(
            id,
            &request.field,
            request.multiple,
            Some(&["tests".to_owned(), "docs".to_owned()]),
        )
        .expect("the answer goes out");

    let (said, _) = one_turn(&mut session, "hello");
    assert_eq!(
        said, "chose: tests, docs",
        "the agent did not get both of the answers"
    );
    session.stop();
}

#[test]
fn a_permission_refused_by_the_person_is_refused_in_the_agents_own_option() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = acp::Session::start(&stand_in(), dir.path(), PermissionPolicy::Ask)
        .expect("the agent starts and answers the handshake");
    session.start_turn("ask").expect("the turn goes out");

    let mut asked_for = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && asked_for.is_none() {
        if let Some(Event::Permission { id, .. }) = session.next_event(Duration::from_millis(200)) {
            asked_for = Some(id);
        }
    }
    let id = asked_for.expect("the agent never asked for permission");

    // Nothing chosen: the person said no, and the agent hears a refusal rather than an option.
    session
        .answer_permission(id, None)
        .expect("the refusal goes out");

    let (said, _) = one_turn(&mut session, "hello");
    assert_eq!(said, "refused", "the agent did not hear the refusal");
    session.stop();
}
