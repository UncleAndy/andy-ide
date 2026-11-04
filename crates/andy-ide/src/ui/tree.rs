//! The project tree panel.
//!
//! Directories are listed only when they are expanded, so opening a large
//! project does not walk it up front.

use std::path::{Path, PathBuf};

use egui::{CollapsingHeader, Ui};
use ide_core::{Entry, EntryKind, Project, TreeOptions};

use crate::state::{Action, AppState, NamingKind, Prompt};

/// Draws the panel: a hidden-files switch, a menu for the root, and the tree itself.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.tree.show_hidden, "hidden files");
        // The root has no row of its own, and a project with nowhere to make a file would make a
        // person open a terminal for the first thing they do in a project.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("New", |ui| new_things(ui, state, Path::new("")))
                .response
                .on_hover_text("A new file or folder in the project");
        });
    });
    ui.separator();

    let Some(project) = state.project.clone() else {
        ui.weak("No project open. Start with --project <path>.");
        return;
    };

    let options = state.tree_options();
    let mut asked: Vec<PathBuf> = Vec::new();
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| match project.list_dir(Path::new(""), &options) {
            Ok(entries) => draw_entries(ui, state, &project, &entries, &options, &mut asked),
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("{error}"));
            }
        });

    state
        .pending
        .extend(asked.into_iter().map(Action::OpenFile));
}

/// The two things that can be made in a folder.
fn new_things(ui: &mut Ui, state: &mut AppState, at: &Path) {
    if ui.button("New file\u{2026}").clicked() {
        state.ask_for_name(NamingKind::File, at.to_path_buf());
        ui.close();
    }
    if ui.button("New folder\u{2026}").clicked() {
        state.ask_for_name(NamingKind::Folder, at.to_path_buf());
        ui.close();
    }
}

/// What can be done to one row: made beside it, renamed, or taken away.
fn row_menu(ui: &mut Ui, state: &mut AppState, rel: &Path, dir: bool) {
    let here = if dir {
        rel.to_path_buf()
    } else {
        rel.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    new_things(ui, state, &here);
    ui.separator();
    if ui.button("Rename\u{2026}").clicked() {
        state.ask_for_name(NamingKind::Rename, rel.to_path_buf());
        ui.close();
    }
    // Deleting asks first, and the question says what goes: a folder goes with everything in it.
    if ui.button("Delete\u{2026}").clicked() {
        state.prompt = Some(Prompt::DeletePath {
            path: rel.to_path_buf(),
            dir,
        });
        ui.close();
    }
}

fn draw_entries(
    ui: &mut Ui,
    state: &mut AppState,
    project: &Project,
    entries: &[Entry],
    options: &TreeOptions,
    asked: &mut Vec<PathBuf>,
) {
    for entry in entries {
        match entry.kind {
            EntryKind::Dir => {
                let header =
                    CollapsingHeader::new(&entry.name)
                        .id_salt(&entry.rel)
                        .show(ui, |ui| match project.list_dir(&entry.rel, options) {
                            Ok(children) => {
                                draw_entries(ui, state, project, &children, options, asked)
                            }
                            Err(error) => {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    format!("{}: {error}", entry.name),
                                );
                            }
                        });
                header
                    .header_response
                    .context_menu(|ui| row_menu(ui, state, &entry.rel, true));
            }
            EntryKind::File => {
                let label = ui.selectable_label(false, &entry.name);
                let _ = label
                    .clone()
                    .context_menu(|ui| row_menu(ui, state, &entry.rel, false));
                if label.double_clicked() {
                    asked.push(entry.rel.clone());
                }
            }
        }
    }
}
