//! The shells an agent asks the client to run, in the environment it was told about.
//!
//! ACP has the agent ask the client to run commands — `terminal/create`, `terminal/output`,
//! `terminal/wait_for_exit`, `terminal/kill`, `terminal/release` — instead of running them
//! itself. That is the whole point of the design: the client is the one that knows where
//! the work belongs. An editor talking to a container, or to a machine reached over `ssh`,
//! is not on the same filesystem as its project, and an agent that shells out on its own
//! would edit the wrong copy of everything.
//!
//! So the environment is a command line — `ssh dev@build`, `docker exec -i dev sh -c` —
//! and every command the agent asks for is appended to it as one argument. No shell of
//! ours sits in between: the words the agent sends are one argument, and the environment's
//! own shell is what parses it, on the machine where it will run.
//!
//! A client that offers this must mean it, so it is offered only when an environment was
//! configured: refusing every request would leave the agent with no way to work at all.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::command;

/// How much of one shell's output is kept: an agent that runs `yes` must not eat the
/// editor's memory through a pipe nobody is reading.
pub const KEPT: usize = 200_000;

/// The shells one conversation has asked for.
pub struct Terminals {
    /// Where the work happens: `ssh dev@build`, `docker exec -i dev sh -c`, or nothing
    /// for the machine the editor itself runs on.
    prefix: Option<String>,
    /// The project, for shells that run here.
    root: PathBuf,
    next: u64,
    shells: BTreeMap<String, Shell>,
}

/// One shell, and what it has said so far.
struct Shell {
    child: Child,
    /// Written to by two threads, one per stream, and read by the panel's thread.
    output: Arc<Mutex<String>>,
}

impl Terminals {
    /// Offers the shells of `prefix`, run from `root` when the prefix is local.
    pub fn new(prefix: Option<String>, root: &Path) -> Self {
        Self {
            prefix: prefix.filter(|prefix| !prefix.trim().is_empty()),
            root: root.to_path_buf(),
            next: 0,
            shells: BTreeMap::new(),
        }
    }

    /// Whether this conversation can run anything at all.
    ///
    /// Always: with no environment configured, commands run here, in the project, through
    /// `sh -c`. Refusing everything would leave an agent that asked politely with no way
    /// to do anything at all.
    pub fn offers(&self) -> bool {
        true
    }

