//! Recent projects and the restored session (open tabs, cursors, layout).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::fs::write_atomic;

/// How many projects the recent list remembers.
pub const RECENT_LIMIT: usize = 20;

/// Recently opened projects, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    entries: Vec<PathBuf>,
}

impl Recent {
    /// Reads the list; a missing or broken file gives an empty list.
    pub fn load(path: &Path) -> Recent {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Recent::default();
        };
        match serde_json::from_str::<Recent>(&text) {
            Ok(recent) => recent,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "recent projects are unreadable, starting empty"
                );
                Recent::default()
            }
        }
    }

    /// Puts `project` at the front, removing any earlier occurrence, and keeps
    /// the list within [`RECENT_LIMIT`].
    pub fn add(&mut self, project: PathBuf) {
        self.entries.retain(|existing| existing != &project);
        self.entries.insert(0, project);
        self.entries.truncate(RECENT_LIMIT);
    }

    /// The remembered projects, newest first.
    pub fn list(&self) -> &[PathBuf] {
        &self.entries
    }

    /// Writes the list atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        write_atomic(path, text.as_bytes())
    }
}

/// One restored tab: the file and where the cursor was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabState {
    pub path: PathBuf,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

/// Everything needed to bring the previous session back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub tabs: Vec<TabState>,
    pub active: Option<usize>,
    /// Serialised dock layout, opaque to this crate.
    pub layout: Option<String>,
}

impl Session {
    /// Reads the session; a missing or broken file gives an empty session.
    pub fn load(path: &Path) -> Session {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Session::default();
        };
        match serde_json::from_str::<Session>(&text) {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "session is unreadable, starting empty"
                );
                Session::default()
            }
        }
    }

    /// Writes the session atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        write_atomic(path, text.as_bytes())
    }
}
