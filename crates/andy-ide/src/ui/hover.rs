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

/// How long a hint stays after the pointer has left the symbol.
///
/// A hint vanishes the instant the pointer leaves the symbol it is about, and the
/// pointer is usually on its way into the hint — to read it, to scroll it, to copy a
/// line out of it. The delay is the time it takes to cross that gap: without it the
/// hint is gone before the hand has moved, which makes a big answer unreadable past
/// its first line.
pub const HOVER_LINGER: f64 = 0.35;

/// The widest a hint gets, in points: a long paragraph of documentation is easier to
/// read in a column than across the window.
pub const HINT_WIDTH: f32 = 560.0;

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

    /// Whether this is the same *place* as another spot, as a resting pointer sees it.
    ///
    /// The measure is the place on screen, not the symbol under it: a hand held still drifts by a pixel or
    /// two, and that drift can cross a line boundary — so counting `line` and `character` made the pointer
    /// look like it had moved on every frame, and the rest clock (`HOVER_DELAY`) never ran out. Andy's
    /// hints were never asked for because of it; measured with `ANDY_IDE_HOVER_PROBE` as `rest 0.00s` on a
    /// symbol he was holding the pointer on.
    ///
    /// Four points is about a line of text's height in these fonts: enough for a hand, far less than a
    /// deliberate move to another word.
    pub fn same_place(&self, other: &Spot) -> bool {
        self.same_symbol(&other.path, other.line, other.character)
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
    /// What is on screen, which outlives the pointer being on the symbol: the hint is
    /// shown from here, so it can stay while the pointer crosses the gap into it.
    shown: Option<Hint>,
    /// When the pointer left the symbol the hint is about, and the clock started.
    leaving: Option<f64>,
    /// Whether the pointer was over the hint itself last frame.
    over: bool,
    /// Where the hint was drawn last frame, for the pointer test and for the tests.
    rect: Option<egui::Rect>,
    /// The shaped Markdown of the last answer drawn: re-shaping it every frame of a
    /// sitting pointer would be work thrown away sixty times a second.
    markdown: egui_commonmark::CommonMarkCache,
}

