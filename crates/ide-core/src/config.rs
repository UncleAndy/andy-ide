//! Application configuration stored in `~/.config/andy-ide/config.toml`.
//!
//! Unknown keys are preserved: the file is read into a struct with a flattened
//! remainder, so a newer version of the editor can add settings without an older
//! one dropping them on save.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use toml::Value;

use crate::fs::write_atomic;

/// Colour scheme of the interface and of syntax highlighting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// User settings.
///
/// `default` at the container level makes every missing key fall back to
/// [`Config::default`], so a file may set only what it needs.
/// A language server as a person wrote it down: the name the registry uses, and the command and
/// arguments that replace the ones built into the window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSetting {
    /// The server's name in the registry: `rust-analyzer`, `pyright`, `gopls`, `clangd`.
    pub name: String,
    /// The program to run; empty keeps the built-in one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Its arguments; empty keeps the built-in ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: Theme,
    pub font_size: f32,
    /// How wide one indent step is, in columns; also how wide a tab is drawn.
    pub tab_width: u8,
    /// Whether an indent step is written with spaces; a tab otherwise.
    pub indent_with_spaces: bool,
    pub show_hidden: bool,
    /// Take the trailing whitespace off every line when a file is saved.
    pub trim_trailing_whitespace: bool,
    /// Save every file that has edits this often, in seconds; zero is never.
    pub autosave_seconds: u32,
    /// The program the terminal runs; empty means `$SHELL`.
    pub terminal_shell: String,
    /// How many lines the terminal keeps behind the screen.
    pub scrollback: usize,
    /// Whether a finished turn or a waiting agent is announced by the desktop.
    pub notifications: bool,
    /// Whether a language server that dies is brought back on its own.
    pub auto_reconnect: bool,
    /// Language servers whose commands a person has replaced, by name.
    pub servers: Vec<ServerSetting>,
    /// Open the window maximized, as it was when it was last closed.
    pub window_maximized: bool,
    /// The project a launch opens when it names none and no recent project is free.
    ///
    /// `None` means the window's own under the home, `<home>/andy-ide/projects/default`.
    pub default_project: Option<PathBuf>,
    /// Keys this version does not know about, kept verbatim.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            font_size: 14.0,
            tab_width: 4,
            indent_with_spaces: true,
            show_hidden: false,
            trim_trailing_whitespace: false,
            autosave_seconds: 0,
            terminal_shell: String::new(),
            scrollback: 2000,
            notifications: true,
            auto_reconnect: true,
            servers: Vec::new(),
            window_maximized: false,
            default_project: None,
            extra: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Reads the configuration; a missing or unreadable file gives the defaults.
    pub fn load_or_default(path: &Path) -> Config {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Config::default();
        };
        match toml::from_str::<Config>(&text) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "configuration is unreadable, using defaults"
                );
                Config::default()
            }
        }
    }

    /// Writes the configuration atomically, creating the parent directory.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        write_atomic(path, text.as_bytes())
    }
}
