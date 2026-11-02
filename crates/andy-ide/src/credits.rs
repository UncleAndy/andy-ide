//! The agent account's top-up credits, asked of the agent itself.
//!
//! ACP knows nothing about money. Hermes' adapter carries no credits and no billing - neither word is
//! in its ACP half - and `/usage`, the command that shows them, is a command of Hermes' own CLI: given
//! to an ACP session it is text for the model. What the same program does print is `hermes usage
//! --json`, whose `details` are the very lines `/usage` shows, under a schema Hermes' documentation
//! calls stable. So the panel asks the agent it already runs, on the same command line, with the same
//! credentials: no token of the person's held by the editor, nothing to keep in step with Hermes, and
//! no Hermes edit.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ide_agents::AgentSpec;
use tracing::{info, warn};

/// How long the agent is given to answer. It fetches the account itself with a timeout of eight
/// seconds; this one is for the way in - a docker that stopped answering - and it ends the process
/// instead of leaving it behind.
const PATIENCE: Duration = Duration::from_secs(20);

/// The line `/usage` prints for the balance this reads: `Top-up credits: $14.06`.
const LINE: &str = "Top-up credits";

/// The amount in a `hermes usage --json` document, or `None` when the document has no top-up line in
/// it: a plan that bought no credits, a provider that has no such account, a fetch that failed
/// (`hermes usage` exits non-zero and prints nothing on stdout), or something that is not that
/// document at all. Nothing is not zero - an account that could not be reached is not an account with
/// no money - so the caller keeps the figure it had.
pub fn top_up_credits(document: &str) -> Option<String> {
    for detail in document_of(document)?.get("details")?.as_array()? {
        let Some(line) = detail.as_str() else {
            continue;
        };
        let Some((label, amount)) = line.split_once(':') else {
            continue;
        };
        if label.trim() == LINE {
            let amount = amount.trim();
            if !amount.is_empty() {
                return Some(amount.to_owned());
            }
        }
    }
    None
}

/// The JSON document inside what a process wrote: between the first brace and the last.
///
/// `hermes usage --json` is asked for a document, but a program that is not quite that program - a
/// wrapper, a version that says something of its own first, a shell that adds a line - writes other
/// things beside it. A document that parses is a document, wherever it sits in the output.
fn document_of(output: &str) -> Option<serde_json::Value> {
    let start = output.find('{')?;
    let end = output.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&output[start..=end]).ok()
}

/// Asks the agent for its account, on a thread of its own: the frame of the panel must not wait on a
/// process and a network fetch. Everything the fetch learns it learns as `None` for nothing.
pub fn ask(spec: AgentSpec) -> Receiver<Option<String>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(fetch(&spec));
    });
    receiver
}

/// Runs the agent's own usage command once and reads the balance out of it.
///
/// What comes back is written to the log as it is: this is the one part of the panel that depends on
/// another program's output, and a figure that does not appear has to be diagnosable from the log
/// alone - which program was asked, with what, what it answered and on which stream.
fn fetch(spec: &AgentSpec) -> Option<String> {
    let args = spec.usage_args()?;
    // The streams go to files rather than to pipes: a child that fills a pipe nobody is reading waits
    // forever, and the deadline below would then kill the wrong thing.
    let (out_path, err_path) = (scratch_path("out"), scratch_path("err"));
    let (Ok(out_file), Ok(err_file)) = (
        std::fs::File::create(&out_path),
        std::fs::File::create(&err_path),
    ) else {
        return None;
    };
    info!(command = %spec.command, ?args, "asking the agent for its account");
    let spawned = Command::new(&spec.command)
        .args(&args)
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            warn!(command = %spec.command, %error, "the agent's usage command could not be started");
            clean(&out_path, &err_path);
            return None;
        }
    };
    let deadline = Instant::now() + PATIENCE;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(outcome)) => break Some(outcome),
            Ok(None) => {
                if Instant::now() >= deadline {
                    warn!(
                        ?PATIENCE,
                        "the agent's usage command did not answer in time"
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                warn!(%error, "the agent's usage command could not be waited for");
                break None;
            }
        }
    };
    let out = std::fs::read_to_string(&out_path).unwrap_or_default();
    let err = std::fs::read_to_string(&err_path).unwrap_or_default();
    clean(&out_path, &err_path);
    // The exit code is not the answer; what the command printed is. `hermes usage` exits non-zero
    // when it could not fetch, and its own words about that are the thing worth reading - so the
    // document is looked for in the output and the code is reported beside it, not instead of it.
    let found = top_up_credits(&out);
    match &found {
        Some(credits) => info!(credits = %credits, "the account balance came back"),
        None => warn!(
            code = ?ended.and_then(|outcome| outcome.code()),
            stdout = %head(&out),
            stderr = %head(&err),
            "no balance in what the agent's usage command printed"
        ),
    }
    found
}

