//! Panels and the dock contents.

pub mod agent;
pub mod diff;
pub mod editor;
pub mod git;
pub mod history;
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
    Diff,
    History,
    Agent,
}

/// Where a panel belongs when it is put back on screen.
///
/// A panel that is put away comes back where its kind belongs, not where it happened to be
/// standing: the position a person gave it is the layout, and the layout of the panels that are
/// on screen is what a workspace remembers. A panel that is off screen has no position to
/// remember, and a guess at one would be remembered in its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// A split down the left of the main leaf.
    Left,
    /// The main leaf itself, where the code is edited.
    Centre,
    /// The strip along the bottom, beside the problems.
    Bottom,
}

impl Tab {
    /// Every panel, in the order the View menu lists them.
    pub const ALL: [Tab; 8] = [
        Tab::Projects,
        Tab::Editor,
        Tab::Problems,
        Tab::Terminal,
        Tab::Git,
        Tab::Diff,
        Tab::History,
        Tab::Agent,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Projects => "Projects",
            Tab::Editor => "Editor",
            Tab::Problems => "Problems",
            Tab::Terminal => "Terminal",
            Tab::Git => "Git",
            Tab::Diff => "Diff",
            Tab::History => "History",
            Tab::Agent => "Agent",
        }
    }

    /// What a saved workspace calls this panel. The title is for people and may be reworded;
    /// this is what is written to `.andy-ide`, so it stays put.
    pub fn slug(self) -> &'static str {
        match self {
            Tab::Projects => "projects",
            Tab::Editor => "editor",
            Tab::Problems => "problems",
            Tab::Terminal => "terminal",
            Tab::Git => "git",
            Tab::Diff => "diff",
            Tab::History => "history",
            Tab::Agent => "agent",
        }
    }

    /// The panel of that name, if the workspace remembers one this build knows.
    pub fn of_slug(slug: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|tab| tab.slug() == slug)
    }

    /// The key that calls this panel, as the menu writes it, where there is one.
    pub fn shortcut(self) -> Option<&'static str> {
        match self {
            Tab::Terminal => Some("Alt+F12"),
            Tab::Git => Some("Alt+9"),
            Tab::Agent => Some("Alt+0"),
            _ => None,
        }
    }

    /// Where the panel belongs when it comes back on screen.
    pub fn place(self) -> Place {
        match self {
            Tab::Projects => Place::Left,
            Tab::Editor => Place::Centre,
            _ => Place::Bottom,
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
            Tab::Diff => diff::panel(ui, self.state),
            Tab::History => history::panel(ui, self.state),
            Tab::Agent => agent::panel(ui, self.state),
        }
    }

    /// Every panel has a cross, and the cross puts the panel away rather than throwing it out:
    /// a panel is a place in the window, not a document, and the state behind it — a running
    /// shell, a conversation, the changes of a repository — is not the dock's to destroy.
    fn is_closeable(&self, _tab: &Tab) -> bool {
        true
    }

    /// A middle click on a tab closes it too, and the dock closes it without asking anybody: the
    /// panel is written down here, so that a tab closed that way is a panel put away rather than
    /// one that comes back on the next launch as though nothing had happened.
    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        if response.middle_clicked() {
            self.state.put_away(*tab);
        }
    }

    /// The cross, or the menu, asks for the panel to be put away. The dock is left to do the
    /// removing — it knows how a leaf that empties out is tidied — and the panel is written down
    /// as put away, which is what brings it back from the View menu and from the next launch.
    fn on_close(&mut self, tab: &mut Tab) -> egui_dock::widgets::tab_viewer::OnCloseResponse {
        self.state.put_away(*tab);
        egui_dock::widgets::tab_viewer::OnCloseResponse::Close
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_dock::TabViewer;

    fn state() -> (tempfile::TempDir, AppState) {
        let config = tempfile::tempdir().unwrap();
        let state = AppState::with_config_dir(config.path().to_path_buf(), None, None);
        (config, state)
    }

    #[test]
    fn every_panel_has_a_cross() {
        // A panel is a place in the window, not a document: putting one away must not need a trip
        // through the menu, and the cross is where a person looks for it.
        let (_config, mut state) = state();
        let viewer = Viewer { state: &mut state };
        for tab in Tab::ALL {
            assert!(viewer.is_closeable(&tab), "{} has no cross", tab.title());
        }
    }

    #[test]
    fn the_cross_puts_the_panel_away_for_the_workspace_to_remember() {
        // What the cross does is not "throw the panel out": the shell keeps running, the
        // conversation keeps its words, and the repository keeps its changes. The panel is
        // written down as put away, which is what the menu ticks and the next launch read.
        let (_config, mut state) = state();
        let mut viewer = Viewer { state: &mut state };
        let answer = viewer.on_close(&mut Tab::Terminal);
        assert!(
            matches!(
                answer,
                egui_dock::widgets::tab_viewer::OnCloseResponse::Close
            ),
            "the dock was told to keep a panel the person put away"
        );
        assert!(viewer.state.hidden.contains(&Tab::Terminal));
        assert!(!viewer.state.hidden.contains(&Tab::Agent));
    }

    #[test]
    fn a_panel_comes_back_from_the_menu_and_is_on_screen_again() {
        let (_config, mut state) = state();
        state.put_away(Tab::Git);
        assert!(state.hidden.contains(&Tab::Git));
        state.bring_back(Tab::Git);
        assert!(!state.hidden.contains(&Tab::Git));
    }
}