    /// The environment, as it should be described to the agent.
    pub fn describe(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    /// Starts the command the agent asked for, and answers with the id it will use.
    pub fn create(
        &mut self,
        command_line: &str,
        args: &[String],
        cwd: Option<&str>,
        limit: Option<u64>,
    ) -> Result<String> {
        // No environment configured means the machine the editor runs on, and the
        // project as the place to work in.
        let prefix = self.prefix.clone().unwrap_or_else(|| "sh -c".to_owned());

        // The words the agent sent become one shell word group: whatever quoting it used
        // is its own, and the environment's shell is the one that reads it.
        let mut line = command::quote_word(command_line);
        for arg in args {
            line.push(' ');
            line.push_str(&command::quote_word(arg));
        }
        if let Some(cwd) = cwd {
            // The environment may be another machine, where our `current_dir` means
            // nothing; `cd` is the only instruction both of them understand.
            line = format!("cd {} && {line}", command::quote_word(cwd));
        }

        let spec = command::parse(&prefix).with_context(|| {
            format!("the terminal in the configuration cannot be read: {prefix}")
        })?;
        let mut started = Command::new(&spec.command);
        started
            .args(&spec.args)
            .arg(&line)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if self.prefix.is_none() {
            started.current_dir(&self.root);
        }
        let mut child = started
            .spawn()
            .with_context(|| format!("starting {prefix}"))?;

        let output = Arc::new(Mutex::new(String::new()));
        let kept = limit.unwrap_or(KEPT as u64) as usize;
        if let Some(stdout) = child.stdout.take() {
            collect(stdout, Arc::clone(&output), kept);
        }
        if let Some(stderr) = child.stderr.take() {
            collect(stderr, Arc::clone(&output), kept);
        }

        self.next += 1;
        let id = format!("t{}", self.next);
        self.shells.insert(id.clone(), Shell { child, output });
        Ok(id)
    }

    /// What the shell has said so far, and whether it has finished.
    pub fn output(&mut self, id: &str) -> Result<Value> {
        let shell = self
            .shells
            .get_mut(id)
            .with_context(|| format!("no shell called {id}"))?;
        let text = shell
            .output
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        let status = match shell.child.try_wait() {
            Ok(Some(status)) => json!({"exitCode": status.code().unwrap_or(0)}),
            // Not finished, and not an error: the agent is allowed to ask again.
            Ok(None) => Value::Null,
            Err(error) => bail!("asking about the shell: {error}"),
        };
        Ok(json!({"output": text, "truncated": false, "exitStatus": status}))
    }

    /// Waits for the shell to finish, up to `timeout`.
    pub fn wait_for_exit(&mut self, id: &str, timeout: Duration) -> Result<Value> {
        let shell = self
            .shells
            .get_mut(id)
            .with_context(|| format!("no shell called {id}"))?;
        let deadline = Instant::now() + timeout;
        loop {
            match shell.child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(json!({"exitCode": status.code().unwrap_or(0)}));
                }
                Ok(None) => {}
                Err(error) => bail!("waiting for the shell: {error}"),
            }
            if Instant::now() >= deadline {
                bail!("the shell did not finish in {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Ends one shell early.
    pub fn kill(&mut self, id: &str) -> Result<()> {
        let shell = self
            .shells
            .get_mut(id)
            .with_context(|| format!("no shell called {id}"))?;
        shell.child.kill().context("killing the shell")?;
        Ok(())
    }

    /// Forgets one shell. Ending it is `kill`; this is only the bookkeeping.
    pub fn release(&mut self, id: &str) -> Result<()> {
        self.shells.remove(id);
        Ok(())
    }

    /// Forgets everything: the conversation is over.
    pub fn close(&mut self) {
        for (_, mut shell) in std::mem::take(&mut self.shells) {
            let _ = shell.child.kill();
            let _ = shell.child.wait();
        }
    }
}

/// Keeps reading one stream into `output` for as long as it is open.
///
/// The reader is a thread of its own because the shell may print nothing for minutes and
/// then print a great deal: a client that read the pipe on its own thread would stall the
/// conversation on the quiet parts and choke on the loud ones.
fn collect(mut stream: impl Read + Send + 'static, output: Arc<Mutex<String>>, kept: usize) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    if let Ok(mut text) = output.lock() {
                        text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                        if text.len() > kept {
                            let cut = text.len() - kept;
                            let cut = (0..=cut)
                                .rev()
                                .find(|at| text.is_char_boundary(*at))
                                .unwrap_or(0);
                            text.drain(..cut);
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here() -> Terminals {
        Terminals::new(None, Path::new("."))
    }

    #[test]
    fn a_command_runs_and_says_what_it_printed() {
        let mut terminals = here();
        let id = terminals
            .create("echo", &["TERM-OK".to_owned()], None, None)
            .expect("a shell started");
        let status = terminals
            .wait_for_exit(&id, Duration::from_secs(10))
            .expect("the shell finished");
        assert_eq!(status["exitCode"], 0);
        let output = terminals.output(&id).expect("the output");
        assert!(
            output["output"]
                .as_str()
                .unwrap_or_default()
                .contains("TERM-OK"),
            "nothing was printed: {output}"
        );
    }

    #[test]
    fn the_environment_is_where_the_command_goes() {
        // A prefix stands for the machine the work belongs to; here it is this one, with
        // a `sh -c` in front, which is exactly how `docker exec -i c sh -c` works.
        let mut terminals = Terminals::new(Some("sh -c".to_owned()), Path::new("."));
        let id = terminals
            .create("echo", &["VIA-PREFIX".to_owned()], None, None)
            .expect("a shell started");
        terminals
            .wait_for_exit(&id, Duration::from_secs(10))
            .expect("the shell finished");
        let output = terminals.output(&id).expect("the output");
        assert!(
            output["output"]
                .as_str()
                .unwrap_or_default()
                .contains("VIA-PREFIX"),
            "the prefix did not reach the command: {output}"
        );
    }

    #[test]
    fn an_unknown_shell_is_an_error_and_not_a_panic() {
        let mut terminals = Terminals::new(None, Path::new("."));
        assert!(terminals.output("t99").is_err());
        assert!(
            terminals
                .wait_for_exit("t99", Duration::from_millis(10))
                .is_err()
        );
        assert!(terminals.kill("t99").is_err());
    }

    #[test]
    fn a_command_that_fails_says_so_in_its_exit_code() {
        let mut terminals = here();
        let id = terminals
            .create("sh", &["-c".to_owned(), "exit 3".to_owned()], None, None)
            .expect("a shell started");
        let status = terminals
            .wait_for_exit(&id, Duration::from_secs(10))
            .expect("the shell finished");
        assert_eq!(status["exitCode"], 3, "a failure looked like success");
    }

    #[test]
    fn a_shell_can_be_killed_and_forgotten() {
        let mut terminals = here();
        let id = terminals
            .create("sleep", &["30".to_owned()], None, None)
            .expect("a shell started");
        terminals.kill(&id).expect("the shell was killed");
        terminals.release(&id).expect("the shell was forgotten");
        assert!(
            terminals.output(&id).is_err(),
            "a killed shell is still there"
        );
    }

    #[test]
    fn the_directory_the_agent_asked_for_is_the_one_it_gets() {
        let dir = tempfile::tempdir().expect("a directory");
        let mut terminals = Terminals::new(Some("sh -c".to_owned()), Path::new("."));
        let id = terminals
            .create("pwd", &[], Some(dir.path().to_str().expect("a path")), None)
            .expect("a shell started");
        terminals
            .wait_for_exit(&id, Duration::from_secs(10))
            .expect("the shell finished");
        let output = terminals.output(&id).expect("the output");
        let expected = dir.path().canonicalize().expect("a canonical path");
        assert!(
            output["output"]
                .as_str()
                .unwrap_or_default()
                .contains(&expected.to_string_lossy().into_owned()),
            "the shell is not where the agent asked: {output}"
        );
    }
}
