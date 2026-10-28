//! Panels and the dock contents.

pub mod editor;
pub mod preview;
pub mod prompt;
pub mod quick_open;
pub mod status;
pub mod tabs;
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
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Projects => "Projects",
            Tab::Editor => "Editor",
            Tab::Problems => "Problems",
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
            Tab::Problems => placeholder(ui, "Diagnostics arrive with the language server (M2)."),
        }
    }

    /// The three fixed panels are part of the layout; only the editor tab is a
    /// real document and could be closed.
    fn is_closeable(&self, tab: &Tab) -> bool {
        !matches!(tab, Tab::Projects | Tab::Editor | Tab::Problems)
    }
}

fn placeholder(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.weak(text);
}
