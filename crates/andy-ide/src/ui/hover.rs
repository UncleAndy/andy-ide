//! The hint that appears when the pointer rests on a symbol.
//!
//! A tooltip cannot be asked for on every frame: the pointer crosses a dozen
//! symbols while it travels, and each crossing would be a request. The pointer
//! has to rest before the question is asked, and a question already asked for
//! this symbol is not asked again — including when the answer was "nothing",
//! which is the common case over whitespace and punctuation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{Id, Order, Pos2};

/// How long the pointer has to rest on a symbol before the server is asked.
pub const HOVER_DELAY: f64 = 0.45;

/// A place in a file the pointer rests on, and where to draw the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
    /// Screen coordinates of the pointer, in points.
    pub screen: Pos2,
}

impl Spot {
    /// Whether two spots are the same symbol in the same place.
    ///
    /// The screen position is not part of it: the pointer may drift a pixel
    /// without leaving the symbol, and the hint must not flicker because of it.
    pub fn same_symbol(&self, path: &Path, line: u32, character: u32) -> bool {
        self.path == path && self.line == line && self.character == character
    }
}

/// What the pointer is doing and what the server said about it.
#[derive(Debug, Default)]
pub struct Hover {
    /// Where the pointer rests this frame, if it rests on text at all.
    current: Option<Spot>,
    /// The spot whose answer is being shown.
    answered: Option<Spot>,
    /// The spot asked about and not yet answered.
    asked: Option<Spot>,
    /// The spot the server said nothing about: asked once, never again.
    nothing: Option<Spot>,
    /// When the pointer arrived at `current`.
    arrived: f64,
    text: Option<String>,
}

impl Hover {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tells the hint where the pointer is. Moving to another spot forgets the
    /// previous answer, so the old signature never hangs beside a new symbol.
    pub fn look_at(&mut self, spot: Option<Spot>, now: f64) {
        match (&self.current, &spot) {
            (None, None) => return,
            (Some(old), Some(new)) if old.same_symbol(&new.path, new.line, new.character) => {
                // Same symbol, possibly a pixel away: only the draw position moves.
                let screen = new.screen;
                self.current = Some(Spot {
                    screen,
                    ..old.clone()
                });
                self.arrived = self.arrived.min(now);
                return;
            }
            _ => {}
        }
        self.current = spot;
        self.arrived = now;
        self.text = None;
        self.answered = None;
        self.asked = None;
        self.nothing = None;
    }

    /// The spot that has been rested on long enough to be worth asking about.
    pub fn due(&self, now: f64) -> Option<Spot> {
        let spot = self.current.as_ref()?;
        if now - self.arrived < HOVER_DELAY {
            return None;
        }
        if let Some(asked) = &self.asked
            && asked.same_symbol(&spot.path, spot.line, spot.character)
        {
            return None;
        }
        if let Some(nothing) = &self.nothing
            && nothing.same_symbol(&spot.path, spot.line, spot.character)
        {
            return None;
        }
        Some(spot.clone())
    }

    /// When the question will be due, for a window that would otherwise sleep
    /// through it.
    pub fn wait(&self) -> Option<Duration> {
        self.current.as_ref()?;
        if self.asked.is_some() || self.text.is_some() {
            return None;
        }
        Some(Duration::from_secs_f64(HOVER_DELAY))
    }

    /// Records that the server has been asked about `spot`.
    pub fn asked(&mut self, spot: Spot) {
        self.asked = Some(spot);
    }

    /// Takes the answer to the question about `path`, `line`, `character`.
    ///
    /// An answer to a question about another place is dropped: the pointer has
    /// moved on while the server was thinking, and its old answer is no longer
    /// about anything on screen.
    pub fn answer(&mut self, path: &Path, line: u32, character: u32, text: Option<String>) {
        let Some(spot) = self.current.clone() else {
            return;
        };
        // Only an answer to the question that was asked counts: the pointer has
        // to be on that symbol, and that symbol has to be the one asked about.
        if !spot.same_symbol(path, line, character) {
            return;
        }
        let Some(asked) = &self.asked else {
            return;
        };
        if !asked.same_symbol(path, line, character) {
            return;
        }
        match text.filter(|text| !text.trim().is_empty()) {
            Some(text) => {
                self.answered = Some(spot);
                self.text = Some(text);
            }
            None => {
                self.nothing = Some(spot);
                self.answered = None;
                self.text = None;
            }
        }
    }

