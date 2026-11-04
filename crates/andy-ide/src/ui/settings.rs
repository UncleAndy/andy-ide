//! The settings dialog: what the window does, in sections.
//!
//! One modal, a list of sections on the left and the section itself on the right. Every change is
//! written the moment it is made — there is no `Apply`: a window that keeps a value nobody has
//! confirmed is a window whose file and whose screen disagree, and a person who leaves by Esc would
//! never know which of the two they got.
//!
//! What belongs to the window is in `config.toml`; what belongs to the project (which panels are on
//! screen, what the tree shows, the preview) is in `.andy-ide/workspace.json`, and this dialog is the
//! face of both.

use egui::{Id, Key, Modal, Modifiers};

use crate::state::{AppState, DEFAULT_FONT_SIZE, MAX_ZOOM, MIN_ZOOM};
use crate::ui::Tab;

/// A section of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    View,
    Editor,
    Panels,
    About,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::View,
        Section::Editor,
        Section::Panels,
        Section::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::View => "View",
            Section::Editor => "Editor",
            Section::Panels => "Panels",
            Section::About => "About",
        }
    }
}

/// The dialog itself: whether it is open, and which section is in front.
#[derive(Debug, Default)]
pub struct Settings {
    pub open: bool,
    pub section: Option<Section>,
}

impl Settings {
    /// Opens the dialog on a section, or on the one it was left on.
    pub fn open_at(&mut self, section: Option<Section>) {
        self.open = true;
        if let Some(section) = section {
            self.section = Some(section);
        }
        self.section.get_or_insert(Section::View);
    }
}

