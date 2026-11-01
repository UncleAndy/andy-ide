//! A conversation held over the standard's own client.
//!
//! The first slice of the migration: the handshake, the updates the agent sends being read into the events the
//! panel already knows, and a turn ending in the agent's own word.

use std::time::{Duration, Instant};

use ide_agents::acp;
use ide_agents::session::{AgentSpec, Event, PermissionPolicy};

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
