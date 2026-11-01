//! A one-shot ACP client built on the official SDK, for seeing what an agent really sends.
//!
//! This is the probe the migration to the SDK is measured with, not part of the panel. It starts an agent
//! the way the panel does — a command line, exactly as `agents.toml` holds it — initializes the connection
//! (the step where the protocol version and the capabilities are agreed), opens a session, sends one
//! prompt, and prints every notification and every request the agent makes, as JSON.
//!
//! ```text
//! cargo run -p ide-agents --example acp_probe -- "docker exec -i hermes-agent hermes acp" "скажи: проба"
//! ```
//!
//! What comes out of it is what the mapping table (`docs/acp-sdk-mapping.md`) is written from: which
//! shapes the SDK already carries, and which of ours have no counterpart in the standard.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, CreateElicitationRequest, CreateElicitationResponse, ElicitationAcceptAction,
    ElicitationAction, ElicitationContentValue, ElicitationMode, ElicitationPropertySchema, InitializeRequest, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TextContent,
};
use agent_client_protocol::{AcpAgent, Client};
use std::str::FromStr;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .unwrap_or_else(|| "docker exec -i hermes-agent hermes acp".to_owned());
    let prompt = args
        .next()
        .unwrap_or_else(|| "Ответь одним словом: проба.".to_owned());

    let agent = AcpAgent::from_str(&command)?;
    println!("START {command}");

    let client = Client
        .builder()
        .name("andy-ide-probe")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                println!("NOTIFY {}", serde_json::to_string(&notification)?);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                println!("ASK {}", serde_json::to_string(&request)?);
                // Answered the way the panel would: the narrowest allow, never by position.
                let option = request
                    .options
                    .iter()
                    .find(|option| {
                        matches!(
                            option.kind,
                            PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
                        )
                    })
                    .map(|option| option.option_id.clone());
                let outcome = match option {
                    Some(id) => {
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id))
                    }
                    None => RequestPermissionOutcome::Cancelled,
                };
                let _ = responder.respond(RequestPermissionResponse::new(outcome));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CreateElicitationRequest, responder, _connection| {
                println!("ELICIT {}", serde_json::to_string(&request)?);
                // The form the panel draws as a question with variants. Answered here as the first
                // variant, which is what "pick the first one" means on the wire.
                let answer = match &request.mode {
                    ElicitationMode::Form(form) => form
                        .requested_schema
                        .properties
                        .get("variant")
                        .and_then(|property| match property {
                            // The variants of a form arrive as `oneOf` of `{const, title}`: the standard
                            // carries both that and a plain `enum` of strings.
                            ElicitationPropertySchema::String(property) => property
                                .one_of
                                .as_ref()
                                .and_then(|options| options.first())
                                .map(|option| option.value.clone()),
                            _ => None,
                        }),
                    _ => None,
                };
                let action = match answer {
                    Some(value) => ElicitationAction::Accept(ElicitationAcceptAction::new().content(
                        std::collections::BTreeMap::from([(
                            "variant".to_owned(),
                            ElicitationContentValue::String(value),
                        )]),
                    )),
                    None => ElicitationAction::Cancel,
                };
                let _ = responder.respond(CreateElicitationResponse::new(action));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        );

    futures::executor::block_on(client.connect_with(agent, async |cx| {
        println!("CONNECTED");
        let init = cx
            .send_request(InitializeRequest::new(ProtocolVersion::V1))
            .block_task()
            .await?;
        println!("INIT-ANSWER");
        println!(
            "INIT protocol={:?} info={:?} caps={:?} auth={:?}",
            init.protocol_version, init.agent_info, init.agent_capabilities, init.auth_methods
        );

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let session = cx.send_request(NewSessionRequest::new(cwd)).block_task().await?;
        println!("SESSION {}", session.session_id);

        let answer = cx
            .send_request(PromptRequest::new(
                session.session_id.clone(),
                vec![ContentBlock::Text(TextContent::new(prompt))],
            ))
            .block_task()
            .await?;
        println!("STOP {:?}", answer.stop_reason);
        Ok(())
    }))?;

    Ok(())
}
