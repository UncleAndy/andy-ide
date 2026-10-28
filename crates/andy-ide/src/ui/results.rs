//! Where a symbol is used: the list `Shift+F12` puts on screen.
//!
//! A modal rather than a docked panel, because a question that has just been
//! asked deserves an answer the user can dismiss with Escape, and because the
//! answers are usually few. The list is built from the server's answer, so it is
//! plain data: the panel only shows it and reports what was chosen.

use std::path::PathBuf;

use egui::{Key, Modal};

use crate::state::{Action, display_name};

/// One place a symbol is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// 0-based line and character, as the server counts them.
    pub line: u32,
    pub character: u32,
    /// The text of that line, for reading the list without opening files.
    pub text: String,
}

/// The list on screen.
#[derive(Debug, Default)]
pub struct Results {
    pub open: bool,
    pub hits: Vec<Hit>,
    pub selected: usize,
}

impl Results {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fills the list and shows it.
    pub fn show_hits(&mut self, hits: Vec<Hit>) {
        self.hits = hits;
        self.selected = 0;
        self.open = !self.hits.is_empty();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.hits.clear();
        self.selected = 0;
    }

    /// Moves the selection, staying inside the list.
    pub fn move_selection(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() as isize - 1;
        let next = (self.selected as isize + delta).clamp(0, last);
        self.selected = next as usize;
    }

    /// The line as the list shows it: `path:line  text`.
    pub fn label(&self, hit: &Hit) -> String {
        format!(
            "{}:{}  {}",
            display_name(&hit.path),
            hit.line + 1,
            hit.text.trim()
        )
    }

    /// Draws the list; returns the action the user asked for.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        if !self.open {
            return None;
        }
        let mut answer = None;
        let mut open = self.open;

        Modal::new(egui::Id::new("andy-ide-results")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.horizontal(|ui| {
                ui.heading(format!(
                    "{} {}",
                    self.hits.len(),
                    if self.hits.len() == 1 {
                        "reference"
                    } else {
                        "references"
                    }
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("x").on_hover_text("close").clicked() {
                        open = false;
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .max_height(360.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (index, hit) in self.hits.clone().iter().enumerate() {
                        let label = self.label(hit);
                        if ui.selectable_label(index == self.selected, label).clicked() {
                            self.selected = index;
                            answer = Some(Action::OpenFileAt(hit.path.clone(), hit.line));
                            open = false;
                        }
                    }
                });
        });

        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            open = false;
            answer = None;
        }
        if ctx.input(|input| input.key_pressed(Key::ArrowDown)) {
            self.move_selection(1);
        }
        if ctx.input(|input| input.key_pressed(Key::ArrowUp)) {
            self.move_selection(-1);
        }
        if ctx.input(|input| input.key_pressed(Key::Enter))
            && let Some(hit) = self.hits.get(self.selected).cloned()
        {
            answer = Some(Action::OpenFileAt(hit.path, hit.line));
            open = false;
        }

        if !open {
            self.close();
        }
        answer
    }
}
