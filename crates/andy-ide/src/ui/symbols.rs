//! Symbols of the project: the field `Ctrl+Shift+F12` opens.
//!
//! The question is a search, not a listing. `workspace/symbol` with an empty
//! query is not a project index: rust-analyzer answers a crate and its top-level
//! types, nothing more, and a client that painted that as "all symbols" would be
//! lying. So the field asks the server for what the user types, and each answer
//! replaces the last one.

use egui::{Context, Key};

use crate::state::Action;
use crate::ui::results::Hit;

/// How many rows the list can show: a dialog is not a code review.
pub const HITS: usize = 300;

/// How long the field waits after a keystroke before asking the server.
///
/// Every letter would otherwise be a request, and the last one would arrive last
/// only by luck: the worker answers in order, but a server is free to be slow.
pub const DEBOUNCE: f64 = 0.18;

/// The state of the project-symbol search.
#[derive(Debug, Default)]
pub struct SymbolSearch {
    pub open: bool,
    pub query: String,
    /// The question the server has been asked, if any.
    sent: Option<String>,
    /// When the query last changed, on the egui clock.
    changed_at: f64,
    /// The query the text was when the clock was last read.
    last_text: String,
    /// The rows on screen, and the query they answer.
    pub hits: Vec<Hit>,
    answered: Option<String>,
    selected: usize,
    /// The field takes the keyboard on the first frame it is shown.
    focus: bool,
}

