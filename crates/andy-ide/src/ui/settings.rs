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

use crate::state::{AppState, DEFAULT_FONT_SIZE};
use crate::ui::Tab;

/// The sizes the dialog offers by hand. Every press lands on one of them, never between two: a slider
/// moves its handle to where the pointer pressed, so touching one either magnified the window or shrank
/// it by a factor nobody asked for. The keys `Ctrl` `+` and `Ctrl` `-` still reach 0.2…5.0 for the screen
/// that needs it, and a size reached that way is shown in the row rather than rounded to a neighbour.
pub const SCALE_STOPS: [f32; 7] = [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 2.0];

/// A section of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    View,
    Editor,
    Panels,
    Tools,
    Agent,
    Keys,
    About,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::View,
        Section::Editor,
        Section::Panels,
        Section::Tools,
        Section::Agent,
        Section::Keys,
        Section::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::View => "View",
            Section::Editor => "Editor",
            Section::Panels => "Panels",
            Section::Tools => "Tools",
            Section::Agent => "Agents",
            Section::Keys => "Keys",
            Section::About => "About",
        }
    }
}

/// What the window leaves around the dialog, so it reads as a sheet lying on the window rather than as
/// the window itself.
const MARGIN: f32 = 64.0;
/// The room the dialog asks for when the window has more: a section has to be readable without the
/// dialog becoming a wall of text.
const LEAST_WIDTH: f32 = 760.0;
const LEAST_HEIGHT: f32 = 520.0;
/// However wide the window is, a settings sheet reads badly past this.
const MOST_WIDTH: f32 = 900.0;
/// The column of section names.
const SECTIONS_WIDTH: f32 = 168.0;
/// The row `Close` stands in, with the space above it.
const FOOT: f32 = 36.0;
/// What is left of the window when it cannot hold the least: a dialog may be cramped, but it may not
/// stand with its foot below the screen.
const GAP: f32 = 8.0;

/// The room the dialog asks for, from the room the window has.
///
/// The window is the same number of pixels at every scale and a different number of points, so at 200 %
/// it has half the points it has at 100 %: the dialog takes what there is, up to a width that still
/// reads, and down to nothing more than the window.
pub(crate) fn room_for(screen: egui::Vec2) -> egui::Vec2 {
    let spare = egui::vec2(
        (screen.x - 2.0 * MARGIN).max(GAP),
        (screen.y - 2.0 * MARGIN).max(GAP),
    );
    egui::vec2(
        spare.x.min(MOST_WIDTH).max(LEAST_WIDTH.min(spare.x)),
        spare.y.max(LEAST_HEIGHT.min(spare.y)),
    )
}

