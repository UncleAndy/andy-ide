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
    let parsed: serde_json::Value = serde_json::from_str(document).ok()?;
    for detail in parsed.get("details")?.as_array()? {
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
fn fetch(spec: &AgentSpec) -> Option<String> {
    let args = spec.usage_args()?;
    // The output goes to a file rather than a pipe: a child that fills a pipe nobody is reading waits
    // forever, and the deadline below would then kill the wrong thing.
    let path = scratch_path();
    let file = std::fs::File::create(&path).ok()?;
    let mut child = Command::new(&spec.command)
        .args(&args)
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PATIENCE;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(outcome)) => break Some(outcome),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };
    let document = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(&path);
    if !ended.is_some_and(|outcome| outcome.success()) {
        return None;
    }
    top_up_credits(&document?)
}

/// A file of this fetch's own: two windows asking at the same moment must not write over each other.
fn scratch_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "andy-ide-usage-{}-{stamp}.json",
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
