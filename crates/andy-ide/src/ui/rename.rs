//! Renaming a symbol: a name to type, then the list of places that would change.
//!
//! The second phase is not decoration. A rename reaches into other files, and the
//! user is entitled to see the extent before text is rewritten — so the server is
//! asked first, its answer is shown, and nothing is written until Enter is pressed
//! on that list.

use std::path::PathBuf;

use egui::{Id, Key, Modal, Modifiers};

/// One use of the symbol, as a byte range of the file and the line it sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub start: usize,
    pub end: usize,
}

/// What the server would change in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChanges {
    pub path: PathBuf,
    pub spans: Vec<Span>,
}

/// What the window should do after drawing the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// Nothing this frame.
    None,
    /// The name was typed and Enter was pressed.
    Submit,
    /// The preview was accepted.
    Apply,
    /// The modal was closed without touching anything.
    Cancel,
}

/// The state of the rename modal.
#[derive(Debug, Default)]
pub struct Rename {
    pub open: bool,
    /// The name the symbol has now.
    pub old_name: String,
    /// The name being typed.
    pub name: String,
    /// Whether the list of changes is being shown instead of the field.
    pub preview: bool,
    /// The changes the server answered with, per file.
    pub changes: Vec<FileChanges>,
    /// The text of each file when the ranges were measured: applying refuses to
    /// write a file that has changed since, so the preview cannot lie.
    pub measured: Vec<(PathBuf, String)>,
    focus: bool,
}

impl Rename {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the field with the current name in it.
    pub fn begin(&mut self, old_name: &str) {
        self.open = true;
        self.old_name = old_name.to_owned();
        self.name = old_name.to_owned();
        self.preview = false;
        self.changes.clear();
        self.measured.clear();
        self.focus = false;
    }

    /// Shows the answer: the list of places, and the text they were measured in.
    pub fn preview(&mut self, changes: Vec<FileChanges>, measured: Vec<(PathBuf, String)>) {
        self.preview = true;
        self.changes = changes;
        self.measured = measured;
    }

    /// Closes the modal and forgets the answer.
    pub fn close(&mut self) {
        self.open = false;
        self.preview = false;
        self.changes.clear();
        self.measured.clear();
    }

    /// How many places would change, and in how many files.
    pub fn extent(&self) -> (usize, usize) {
        (
            self.changes.iter().map(|file| file.spans.len()).sum(),
            self.changes.len(),
        )
    }

    /// Draws the modal; the window decides what to do with the answer.
    pub fn show(&mut self, ctx: &egui::Context) -> Reply {
        if !self.open {
            return Reply::None;
        }
        let escape = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        if escape {
            self.close();
            return Reply::Cancel;
        }
        let enter = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
        if enter && !self.preview && !self.name.trim().is_empty() {
            return Reply::Submit;
        }
        if enter && self.preview {
            return Reply::Apply;
        }

        let mut reply = Reply::None;
        let title = if self.preview {
            format!("{} → {}", self.old_name, self.name)
        } else {
            format!("rename {}", self.old_name)
        };
        Modal::new(Id::new("rename_modal")).show(ctx, |ui| {
            ui.set_min_width(460.0);
            ui.heading(title);
            if self.preview {
                let (uses, files) = self.extent();
                ui.label(extent_label(uses, files));
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for file in &self.changes {
                            ui.label(egui::RichText::new(file.path.display().to_string()).strong());
                            for span in &file.spans {
                                ui.label(format!("    line {}", span.line + 1));
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        reply = Reply::Apply;
                    }
                    if ui.button("Cancel").clicked() {
                        reply = Reply::Cancel;
                    }
                });
            } else {
                let field = ui.text_edit_singleline(&mut self.name);
                if !self.focus {
                    self.focus = true;
                    field.request_focus();
                }
                ui.horizontal(|ui| {
                    let ready = !self.name.trim().is_empty();
                    if ui.add_enabled(ready, egui::Button::new("Rename")).clicked() {
                        reply = Reply::Submit;
                    }
                    if ui.button("Cancel").clicked() {
                        reply = Reply::Cancel;
                    }
                });
            }
        });
        if reply == Reply::Cancel {
            self.close();
        }
        reply
    }
}

/// How many places the rename would touch, in words.
pub fn extent_label(uses: usize, files: usize) -> String {
    let uses = if uses == 1 {
        "1 use".to_owned()
    } else {
        format!("{uses} uses")
    };
    let files = if files == 1 {
        "1 file".to_owned()
    } else {
        format!("{files} files")
    };
    format!("{uses} in {files}")
}

/// The line of a byte offset, for the preview.
pub fn line_of(text: &str, byte: usize) -> u32 {
    text[..byte.min(text.len())].matches('\n').count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_holds_the_current_name() {
        let mut rename = Rename::new();
        rename.begin("value");
        assert!(rename.open);
        assert_eq!(rename.name, "value");
        assert_eq!(rename.old_name, "value");
        assert!(!rename.preview, "the preview was shown before the answer");
    }

    #[test]
    fn the_preview_counts_uses_and_files() {
        let mut rename = Rename::new();
        rename.begin("value");
        rename.preview(
            vec![
                FileChanges {
                    path: PathBuf::from("a.rs"),
                    spans: vec![
                        Span {
                            line: 0,
                            start: 4,
                            end: 9,
                        },
                        Span {
                            line: 3,
                            start: 40,
                            end: 45,
                        },
                    ],
                },
                FileChanges {
                    path: PathBuf::from("b.rs"),
                    spans: vec![Span {
                        line: 7,
                        start: 90,
                        end: 95,
                    }],
                },
            ],
            Vec::new(),
        );
        assert_eq!(rename.extent(), (3, 2));
    }

    #[test]
    fn closing_forgets_the_answer() {
        let mut rename = Rename::new();
        rename.begin("value");
        rename.preview(
            vec![FileChanges {
                path: PathBuf::from("a.rs"),
                spans: vec![Span {
                    line: 0,
                    start: 4,
                    end: 9,
                }],
            }],
            Vec::new(),
        );
        rename.close();
        assert!(!rename.open);
        assert!(rename.changes.is_empty());
        assert!(rename.measured.is_empty());
    }

    #[test]
    fn a_line_number_is_counted_from_the_text() {
        assert_eq!(line_of("one\ntwo\nthree", 0), 0);
        assert_eq!(line_of("one\ntwo\nthree", 4), 1);
        assert_eq!(line_of("one\ntwo\nthree", 12), 2);
    }
}

#[cfg(test)]
mod words {
    use super::extent_label;

    #[test]
    fn one_of_something_is_not_one_of_somethings() {
        assert_eq!(extent_label(1, 1), "1 use in 1 file");
        assert_eq!(extent_label(2, 3), "2 uses in 3 files");
    }
}
