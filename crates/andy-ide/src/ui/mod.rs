//! Panels and the dock contents.

pub mod editor;
pub mod git;
pub mod hover;
pub mod preview;
pub mod problems;
pub mod prompt;
pub mod quick_open;
pub mod rename;
pub mod results;
pub mod status;
pub mod symbols;
pub mod tabs;
pub mod terminal;
pub mod tree;

use egui::{Id, Ui, WidgetText};
use egui_dock::TabViewer;

use crate::state::AppState;

/// A panel of the dock area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    Projects,
    Editor,
    Problems,
    Terminal,
    Git,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Projects => "Projects",
            Tab::Editor => "Editor",
            Tab::Problems => "Problems",
            Tab::Terminal => "Terminal",
            Tab::Git => "Git",
        }
    }
}

/// Draws the dock's tabs against the application state.
pub struct Viewer<'a> {
    pub state: &'a mut AppState,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match tab {
            Tab::Projects => tree::panel(ui, self.state),
            Tab::Editor => editor::panel(ui, self.state),
            Tab::Problems => problems::panel(ui, self.state),
            Tab::Terminal => self.state.terminal.show(ui),
            Tab::Git => git::panel(ui, self.state),
        }
    }

    /// The three fixed panels are part of the layout; only the editor tab is a
    /// real document and could be closed.
    fn is_closeable(&self, tab: &Tab) -> bool {
        !matches!(
            tab,
            Tab::Projects | Tab::Editor | Tab::Problems | Tab::Terminal | Tab::Git
        )
    }
}
