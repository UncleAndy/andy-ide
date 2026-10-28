//! Where a symbol is used: the list `Shift+F12` puts on screen.
//!
//! A modal rather than a docked panel, because a question that has just been
//! asked deserves an answer the user can dismiss with Escape, and because the
//! answers are usually few. The list is built from the server's answer, so it is
//! plain data: the panel only shows it and reports what was chosen.

use std::path::PathBuf;

use egui::{Key, Modal};

use crate::state::{Action, display_name};

/// The kinds of symbol the list distinguishes.
///
/// The server reports about two dozen kinds; a file's outline needs to tell a
/// function from a type at a glance and does not care whether a type is a
/// struct or an enum. The marker in front of each row says which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Type,
    Constant,
    Variable,
    Module,
    Field,
    Other,
}

impl SymbolKind {
    /// Reads the server's kind. The numeric constants are the ones the protocol
    /// defines; anything newer falls into `Other` rather than being dropped.
    pub fn from_lsp(kind: lsp_types::SymbolKind) -> Self {
        use lsp_types::SymbolKind as Lsp;
        match kind {
            Lsp::FUNCTION => Self::Function,
            Lsp::METHOD | Lsp::CONSTRUCTOR => Self::Method,
            Lsp::STRUCT | Lsp::ENUM | Lsp::INTERFACE | Lsp::CLASS | Lsp::TYPE_PARAMETER => {
                Self::Type
            }
            Lsp::CONSTANT => Self::Constant,
            Lsp::VARIABLE => Self::Variable,
            Lsp::MODULE | Lsp::NAMESPACE | Lsp::PACKAGE => Self::Module,
            Lsp::FIELD | Lsp::PROPERTY | Lsp::ENUM_MEMBER => Self::Field,
            _ => Self::Other,
        }
    }

    /// The word the list shows in front of a name.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Method => "method",
            Self::Type => "type",
            Self::Constant => "const",
            Self::Variable => "var",
            Self::Module => "mod",
            Self::Field => "field",
            Self::Other => "symbol",
        }
    }
}

/// One place a symbol is used, or one symbol of an outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// 0-based line and character, as the server counts them.
    pub line: u32,
    pub character: u32,
    /// The text of that line for a reference; the name for a symbol.
    pub text: String,
    /// How deep in the outline the row sits; always 0 for references.
    pub depth: usize,
    /// The kind of symbol; `None` for a reference, which is a place, not a symbol.
    pub kind: Option<SymbolKind>,
}

impl Hit {
    /// A row that is a place in the code rather than a symbol.
    pub fn reference(path: PathBuf, line: u32, character: u32, text: String) -> Self {
        Self {
            path,
            line,
            character,
            text,
            depth: 0,
            kind: None,
        }
    }

    /// A row that is a named symbol, possibly nested in another.
    pub fn symbol(
        path: PathBuf,
        line: u32,
        character: u32,
        name: String,
        depth: usize,
        kind: SymbolKind,
    ) -> Self {
        Self {
            path,
            line,
            character,
            text: name,
            depth,
            kind: Some(kind),
        }
    }
}

/// The list on screen.
#[derive(Debug, Default)]
pub struct Results {
    pub open: bool,
    /// What the list is, when the count alone does not say it: "Symbols in
    /// `app.rs`" reads better than "12 symbols".
    pub title: Option<String>,
    pub hits: Vec<Hit>,
    pub selected: usize,
}

impl Results {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fills the list and shows it.
    pub fn show_hits(&mut self, hits: Vec<Hit>) {
        self.title = None;
        self.fill(hits);
    }

    /// Fills the list with an outline and shows it under `title`.
    pub fn show_symbols(&mut self, title: String, hits: Vec<Hit>) {
        self.title = Some(title);
        self.fill(hits);
    }

