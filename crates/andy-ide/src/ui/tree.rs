//! The project tree panel.
//!
//! Directories are listed only when they are expanded, so opening a large
//! project does not walk it up front.

use std::path::{Path, PathBuf};

use egui::{CollapsingHeader, Ui};
use ide_core::{Entry, EntryKind, Project, TreeOptions};

use crate::state::{Action, AppState};

/// Draws the panel: a hidden-files switch and the tree itself.
pub fn panel(ui: &mut Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.tree.show_hidden, "hidden files");
    });
    ui.separator();

    let Some(project) = state.project.clone() else {
        ui.weak("No project open. Start with --project <path>.");
        return;
    };

    let options = state.tree_options();
    let mut actions: Vec<PathBuf> = Vec::new();
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| match project.list_dir(Path::new(""), &options) {
            Ok(entries) => draw_entries(ui, &project, &entries, &options, &mut actions),
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("{error}"));
            }
        });

    state
        .pending
        .extend(actions.into_iter().map(Action::OpenFile));
}

fn draw_entries(
    ui: &mut Ui,
    project: &Project,
    entries: &[Entry],
    options: &TreeOptions,
    actions: &mut Vec<PathBuf>,
) {
    for entry in entries {
        match entry.kind {
            EntryKind::Dir => {
                CollapsingHeader::new(&entry.name)
                    .id_salt(&entry.rel)
                    .show(ui, |ui| match project.list_dir(&entry.rel, options) {
                        Ok(children) => draw_entries(ui, project, &children, options, actions),
                        Err(error) => {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                format!("{}: {error}", entry.name),
                            );
                        }
                    });
            }
            EntryKind::File => {
                let label = ui.selectable_label(false, &entry.name);
                if label.double_clicked() {
                    actions.push(entry.rel.clone());
                }
            }
        }
    }
}