/// The dialog itself: whether it is open, which section is in front, and what is being typed.
#[derive(Debug, Default)]
pub struct Settings {
    pub open: bool,
    pub section: Option<Section>,
    /// The arguments of a language server as they are being typed, by server name.
    ///
    /// A field bound to a `Vec` rebuilt on every frame loses a trailing space while the person types
    /// it, which is the one keystroke that separates two arguments. The line is kept here, converted
    /// when it changes.
    server_args: std::collections::BTreeMap<String, String>,
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
    // The room is taken from the window, not from the content: a dialog sized by what it holds gives a
    // section a scrollbar while the window around it stands empty, and the section underneath the
    // scrollbar is the one a person came to read.
    let room = room_for(ctx.content_rect().size());
    let body = (room.y - FOOT).max(120.0);
    let mut close = false;
    Modal::new(Id::new("andy-ide-settings")).show(ctx, |ui| {
        ui.set_min_size(room);
        ui.horizontal(|ui| {
            // The list of sections: a column of its own, told the same height as the body, so the
            // separator between the two runs the whole way down.
            ui.allocate_ui_with_layout(
                egui::vec2(SECTIONS_WIDTH, body),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
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
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(room.x - SECTIONS_WIDTH - 24.0, body),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(body)
                        .auto_shrink([false, false])
                        .show(ui, |ui| match section {
                            Section::View => view_section(ui, state, ctx),
                            Section::Editor => editor_section(ui, state),
                            Section::Panels => panels_section(ui, state, dock),
                            Section::Tools => tools_section(ui, state),
                            Section::Agent => agent_section(ui, state),
                            Section::Keys => keys_section(ui),
                            Section::About => about_section(ui, state, ctx),
                        });
                },
            );
        });
        ui.add_space(6.0);
        // The foot keeps the button in the corner a dialog keeps it in, so the eye finds it where the
        // eye already looks for the way out.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
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

    // The scale is the zoom egui applies to everything: the same number `Ctrl` `+` moves.
    let mut zoom = state.config.font_size / DEFAULT_FONT_SIZE;
    ui.label("Interface scale");
    let mut chosen: Option<f32> = None;
    ui.horizontal_wrapped(|ui| {
        for stop in SCALE_STOPS {
            let here = (zoom - stop).abs() < 0.005;
            if ui
                .selectable_label(here, format!("{:.0}%", stop * 100.0))
                .clicked()
                && !here
            {
                chosen = Some(stop);
            }
        }
        // A size the keys reached stands where it is, so the row never shows a size the window is not at.
        if !SCALE_STOPS.iter().any(|stop| (zoom - stop).abs() < 0.005) {
            ui.strong(format!("{:.0}%", zoom * 100.0));
        }
    });
    if let Some(stop) = chosen {
        zoom = stop;
        ctx.set_zoom_factor(zoom);
        state.config.font_size = (zoom * DEFAULT_FONT_SIZE * 100.0).round() / 100.0;
        state.save_config();
    }
    ui.weak(
        "`Ctrl` `+` and `Ctrl` `-` reach further than these; a size reached that way is kept for the \
         next launch, like these.",
    );
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

/// The programs the window runs on the project's behalf: the language servers and the terminal.
fn tools_section(ui: &mut egui::Ui, state: &mut AppState) {
    ui.label("Language servers");
    ui.weak(
        "A server that is not installed is reported in the status bar instead of being fatal: \
         editing works without one.",
    );
    ui.add_space(4.0);
    for server in ide_lsp::SERVERS {
        let installed = ide_lsp::is_installed(server);
        egui::CollapsingHeader::new(format!(
            "{} — {}",
            server.name,
            if installed {
                "installed"
            } else {
                "not installed"
            }
        ))
        .id_salt(("settings_server", server.name))
        .show(ui, |ui| {
            lower_the_server(ui, state, server);
        });
    }
    ui.add_space(6.0);
    let running = state.lsp_status.trim().to_owned();
    ui.horizontal(|ui| {
        if ui
            .button("Restart the server")
            .on_hover_text("Drops the running server and starts it again for the file in front")
            .clicked()
        {
            state.restart_lsp_now();
        }
        if running.is_empty() {
            ui.weak("nothing is running");
        } else {
            ui.weak(format!("now: {running}"));
        }
    });
    ui.add_space(10.0);

    ui.label("Terminal");
    ui.horizontal(|ui| {
        ui.label("Shell");
        if ui
            .add(
                egui::TextEdit::singleline(&mut state.config.terminal_shell)
                    .hint_text("$SHELL")
                    .desired_width(240.0),
            )
            .on_hover_text("The program a new session runs; empty means the shell $SHELL names")
            .changed()
        {
            state.save_config();
        }
    });
    let mut kept = state.config.scrollback as i32;
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::DragValue::new(&mut kept)
                    .range(100..=100_000)
                    .suffix(" lines"),
            )
            .on_hover_text("How much a session keeps above the screen")
            .changed()
        {
            state.config.scrollback = kept.clamp(100, 100_000) as usize;
            state.save_config();
        }
        ui.label("kept above the screen");
    });
    ui.weak("A shell already running keeps running: both of these are about the next one.");
}

