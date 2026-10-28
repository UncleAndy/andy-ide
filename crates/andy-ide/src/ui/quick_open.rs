//! Quick-open: a modal list of project files, filtered as you type.

use std::path::PathBuf;

use egui::{Context, Key};
use ide_core::{FileIndex, SearchHit};

/// How many hits the list shows; more than a screenful is not a list any more.
pub const HITS: usize = 20;

/// The state of the quick-open field.
#[derive(Debug, Default)]
pub struct QuickOpen {
    pub query: String,
    pub open: bool,
    selected: usize,
    /// The field takes the keyboard on the first frame it is shown.
    focus: bool,
}

impl QuickOpen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the field with a clean query.
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.focus = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus = false;
    }

    /// The files to show, best match first.
    pub fn hits(&self, index: Option<&FileIndex>) -> Vec<SearchHit> {
        match index {
            Some(index) => index.search(&self.query, HITS),
            None => Vec::new(),
        }
    }

    /// Moves the selection, staying inside the list.
    pub fn move_selection(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.selected = 0;
            return;
        }
        let last = count - 1;
        let moved = self.selected as isize + delta;
        self.selected = moved.clamp(0, last as isize) as usize;
    }

    /// Draws the field and its list; returns the file the user picked.
    pub fn show(&mut self, ctx: &Context, index: Option<&FileIndex>) -> Option<PathBuf> {
        if !self.open {
            return None;
        }

        // The list owns the arrow keys while it is up, so they are taken before
        // the text field can use them to move its own caret.
        let hits = self.hits(index);
        let mut picked = None;
        let mut close = false;
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown)) {
            self.move_selection(1, hits.len());
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp)) {
            self.move_selection(-1, hits.len());
        }
        let enter = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
        let escape = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));

        egui::Modal::new(egui::Id::new("andy-ide-quick-open")).show(ctx, |ui| {
            ui.set_width(520.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("file name")
                    .desired_width(f32::INFINITY),
            );
            if self.focus {
                field.request_focus();
                self.focus = false;
            }
            if field.changed() {
                // A new query means the old selection is meaningless.
                self.selected = 0;
            }
            ui.separator();

            if hits.is_empty() {
                ui.weak(if self.query.is_empty() {
                    "type to search"
                } else {
                    "nothing matches"
                });
                return;
            }

            self.selected = self.selected.min(hits.len() - 1);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (position, hit) in hits.iter().enumerate() {
                        let label = hit.rel.display().to_string();
                        let chosen = ui.selectable_label(position == self.selected, label);
                        if chosen.clicked() {
                            self.selected = position;
                            picked = Some(hit.rel.clone());
                        }
                    }
                });
            let _ = self.selected;
        });

        if escape {
            close = true;
        }
        if enter && let Some(hit) = hits.get(self.selected) {
            picked = Some(hit.rel.clone());
        }
        if picked.is_some() || close {
            self.close();
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn index() -> FileIndex {
        FileIndex::from_paths(vec![
            PathBuf::from("crates/andy-ide/src/app.rs"),
            PathBuf::from("crates/ide-core/src/config.rs"),
            PathBuf::from("README.md"),
        ])
    }

    #[test]
    fn the_selection_stays_inside_the_list() {
        let mut quick = QuickOpen::new();
        quick.move_selection(-1, 3);
        assert_eq!(quick.selected, 0);
        quick.move_selection(5, 3);
        assert_eq!(quick.selected, 2);
        quick.move_selection(1, 3);
        assert_eq!(quick.selected, 2);
    }

    #[test]
    fn an_empty_list_has_no_selection() {
        let mut quick = QuickOpen::new();
        quick.move_selection(3, 0);
        assert_eq!(quick.selected, 0);
    }

    #[test]
    fn opening_clears_the_previous_query() {
        let mut quick = QuickOpen::new();
        quick.query = "old".to_owned();
        quick.open();
        assert!(quick.open);
        assert!(quick.query.is_empty());
        assert_eq!(quick.selected, 0);
    }

    #[test]
    fn hits_follow_the_query() {
        let mut quick = QuickOpen::new();
        let index = index();
        assert_eq!(
            quick.hits(Some(&index)).len(),
            3,
            "everything on an empty query"
        );

        quick.query = "config".to_owned();
        let hits = quick.hits(Some(&index));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, PathBuf::from("crates/ide-core/src/config.rs"));

        assert!(quick.hits(None).is_empty(), "no index, no list");
    }
}