    /// Forgets everything: a modal, a tab switch or a closed file makes any hint
    /// on screen a lie.
    pub fn forget(&mut self) {
        self.current = None;
        self.answered = None;
        self.asked = None;
        self.nothing = None;
        self.text = None;
    }

    /// Forgets everything while a modal covers the editor: the pointer is over
    /// the modal, not over the symbol the hint is about.
    pub fn forget_if_modal(&mut self, modal: bool) {
        if modal {
            self.forget();
        }
    }

    /// The text of the hint, if one is to be drawn.
    pub fn text(&self) -> Option<&str> {
        let answered = self.answered.as_ref()?;
        let current = self.current.as_ref()?;
        if !current.same_symbol(&answered.path, answered.line, answered.character) {
            return None;
        }
        self.text.as_deref()
    }

    /// Draws the hint beside the pointer; returns whether it drew anything.
    pub fn show(&self, ctx: &egui::Context) -> bool {
        let Some(text) = self.text() else {
            return false;
        };
        let Some(spot) = self.current.as_ref() else {
            return false;
        };
        let (signature, prose) = split_hover(text);
        let anchor = spot.screen + egui::vec2(14.0, 20.0);
        egui::Area::new(Id::new("hover_hint"))
            .order(Order::Tooltip)
            .fixed_pos(anchor)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(560.0);
                    if let Some(signature) = &signature {
                        ui.label(egui::RichText::new(signature).monospace());
                    }
                    if !prose.is_empty() {
                        if signature.is_some() {
                            ui.separator();
                        }
                        ui.label(prose);
                    }
                });
            });
        true
    }
}

