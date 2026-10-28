//! Application configuration stored in `~/.config/andy-ide/config.toml`.
//!
//! Unknown keys are preserved: the file is read into a struct with a flattened
//! remainder, so a newer version of the editor can add settings without an older
//! one dropping them on save.

use std::collections::BTreeMap;
use std::path::Path;

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: Theme,
    pub font_size: f32,
    pub tab_width: u8,
    pub show_hidden: bool,
    /// Open the window maximized, as it was when it was last closed.
    pub window_maximized: bool,
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
            show_hidden: false,
            window_maximized: false,
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
