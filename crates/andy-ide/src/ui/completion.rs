//! The list of words the server offers for the word under the caret.
//!
//! The path is the one the hint at the pointer already takes, with the same traps: the server is
//! asked after the hand has been still for a moment rather than on every letter, and an answer that
//! arrives about another place is thrown away instead of being shown against the wrong word.
//!
//! The list itself is drawn by the editor (see `editor::view`), which is why nothing here touches
//! egui beyond reading the keys while the list is open.

use std::path::PathBuf;

use egui::Key;

/// How long the hand rests on a half-written word before the server is asked, in seconds.
///
/// Shorter than the hint's rest: a person writing a name is not reading, they are waiting for the
/// list, and four tenths of a second of nothing feels broken while typing.
pub const COMPLETION_DELAY: f64 = 0.2;

/// How many words are offered at most — a longer list is one nobody reads.
pub const COMPLETION_LIMIT: usize = 12;

/// One word on offer.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub label: String,
    /// What is written when the word is taken: the server's `insertText` when it sent one, since a
    /// word may be written differently from how it is shown.
    pub insert: String,
    /// What the word is, in a word: `fn`, `field`, `type`, `mod`, `keyword`, …
    pub kind: String,
    /// Whatever the server said beside it — a type, a signature.
    pub detail: String,
}

/// Where a word is: the file, and the place in it the list was asked about.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
}

/// The word being written under the caret: the byte it starts at, and its letters.
///
/// The caret may stand inside a word, not only at its end — a person editing the middle of a name
/// gets a list too, and the word being replaced is the whole one.
pub fn word_under_caret(text: &str, caret: usize) -> Option<(usize, String)> {
    let caret = on_a_character(text, caret);
    let mut start = caret;
    for (index, ch) in text[..caret].char_indices().rev() {
        if ch.is_alphanumeric() || ch == '_' {
            start = index;
        } else {
            break;
        }
    }
    if start == caret {
        return None;
    }
    Some((start, text[start..caret].to_owned()))
}

/// Whether this place is worth asking the server about.
///
/// A half-written word always is. An empty one is worth asking only after `.` or `::` — the two
/// places where a person has plainly begun to choose something — so that a caret resting in
/// whitespace does not pull a list out of the server on its own.
pub fn worth_asking(text: &str, caret: usize) -> bool {
    if word_under_caret(text, caret).is_some() {
        return true;
    }
    let before = &text[..on_a_character(text, caret)];
    before.ends_with('.') || before.ends_with("::")
}

/// A byte offset rounded onto a character boundary, so a caret inside a Cyrillic letter does not
/// split it — the same trap the editor menu hit.
fn on_a_character(text: &str, caret: usize) -> usize {
    let mut caret = caret.min(text.len());
    while caret > 0 && !text.is_char_boundary(caret) {
        caret -= 1;
    }
    caret
}

/// What the server called the kind of a word, as a word a person reads.
fn kind_in_words(kind: Option<lsp_types::CompletionItemKind>) -> String {
    use lsp_types::CompletionItemKind as K;
    match kind {
        Some(K::TEXT) => "text",
        Some(K::METHOD) => "method",
        Some(K::FUNCTION) => "fn",
        Some(K::CONSTRUCTOR) => "ctor",
        Some(K::FIELD) => "field",
        Some(K::VARIABLE) => "var",
        Some(K::CLASS) => "class",
        Some(K::INTERFACE) => "trait",
        Some(K::MODULE) => "mod",
        Some(K::PROPERTY) => "prop",
        Some(K::UNIT) => "unit",
        Some(K::VALUE) => "value",
        Some(K::ENUM) => "enum",
        Some(K::KEYWORD) => "keyword",
        Some(K::SNIPPET) => "snippet",
        Some(K::COLOR) => "colour",
        Some(K::FILE) => "file",
        Some(K::REFERENCE) => "ref",
        Some(K::FOLDER) => "folder",
        Some(K::ENUM_MEMBER) => "variant",
        Some(K::CONSTANT) => "const",
        Some(K::STRUCT) => "struct",
        Some(K::EVENT) => "event",
        Some(K::OPERATOR) => "op",
        Some(K::TYPE_PARAMETER) => "type",
        _ => "",
    }
    .to_owned()
}

