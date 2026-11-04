//! The folder dialog: a project opened by picking a directory inside the window.
//!
//! The desktop has a dialog of its own, and it is not the one drawn here. Through the portal it
//! never appeared on this desk — the button did nothing at all — and a button that does nothing is
//! worse than a dialog that belongs to the window. This one is drawn by egui like every other
//! question the editor asks: it lists the directories of one folder, it walks by rows, and it says
//! what it is looking at.

use std::path::{Path, PathBuf};

use crate::state::{AppState, display_name};

/// What the dialog shows: the directory in front, and the directories inside it.
#[derive(Debug, Default)]
pub struct FolderDialog {
    /// Whether the dialog is on screen.
    pub open: bool,
    /// The directory being looked at. `None` before it has ever been opened.
    pub current: Option<PathBuf>,
    /// The directories inside it, in name order.
    pub entries: Vec<PathBuf>,
    /// Why the listing is empty, when it is empty for a reason worth saying.
    pub trouble: Option<String>,
}

impl FolderDialog {
    /// Opens the dialog at a directory: the one given, or the home of the person.
    pub fn open_at(&mut self, start: Option<&Path>) {
        self.open = true;
        let at = start
            .filter(|path| path.is_dir())
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        self.current = at;
        self.read();
    }

    /// Looks into another directory.
    pub fn descend(&mut self, into: PathBuf) {
        self.current = Some(into);
        self.read();
    }

    /// Goes one directory up, as far as the root.
    pub fn up(&mut self) {
        if let Some(parent) = self.current.as_ref().and_then(|path| path.parent()) {
            let parent = parent.to_path_buf();
            self.descend(parent);
        }
    }

    /// Reads the directories of the one in front, and says why if it cannot.
    fn read(&mut self) {
        self.entries.clear();
        self.trouble = None;
        let Some(current) = self.current.clone() else {
            self.trouble = Some("nowhere to look".to_owned());
            return;
        };
        let listing = match std::fs::read_dir(&current) {
            Ok(listing) => listing,
            Err(error) => {
                self.trouble = Some(format!("{}: {error}", current.display()));
                return;
            }
        };
        for entry in listing.flatten() {
            // Only directories are offered: this dialog opens a project, and a project is a folder.
            // What is hidden is left out as well, the way the tree leaves it out.
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                self.entries.push(entry.path());
            }
        }
        self.entries.sort();
        if self.entries.is_empty() {
            self.trouble = Some(format!("no folders in {}", display_name(&current)));
        }
    }
}

/// Draws the dialog, if it is open; a choice is left for the frame that holds the panels.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.folder.open {
        return;
    }
    let mut asked: Option<PathBuf> = None;
    let mut close = false;
    egui::Modal::new(egui::Id::new("andy-ide-folder")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading("Open a project");
        ui.add_space(4.0);
        match state.folder.current.as_ref() {
            Some(current) => {
                ui.label(current.display().to_string());
            }
            None => {
                ui.weak("nowhere to look");
            }
        }
        ui.separator();
        let mut into = None;
        let mut up = false;
        egui::ScrollArea::vertical()
            .max_height(280.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if state
                    .folder
                    .current
                    .as_ref()
                    .is_some_and(|path| path.parent().is_some())
                    && ui.button("..").clicked()
                {
                    up = true;
                }
                for entry in &state.folder.entries {
                    // A single click walks into a directory: a folder has nothing to say for
                    // itself, and asking for a second click would ask for nothing.
                    if ui
                        .button(format!("{}/", display_name(entry)))
                        .on_hover_text(entry.display().to_string())
                        .clicked()
                    {
                        into = Some(entry.clone());
                    }
                }
                if let Some(trouble) = &state.folder.trouble {
                    ui.weak(trouble.clone());
                }
            });
        if up {
            state.folder.up();
        }
        if let Some(into) = into {
            state.folder.descend(into);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Open this folder").clicked() {
                asked = state.folder.current.clone();
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if let Some(path) = asked {
        state.folder.open = false;
        // Opening it is the same errand as a choice from the list of recent projects: the frame
        // that holds the panels is the one that does it.
        state.switch_asked = Some(path);
    }
    if close {
        state.folder.open = false;
        state.status = "no project chosen".to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_lists_only_the_directories_of_one_folder() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("inner")).unwrap();
        std::fs::create_dir_all(root.path().join(".hidden")).unwrap();
        std::fs::write(root.path().join("a.rs"), "fn a() {}\n").unwrap();

        let mut dialog = FolderDialog::default();
        dialog.open_at(Some(root.path()));
        assert!(dialog.open);
        assert_eq!(dialog.entries, vec![root.path().join("inner")]);
        assert!(dialog.trouble.is_none());

        // A directory with nothing in it says so rather than looking broken.
        dialog.descend(root.path().join("inner"));
        assert_eq!(dialog.trouble.as_deref(), Some("no folders in inner"));

        // And one step back is the folder it came from.
        dialog.up();
        assert_eq!(dialog.current.as_deref(), Some(root.path()));
        assert_eq!(dialog.entries.len(), 1);
    }
}
