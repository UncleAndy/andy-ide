//! Which agents there are, read from `agents.toml`.
//!
//! The list is configuration and not code, because the requirement is that a second
//! agent is connected by editing this file. Nothing in the editor knows the name
//! "Hermes" beyond the default that is written when there is no file yet: the panel
//! lists what the file says and starts what it names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::session::AgentSpec;

/// The file the agents are read from, relative to the project's `.andy-ide` directory.
pub const FILE: &str = "agents.toml";

/// What the file says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    /// The agents, in the order the file lists them.
    pub agents: Vec<AgentSpec>,
}

/// The shape of the file, as `serde` reads it.
#[derive(Debug, Deserialize)]
struct File {
    #[serde(default)]
    agent: Vec<Entry>,
}

/// One `[[agent]]` table.
#[derive(Debug, Deserialize)]
struct Entry {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Where the agent works: `ssh dev@build`, `docker exec -i dev sh -c`.
    #[serde(default)]
    terminal: Option<String>,
}

impl Registry {
    /// Reads the file's text.
    pub fn parse(text: &str) -> Result<Self> {
        let file: File = toml::from_str(text).context("reading agents.toml")?;
        let agents = file
            .agent
            .into_iter()
            .map(|entry| AgentSpec {
                name: entry.name,
                command: entry.command,
                args: entry.args,
                env: entry.env,
                terminal: entry.terminal,
            })
            .collect();
        Ok(Self { agents })
    }

    /// Reads the file at `path`, and falls back to the default when it is missing.
    ///
    /// A missing file is not an error: the editor opens on a project that has never
    /// heard of agents, and a panel that refuses to work until a file is created would
    /// be asking the user to configure what it could have written itself.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::defaults()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// The agents this editor knows to start without being told.
    pub fn defaults() -> Self {
        Self {
            agents: vec![AgentSpec {
                name: "Hermes".to_owned(),
                command: "hermes".to_owned(),
                args: vec!["acp".to_owned()],
                env: BTreeMap::new(),
                terminal: None,
            }],
        }
    }

    /// The text of a file that lists `self`, in a form a person can edit.
    pub fn to_toml(&self) -> String {
        let mut out = String::from(
            "# The agents the panel can start. Add a table and it appears in the list.\n",
        );
        for agent in &self.agents {
            out.push_str("\n[[agent]]\n");
            out.push_str(&format!("name = {:?}\n", agent.name));
            out.push_str(&format!("command = {:?}\n", agent.command));
            let args: Vec<String> = agent.args.iter().map(|arg| format!("{arg:?}")).collect();
            out.push_str(&format!("args = [{}]\n", args.join(", ")));
            if let Some(terminal) = &agent.terminal {
                out.push_str(&format!("terminal = {terminal:?}\n"));
            }
            if !agent.env.is_empty() {
                out.push_str("[agent.env]\n");
                for (key, value) in &agent.env {
                    out.push_str(&format!("{key} = {value:?}\n"));
                }
            }
        }
        out
    }

    /// Writes `to_toml` to `path` if the file is not there yet, and reads it back.
    ///
    /// The file is written once and then belongs to the user: rewriting it on every
    /// start would throw away the agents they added.
    pub fn load_or_write(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let defaults = Self::defaults();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, defaults.to_toml())
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(defaults)
    }

    /// The agent with that name, if the file lists one.
    pub fn get(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    /// Adds an agent, or replaces the one with that name.
    ///
    /// Order is kept: a list that reorders itself when a command is saved is a list a
    /// person has to read again every time.
    pub fn remember(&mut self, spec: AgentSpec) {
        match self.agents.iter_mut().find(|agent| agent.name == spec.name) {
            Some(existing) => *existing = spec,
            None => self.agents.push(spec),
        }
    }

    /// Writes the list where it is kept.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, self.to_toml()).with_context(|| format!("writing {}", path.display()))
    }

    /// The first agent: what the panel opens with.
    pub fn first(&self) -> Option<&AgentSpec> {
        self.agents.first()
    }
}

/// Where the agents are looked for: the project's own `.andy-ide` directory, then the
/// user's configuration directory.
///
/// The project's file wins, because a project may pin a version of an agent that the
/// rest of the machine should not follow.
pub fn places(project: Option<&Path>, config: &Path) -> Vec<PathBuf> {
    let mut places = Vec::new();
    if let Some(root) = project {
        places.push(root.join(".andy-ide").join(FILE));
    }
    places.push(config.join(FILE));
    places
}