/// What the editor needs of the list for one frame.
///
/// The list is drawn by the editor, next to the caret, and the word taken with Enter is written by
/// the editor too — so that a word taken from the list goes through the same path as typing: the
/// file is the editor's, and the server hears about the change the way it hears about any other.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The word taken with Enter, for the editor to write over the word under the caret.
    pub taken: Option<String>,
    /// The lines to show: the word, and what stands beside it.
    pub lines: Vec<(String, String)>,
    /// Which of them is chosen.
    pub chosen: usize,
}

/// The list, and what it is about.
#[derive(Debug, Default)]
pub struct Completion {
    /// The word under the caret and where it starts, when there is one.
    word: Option<(usize, String)>,
    /// The place the list belongs to.
    place: Option<Place>,
    /// The place that has been asked about and not yet answered, with the word it was asked for.
    asked: Option<(Place, String)>,
    /// What the server offered for the place now on screen.
    offers: Vec<Offer>,
    selected: usize,
    /// When the hand settled on this word — the rest clock.
    arrived: f64,
    /// The word the person closed with Escape: not asked about again until it changes.
    closed: Option<(Place, String)>,
    /// Set when Enter took a word: the window writes it into the file and clears this.
    accepted: bool,
}

impl Completion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether Enter has taken a word that has not been written into the file yet.
    pub fn accepted(&self) -> bool {
        self.accepted
    }

    /// Whether a list is on screen.
    pub fn is_open(&self) -> bool {
        !self.offers.is_empty()
    }

    pub fn offers(&self) -> &[Offer] {
        &self.offers
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The word the list is about, for the probe and for the checks.
    pub fn word(&self) -> Option<&str> {
        self.word.as_ref().map(|(_, word)| word.as_str())
    }

    /// Tells the list where the caret is. The rest clock is restarted only when the word itself
    /// changes: a hand is never still, and comparing exact positions would restart it every frame.
    pub fn look_at(&mut self, text: &str, caret: usize, place: Place, now: f64) {
        let word = if worth_asking(text, caret) {
            word_under_caret(text, caret)
        } else {
            None
        };
        let same = match (&self.word, &word) {
            (Some((old_start, old)), Some((start, new))) => old_start == start && old == new,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.word = word;
            self.arrived = now;
            self.asked = None;
            self.offers.clear();
            self.selected = 0;
        }
        self.place = Some(place);
    }

    /// The place to ask about, when the word has rested long enough and is not one the person has
    /// just closed with Escape.
    pub fn due(&self, now: f64) -> Option<Place> {
        let (_, word) = self.word.as_ref()?;
        let place = self.place.clone()?;
        if now - self.arrived < COMPLETION_DELAY {
            return None;
        }
        if self
            .asked
            .as_ref()
            .is_some_and(|(asked, w)| asked == &place && w == word)
        {
            return None;
        }
        if self
            .closed
            .as_ref()
            .is_some_and(|(closed, w)| closed == &place && w == word)
        {
            return None;
        }
        Some(place)
    }

    /// Remembers what was asked, so the same word is not asked twice.
    pub fn asked(&mut self, place: Place) {
        let word = self
            .word
            .as_ref()
            .map(|(_, word)| word.clone())
            .unwrap_or_default();
        self.asked = Some((place, word));
    }

    /// How long the rest clock still has to run, for a window that would otherwise sleep through it.
    pub fn wait(&self, now: f64) -> Option<std::time::Duration> {
        if self.word.is_none() || self.is_open() {
            return None;
        }
        let left = COMPLETION_DELAY - (now - self.arrived);
        (left > 0.0).then(|| std::time::Duration::from_secs_f64(left))
    }

    /// What the server answered about a place.
    ///
    /// An answer about another place, or arriving after the word has moved on, is thrown away: it is
    /// about a word that is no longer being written. What survives is filtered once more by the word
    /// on screen, so a list can never contradict the letters the person can see.
    pub fn answer(&mut self, place: Place, items: Vec<lsp_types::CompletionItem>) {
        let current = self.place.as_ref().is_some_and(|here| here == &place);
        if !current {
            return;
        }
        let prefix = self
            .word
            .as_ref()
            .map(|(_, word)| word.to_lowercase())
            .unwrap_or_default();
        let mut offers: Vec<Offer> = items
            .into_iter()
            .map(|item| Offer {
                label: item.label.clone(),
                insert: item.insert_text.unwrap_or(item.label),
                kind: kind_in_words(item.kind),
                detail: item.detail.unwrap_or_default(),
            })
            .filter(|offer| offer.label.to_lowercase().starts_with(&prefix))
            .collect();
        // The server decides the order (it knows what fits best); only the length is ours.
        offers.truncate(COMPLETION_LIMIT);
        self.offers = offers;
        self.selected = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.offers.is_empty() {
            return;
        }
        let count = self.offers.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(count);
        self.selected = next as usize;
    }

    /// The word to write, when one is taken. Taking is what closes the list.
    pub fn take(&mut self) -> Option<String> {
        let offer = self.offers.get(self.selected)?.clone();
        self.close();
        Some(offer.insert)
    }

    /// Takes the accepted flag away, once the word has been written.
    pub fn accepted_handled(&mut self) {
        self.accepted = false;
    }

    /// Closes the list under this word: Escape, or the caret leaving the word. It is not asked about
    /// again until the word changes.
    pub fn close(&mut self) {
        if let (Some(place), Some((_, word))) = (self.place.clone(), self.word.clone()) {
            self.closed = Some((place, word));
        }
        self.offers.clear();
        self.selected = 0;
    }

    /// Everything, for the window closing a file or a project.
    pub fn forget(&mut self) {
        *self = Self::new();
    }

    /// One line of state, for an env-gated probe.
    pub fn probe_line(&self) -> String {
        format!(
            "word {} | open {} ({} of {}) | asked {} | place {}",
            self.word().unwrap_or("-"),
            self.offers.len(),
            self.selected + 1,
            self.offers.len(),
            self.asked.is_some(),
            self.place
                .as_ref()
                .map(|place| format!(
                    "{}:{}:{}",
                    place.path.display(),
                    place.line,
                    place.character
                ))
                .unwrap_or_else(|| "-".to_owned()),
        )
    }
}