/// One language server's command and arguments, with the way back to the built-in ones.
fn lower_the_server(
    ui: &mut egui::Ui,
    state: &mut AppState,
    server: &'static ide_lsp::ServerConfig,
) {
    let built_in = format!("{} {}", server.command, server.args.join(" "));
    ui.weak(format!("built in: {built_in}"));
    let setting = state
        .config
        .servers
        .iter_mut()
        .find(|setting| setting.name == server.name);
    let mut edited = false;
    match setting {
        None => {
            ui.horizontal(|ui| {
                if ui.button("Run something else").clicked() {
                    state.config.servers.push(ide_core::ServerSetting {
                        name: server.name.to_owned(),
                        command: server.command.to_owned(),
                        args: server.args.iter().map(|arg| (*arg).to_owned()).collect(),
                    });
                    state.save_config();
                }
            });
        }
        Some(setting) => {
            ui.horizontal(|ui| {
                ui.label("Command");
                edited |= ui
                    .add(egui::TextEdit::singleline(&mut setting.command).desired_width(300.0))
                    .changed();
            });
            let args_line = state
                .settings
                .server_args
                .entry(server.name.to_owned())
                .or_insert_with(|| setting.args.join(" "));
            ui.horizontal(|ui| {
                ui.label("Arguments");
                let response = ui.add(egui::TextEdit::singleline(args_line).desired_width(300.0));
                if response.changed() {
                    setting.args = args_line.split_whitespace().map(str::to_owned).collect();
                    edited = true;
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .button("Back to the built-in ones")
                    .on_hover_text("Forgets this server's command and arguments")
                    .clicked()
                {
                    let name = server.name;
                    state.config.servers.retain(|setting| setting.name != name);
                    state.settings.server_args.remove(name);
                    state.save_config();
                }
            });
        }
    }
    if edited {
        state.save_config();
    }
}

/// The agents of this project: the entries of `agents.toml`, as a form.
fn agent_section(ui: &mut egui::Ui, state: &mut AppState) {
    ui.label("Talking to an agent");
    let mut reconnect = state.config.auto_reconnect;
    if ui
        .checkbox(&mut reconnect, "Start an agent again when it falls over")
        .on_hover_text("Off, a crash waits for a person to start it")
        .changed()
    {
        state.config.auto_reconnect = reconnect;
        state.agent.auto_reconnect = reconnect;
        state.save_config();
    }
    let mut notices = state.config.notifications;
    if ui
        .checkbox(&mut notices, "Notify the desktop when an agent needs me")
        .on_hover_text("Notices are held back while the panel is in front of you")
        .changed()
    {
        state.config.notifications = notices;
        state.save_config();
    }
    ui.add_space(10.0);

    let Some(root) = state
        .project
        .as_ref()
        .map(|project| project.root().to_path_buf())
    else {
        ui.weak("Open a project to see the agents it knows.");
        return;
    };
    let place = ide_agents::registry::locate(Some(&root), &state.config_dir);
    ui.weak(place.display().to_string());
    // A project that has never had the file gets the ones the window would have used: a form that
    // shows nothing for a project with an agent running in it is a form that lies.
    let mut registry = match ide_agents::Registry::load_or_write(&place) {
        Ok(registry) => registry,
        Err(error) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("cannot read it: {error}"),
            );
            return;
        }
    };

    let mut changed = false;
    let mut drop: Option<String> = None;
    for spec in registry.agents.iter_mut() {
        egui::CollapsingHeader::new(spec.name.clone())
            .id_salt(("settings_agent", spec.name.clone()))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut spec.name).desired_width(200.0))
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Command");
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut spec.command).desired_width(300.0))
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Arguments");
                    let mut line = spec.args.join(" ");
                    if ui
                        .add(egui::TextEdit::singleline(&mut line).desired_width(300.0))
                        .changed()
                    {
                        spec.args = line.split_whitespace().map(str::to_owned).collect();
                        changed = true;
                    }
                });
                for (key, value) in spec.env.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.label(key.as_str());
                        changed |= ui
                            .add(egui::TextEdit::singleline(value).desired_width(240.0))
                            .changed();
                    });
                }
                if ui.button("Forget this agent").clicked() {
                    drop = Some(spec.name.clone());
                }
            });
    }
    if let Some(name) = drop {
        registry.agents.retain(|spec| spec.name != name);
        changed = true;
    }
    if ui.button("Another agent").clicked() {
        registry.agents.push(ide_agents::AgentSpec {
            name: format!("agent-{}", registry.agents.len() + 1),
            command: String::new(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
        });
        changed = true;
    }
    if changed {
        // Written where the window reads it, so the panel and the file cannot disagree.
        match registry.save(&place) {
            Ok(()) => state.status = format!("{} written", place.display()),
            Err(error) => state.status = format!("cannot write {}: {error}", place.display()),
        }
    }
}

/// Every key the window answers to, as the table in the code has it.
fn keys_section(ui: &mut egui::Ui) {
    ui.label("Keys");
    ui.weak("Read from the one table the window answers to; changing them is not here yet.");
    ui.add_space(4.0);
    egui::Grid::new("settings_keys")
        .num_columns(2)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for (shortcut, command) in crate::app::KEYS {
                ui.strong(format_shortcut(*shortcut));
                ui.label(command.name());
                ui.end_row();
            }
        });
}

/// One key in words: `Ctrl+S`, `Ctrl+Shift+P`, `F5`.
pub(crate) fn format_shortcut(shortcut: egui::KeyboardShortcut) -> String {
    let mut said = String::new();
    for (held, name) in [
        (egui::Modifiers::COMMAND, "Ctrl"),
        (egui::Modifiers::CTRL, "Ctrl"),
        (egui::Modifiers::ALT, "Alt"),
        (egui::Modifiers::SHIFT, "Shift"),
    ] {
        if shortcut.modifiers.matches_logically(held) && !said.contains(name) {
            if !said.is_empty() {
                said.push('+');
            }
            said.push_str(name);
        }
    }
    let key = format!("{:?}", shortcut.logical_key);
    if !said.is_empty() {
        said.push('+');
    }
    said.push_str(&key);
    said
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