/// The signature and the prose of an answer.
///
/// Servers answer in Markdown, and the first block is nearly always a fenced
/// signature. Painting the fences and the backticks as they are is noise, so the
/// block is shown as a signature and the rest as text.
pub fn split_hover(text: &str) -> (Option<String>, String) {
    let text = text.trim();
    let mut rest = text;
    let mut code: Vec<&str> = Vec::new();
    // Servers put the type of the symbol and its signature in separate blocks, so
    // every leading block is code, not only the first one.
    while let Some(after) = rest.strip_prefix("```") {
        let Some(newline) = after.find('\n') else {
            break;
        };
        let body = &after[newline + 1..];
        // The closing fence is a line of its own, and may be the very first line
        // when the block is empty.
        let end = if body.starts_with("```") {
            0
        } else {
            let Some(offset) = body.find("\n```") else {
                break;
            };
            offset + 1
        };
        let block = body[..end].trim_end();
        if !block.is_empty() {
            code.push(block);
        }
        rest = body[end + 3..].trim_start();
    }
    // rust-analyzer separates the signature from the documentation with a rule.
    let prose = rest.strip_prefix("---").unwrap_or(rest).trim();
    if code.is_empty() {
        return (None, prose.to_owned());
    }
    (Some(code.join("\n")), prose.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(line: u32, character: u32) -> Spot {
        Spot {
            path: PathBuf::from("src/main.rs"),
            line,
            character,
            screen: Pos2::new(300.0, 200.0),
        }
    }

    #[test]
    fn nothing_is_asked_before_the_pointer_rests() {
        let mut hover = Hover::new();
        hover.look_at(Some(spot(3, 5)), 10.0);

        assert!(
            hover.due(10.0 + HOVER_DELAY / 2.0).is_none(),
            "the question was asked while the pointer was still moving"
        );
        assert!(
            hover.due(10.0 + HOVER_DELAY + 0.01).is_some(),
            "the question was never asked"
        );
    }

    #[test]
    fn the_same_symbol_is_asked_once() {
        let mut hover = Hover::new();
        hover.look_at(Some(spot(3, 5)), 0.0);
        let due = hover.due(HOVER_DELAY + 0.01).unwrap();
        hover.asked(due);
        assert!(hover.due(HOVER_DELAY + 5.0).is_none());
    }

    #[test]
    fn a_symbol_the_server_said_nothing_about_is_not_asked_again() {
        let mut hover = Hover::new();
        hover.look_at(Some(spot(3, 5)), 0.0);
        hover.asked(spot(3, 5));
        hover.answer(Path::new("src/main.rs"), 3, 5, None);

        assert!(hover.text().is_none());
        assert!(
            hover.due(HOVER_DELAY + 5.0).is_none(),
            "an answer of \"nothing\" was asked for again"
        );
    }

    #[test]
    fn moving_to_another_symbol_forgets_the_answer() {
        let mut hover = Hover::new();
        hover.look_at(Some(spot(3, 5)), 0.0);
        hover.asked(spot(3, 5));
        hover.answer(
            Path::new("src/main.rs"),
            3,
            5,
            Some("```rust\nfn a()\n```".to_owned()),
        );
        assert!(hover.text().is_some());

        hover.look_at(Some(spot(7, 1)), 1.0);
        assert!(
            hover.text().is_none(),
            "the old signature hung beside another symbol"
        );
    }

    #[test]
    fn an_answer_about_another_place_is_dropped() {
        let mut hover = Hover::new();
        hover.look_at(Some(spot(3, 5)), 0.0);
        hover.asked(spot(3, 5));
        // The pointer moved while the server was thinking.
        hover.look_at(Some(spot(9, 2)), 0.5);
        hover.answer(
            Path::new("src/main.rs"),
            3,
            5,
            Some("```rust\nfn a()\n```".to_owned()),
        );

        assert!(
            hover.text().is_none(),
            "an answer about the old spot was shown"
        );
    }

    #[test]
    fn a_pointer_a_pixel_away_keeps_the_answer() {
        let mut hover = Hover::new();
        hover.look_at(Some(spot(3, 5)), 0.0);
        hover.asked(spot(3, 5));
        hover.answer(
            Path::new("src/main.rs"),
            3,
            5,
            Some("```rust\nfn a()\n```".to_owned()),
        );

        let mut moved = spot(3, 5);
        moved.screen = Pos2::new(301.0, 201.0);
        hover.look_at(Some(moved), 0.1);

        assert!(hover.text().is_some(), "the hint flickered away");
        assert!(
            hover.due(HOVER_DELAY + 9.0).is_none(),
            "the same symbol was asked about a second time"
        );
    }

    #[test]
    fn leaving_the_text_forgets_the_hint() {
        let mut hover = Hover::new();
        hover.look_at(Some(spot(3, 5)), 0.0);
        hover.asked(spot(3, 5));
        hover.answer(
            Path::new("src/main.rs"),
            3,
            5,
            Some("```rust\nfn a()\n```".to_owned()),
        );

        hover.look_at(None, 1.0);
        assert!(hover.text().is_none());
        assert!(hover.due(100.0).is_none());
    }

    #[test]
    fn a_signature_and_its_documentation_are_split() {
        let text = "```rust\nfn make(x: u8) -> Point\n```\n---\n\nMakes a point.";
        assert_eq!(
            split_hover(text),
            (
                Some("fn make(x: u8) -> Point".to_owned()),
                "Makes a point.".to_owned()
            )
        );
    }

    #[test]
    fn an_answer_without_a_fence_is_all_text() {
        assert_eq!(split_hover("just words"), (None, "just words".to_owned()));
        assert_eq!(
            split_hover("```rust\nno end"),
            (None, "```rust\nno end".to_owned())
        );
    }

    #[test]
    fn an_empty_signature_still_leaves_the_prose() {
        let text = "```\n```\nSome words";
        assert_eq!(split_hover(text), (None, "Some words".to_owned()));
    }
}

#[cfg(test)]
mod unsolicited {
    use super::*;

    #[test]
    fn an_answer_nobody_asked_for_is_ignored() {
        let mut hover = Hover::new();
        hover.look_at(
            Some(Spot {
                path: PathBuf::from("a.rs"),
                line: 1,
                character: 2,
                screen: Pos2::ZERO,
            }),
            0.0,
        );
        // Nothing was asked: the pointer only just arrived.
        hover.answer(
            Path::new("a.rs"),
            1,
            2,
            Some("```rust\nfn a()\n```".to_owned()),
        );
        assert!(
            hover.text().is_none(),
            "an answer to a question that was never asked was shown"
        );
    }
}

#[cfg(test)]
mod a_real_answer {
    use super::split_hover;

    #[test]
    fn the_type_and_the_signature_are_two_blocks_of_code() {
        // What rust-analyzer answered for `Point::new` in the measured run.
        let text = "```rust\nsample::Point\n```\n\n```rust\npub fn new(x: i32, y: i32) -> Self\n```\n\n---\n\nMakes the origin.";
        assert_eq!(
            split_hover(text),
            (
                Some("sample::Point\npub fn new(x: i32, y: i32) -> Self".to_owned()),
                "Makes the origin.".to_owned()
            )
        );
    }
}