impl SymbolSearch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the field with a clean query and nothing on screen.
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.last_text.clear();
        self.sent = None;
        self.changed_at = 0.0;
        self.hits.clear();
        self.answered = None;
        self.selected = 0;
        self.focus = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus = false;
    }

    /// The query to ask the server about now, if the field has settled.
    ///
    /// Called once per frame. A query the server has already been asked is not
    /// asked again, so a frame loop cannot turn into a request loop.
    pub fn due(&mut self, now: f64) -> Option<String> {
        if !self.open {
            return None;
        }
        if self.query != self.last_text {
            self.last_text = self.query.clone();
            self.changed_at = now;
        }
        if self.query.trim().is_empty() {
            return None;
        }
        if self.sent.as_deref() == Some(self.query.as_str()) {
            return None;
        }
        if now - self.changed_at < DEBOUNCE {
            return None;
        }
        self.sent = Some(self.query.clone());
        Some(self.query.clone())
    }

    /// Takes an answer, dropping it when the user has typed on since.
    pub fn apply(&mut self, query: &str, hits: Vec<Hit>) {
        if !self.open || query != self.query {
            return;
        }
        self.hits = hits;
        self.answered = Some(query.to_owned());
        self.selected = 0;
    }

    /// What the list says when it has nothing to show yet.
    pub fn hint(&self) -> &'static str {
        if self.query.trim().is_empty() {
            "type a name: the server searches the project"
        } else if self.answered.as_deref() == Some(self.query.as_str()) {
            "nothing matches"
        } else {
            "searching..."
        }
    }

    /// The line as the list shows it: `kind name  path:line`.
    pub fn label(&self, hit: &Hit) -> String {
        let line = hit.line + 1;
        match hit.kind {
            Some(kind) => format!(
                "{} {}  {}:{line}",
                kind.marker(),
                hit.text,
                crate::state::display_name(&hit.path)
            ),
            None => format!(
                "{}:{}  {}",
                crate::state::display_name(&hit.path),
                line,
                hit.text
            ),
        }
    }

    /// Moves the selection, staying inside the list.
    pub fn move_selection(&mut self, delta: isize) {
        if self.hits.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.hits.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Draws the field and its list; returns the action the user asked for.
    pub fn show(&mut self, ctx: &Context) -> Option<Action> {
        if !self.open {
            return None;
        }

        // The debounce has to run out in an idle window too, so the field asks
        // for the frames that will send the query.
        ctx.request_repaint_after(std::time::Duration::from_millis(60));

        // The list owns the arrow keys while it is up, so they are taken before
        // the text field can use them to move its own caret.
        let mut answer = None;
        let mut close = false;
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown)) {
            self.move_selection(1);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp)) {
            self.move_selection(-1);
        }
        let enter = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
        let escape = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));

        egui::Modal::new(egui::Id::new("andy-ide-symbols")).show(ctx, |ui| {
            ui.set_width(620.0);
            ui.horizontal(|ui| {
                ui.label("Symbol");
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("name or part of it")
                        .desired_width(f32::INFINITY),
                );
                if self.focus {
                    field.request_focus();
                    self.focus = false;
                }
            });
            ui.separator();

            if self.hits.is_empty() {
                ui.label(self.hint());
            } else {
                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, hit) in self.hits.clone().iter().enumerate().take(HITS) {
                            let label = self.label(hit);
                            if ui.selectable_label(index == self.selected, label).clicked() {
                                self.selected = index;
                                answer = Some(Action::OpenFileAt(
                                    hit.path.clone(),
                                    hit.line,
                                    hit.character,
                                ));
                                close = true;
                            }
                        }
                    });
            }
        });

        if escape {
            close = true;
            answer = None;
        }
        if enter && let Some(hit) = self.hits.get(self.selected).cloned() {
            answer = Some(Action::OpenFileAt(hit.path, hit.line, hit.character));
            close = true;
        }
        if close {
            self.close();
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::{DEBOUNCE, SymbolSearch};
    use crate::ui::results::{Hit, SymbolKind};

    fn hit(name: &str) -> Hit {
        Hit::symbol(
            "a.rs".into(),
            1,
            4,
            name.to_owned(),
            0,
            SymbolKind::Function,
        )
    }

    #[test]
    fn a_query_goes_out_once_it_has_settled() {
        let mut search = SymbolSearch::new();
        search.open();
        search.query = "ma".to_owned();

        assert_eq!(search.due(1.0), None, "the query went out while typing");
        assert_eq!(
            search.due(1.0 + DEBOUNCE / 2.0),
            None,
            "the debounce was shorter than promised"
        );
        assert_eq!(
            search.due(1.0 + DEBOUNCE + 0.01),
            Some("ma".to_owned()),
            "the settled query never went out"
        );
        assert_eq!(
            search.due(10.0),
            None,
            "the same query was asked again: the frame loop became a request loop"
        );
    }

    #[test]
    fn typing_again_asks_again() {
        let mut search = SymbolSearch::new();
        search.open();
        search.query = "a".to_owned();
        assert_eq!(search.due(1.0), None);
        assert_eq!(search.due(2.0), Some("a".to_owned()));

        search.query = "ab".to_owned();
        assert_eq!(search.due(2.1), None, "the new query did not wait");
        assert_eq!(search.due(2.1 + DEBOUNCE), Some("ab".to_owned()));
    }

    #[test]
    fn an_empty_query_is_not_sent() {
        let mut search = SymbolSearch::new();
        search.open();
        assert_eq!(search.due(10.0), None);
        search.query = "   ".to_owned();
        assert_eq!(search.due(20.0), None, "blank space was searched for");
    }

    #[test]
    fn an_answer_to_a_forgotten_query_is_dropped() {
        let mut search = SymbolSearch::new();
        search.open();
        search.query = "ab".to_owned();
        let _ = search.due(1.0);

        // The user typed on before the answer arrived.
        search.query = "abc".to_owned();
        search.apply("ab", vec![hit("stale")]);
        assert!(search.hits.is_empty(), "a stale answer was shown");
        assert_eq!(search.hint(), "searching...");

        search.apply("abc", vec![hit("fresh")]);
        assert_eq!(search.hits.len(), 1);
        assert_eq!(search.hits[0].text, "fresh");
    }

    #[test]
    fn a_closed_search_takes_no_answers() {
        let mut search = SymbolSearch::new();
        search.query = "a".to_owned();
        search.apply("a", vec![hit("x")]);
        assert!(search.hits.is_empty(), "an answer filled a closed search");
    }

    #[test]
    fn nothing_matching_says_so() {
        let mut search = SymbolSearch::new();
        search.open();
        search.query = "zzz".to_owned();
        let _ = search.due(1.0);
        search.apply("zzz", Vec::new());
        assert_eq!(search.hint(), "nothing matches");
    }

    #[test]
    fn a_row_names_its_kind_and_place() {
        let search = SymbolSearch::new();
        assert_eq!(search.label(&hit("draw")), "fn draw  a.rs:2");
    }
}