/// The keys the list takes while it is open, read before the editor sees them: while a list is up,
/// the arrows walk it, Enter takes a word and Escape puts it away. Answers whether it took the keys.
pub fn handle_keys(ui: &egui::Ui, list: &mut Completion) -> bool {
    if !list.is_open() {
        return false;
    }
    let mut took = false;
    ui.ctx().input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
            list.move_selection(1);
            took = true;
        }
        if input.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
            list.move_selection(-1);
            took = true;
        }
        if input.consume_key(egui::Modifiers::NONE, Key::Enter) {
            list.accepted = true;
            took = true;
        }
        if input.consume_key(egui::Modifiers::NONE, Key::Escape) {
            list.close();
            took = true;
        }
        // The caret leaving the word leaves the list behind: what is offered no longer fits the
        // letters the person is about to stand among.
        if input.consume_key(egui::Modifiers::NONE, Key::ArrowLeft)
            || input.consume_key(egui::Modifiers::NONE, Key::ArrowRight)
        {
            list.close();
        }
    });
    took
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place() -> Place {
        Place {
            path: PathBuf::from("a.rs"),
            line: 3,
            character: 8,
        }
    }

    #[test]
    fn a_word_is_read_from_under_the_caret() {
        // At the end of a word, in its middle, and in a word of Cyrillic letters: the list is about
        // the same word in each case.
        assert_eq!(word_under_caret("let alp", 7), Some((4, "alp".to_owned())));
        assert_eq!(word_under_caret("alpha", 2), Some((0, "al".to_owned())));
        assert_eq!(
            word_under_caret("let слово", "let слово".len()),
            Some((4, "слово".to_owned())),
            "a word of Cyrillic letters is a word"
        );
        // And whitespace holds no word.
        assert_eq!(word_under_caret("let  ", 5), None);
    }

    #[test]
    fn an_empty_word_is_asked_about_only_after_a_dot() {
        assert!(worth_asking("self.", 5), "after a dot a list is wanted");
        assert!(worth_asking("Vec::", 5), "after :: a list is wanted");
        assert!(worth_asking("let al", 6), "half a word is worth asking");
        assert!(
            !worth_asking("let  ", 5),
            "a caret resting in whitespace must not pull a list out of the server"
        );
        // The dot counts only when it stands right behind the caret: a dot further back in the line
        // is a dot the person has already left.
        assert!(
            !worth_asking("let self . thing", 4),
            "a dot elsewhere in the line is not a place"
        );
    }

    #[test]
    fn the_server_is_asked_once_the_word_has_rested() {
        let mut list = Completion::new();
        list.look_at("let al", 6, place(), 10.0);
        assert!(
            list.due(10.0 + COMPLETION_DELAY / 2.0).is_none(),
            "asked too soon"
        );
        // A tenth past the delay, not exactly on it: a check sitting on a floating-point boundary
        // reads the boundary itself, not the rule.
        let due = list
            .due(10.0 + COMPLETION_DELAY + 0.1)
            .expect("asked once rested");
        list.asked(due);
        assert!(
            list.due(10.0 + COMPLETION_DELAY * 2.0).is_none(),
            "the same word was asked about twice"
        );
        // Typing one more letter is a new word, and it is worth asking again.
        list.look_at("let alp", 7, place(), 11.0);
        assert!(
            list.due(11.0 + COMPLETION_DELAY + 0.1).is_some(),
            "the new word is not asked"
        );
    }

    #[test]
    fn an_answer_about_another_place_is_thrown_away() {
        let mut list = Completion::new();
        list.look_at("let al", 6, place(), 0.0);
        let elsewhere = Place { line: 9, ..place() };
        list.answer(
            elsewhere,
            vec![lsp_types::CompletionItem {
                label: "alpha".to_owned(),
                ..Default::default()
            }],
        );
        assert!(!list.is_open(), "a list about another place was shown");

        list.answer(
            place(),
            vec![lsp_types::CompletionItem {
                label: "alpha".to_owned(),
                ..Default::default()
            }],
        );
        assert_eq!(list.offers().len(), 1);
        assert_eq!(
            list.offers()[0].insert,
            "alpha",
            "the label is the word to write"
        );
    }

    #[test]
    fn only_words_that_fit_the_letters_on_screen_are_shown() {
        let mut list = Completion::new();
        list.look_at("let al", 6, place(), 0.0);
        let item = |label: &str| lsp_types::CompletionItem {
            label: label.to_owned(),
            ..Default::default()
        };
        list.answer(place(), vec![item("alpha"), item("beta"), item("alpine")]);
        let labels: Vec<&str> = list
            .offers()
            .iter()
            .map(|offer| offer.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["alpha", "alpine"],
            "a word that does not fit was offered"
        );
    }

    #[test]
    fn taking_a_word_closes_the_list_and_escape_keeps_it_closed() {
        let mut list = Completion::new();
        list.look_at("let al", 6, place(), 0.0);
        let item = |label: &str| lsp_types::CompletionItem {
            label: label.to_owned(),
            ..Default::default()
        };
        list.answer(place(), vec![item("alpha"), item("alpine")]);
        list.move_selection(1);
        assert_eq!(list.take().as_deref(), Some("alpine"));
        assert!(!list.is_open(), "the list stayed after a word was taken");

        // Escape, and the same word is not asked about again; a new letter is a new word.
        list.look_at("let al", 6, place(), 1.0);
        list.answer(place(), vec![item("alpha")]);
        list.close();
        list.look_at("let al", 6, place(), 2.0);
        assert!(
            list.due(2.0 + COMPLETION_DELAY + 0.1).is_none(),
            "the word closed with Escape was asked about again"
        );
        list.look_at("let alp", 7, place(), 3.0);
        assert!(
            list.due(3.0 + COMPLETION_DELAY + 0.1).is_some(),
            "a word the person went on writing must be asked about"
        );
    }
}
