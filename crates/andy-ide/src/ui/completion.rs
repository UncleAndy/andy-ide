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

/// Whether two places are about the same word: the same file, on the same line.
///
/// The character is left out on purpose. Letters are added to the right of a word, which moves the caret with
/// every key — so an answer about this object would be thrown away for a caret that took a step while the
/// answer was on its way. That is why a method list appeared only once the hand stopped: Andy, "варианты
/// начинают предлагаться только после набора первых 3-х символов из имени метода". Which word it is on that
/// line is decided by the letters themselves (see the prefix the offers are filtered by).
fn about_the_same_line(one: &Place, other: &Place) -> bool {
    one.path == other.path && one.line == other.line
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

/// What the list writes when a word is taken: what the server said to write, with the snippet's
/// placeholders taken out.
///
/// A method comes from the server with its brackets and a place for the caret (`unwrap()$0`), and a person
/// who takes a method wants exactly that — the brackets, and the caret inside them, ready for the
/// arguments. The bare label is what was taken before, which left the brackets to be typed by hand. Andy:
/// "при выборе метода хорошо бы его добавлять уже с первой скобкой".
fn written(item: &lsp_types::CompletionItem) -> String {
    let text = item
        .text_edit
        .as_ref()
        .map(|edit| match edit {
            lsp_types::CompletionTextEdit::Edit(edit) => edit.new_text.clone(),
            lsp_types::CompletionTextEdit::InsertAndReplace(edit) => edit.new_text.clone(),
        })
        .or_else(|| item.insert_text.clone())
        .unwrap_or_else(|| item.label.clone());
    without_placeholders(&text)
}

/// The snippet's placeholders taken out: `unwrap()$0` is written as `unwrap()`, and `${1:name}` as `name`.
fn without_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('0'..='9') => {
                while matches!(chars.peek(), Some('0'..='9')) {
                    chars.next();
                }
            }
            Some('{') => {
                chars.next();
                let mut inner = String::new();
                let mut depth = 1usize;
                for letter in chars.by_ref() {
                    match letter {
                        '{' => {
                            depth += 1;
                            inner.push('{');
                        }
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            inner.push('}');
                        }
                        _ => inner.push(letter),
                    }
                }
                // `${1:name}` keeps the name; `${1|one,two|}` keeps the first choice — and in that second
                // form the number stands before the first bar, so the choices start after it.
                let kept = if let Some((_, rest)) = inner.split_once(':') {
                    rest.to_owned()
                } else if let Some((_, rest)) = inner.split_once('|') {
                    rest.split('|')
                        .next()
                        .unwrap_or("")
                        .split(',')
                        .next()
                        .unwrap_or("")
                        .to_owned()
                } else {
                    inner.clone()
                };
                out.push_str(&kept);
            }
            _ => out.push('$'),
        }
    }
    out
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
    /// The key the editor met while the list was open, to be answered where the list lives.
    ///
    /// The editor reads a *clone* of the frame's events (`input.events.clone()`), so taking a key in
    /// the input does not keep it out of the editor's hands; the list therefore says which key it
    /// wants and answers it at the end of the frame, where the words are.
    pub taken_key: Option<egui::Key>,
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
    /// Whether the word on screen is one the person wrote: the list is for writing, not for reading.
    ///
    /// Andy: "при переходе текстового курсора на любой уже существующий идентификатор вылезает
    /// подсказка для автозаполнения, хотя там нечего автозаполнять". A word the caret merely walked
    /// into is not being written, and nothing is asked about it.
    arming: bool,
    /// The file's key count as it was when this word was last looked at: it has grown since only if a
    /// key wrote something, which is what "being written" means here. A count, not a flag: a flag for
    /// the frame is lost the moment the buffer is drawn twice in one frame.
    seen_keystrokes: u64,
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
    pub fn look_at(&mut self, text: &str, caret: usize, place: Place, now: f64, keystrokes: u64) {
        // A key has written something since the last look — and a count cannot be lost the way a
        // per-frame flag could be.
        let typing = keystrokes != self.seen_keystrokes;
        self.seen_keystrokes = keystrokes;
        let word = if worth_asking(text, caret) {
            // A dot or `::` just behind the caret: the person is choosing a member of what stands before it,
            // and what they are choosing begins where the caret stands — empty, but worth asking about. Until
            // this existed a dot asked nothing at all (`due` needs a word), so no list could come out of one.
            word_under_caret(text, caret)
                .or_else(|| Some((on_a_character(text, caret), String::new())))
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
            self.selected = 0;
            // A word the person is writing is worth asking about; one the caret only arrived at is not.
            self.arming = typing;
            // And a word that is gone — the last letter deleted — has nothing left to offer: a list still
            // standing over it belongs to letters that are no longer there. Andy: "после удаления
            // последнего символа окно подсказки должно скрыться, а оно всё равно висит".
            if !typing || self.word.is_none() {
                self.offers.clear();
            }
            // What was offered for a longer word still stands for its beginning, but nothing that no longer
            // fits the letters on screen: the list is about this word, not the one it grew out of.
            if let Some((_, word)) = &self.word {
                let prefix = word.to_lowercase();
                self.offers
                    .retain(|offer| offer.label.to_lowercase().starts_with(&prefix));
            }
        }
        self.place = Some(place);
    }

    /// The place to ask about, when the word has rested long enough and is not one the person has
    /// just closed with Escape.
    pub fn due(&self, now: f64) -> Option<Place> {
        if !self.arming {
            return None;
        }
        let (_, word) = self.word.as_ref()?;
        let place = self.place.clone()?;
        if now - self.arrived < COMPLETION_DELAY {
            return None;
        }
        if self
            .asked
            .as_ref()
            .is_some_and(|(asked, w)| about_the_same_line(asked, &place) && w == word)
        {
            return None;
        }
        if self
            .closed
            .as_ref()
            .is_some_and(|(closed, w)| about_the_same_line(closed, &place) && w == word)
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
        if self.word.is_none() || self.is_open() || !self.arming {
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
        let current = self
            .place
            .as_ref()
            .is_some_and(|here| about_the_same_line(here, &place));
        if !current {
            return;
        }
        // An answer arriving after the word has been deleted has no letters to belong to, whatever place
        // it names.
        if self.word.is_none() {
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
                insert: written(&item),
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

/// The keys the list answers to, and only those: the arrows to walk it, Enter to take a word, Escape
/// to put it away. Left and right are the list's too — a caret that walks out of the word has nothing
/// left to complete — and the response there is to close rather than to move.
pub fn list_key(key: Key, modifiers: egui::Modifiers) -> Option<Key> {
    if modifiers != egui::Modifiers::NONE {
        return None;
    }
    matches!(
        key,
        Key::ArrowUp | Key::ArrowDown | Key::Enter | Key::Escape | Key::ArrowLeft | Key::ArrowRight
    )
    .then_some(key)
}

/// Answers one key the editor met while the list was open. True when the list used it.
pub fn handle_key(list: &mut Completion, key: Key) -> bool {
    if !list.is_open() {
        return false;
    }
    match key {
        Key::ArrowDown => list.move_selection(1),
        Key::ArrowUp => list.move_selection(-1),
        Key::Enter => list.accepted = true,
        Key::Escape => list.close(),
        // The caret leaving the word leaves the list behind: what is offered no longer fits the letters
        // the person is about to stand among.
        Key::ArrowLeft | Key::ArrowRight => list.close(),
        _ => return false,
    }
    true
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
    fn the_keys_the_list_answers_to_are_its_own() {
        assert_eq!(
            list_key(Key::ArrowDown, egui::Modifiers::NONE),
            Some(Key::ArrowDown)
        );
        assert_eq!(
            list_key(Key::Enter, egui::Modifiers::SHIFT),
            None,
            "Shift+Enter is the editor's: a new line, not a word"
        );
        assert_eq!(
            list_key(Key::A, egui::Modifiers::NONE),
            None,
            "letters belong to the text"
        );
        // And what it does with one: the arrows walk, Enter takes, Escape puts away.
        let mut list = Completion::new();
        list.look_at("let al", 6, place(), 0.0, 1);
        list.answer(
            place(),
            vec![
                lsp_types::CompletionItem {
                    label: "alpha".to_owned(),
                    ..Default::default()
                },
                lsp_types::CompletionItem {
                    label: "alpine".to_owned(),
                    ..Default::default()
                },
            ],
        );
        assert!(
            handle_key(&mut list, Key::ArrowDown),
            "the list refused an arrow"
        );
        assert_eq!(list.selected(), 1);
        assert!(handle_key(&mut list, Key::Enter), "the list refused Enter");
        assert!(list.accepted());
        // Taking is what closes it: the state takes the word at the end of the frame, once the word has
        // been written into the file — not the moment Enter is read.
        assert_eq!(list.take().as_deref(), Some("alpine"));
        assert!(
            !list.is_open(),
            "the list stayed open after a word was taken"
        );
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

    /// The place with the caret standing at the told character, so a check can say where it looks.
    fn place_at(character: u32) -> Place {
        Place {
            character,
            ..place()
        }
    }

    /// One thing the server could offer.
    fn offered(label: &str) -> lsp_types::CompletionItem {
        lsp_types::CompletionItem {
            label: label.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn a_dot_asks_for_the_members_of_what_stands_before_it() {
        // Andy: "разве список всех методов не должен выдаваться сразу после нажатия на точку?" — and it did
        // not: `due` needs a word, and after a dot there is none.
        let mut list = Completion::new();
        list.look_at("value.", 6, place_at(6), 0.0, 1);
        assert_eq!(
            list.due(0.3),
            Some(place_at(6)),
            "the dot was not asked about at all"
        );
        list.answer(
            place_at(6),
            vec![offered("len"), offered("lines"), offered("push")],
        );
        assert!(
            list.is_open(),
            "the members of what stands before the dot never arrived"
        );
    }

    #[test]
    fn an_answer_about_a_member_survives_the_letters_typed_after_the_dot() {
        // The caret moves a step to the right with every letter, but the place the question was asked at does
        // not: it is where the word starts. So an answer that is still on its way is not thrown away for a
        // caret that has moved — which is what made the list appear only after three letters.
        let mut list = Completion::new();
        list.look_at("value.", 6, place_at(6), 0.0, 1);
        assert_eq!(
            list.due(0.3),
            Some(place_at(6)),
            "the dot was not asked about"
        );

        list.look_at("value.l", 7, place_at(7), 0.4, 2);
        list.answer(
            place_at(6),
            vec![offered("len"), offered("lines"), offered("push")],
        );
        assert!(
            list.is_open(),
            "an answer about this object was thrown away because the caret moved"
        );
        assert_eq!(
            list.offers().len(),
            2,
            "the list showed words that do not fit the letter on screen: {:?}",
            list.offers()
                .iter()
                .map(|o| o.label.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_taken_method_is_written_with_its_brackets() {
        // Andy: "при выборе метода хорошо бы его добавлять уже с первой скобкой" — the server says so in the
        // text it wants written, and the label alone left the brackets to be typed by hand.
        let mut called = lsp_types::CompletionItem {
            label: "unwrap".to_owned(),
            text_edit: Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
                range: lsp_types::Range::default(),
                new_text: "unwrap()$0".to_owned(),
            })),
            ..Default::default()
        };
        assert_eq!(written(&called), "unwrap()");

        // A placeholder that stands for a name keeps the name, and the first of several choices is taken.
        called.text_edit = Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
            range: lsp_types::Range::default(),
            new_text: "push(${1:value})$0".to_owned(),
        }));
        assert_eq!(written(&called), "push(value)");
        called.text_edit = Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
            range: lsp_types::Range::default(),
            new_text: "push(${1|one,two|})$0".to_owned(),
        }));
        assert_eq!(written(&called), "push(one)");

        // With nothing to write given, the label stands as it always did.
        called.text_edit = None;
        assert_eq!(written(&called), "unwrap");
    }

    #[test]
    fn a_word_deleted_to_nothing_takes_the_list_with_it() {
        let item = |label: &str| lsp_types::CompletionItem {
            label: label.to_owned(),
            ..Default::default()
        };
        let mut list = Completion::new();
        list.look_at("let par", 7, place(), 0.0, 1);
        list.answer(place(), vec![item("part")]);
        assert!(list.is_open(), "the list did not open over a written word");

        // Letters go one at a time: while any is left the list stays, and the last one takes it away.
        list.look_at("let pa", 6, place(), 0.1, 2);
        list.answer(place(), vec![item("part")]);
        assert!(
            list.is_open(),
            "the list closed while letters were still there"
        );
        list.look_at("let p", 5, place(), 0.2, 3);
        list.answer(place(), vec![item("part")]);
        assert!(
            list.is_open(),
            "the list closed while letters were still there"
        );
        list.look_at("let ", 4, place(), 0.3, 4);
        assert!(
            !list.is_open(),
            "the list stayed over a word that had been deleted to nothing"
        );
        // And an answer that arrives after that is not shown either.
        list.answer(place(), vec![item("part")]);
        assert!(
            !list.is_open(),
            "an answer arriving after the deletion put the list back"
        );
    }

    #[test]
    fn a_word_the_caret_only_walked_into_is_not_asked_about() {
        let mut list = Completion::new();
        // The person writes `alpha`, and the count grows: this one is being written, so it is asked
        // about.
        list.look_at("let alpha = 1;", 9, place(), 0.0, 1);
        assert_eq!(list.word(), Some("alpha"));
        assert!(
            list.due(0.0 + COMPLETION_DELAY + 0.1).is_some(),
            "a written word is not asked about"
        );

        // The caret then walks into another word with no key written — the count stands still — and
        // nothing is asked about it: Andy's "вылезает подсказка, хотя там нечего автозаполнять".
        list.look_at("let alpha = beta;", 16, place(), 1.0, 1);
        assert_eq!(
            list.word(),
            Some("beta"),
            "the check is not on the walked-into word"
        );
        assert!(
            list.due(1.0 + COMPLETION_DELAY + 0.1).is_none(),
            "an existing identifier was asked about"
        );

        // One key written into it — the count grows — and it is worth asking about again.
        list.look_at("let alpha = betax;", 17, place(), 2.0, 2);
        assert_eq!(list.word(), Some("betax"));
        assert!(
            list.due(2.0 + COMPLETION_DELAY + 0.1).is_some(),
            "a word being written is not asked about"
        );
    }

    #[test]
    fn the_server_is_asked_once_the_word_has_rested() {
        let mut list = Completion::new();
        list.look_at("let al", 6, place(), 10.0, 1);
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
        list.look_at("let alp", 7, place(), 11.0, 2);
        assert!(
            list.due(11.0 + COMPLETION_DELAY + 0.1).is_some(),
            "the new word is not asked"
        );
    }

    #[test]
    fn an_answer_about_another_place_is_thrown_away() {
        let mut list = Completion::new();
        list.look_at("let al", 6, place(), 0.0, 1);
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
        list.look_at("let al", 6, place(), 0.0, 1);
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
        list.look_at("let al", 6, place(), 0.0, 1);
        let item = |label: &str| lsp_types::CompletionItem {
            label: label.to_owned(),
            ..Default::default()
        };
        list.answer(place(), vec![item("alpha"), item("alpine")]);
        list.move_selection(1);
        assert_eq!(list.take().as_deref(), Some("alpine"));
        assert!(!list.is_open(), "the list stayed after a word was taken");

        // Escape, and the same word is not asked about again; a new letter is a new word.
        list.look_at("let al", 6, place(), 1.0, 1);
        list.answer(place(), vec![item("alpha")]);
        list.close();
        list.look_at("let al", 6, place(), 2.0, 1);
        assert!(
            list.due(2.0 + COMPLETION_DELAY + 0.1).is_none(),
            "the word closed with Escape was asked about again"
        );
        list.look_at("let alp", 7, place(), 3.0, 2);
        assert!(
            list.due(3.0 + COMPLETION_DELAY + 0.1).is_some(),
            "a word the person went on writing must be asked about"
        );
    }
}