/// A hint on screen: what it says and where it is anchored.
#[derive(Debug, Clone)]
struct Hint {
    spot: Spot,
    text: String,
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
            // The rest clock is a debounce, not a lock. A hand is never still, and a pointer a pixel away
            // can land on the *neighbouring character*: counting that as "moved on" reset the clock on
            // every frame, so the delay never elapsed and the server was never asked at all. Andy's
            // missing hints, measured with `ANDY_IDE_HOVER_PROBE`: `rest 0.00s` on a symbol he was holding
            // the pointer on. A file change or another line still counts as moving on.
            (Some(old), Some(new)) if old.same_place(new) => {
                // Same place, a pixel or two away: only the draw position moves.
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

    /// Forgets which symbols were written off while there was no server to ask.
    ///
    /// A spot entered during the seconds rust-analyzer spends indexing is marked as asked, because there is
    /// nothing to ask yet — and that mark must not outlive the wait, or every symbol the pointer crossed in
    /// those seconds stays silent for good. Andy's report: ten to fifteen seconds of nothing, then hints.
    pub fn forget_marks(&mut self) {
        self.asked = None;
        self.nothing = None;
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

    /// Decides what is on screen this frame, and whether the clock is running.
    ///
    /// Called once per frame, after the pointer has been looked at and before the hint
    /// is drawn. An answer that has just arrived is what is shown; when the pointer has
    /// left the symbol, the hint stays for the length of the linger, and while the
    /// pointer is on the hint itself it does not go away at all — that is what makes it
    /// possible to scroll a long answer, select a line out of it and follow a link.
    pub fn settle(&mut self, now: f64) {
        if let Some(text) = self.text().map(str::to_owned)
            && let Some(spot) = self.current.clone()
        {
            self.shown = Some(Hint { spot, text });
            self.leaving = None;
            return;
        }
        if self.shown.is_none() {
            self.leaving = None;
            return;
        }
        // On the hint: the clock does not run, and a pointer that comes back to the
        // symbol starts the next frame as if it had never left.
        if self.over {
            self.leaving = None;
            return;
        }
        match self.leaving {
            None => self.leaving = Some(now),
            Some(since) if now - since >= HOVER_LINGER => {
                self.shown = None;
                self.leaving = None;
                self.rect = None;
            }
            Some(_) => {}
        }
    }

    /// How long the hint has left, for a window that would otherwise sleep through the
    /// moment it disappears.
    pub fn leaving_in(&self, now: f64) -> Option<Duration> {
        let since = self.leaving?;
        let left = (HOVER_LINGER - (now - since)).max(0.0);
        Some(Duration::from_secs_f64(left))
    }

    /// Whether the pointer is on the hint itself, which is on top of the text: what is
    /// under the hint is not what the pointer is on, and must not be asked about.
    pub fn is_over_hint(&self) -> bool {
        self.over
    }

    /// Whether the pointer is on the hint as it was last drawn — and a note that it is.
    ///
    /// The hint is drawn after the spot under the pointer has been read, so `over`, set once it is drawn, is a
    /// frame behind. The frame that first brings the pointer onto the hint would otherwise still ask about the
    /// text *under* it and move the hint to the spot found there: a hint that jumps aside the moment the
    /// pointer goes to scroll it. Andy's report, and why the panel's geometry decided it — a symbol has to
    /// happen to sit under the pointer for the jump to show.
    pub fn pointer_on_the_hint(&mut self, pos: egui::Pos2) -> bool {
        if self.over {
            return true;
        }
        if self.rect.is_some_and(|rect| rect.contains(pos)) {
            self.over = true;
            return true;
        }
        false
    }

    /// Where the hint was drawn, if it is on screen: where the pointer has to be for
    /// the hint to stay, and where a test looks for it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn shown_rect(&self) -> Option<egui::Rect> {
        self.rect
    }

    /// Forgets everything: a modal, a tab switch or a closed file makes any hint
    /// on screen a lie.
    pub fn forget(&mut self) {
        self.current = None;
        self.answered = None;
        self.asked = None;
        self.nothing = None;
        self.text = None;
        self.shown = None;
        self.leaving = None;
        self.rect = None;
        self.over = false;
    }

    /// Forgets everything while a modal covers the editor: the pointer is over
    /// the modal, not over the symbol the hint is about.
    pub fn forget_if_modal(&mut self, modal: bool) {
        if modal {
            self.forget();
        }
    }

    /// The text of the hint, if one is to be drawn.
    /// The whole state of the hint in one line, for the `ANDY_IDE_HOVER_PROBE` run: how long the pointer
    /// has rested, whether it is over the hint itself, which spot it is on, which one was asked about,
    /// which one the server said nothing about, and whether an answer is held. A hint that never appears is
    /// one of a few things, and this tells them apart without guessing.
    pub fn probe_line(&self, now: f64) -> String {
        let spot = |what: &Option<Spot>| match what {
            Some(spot) => format!("{}:{}:{}", spot.path.display(), spot.line, spot.character),
            None => "-".to_owned(),
        };
        format!(
            "rest {:.2}s | over_hint {} | current {} | asked {} | nothing {} | answered {} | text {}",
            (now - self.arrived).max(0.0),
            self.is_over_hint(),
            spot(&self.current),
            spot(&self.asked),
            spot(&self.nothing),
            spot(&self.answered),
            self.text.as_ref().map_or(0, |text| text.len()),
        )
    }

    pub fn text(&self) -> Option<&str> {
        let answered = self.answered.as_ref()?;
        let current = self.current.as_ref()?;
        if !current.same_symbol(&answered.path, answered.line, answered.character) {
            return None;
        }
        self.text.as_deref()
    }

    /// Draws the hint; returns whether it drew anything.
    ///
    /// Servers write their documentation in Markdown — `**bold**`, `- lists`, `` `code` ``
    /// and tables — and a tooltip that shows the punctuation shows the source, not the
    /// documentation. The signature keeps its monospace face (it is code, and its
    /// indentation carries meaning), the prose is read as Markdown while it is drawn.
    ///
    /// The hint is a window, not a picture of one: the pointer can move into it without
    /// it vanishing, the wheel scrolls an answer that does not fit, and the text can be
    /// selected and copied. It is kept inside the window, and never grows taller than the
    /// window can show — a scroll bar appears instead of the answer running off the
    /// bottom of the screen.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let Some(showing) = self.shown.clone() else {
            self.over = false;
            self.rect = None;
            return false;
        };
        let (signature, prose) = split_hover(&showing.text);
        let viewport = ctx.viewport_rect();
        let margin = 12.0;
        // The padding of the popup frame is part of the hint's height, so what the text
        // itself may use is the window minus the margins and that padding.
        let max_width = HINT_WIDTH.min(viewport.width() - 2.0 * margin);
        // The whole window is the budget, not a part of it: a hint is scrolled only when
        // it has taken all the height there is and still has more to say. The frame's own
        // padding is taken off inside, where the style is at hand.
        let max_height = (viewport.height() - 2.0 * margin).max(120.0);

