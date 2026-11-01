//! The record of the wire, kept over the standard's own SDK.
//!
//! The rule the panel cannot do without (`crate::wire`): every line that crosses is written down as it
//! crosses, because a panel that draws the conversation can hide a line, and a hidden line looks exactly like
//! silence. The SDK is driven through wrappers that keep it, and this asks whether both directions really
//! come out.

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_client_protocol::Client;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, TextContent,
};
use ide_agents::acp;
use ide_agents::wire::{Kept, Wire};

#[test]
fn every_line_of_a_session_on_the_sdk_is_written_down() {
    let kept = Arc::new(Kept::default());
    let started = acp::start(
        env!("CARGO_BIN_EXE_acp_test_agent"),
        &[],
        &BTreeMap::new(),
        Arc::clone(&kept) as Arc<dyn Wire>,
    )
    .expect("the stand-in started");

    futures::executor::block_on(Client.builder().name("andy-ide-wire-check").connect_with(
        started.transport,
        async |cx| {
            let init = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert_eq!(
                init.protocol_version,
                ProtocolVersion::V1,
                "the version was not agreed"
            );
            let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
            let session = cx
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            cx.send_request(PromptRequest::new(
                session.session_id.clone(),
                vec![ContentBlock::Text(TextContent::new("hello"))],
            ))
            .block_task()
            .await?;
            Ok(())
        },
    ))
    .expect("the conversation runs to its end");

    let sent = kept.of("->");
    let got = kept.of("<-");
    assert!(
        sent.iter().any(|line| line.contains("\"initialize\"")),
        "the handshake was not written down: {sent:?}"
    );
    assert!(
        sent.iter().any(|line| line.contains("\"session/prompt\"")),
        "the prompt was not written down: {sent:?}"
    );
    assert!(
        got.iter().any(|line| line.contains("\"protocolVersion\"")),
        "the agent's answer was not written down: {got:?}"
    );
    assert!(
        got.iter().any(|line| line.contains("\"stopReason\"")),
        "the end of the turn was not written down: {got:?}"
    );
}