/// Removes the two files of a fetch that is over.
fn clean(out: &PathBuf, err: &PathBuf) {
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(err);
}

/// The beginning of what a process wrote, for the log: enough to recognise an error, short enough to
/// read in one line of it.
fn head(text: &str) -> String {
    let mut out: String = text.chars().take(200).collect();
    if text.chars().count() > 200 {
        out.push('\u{2026}');
    }
    out.replace('\n', " \u{21b5} ")
}

/// A file of this fetch's own: two windows asking at the same moment must not write over each other.
fn scratch_path(stream: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "andy-ide-usage-{}-{stamp}.{stream}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `hermes usage --json` printed for Andy's account on 2026-11-02, trimmed to the lines this
    /// reads: the document is the agent's own, so the shape is its, not a guess of ours.
    const DOCUMENT: &str = r#"{
      "provider": "nous",
      "source": "portal-account",
      "title": "Nous credits",
      "plan": "Free",
      "fetched_at": "2026-11-02T09:54:22+00:00",
      "windows": [],
      "details": [
        "Provider: nous (Free)",
        "Subscription credits: $0.00",
        "Top-up credits: $14.06",
        "Total usable: $14.06",
        "Renews: 2026-11-30T00:18:34.000Z"
      ],
      "unavailable_reason": null
    }"#;

    #[test]
    fn the_balance_is_read_out_of_the_agents_own_document() {
        assert_eq!(top_up_credits(DOCUMENT).as_deref(), Some("$14.06"));
    }

    #[test]
    fn a_notice_before_the_document_does_not_hide_the_balance() {
        // A version that says something of its own first - a banner, a notice, a shell line - is still
        // answering the question. The document is read wherever it sits in the output.
        let noisy = format!("Hermes 0.22.5\nchecking for updates...\n{DOCUMENT}\n");
        assert_eq!(top_up_credits(&noisy).as_deref(), Some("$14.06"));
    }

    #[test]
    fn the_command_is_started_as_the_agent_would_be_and_its_answer_read() {
        // The whole way round without a docker and without an account: a program that answers the way
        // `hermes usage --json` does, started the way a spec starts one, read the way the panel reads
        // it. No shebang script is used: this host has no `/bin/sh` at all, and a fake that cannot be
        // started would prove nothing about the fetch. `bash -c` is asked directly instead, and it
        // answers only the command line the spec derives (`usage --json` - `AgentSpec::usage_args`),
        // taking the document from the environment, which is the other half of what a spec carries.
        use std::collections::BTreeMap;

        let fake = "if [ \"$0 $*\" = \"usage --json\" ]; then printf '%s' \"$FAKE_DOCUMENT\"; \
                    else echo \"asked: $0 $*\"; fi";
        let mut env = BTreeMap::new();
        env.insert("FAKE_DOCUMENT".to_owned(), DOCUMENT.to_owned());
        let spec = AgentSpec {
            name: "Hermes Docker".to_owned(),
            command: "bash".to_owned(),
            args: vec!["-c".to_owned(), fake.to_owned(), "acp".to_owned()],
            env,
        };

        let found = ask(spec)
            .recv_timeout(Duration::from_secs(30))
            .expect("the fetch came back");
        assert_eq!(
            found.as_deref(),
            Some("$14.06"),
            "the balance of the fake agent's document was not read"
        );
    }

    #[test]
    fn a_document_without_a_balance_says_nothing_rather_than_zero() {
        // A plan with no credits bought, a provider with no such account, a fetch that failed: the
        // panel is told nothing, and keeps whatever it was told before.
        assert_eq!(
            top_up_credits(r#"{"details": ["Total usable: $14.06"]}"#),
            None
        );
        assert_eq!(top_up_credits(r#"{"details": []}"#), None);
        assert_eq!(
            top_up_credits(r#"{"details": ["Top-up credits: "]} "#),
            None
        );
        // An empty stdout, or an error message on it: not a document, so not a figure.
        assert_eq!(top_up_credits(""), None);
        assert_eq!(
            top_up_credits("No account usage available for provider 'nous'"),
            None
        );
    }
}
