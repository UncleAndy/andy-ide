//! The session the panel speaks to: the standard's own client, behind this project's own surface.
//!
//! What the panel asks for has not changed. A session is started, turns are sent, events are read and
//! questions are answered exactly as before, and what carries it has: the connection is the standard's own
//! client (`agent-client-protocol`), driven on a thread of its own, over the child's own pipes — so every
//! line that crosses is still written down ([`crate::wire`]), which is the rule this project cannot do
//! without. The implementation lives in [`crate::acp`]; this module is the surface the rest of the project
//! knows it by, and the words that surface is spoken in.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::protocol::{
    AgentCommand, ChoiceRequest, PermissionRequest, SessionInfo, SessionState, Usage,
};

/// How long the agent gets to start and say hello.
pub const HELLO: Duration = Duration::from_secs(60);

/// How long one request may go unanswered before the caller is told so.
pub const ANSWER: Duration = Duration::from_secs(120);

/// A running agent: what the panel holds, whatever carries it underneath.
///
/// Kept under this name and this surface so that nothing above it has to know the wire changed — the panel
/// asked for a session before the migration and asks for the same one after it.
pub use crate::acp::Session;

/// Which agent to run, and how.
///
/// The command is data, not code: a second agent is a line in `agents.toml`, and the
/// editor does not have to learn anything about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    /// The name shown in the panel.
    pub name: String,
    /// The program to run.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Extra environment, as written in the configuration.
    pub env: BTreeMap<String, String>,
}

/// What the client does when the agent asks for permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Allow the first time the agent asks, which is what a test wants.
    Allow,
    /// Refuse everything, which is the safe default.
    #[default]
    Deny,
    /// Ask the person in the panel: the request is handed over and the agent waits.
    Ask,
}

/// Something the agent said or did, for the panel to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A piece of the answer, to append.
    Text(String),
    /// A replayed `role="user"` row: the person's own words from a restored history, or a row
    /// the runtime wrote itself. Which of the two it is, the panel tells by the shape of the
    /// opening words.
    You(String),
    /// A piece of the agent's reasoning, to be kept apart from the answer and folded
    /// away: a person reading an answer wants the answer, and wants the reasoning to
    /// be there when they ask for it.
    Thinking(String),
    /// A tool call, started or finished.
    ToolCall {
        /// The agent's id for it.
        id: String,
        /// Its title.
        title: String,
        /// `pending`, `in_progress`, `completed`, `failed`.
        status: String,
    },
    /// The plan the agent is following.
    Plan(Vec<String>),
    /// The conversations the agent knows of, asked for with [`Session::list_sessions`]: what the
    /// panel offers when the person wants to move to an earlier one.
    Sessions(Vec<SessionInfo>),
    /// An update this version does not show.
    Other {
        /// The tag it arrived with.
        kind: String,
    },
    /// The agent wants permission; the answer goes back through
    /// [`Session::answer_permission`].
    Permission {
        /// The request id, to answer with.
        id: u64,
        /// What is being asked.
        request: PermissionRequest,
    },
    /// The agent offers variants and waits for the person to choose one; the answer goes
    /// back through [`Session::answer_choice`].
    Choice {
        /// The request id, to answer with.
        id: u64,
        /// The variants, and what is being asked.
        request: ChoiceRequest,
    },
    /// The conversation the agent opened: the models and modes it offers, and which of
    /// each is in use. Sent once, as soon as the session exists.
    Setup(SessionState),
    /// How much of the context the conversation is using, when the agent says.
    Usage(Usage),
    /// The commands the agent offers, for the panel to offer them back.
    Commands(Vec<AgentCommand>),
    /// The turn ended, in the agent's own word: `end_turn`, `refusal`, `cancelled`.
    Finished(String),
    /// The agent closed its output: the process is gone.
    Closed,
    /// Its own words, when it failed.
    Failed(String),
}

impl AgentSpec {
    /// The same agent, asked for its account instead of a conversation.
    ///
    /// A Hermes agent answers ACP as `... hermes acp`; the same program with `usage --json` prints the
    /// account block `/usage` shows, as a JSON document whose schema Hermes' own documentation calls
    /// stable. Any other command line is not known to have such a door, so it gets nothing and the panel
    /// asks nobody: a figure from a guessed invocation would be a figure made up.
    pub fn usage_args(&self) -> Option<Vec<String>> {
        if self.args.last().map(String::as_str) != Some("acp") {
            return None;
        }
        let mut args = self.args.clone();
        args.pop();
        args.push("usage".to_owned());
        args.push("--json".to_owned());
        Some(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(args: &[&str]) -> AgentSpec {
        AgentSpec {
            name: "Hermes Docker".to_owned(),
            command: "docker".to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn the_account_is_asked_on_the_same_command_line_as_the_conversation() {
        // The agent the panel talks to is `docker exec -i hermes-agent hermes acp`. The account is the
        // same program with `usage --json` where `acp` was: the credentials, the home and the version
        // are the agent's own, and the editor holds no copy of any of them.
        let asked = spec(&["exec", "-i", "hermes-agent", "hermes", "acp"])
            .usage_args()
            .expect("a Hermes agent can be asked for its account");
        assert_eq!(
            asked,
            vec!["exec", "-i", "hermes-agent", "hermes", "usage", "--json"]
        );
    }

    #[test]
    fn an_agent_that_is_not_that_command_line_is_asked_nothing() {
        // A stand-in, or any other agent: there is no reason to believe it has a usage command, and a
        // stray process after every turn is worse than a figure that is not there.
        assert_eq!(spec(&[]).usage_args(), None);
        assert_eq!(spec(&["serve"]).usage_args(), None);
        assert_eq!(spec(&["acp", "--debug"]).usage_args(), None);
    }
}