/// Draws the dialog, if it is open. `dock` is here because a panel's visibility is the dock's, not
/// the state's.
pub fn show(ctx: &egui::Context, state: &mut AppState, dock: &mut egui_dock::DockState<Tab>) {
    if !state.settings.open {
        return;
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        state.settings.open = false;
        return;
    }
    let section = state.settings.section.unwrap_or(Section::View);
    let mut close = false;
    Modal::new(Id::new("andy-ide-settings")).show(ctx, |ui| {
        ui.set_min_size(egui::vec2(760.0, 520.0));
        ui.horizontal(|ui| {
            // The list of sections: a column of its own, so the section in front keeps the whole
            // width for what it says.
            ui.vertical(|ui| {
                ui.set_width(150.0);
                ui.heading("Settings");
                ui.add_space(6.0);
                for candidate in Section::ALL {
                    if ui
                        .selectable_label(candidate == section, candidate.title())
                        .clicked()
                    {
                        state.settings.section = Some(candidate);
                    }
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(560.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match section {
                        Section::View => view_section(ui, state, ctx),
                        Section::Editor => editor_section(ui, state),
                        Section::Panels => panels_section(ui, state, dock),
                        Section::About => about_section(ui, state, ctx),
                    });
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    });
    if close {
        state.settings.open = false;
    }
}

/// The theme and the scale of the whole window.
fn view_section(ui: &mut egui::Ui, state: &mut AppState, ctx: &egui::Context) {
    let mut theme = state.config.theme;
    ui.label("Theme");
    ui.horizontal(|ui| {
        for (candidate, title) in [
            (ide_core::Theme::Dark, "Dark"),
            (ide_core::Theme::Light, "Light"),
        ] {
            ui.selectable_value(&mut theme, candidate, title);
        }
    });
    if theme != state.config.theme {
        state.config.theme = theme;
        state.save_config();
    }
    ui.add_space(8.0);

    // The scale is the zoom egui applies to everything: the same number `Ctrl`+`+` moves.
    let mut zoom = state.config.font_size / DEFAULT_FONT_SIZE;
    ui.label("Interface scale");
    let slider = ui.add(
        egui::Slider::new(&mut zoom, MIN_ZOOM..=MAX_ZOOM)
            .custom_formatter(|value, _| format!("{:.0}%", value * 100.0))
            .custom_parser(|text| {
                text.trim_end_matches('%')
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .map(|percent| percent / 100.0)
            }),
    );
    if slider.changed() {
        ctx.set_zoom_factor(zoom);
        state.config.font_size = zoom * DEFAULT_FONT_SIZE;
        state.save_config();
    }
    ui.weak("The same scale `Ctrl` `+` and `Ctrl` `-` move, and it is kept for the next launch.");
}

/// How the editor writes and saves.
fn editor_section(ui: &mut egui::Ui, state: &mut AppState) {
    ui.label("Indentation");
    let mut width = i32::from(state.config.tab_width);
    let changed = ui
        .add(
            egui::DragValue::new(&mut width)
                .range(1..=16)
                .suffix(" columns"),
        )
        .changed();
    if ui
        .checkbox(
            &mut state.config.indent_with_spaces,
            "Write a step as spaces",
        )
        .on_hover_text("Off, a step is a tab; the width above is how wide the tab is drawn")
        .changed()
        || changed
    {
        state.config.tab_width = width.clamp(1, 16) as u8;
        state.save_config();
    }
    ui.weak("Files already open keep the indentation they were opened with.");
    ui.add_space(8.0);

    ui.label("Saving");
    if ui
        .checkbox(
            &mut state.config.trim_trailing_whitespace,
            "Take trailing whitespace off the lines of a saved file",
        )
        .changed()
    {
        state.save_config();
    }
    let mut every = state.config.autosave_seconds as i32;
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::DragValue::new(&mut every)
                    .range(0..=3600)
                    .suffix(" s"),
            )
            .changed()
        {
            state.config.autosave_seconds = every.clamp(0, 3600) as u32;
            state.save_config();
        }
        ui.label("between autosaves; zero is never");
    });
}

/// What the project remembers of the window: the panels, and how the tree and the preview behave.
fn panels_section(ui: &mut egui::Ui, state: &mut AppState, dock: &mut egui_dock::DockState<Tab>) {
    ui.horizontal(|ui| {
        ui.label("Panels on screen, in this project");
        // The way back stands at the top of the list, where it is seen before a person has unticked
        // anything: at the bottom of a section it scrolls out of reach exactly when it is wanted.
        if ui
            .button("Put back")
            .on_hover_text("A default layout, and the panels this build starts without put away")
            .clicked()
        {
            crate::app::reset_layout(state, dock);
        }
    });
    let mut asked: Option<(Tab, bool)> = None;
    for tab in Tab::ALL {
        let mut shown = dock.find_tab(&tab).is_some();
        if ui.checkbox(&mut shown, tab.title()).changed() {
            asked = Some((tab, shown));
        }
    }
    match asked {
        Some((tab, true)) => crate::app::show_panel(dock, state, tab),
        Some((tab, false)) => crate::app::hide_panel(dock, state, tab),
        None => {}
    }
    ui.add_space(8.0);

    ui.label("This project");
    let mut changed = false;
    changed |= ui
        .checkbox(&mut state.tree.show_hidden, "Hidden files in the tree")
        .changed();
    changed |= ui
        .checkbox(
            &mut state.preview_open,
            "Markdown preview beside the source",
        )
        .changed();
    changed |= ui
        .checkbox(&mut state.hints, "Hints that follow the pointer")
        .changed();
    if changed {
        state.status = "the project's switches".to_owned();
    }
}

/// What this build is, and where it keeps what it knows.
fn about_section(ui: &mut egui::Ui, state: &mut AppState, ctx: &egui::Context) {
    ui.label(format!("andy-ide {}", env!("CARGO_PKG_VERSION")));
    ui.add_space(6.0);
    let mut shown = vec![("Settings", state.config_dir.join("config.toml"))];
    if let Some(project) = state.project.as_ref() {
        shown.push((
            "This project",
            project.root().join(".andy-ide").join("workspace.json"),
        ));
        shown.push((
            "Its agents",
            project.root().join(".andy-ide").join("agents.toml"),
        ));
    }
    shown.push(("The log", crate::log::path()));
    for (what, path) in &shown {
        ui.horizontal(|ui| {
            ui.label(*what);
            ui.weak(path.display().to_string());
        });
    }
    ui.add_space(8.0);
    if ui
        .button("Open the log")
        .on_hover_text("The file the window wrote while it ran")
        .clicked()
    {
        ctx.open_url(egui::OpenUrl::new_tab(format!(
            "file://{}",
            crate::log::path().display()
        )));
    }
}