    fn fill(&mut self, hits: Vec<Hit>) {
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

    /// The line as the list shows it.
    ///
    /// A reference reads `path:line  text`. An outline row indents by depth and
    /// names the kind, and it leaves the path out when every row has the same
    /// one — a file's outline would otherwise repeat the file name on each line.
    pub fn label(&self, hit: &Hit) -> String {
        let line = hit.line + 1;
        match hit.kind {
            None => format!("{}:{}  {}", display_name(&hit.path), line, hit.text.trim()),
            Some(kind) => {
                let indent = "  ".repeat(hit.depth);
                let same_file = self.hits.iter().all(|other| other.path == hit.path);
                if same_file {
                    format!("{indent}{} {}  :{line}", kind.marker(), hit.text.trim())
                } else {
                    format!(
                        "{indent}{} {}  {}:{line}",
                        kind.marker(),
                        hit.text.trim(),
                        display_name(&hit.path)
                    )
                }
            }
        }
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
                let heading = match &self.title {
                    Some(title) => title.clone(),
                    None => format!(
                        "{} {}",
                        self.hits.len(),
                        if self.hits.len() == 1 {
                            "reference"
                        } else {
                            "references"
                        }
                    ),
                };
                ui.heading(heading);
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
                            answer = Some(Action::OpenFileAt(
                                hit.path.clone(),
                                hit.line,
                                hit.character,
                            ));
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
            answer = Some(Action::OpenFileAt(hit.path, hit.line, hit.character));
            open = false;
        }

        if !open {
            self.close();
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::{Hit, Results, SymbolKind};

    fn outline() -> Results {
        let mut results = Results::new();
        results.show_symbols(
            "Symbols in app.rs: 3".to_owned(),
            vec![
                Hit::symbol("app.rs".into(), 9, 8, "App".to_owned(), 0, SymbolKind::Type),
                Hit::symbol(
                    "app.rs".into(),
                    12,
                    4,
                    "draw".to_owned(),
                    1,
                    SymbolKind::Method,
                ),
                Hit::symbol(
                    "app.rs".into(),
                    40,
                    0,
                    "run".to_owned(),
                    0,
                    SymbolKind::Function,
                ),
            ],
        );
        results
    }

    #[test]
    fn a_reference_reads_as_path_line_and_text() {
        let mut results = Results::new();
        results.show_hits(vec![Hit::reference(
            "src/app.rs".into(),
            4,
            2,
            "x".to_owned(),
        )]);
        assert_eq!(results.label(&results.hits[0].clone()), "app.rs:5  x");
    }

    #[test]
    fn an_outline_indents_children_and_names_the_kind() {
        let results = outline();
        let rows: Vec<String> = results
            .hits
            .clone()
            .iter()
            .map(|hit| results.label(hit))
            .collect();
        assert_eq!(rows[0], "type App  :10");
        assert_eq!(rows[1], "  method draw  :13", "a child is not indented");
        assert_eq!(rows[2], "fn run  :41");
    }

    #[test]
    fn an_outline_names_the_file_when_the_rows_span_files() {
        let mut results = Results::new();
        results.show_symbols(
            "Project symbols: 2".to_owned(),
            vec![
                Hit::symbol(
                    "a.rs".into(),
                    0,
                    0,
                    "one".to_owned(),
                    0,
                    SymbolKind::Function,
                ),
                Hit::symbol("b/c.rs".into(), 5, 0, "two".to_owned(), 0, SymbolKind::Type),
            ],
        );
        assert_eq!(results.label(&results.hits[0].clone()), "fn one  a.rs:1");
        assert_eq!(results.label(&results.hits[1].clone()), "type two  c.rs:6");
    }

    #[test]
    fn the_server_kinds_map_to_the_few_the_list_shows() {
        use lsp_types::SymbolKind as Lsp;
        assert_eq!(SymbolKind::from_lsp(Lsp::FUNCTION), SymbolKind::Function);
        assert_eq!(SymbolKind::from_lsp(Lsp::STRUCT), SymbolKind::Type);
        assert_eq!(SymbolKind::from_lsp(Lsp::ENUM), SymbolKind::Type);
        assert_eq!(SymbolKind::from_lsp(Lsp::METHOD), SymbolKind::Method);
        assert_eq!(SymbolKind::from_lsp(Lsp::FIELD), SymbolKind::Field);
        assert_eq!(SymbolKind::from_lsp(Lsp::MODULE), SymbolKind::Module);
        assert_eq!(SymbolKind::from_lsp(Lsp::CONSTANT), SymbolKind::Constant);
        assert_eq!(SymbolKind::from_lsp(Lsp::BOOLEAN), SymbolKind::Other);
    }

    #[test]
    fn a_title_replaces_the_count_of_references() {
        let results = outline();
        assert_eq!(results.title.as_deref(), Some("Symbols in app.rs: 3"));
        let mut references = Results::new();
        references.show_hits(vec![Hit::reference("a.rs".into(), 0, 0, "x".to_owned())]);
        assert_eq!(
            references.title, None,
            "a reference list got an outline title"
        );
    }
}