/// The file to read: the first place that has one, or the project's place to write.
pub fn locate(project: Option<&Path>, config: &Path) -> PathBuf {
    for place in places(project, config) {
        if place.exists() {
            return place;
        }
    }
    places(project, config)
        .into_iter()
        .next()
        .unwrap_or_else(|| config.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = r#"
[[agent]]
name = "Hermes"
command = "hermes"
args = ["acp"]

[[agent]]
name = "Mine"
command = "/opt/thing"
args = ["--stdio"]
[agent.env]
THING_KEY = "1"
"#;

    #[test]
    fn a_second_agent_is_a_table_in_the_file() {
        let registry = Registry::parse(TWO).expect("the file");
        assert_eq!(registry.agents.len(), 2);
        assert_eq!(registry.agents[0].name, "Hermes");
        assert_eq!(registry.agents[0].args, vec!["acp".to_owned()]);
        assert_eq!(registry.agents[1].command, "/opt/thing");
        assert_eq!(
            registry.agents[1].env.get("THING_KEY").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn a_missing_file_falls_back_to_hermes() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::load(&dir.path().join("agents.toml")).expect("the default");
        assert_eq!(registry.agents.len(), 1);
        assert_eq!(registry.agents[0].command, "hermes");
    }

    #[test]
    fn a_broken_file_is_an_error_and_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.toml");
        std::fs::write(&path, "[[agent]]\nname = \"no command\"\n").unwrap();
        assert!(
            Registry::load(&path).is_err(),
            "a table without a command was accepted"
        );
    }

    #[test]
    fn writing_and_reading_back_gives_the_same_agents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".andy-ide").join("agents.toml");
        let written = Registry::load_or_write(&path).expect("the write");
        assert!(path.exists(), "the file was not written");
        let read = Registry::load(&path).expect("the read");
        assert_eq!(read, written);
    }

    #[test]
    fn a_second_start_does_not_overwrite_what_the_user_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".andy-ide").join("agents.toml");
        Registry::load_or_write(&path).expect("the write");
        std::fs::write(&path, TWO).unwrap();

        let read = Registry::load_or_write(&path).expect("the second read");
        assert_eq!(read.agents.len(), 2, "the user's agents were replaced");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().trim(),
            TWO.trim(),
            "the file was rewritten"
        );
    }

    #[test]
    fn the_projects_file_wins_over_the_users() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let config = dir.path().join("config");
        std::fs::create_dir_all(project.join(".andy-ide")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(project.join(".andy-ide").join(FILE), TWO).unwrap();
        std::fs::write(config.join(FILE), "").unwrap();

        let place = locate(Some(&project), &config);
        assert_eq!(place, project.join(".andy-ide").join(FILE));

        let registry = Registry::load(&place).expect("the read");
        assert_eq!(registry.agents.len(), 2);
    }
}

#[cfg(test)]
mod saving_tests {
    use super::*;

    fn hermes() -> AgentSpec {
        AgentSpec {
            name: "Hermes".to_owned(),
            command: "hermes".to_owned(),
            args: vec!["acp".to_owned()],
            env: BTreeMap::new(),
            terminal: None,
        }
    }

    fn typed() -> AgentSpec {
        AgentSpec {
            name: "agent".to_owned(),
            command: "/opt/my agent/agent".to_owned(),
            args: vec!["--stdio".to_owned()],
            env: BTreeMap::new(),
            terminal: None,
        }
    }

    #[test]
    fn a_remembered_agent_survives_a_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".andy-ide").join(FILE);
        let mut registry = Registry::defaults();
        registry.remember(typed());
        registry.save(&path).expect("the write");

        let read = Registry::load(&path).expect("the read");
        assert_eq!(read.agents.len(), 2);
        assert_eq!(read.agents[1].name, "agent");
        assert_eq!(read.agents[1].command, "/opt/my agent/agent");
        assert_eq!(read.agents[1].args, vec!["--stdio".to_owned()]);
    }

    #[test]
    fn remembering_the_same_name_replaces_it_and_keeps_the_order() {
        let mut registry = Registry::defaults();
        registry.remember(typed());
        let mut changed = hermes();
        changed.args = vec!["--acp".to_owned()];
        registry.remember(changed);

        assert_eq!(registry.agents.len(), 2, "a name was added twice");
        assert_eq!(registry.agents[0].name, "Hermes");
        assert_eq!(registry.agents[0].args, vec!["--acp".to_owned()]);
        assert_eq!(registry.agents[1].name, "agent");
    }
}