        // Beside the pointer, and the area's own constraint keeps the rest of it inside
        // the window: a hint that does not fit below the pointer is moved up until it
        // does, and only a hint taller than the window is scrolled.
        let wanted = showing.spot.screen + egui::vec2(14.0, 20.0);
        let anchor = egui::pos2(
            wanted
                .x
                .min(viewport.right() - margin - 40.0)
                .max(viewport.left() + margin),
            wanted
                .y
                .min(viewport.bottom() - margin - 40.0)
                .max(viewport.top() + margin),
        );

        let drawn = egui::Area::new(Id::new("hover_hint"))
            .order(Order::Tooltip)
            .fixed_pos(anchor)
            .interactable(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    // What the text may use: the window, less the margins and the frame's
                    // own padding, which is part of the hint's height.
                    let room = (max_height - ui.style().spacing.menu_margin.sum().y).max(100.0);
                    ui.set_max_width(max_width);
                    // The area is given the height this hint is allowed: without it egui
                    // caps the popup at a fraction of the screen, and a long answer is
                    // clipped down to a handful of lines while the rest of the window
                    // stands empty.
                    ui.set_max_height(room);
                    egui::ScrollArea::vertical()
                        .id_salt("hover_hint_scroll")
                        .max_height(room)
                        .show(ui, |ui| {
                            if let Some(signature) = &signature {
                                // The key words of a hint: the name and the shape it is written in are what a
                                // person looks for, and the prose below is what they read afterwards. So the
                                // signature is drawn in the theme's own bright colour, and the description is
                                // left as it was. Andy: "во всех всплывающих подсказках по методам или
                                // переменным ключевые слова надо подсвечивать ярче, а описание можно и так как
                                // сейчас".
                                ui.label(
                                    egui::RichText::new(signature)
                                        .monospace()
                                        .color(ui.visuals().strong_text_color()),
                                );
                            }
                            if !prose.is_empty() {
                                if signature.is_some() {
                                    ui.separator();
                                }
                                egui_commonmark::CommonMarkViewer::new().show(
                                    ui,
                                    &mut self.markdown,
                                    &prose,
                                );
                            }
                        });
                });
                ui.min_rect()
            });

        self.rect = Some(drawn.inner);
        self.over = ctx
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| drawn.inner.contains(pos));
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

    /// A spot at a symbol of the one file these checks hover in.
    ///
    /// The place *on screen* follows the symbol: a pointer that moved to another symbol is somewhere else on
    /// screen, and the rest clock reads places rather than symbols. A fixture with one fixed screen position
    /// for every symbol would describe a pointer that never moved — which the checks about "the pointer went
    /// elsewhere" then read as "it stayed", because it really had not.
    fn spot(line: u32, character: u32) -> Spot {
        Spot {
            path: PathBuf::from("src/main.rs"),
            line,
            character,
            screen: Pos2::new(300.0 + character as f32 * 8.0, 200.0 + line as f32 * 17.0),
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
