//! Everything that crosses the wire between the editor and an agent, written down as it happens.
//!
//! The protocol is the only truth about what an agent was told and what it answered. A panel that
//! draws the conversation can hide a line, and a mistake in that drawing — a receipt of a background
//! process taken for the words of a person, a summary of a compacted context taken for a message —
//! looks exactly like the agent having said nothing at all. The raw lines settle it: what was asked,
//! in what order, and what came back, including the lines that are not valid JSON.
//!
//! The threads that own the pipes call these methods (`crate::session`), so a record is written by
//! the same thread that moves the bytes and cannot get ahead of or behind the wire. A session with
//! no record asked for gets [`Silent`]: a check does not write files, and neither does a window
//! whose log could not be opened.

use std::sync::Mutex;

/// Where the lines of a session go.
///
/// Implemented by the editor over its own rotated file, and by [`Kept`] where a check wants the
/// lines in memory. Every method is called from a thread that owns a pipe, so an implementation
/// must be safe to call from several threads and must never block for long: the wire waits for it.
pub trait Wire: Send + Sync + 'static {
    /// A line on its way to the agent's input, before it is written.
    fn sent(&self, line: &str);

    /// A line read from the agent's output — including one that is not valid JSON, which is
    /// exactly the kind of line a person reading the record needs to see.
    fn got(&self, line: &str);

    /// Something that is not a frame of the protocol: the agent's error output, a pipe that
    /// closed, the line that says where a session began.
    fn note(&self, text: &str);
}

/// The record nobody asked for.
pub struct Silent;

impl Wire for Silent {
    fn sent(&self, _line: &str) {}
    fn got(&self, _line: &str) {}
    fn note(&self, _text: &str) {}
}

/// A record kept in memory, in the order the lines crossed the wire.
///
/// For checks: they want to know that both directions are written down and in what order, without
/// a file to read afterwards.
#[derive(Default)]
pub struct Kept {
    lines: Mutex<Vec<String>>,
}

impl Kept {
    /// Everything recorded so far, oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|lines| lines.clone())
            .unwrap_or_default()
    }

    /// The lines of one direction, without the mark that says which it was: `->`, `<-` or `!!`.
    pub fn of(&self, mark: &str) -> Vec<String> {
        let head = format!("{mark} ");
        self.lines()
            .into_iter()
            .filter_map(|line| line.strip_prefix(&head).map(str::to_owned))
            .collect()
    }

    fn keep(&self, mark: &str, text: &str) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(format!("{mark} {text}"));
        }
    }
}

impl Wire for Kept {
    fn sent(&self, line: &str) {
        self.keep("->", line);
    }

    fn got(&self, line: &str) {
        self.keep("<-", line);
    }

    fn note(&self, text: &str) {
        self.keep("!!", text);
    }
}
