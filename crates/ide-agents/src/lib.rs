//! Agents, spoken to over the Agent Client Protocol.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`.
//!
//! Three parts, in the order they matter:
//!
//! - [`protocol`] — the messages of ACP, in both directions, and nothing else.
//! - [`session`] — one agent process, its two threads, the deadlines on every wait, and
//!   the answers the agent asks the editor for.
//! - [`registry`] — which agents exist, read from `agents.toml`: a second agent is a
//!   line in a file, not a change to this code.
//!
//! Nothing here blocks the window. A session hands out events as they happen and answers
//! a request with a deadline; the panel draws whatever has arrived and asks again.

use std::path::Path;

pub mod command;
pub mod protocol;
pub mod registry;
pub mod session;

pub use command::{parse as parse_command, parse_named, quote as quote_command};
pub use protocol::{
    AgentCommand, ChoiceOption, ChoiceRequest, ConfigChoice, ConfigOption, Mode, Model,
    PermissionOption, PermissionRequest, REASONING, SessionState, THOUGHT_LEVEL, Update, Usage,
};
pub use registry::Registry;
pub use session::{AgentSpec, Event, PermissionPolicy, Session};

/// Version of the crate, taken from its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Starts the agent named in the registry, in that project.
///
/// The name is looked up rather than assumed: nothing here knows what an agent is called,
/// which is what makes a second one a matter of configuration.
pub fn start(
    registry: &Registry,
    name: &str,
    root: &Path,
    policy: PermissionPolicy,
) -> anyhow::Result<Session> {
    let spec = registry
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("no agent called {name} in agents.toml"))?;
    Session::start(spec, root, policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn starting_an_agent_the_file_does_not_list_says_so() {
        let registry = Registry::defaults();
        let error = start(&registry, "Nobody", Path::new("."), PermissionPolicy::Deny)
            .err()
            .expect("an unknown agent was started");
        assert!(
            format!("{error}").contains("Nobody"),
            "the refusal does not name the agent: {error}"
        );
    }

    #[test]
    fn an_agent_that_does_not_exist_is_a_refusal_not_a_panic() {
        let registry = Registry {
            agents: vec![AgentSpec {
                name: "Ghost".to_owned(),
                command: "no-such-agent-binary-anywhere".to_owned(),
                args: Vec::new(),
                env: Default::default(),
            }],
        };
        let dir = tempfile::tempdir().unwrap();
        let error = start(&registry, "Ghost", dir.path(), PermissionPolicy::Deny)
            .err()
            .expect("a missing program was started");
        assert!(
            format!("{error}").contains("Ghost"),
            "the refusal does not name the agent: {error}"
        );
    }

    #[test]
    fn the_agents_file_lives_in_the_project() {
        let project = PathBuf::from("/tmp/project");
        let place = registry::locate(Some(&project), Path::new("/tmp/config"));
        assert_eq!(place, project.join(".andy-ide").join(registry::FILE));
    }
}
