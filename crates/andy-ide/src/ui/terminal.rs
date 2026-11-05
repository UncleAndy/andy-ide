//! The terminal panel: a real shell on a pseudo-terminal, drawn cell by cell.
//!
//! The session and the screen live in `ide-term`; this module only turns the screen
//! into rectangles and glyphs and turns egui's keys into the bytes a terminal expects.
//! A terminal is not a text field — `Ctrl+C` is byte 3, arrows are escape sequences —
//! so the mapping is a function of its own, tested without a window.

use egui::{Key, Modifiers, Ui};

use ide_term::Session;
use ide_term::keys::{Direction, Input, Mods};
use ide_term::mouse;

/// One session with everything that belongs to it alone.
///
/// Two shells on screen must not share a selection, a pointer position or a scroll
/// remainder, or clicking in one would extend the highlight in the other.
struct Pane {
    session: Session,
    /// What the tab says.
    title: String,
    /// The text the user has picked out with the mouse, if any.
    selection: Option<Selection>,
    /// Whether a drag is in progress, so a moving pointer keeps extending it.
    dragging: bool,
    /// Where the press landed. A press is not a selection by itself: a click that never moves picks
    /// nothing out, and the selection appears when the pointer reaches another cell.
    anchor: Option<(u16, u16)>,
    /// The cell the pointer was over last frame, so a movement is reported once
    /// rather than sixty times a second.
    mouse_cell: Option<(u16, u16)>,
    /// Scrolling arrives in pixels and is reported in notches; the remainder is
    /// kept here so a slow wheel is not lost.
    scroll_left: f32,
    /// How many lines above the live screen the pane is looking, for `Shift+PageUp`.
    ///
    /// Zero is the bottom — where the program is writing. The lines themselves are kept by the session
    /// (`ide_term::SCROLLBACK`); this is only where the eye stands.
    view: usize,
}

impl Pane {
    fn new(session: Session, title: String) -> Self {
        Self {
            session,
            title,
            selection: None,
            dragging: false,
            anchor: None,
            mouse_cell: None,
            scroll_left: 0.0,
            view: 0,
        }
    }
}

/// A terminal tab: one session, or the reason it could not start.
pub struct Terminal {
    /// The sessions, in the order of their tabs.
    panes: Vec<Pane>,
    /// Which of them is in front.
    active: usize,
    pub error: Option<String>,
    cols: u16,
    rows: u16,
    /// Where new shells start: the project's own root, not wherever the editor was
    /// started from. Set by the application, which is the only thing that knows which
    /// project is open.
    pub cwd: Option<std::path::PathBuf>,
    /// The program a new session runs; empty means the shell `$SHELL` names.
    pub shell: String,
    /// How many lines above the screen a session keeps.
    pub scrollback: usize,
    /// Whether keys typed in this frame belong to the terminal.
    pub focused: bool,
    /// When a key was last handed to the shell, in the window's own seconds.
    ///
    /// The cursor is lit while a person is typing and only blinks once the keys have gone quiet: a
    /// block that goes dark in the middle of a word reads as a cursor that has been lost.
    last_key_at: Option<f64>,
    /// Whether the panel was drawn at all in the last frame.
    ///
    /// The panel is only drawn while its tab is visible, so this flag is the only way
    /// for the application to notice that the keyboard must be handed back: a hidden
    /// terminal that keeps focus would swallow every keystroke of the editor.
    pub shown: bool,
    /// Where the panel was last drawn, for the tests that click it.
    pub shown_rect: Option<egui::Rect>,
    /// A link that was `Ctrl`-clicked this frame, for the application to act on.
    link_clicked: Option<ide_term::link::Link>,
}

/// The colour under selected cells.
const SELECTION: egui::Color32 = egui::Color32::from_rgb(38, 79, 120);

/// The panel the terminal's text stands on. A cursor cell is the one place where it is drawn *over*
/// the text, so it has to be known by name: the letter under the block is put back in this colour.
const PAPER: egui::Color32 = egui::Color32::from_rgb(16, 18, 20);

/// Text picked out with the mouse, in cells of the screen.
///
/// The two ends are kept as they were clicked rather than sorted, so dragging back
/// over the start of a selection grows it the other way instead of collapsing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Where the drag began: `(column, row)`.
    pub anchor: (u16, u16),
    /// Where the pointer is now.
    pub head: (u16, u16),
}

impl Selection {
    /// The two ends in reading order, start first.
    pub fn ordered(self) -> ((u16, u16), (u16, u16)) {
        let key = |(column, row): (u16, u16)| (row, column);
        if key(self.anchor) <= key(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether a cell is inside the selection.
    pub fn contains(self, column: u16, row: u16) -> bool {
        let (start, end) = self.ordered();
        let (first, last) = ((start.1, start.0), (end.1, end.0));
        (row, column) >= first && (row, column) <= last
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            panes: Vec::new(),
            active: 0,
            error: None,
            cols: 80,
            rows: 24,
            cwd: None,
            shell: String::new(),
            scrollback: ide_term::SCROLLBACK,
            focused: false,
            last_key_at: None,
            shown: false,
            shown_rect: None,
            link_clicked: None,
        }
    }

    /// The session in front.
    fn pane(&self) -> Option<&Pane> {
        self.panes.get(self.active)
    }

    fn pane_mut(&mut self) -> Option<&mut Pane> {
        self.panes.get_mut(self.active)
    }

    /// Starts the shell once, at the size the panel had room for.
    pub fn ensure_started(&mut self, cols: u16, rows: u16) {
        if !self.panes.is_empty() || self.error.is_some() {
            return;
        }
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.new_pane();
    }

    /// Opens another session and puts it in front.
    ///
    /// A second shell is a second `Session`, not a second view of the first: it has
    /// its own process, its own screen and its own selection.
    pub fn new_pane(&mut self) {
        match Session::shell_as(
            Some(&self.shell),
            self.scrollback,
            self.cols,
            self.rows,
            self.cwd.as_deref(),
        ) {
            Ok(session) => {
                // Numbered from the start: the dock tab is called `Terminal`, and two
                // things with one name cannot be told apart by the tests or the eye.
                let title = String::new();
                self.panes.push(Pane::new(session, title));
                self.active = self.panes.len() - 1;
                self.renumber();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    /// Closes one session. Closing the last leaves the panel empty, and the next
    /// frame starts a fresh shell.
    pub fn close_pane(&mut self, index: usize) {
        if index >= self.panes.len() {
            return;
        }
        let mut pane = self.panes.remove(index);
        pane.session.kill();
        self.renumber();
        if self.active > index {
            self.active -= 1;
        }
        if self.active >= self.panes.len() {
            self.active = self.panes.len().saturating_sub(1);
        }
    }

    /// Numbers the tabs by where they stand, so closing the first leaves `Terminal 1`
    /// rather than a lone `Terminal 2`.
    fn renumber(&mut self) {
        for (index, pane) in self.panes.iter_mut().enumerate() {
            pane.title = format!("Terminal {}", index + 1);
        }
    }

    /// How many sessions the panel holds, and which one is in front.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn active_pane(&self) -> usize {
        self.active
    }

    /// The bytes typed this frame, taken out of egui's events.
    ///
    /// The events are **removed**, not read: with the terminal focused the keys belong
    /// to the shell, and an editor that also saw them would type the same text into the
    /// open file — which is exactly what happened before this was a `take`.
    pub fn take_keys(&mut self, ui: &mut Ui) -> Vec<u8> {
        // debug
        // Read once per frame, before anything is written: a program that has just
        // turned the mode on expects the very next arrow in the new form.
        let app = self.application_cursor();
        // Read once: the copy and cut events carry no modifiers of their own, and which chord sent
        // them is the whole difference between copying and stopping a program.
        let modifiers_now = ui.input(|input| input.modifiers);
        let events = ui.input_mut(|input| Self::split_terminal_events(&mut input.events));
        let mut bytes = Vec::new();
        let mut wanted_view: Option<usize> = None;
        {
            for event in &events {
                match event {
                    egui::Event::Text(text)
                        // Control characters arrive as text too; they are already in
                        // the byte stream and must not be doubled.
                        if !text.is_empty() && !text.chars().all(char::is_control) =>
                    {
                        bytes.extend_from_slice(text.as_bytes());
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        // `Ctrl+Shift+C` and `Ctrl+Insert` copy the selection: a
                        // terminal needs a copy key of its own, because `Ctrl+C`
                        // there means "stop". `Ctrl+Insert` is the one the editor
                        // already uses for the same thing.
                        let copying = (*key == egui::Key::C && modifiers.ctrl && modifiers.shift)
                            || (*key == egui::Key::Insert && modifiers.ctrl);
                        if copying {
                            self.copy_selection(ui);
                            continue;
                        }
                        // `Shift+Insert` pastes, the other habit from the same
                        // keyboard. The backend owns the clipboard, so it is asked to
                        // read the text and send it back as a paste event, exactly as
                        // the editor does.
                        if (*key == egui::Key::Insert && modifiers.shift)
                            || (*key == egui::Key::V && modifiers.ctrl)
                        {
                            // `Ctrl+V` as well as `Shift+Insert`: pasting is what a person asks for
                            // with either, and a terminal that sent the control byte instead would
                            // answer with a quoted-insert prompt nobody wants.
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                            continue;
                        }
                        // The terminal's own scrollback keys, before anything is typed: see
                        // `walks_the_scrollback`. What they ask for is applied once the events have been read.
                        if walks_the_scrollback(*key, *modifiers) {
                            let up = *key == egui::Key::PageUp;
                            let now = self.pane().map(|pane| pane.view).unwrap_or(0);
                            wanted_view = Some(scrolled(now, up, self.rows, ide_term::SCROLLBACK));
                            continue;
                        }
                        if let Some(byte) = control_letter(*key, *modifiers) {
                            bytes.push(byte);
                        } else if let Some(input) = input_for_key(*key, *modifiers, app) {
                            bytes.extend_from_slice(&input.bytes());
                        }
                    }
                    // The platform's copy chord never arrives as a key: `egui-winit` turns
                    // `Ctrl`+`C` into this event, because `is_copy_command` asks
                    // `modifiers.command`, and on Linux and Windows that is `Ctrl`. A terminal that
                    // answered a copy with a copy would leave a running program unstoppable, so the
                    // plain chord is byte 3 and the copy is the one holding `Shift`.
                    egui::Event::Copy => match chord_of(modifiers_now) {
                        Chord::Copy => self.copy_selection(ui),
                        Chord::ToTheShell => bytes.push(0x03),
                    },
                    // `Ctrl`+`X` is handed over the same way; with nothing to cut here, the shell is
                    // given the byte.
                    egui::Event::Cut if chord_of(modifiers_now) == Chord::ToTheShell => {
                        bytes.push(0x18)
                    }
                    egui::Event::Cut => {}
                    egui::Event::Paste(text) => bytes.extend_from_slice(text.as_bytes()),
                    _ => {}
                }
            }
        }
        if !bytes.is_empty() {
            self.last_key_at = Some(ui.input(|input| input.time));
        }
        if let Some(pane) = self.pane_mut() {
            // A page key moves the eye; anything written moves it back to the bottom, because a person who
            // types wants to see what they type.
            pane.view = match wanted_view {
                Some(view) => view,
                None if !bytes.is_empty() => 0,
                None => pane.view,
            };
        }
        bytes
    }

    /// Adds the terminal's own events to `bytes` and takes them out of `events`.
    ///
    /// Pointer, scroll and focus events stay where they are: they are not typing.
    pub fn split_terminal_events(events: &mut Vec<egui::Event>) -> Vec<egui::Event> {
        let mut taken = Vec::new();
        events.retain(|event| {
            let mine = matches!(
                event,
                egui::Event::Text(_)
                    | egui::Event::Copy
                    | egui::Event::Cut
                    | egui::Event::Paste(_)
                    | egui::Event::Key { pressed: true, .. }
            );
            if mine {
                taken.push(event.clone());
            }
            !mine
        });
        taken
    }

    /// The row of sessions: one button each, and the one that opens another.
    ///
    /// It sits above the screen rather than beside it, so the screen keeps the full
    /// width — the same reason the bottom of the window is one strip of tabs.
    /// Returns whether the session in front was changed: the keys of the window belong to whichever
    /// session a person has just picked, and the press that picked it landed on the strip — outside
    /// the canvas — which is what the canvas reads as "the keys are not mine".
    fn session_strip(&mut self, ui: &mut Ui) -> bool {
        let mut choose = None;
        let mut close = None;
        let mut open = false;
        ui.horizontal(|ui| {
            for (index, pane) in self.panes.iter().enumerate() {
                let title = egui::RichText::new(tab_label(&pane.title, pane.view)).monospace();
                let title = if index == self.active {
                    title.strong()
                } else {
                    title
                };
                if ui.selectable_label(index == self.active, title).clicked() {
                    choose = Some(index);
                }
                if ui
                    .add(egui::Button::new("×").small().frame(false))
                    .on_hover_text("close this session")
                    .clicked()
                {
                    close = Some(index);
                }
                ui.separator();
            }
            if ui
                .add(egui::Button::new("+").small())
                .on_hover_text("open another session")
                .clicked()
            {
                open = true;
            }
        });
        let mut switched = false;
        if let Some(index) = choose {
            self.active = index;
            switched = true;
        }
        if let Some(index) = close {
            self.close_pane(index);
        }
        if open {
            self.new_pane();
            // A session opened to be used: the person who pressed `+` is about to type in it.
            switched = true;
        }
        switched
    }

    /// Draws the screen and lets the user type into it.
    pub fn show(&mut self, ui: &mut Ui) {
        self.shown = true;
        let switched = self.session_strip(ui);
        let width = ui.available_width();
        let height = ui.available_height();
        let font = egui::FontId::monospace(13.0);
        // The width of one character, measured by the painter that will draw it.
        let cell = ui
            .painter()
            .layout_no_wrap("M".to_owned(), font.clone(), egui::Color32::WHITE)
            .size();
        let (cell_width, cell_height) = (cell.x.max(1.0), cell.y.max(1.0));
        let cols = (width / cell_width).floor().max(1.0) as u16;
        let rows = (height / cell_height).floor().max(1.0) as u16;
        self.ensure_started(cols, rows);
        // The reader thread has nothing of its own to ask for a frame with, and a terminal whose screen
        // moves only when the mouse passes over it is what that looks like. Handed in on every frame the
        // terminal is drawn, so the ask always reaches the window that is drawing.
        {
            let ctx = ui.ctx().clone();
            let change: ide_term::Change = std::sync::Arc::new(move || ctx.request_repaint());
            for pane in &self.panes {
                pane.session.watch(std::sync::Arc::clone(&change));
            }
        }
        if (cols, rows) != (self.cols, self.rows) {
            for pane in &mut self.panes {
                let _ = pane.session.resize(cols, rows);
            }
            self.cols = cols;
            self.rows = rows;
        }

        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        self.shown_rect = Some(rect);
        // A right-click on the terminal asks what to do with what is on it: take the selection, or
        // paste into the shell. The selection is this pane's own, the clipboard belongs to the
        // window — the same division the copy keys already follow.
        response.context_menu(|ui| {
            if ui.button("Copy text").clicked() {
                self.copy_selection(ui);
                ui.close();
            }
            if ui.button("Paste").clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                ui.close();
            }
        });
        // A press belongs to the terminal only when the pointer is over it and nothing is drawn on
        // top of it. `button_pressed` is the state of the whole window, so a click on the context
        // menu — which stands over the canvas — used to arrive here as a click on the canvas, and
        // wiped the very selection that menu was about to copy. `hovered` is false as soon as a
        // popup covers the pointer, which is exactly the difference.
        // The selection needs the gate above; the keyboard does not. A press outside the terminal is
        // a person going back to the code and must take the keys away from here (`focus_after`), and a
        // press that is *inside* the rectangle but aimed at the menu standing over it does nothing to
        // the focus either way.
        let mine = response.hovered() || response.is_pointer_button_down_on();
        let pressed_at = ui.input(|input| {
            if input.pointer.any_pressed() {
                input.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(focused) = focus_after(rect, pressed_at, response.clicked()) {
            self.focused = focused;
        }
        // Choosing a session is a person saying where they will type: the press itself landed on the
        // strip, so `focus_after` read it as "not the terminal" and the keys would stay with nobody
        // until the canvas was clicked.
        if switched {
            self.focused = true;
        }
        let cell_size = egui::vec2(cell_width, cell_height);
        // Whose keys these are. `focused` is the flag the application reads to decide whether the
        // strokes of this frame go to the shell or to the editor (`take_keys`), and that is what a
        // person means by "the terminal I am typing into". The canvas' own egui focus is **not** that
        // flag: the keyboard is never given to the canvas, so `has_focus()` is false even while the
        // shell is taking every key — which is how the cursor stayed hollow in the active terminal.
        let has_keyboard = self.focused;
        self.track_selection(ui, rect, cell_size, cols, rows, mine);
        let hovered = self.link_under_pointer(ui, rect, cell_size, cols, rows);
        self.follow_clicked_link(ui, &hovered);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, PAPER);
        // A shell that has finished is worth saying out loud; otherwise the tab looks
        // frozen rather than done.
        if !self.panes.is_empty() && !self.is_alive() {
            painter.text(
                rect.left_bottom() + egui::vec2(6.0, -18.0),
                egui::Align2::LEFT_BOTTOM,
                "[process exited]",
                egui::FontId::monospace(12.0),
                egui::Color32::from_rgb(150, 120, 60),
            );
        }
        if let Some(error) = &self.error {
            painter.text(
                rect.left_top(),
                egui::Align2::LEFT_TOP,
                error,
                egui::FontId::monospace(13.0),
                egui::Color32::LIGHT_RED,
            );
            return;
        }
        let Some(pane) = self.pane() else {
            return;
        };
        let session = &pane.session;
        let selection = pane.selection;
        session.with_screen_scrolled(pane.view, |screen| {
            for row in 0..rows {
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    let position = rect.left_top()
                        + egui::vec2(col as f32 * cell_width, row as f32 * cell_height);
                    let background = cell.bgcolor();
                    let picked = selection.is_some_and(|selection| selection.contains(col, row));
                    if picked {
                        // The selection wins over the program's own background: a
                        // highlighted cell is what the user is looking for.
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                position,
                                egui::vec2(cell_width, cell_height),
                            ),
                            0.0,
                            SELECTION,
                        );
                    } else if background != vt100::Color::Default {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                position,
                                egui::vec2(cell_width, cell_height),
                            ),
                            0.0,
                            colour(background),
                        );
                    }
                    let contents = cell.contents();
                    if contents.trim().is_empty() {
                        continue;
                    }
                    painter.text(
                        position,
                        egui::Align2::LEFT_TOP,
                        contents,
                        font.clone(),
                        colour(cell.fgcolor()),
                    );
                }
            }
            // A link under the pointer is underlined, and only then: a screen full
            // of underlined prose would be noise.
            if let Some((start, end, row, _)) = &hovered {
                let underline = egui::Rect::from_min_size(
                    rect.left_top()
                        + egui::vec2(
                            *start as f32 * cell_width,
                            (*row as f32 + 1.0) * cell_height - 2.0,
                        ),
                    egui::vec2((*end as f32 - *start as f32) * cell_width, 2.0),
                );
                painter.rect_filled(underline, 0.0, egui::Color32::from_rgb(120, 180, 255));
            }
            // The cursor is drawn last so that a block under a glyph is visible. Filled and blinking
            // it says the keys go here; hollow and still it says the terminal is there with the
            // keyboard somewhere else — the one thing a person has to be able to tell at a glance.
            // The cursor stands on the living line at the bottom of the session. With the view
            // raised it is not on screen at all, and a block drawn over history would say the keys
            // go somewhere they do not. It comes back when the view returns to the bottom — which
            // any typing does, by jumping there. Andy: «если в нижней строке при прокрутке не
            // показана именно рабочая строка ввода, нужно ещё и скрывать текстовый курсор».
            if cursor_is_drawn(screen.hide_cursor(), pane.view) {
                let (row, col) = screen.cursor_position();
                let position =
                    rect.left_top() + egui::vec2(col as f32 * cell_width, row as f32 * cell_height);
                let cursor_cell =
                    egui::Rect::from_min_size(position, egui::vec2(cell_width, cell_height));
                let now = ui.input(|input| input.time);
                // A key went to the shell a moment ago: the block is lit, and it is not blinking.
                let working = self
                    .last_key_at
                    .is_some_and(|at| now - at < STEADY_AFTER_A_KEY);
                match cursor_of(has_keyboard, now, working) {
                    Cursor::Block => {
                        // The cell turned over: the block takes the colour the letter had, and the
                        // letter is put back in the colour of the paper. That is what makes the
                        // character readable under the cursor instead of hidden for the half second
                        // the block is lit.
                        let under = screen.cell(row, col);
                        let (ink, paper) = cursor_ink_and_paper(
                            under.map(|cell| cell.fgcolor()),
                            under.map(|cell| cell.bgcolor()),
                        );
                        painter.rect_filled(cursor_cell, 0.0, ink);
                        if let Some(cell) = under {
                            let contents = cell.contents();
                            if !contents.trim().is_empty() {
                                painter.text(
                                    cursor_cell.left_top(),
                                    egui::Align2::LEFT_TOP,
                                    contents,
                                    font.clone(),
                                    paper,
                                );
                            }
                        }
                    }
                    Cursor::Hollow => {
                        painter.rect_stroke(
                            cursor_cell,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::LIGHT_GRAY),
                            egui::StrokeKind::Inside,
                        );
                    }
                    // The dark half of the blink: nothing at all is drawn.
                    Cursor::Nothing => {}
                }
                if has_keyboard {
                    // The blink needs frames, and this window draws on demand: without the ask the
                    // block would stand still and lit.
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(120));
                }
            }
        });
        self.forward_mouse(ui, rect, egui::vec2(cell_width, cell_height), cols, rows);
    }

    /// The cell under a point of the panel, clamped to the screen.
    fn cell_at(
        rect: egui::Rect,
        cell: egui::Vec2,
        pos: egui::Pos2,
        cols: u16,
        rows: u16,
    ) -> (u16, u16) {
        let column = (((pos.x - rect.left()) / cell.x).floor().max(0.0) as u16).min(cols - 1);
        let row = (((pos.y - rect.top()) / cell.y).floor().max(0.0) as u16).min(rows - 1);
        (column, row)
    }

    /// Follows the pointer and keeps the selection under it.
    ///
    /// `Shift` is the terminal's own mouse button: with it held the pointer picks
    /// text even from a program that wants the mouse, which is the only way to get a
    /// path out of `mc` or `vim`. Where no program asked for the mouse — a shell at
    /// its prompt — a plain drag selects too, and nothing is lost by it.
    fn track_selection(
        &mut self,
        ui: &Ui,
        rect: egui::Rect,
        cell: egui::Vec2,
        cols: u16,
        rows: u16,
        over_the_canvas: bool,
    ) {
        let (pos, shift, ctrl, pressed, down, released) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.modifiers.shift,
                input.modifiers.ctrl,
                input.pointer.button_pressed(egui::PointerButton::Primary),
                input.pointer.button_down(egui::PointerButton::Primary),
                input.pointer.button_released(egui::PointerButton::Primary),
            )
        });
        // A press somewhere else — a popup over the canvas, the panel behind it — is not a press on
        // the terminal, and must not touch the selection.
        let pressed = pressed && over_the_canvas;
        let wants_mouse = self
            .pane()
            .is_some_and(|pane| pane.session.mouse_protocol().enabled);
        // `Ctrl` is the link key: a click with it opens what is under the pointer, and
        // must not start a selection as well.
        if ctrl {
            if let Some(pane) = self.pane_mut() {
                pane.dragging = false;
            }
            return;
        }
        let selecting = shift || !wants_mouse;
        let Some(pane) = self.pane_mut() else {
            return;
        };

        if released {
            pane.dragging = false;
            pane.anchor = None;
        }
        let Some(pos) = pos.filter(|pos| rect.contains(*pos)) else {
            return;
        };
        let target = Self::cell_at(rect, cell, pos, cols, rows);

        if pressed {
            if selecting {
                // Where a drag would begin, and nothing picked out yet: a click that never moves
                // leaves no highlight behind — a single cell lit up by a click reads as a selection
                // nobody asked for.
                pane.selection = None;
                pane.anchor = Some(target);
                pane.dragging = true;
            } else {
                // A plain click belongs to the program; the old selection goes.
                pane.selection = None;
                pane.anchor = None;
                pane.dragging = false;
            }
            return;
        }
        if pane.dragging && down {
            let anchor = pane.anchor.unwrap_or(target);
            if anchor == target {
                // Still on the cell the press landed on: nothing is picked out yet.
                pane.selection = None;
            } else {
                pane.selection = Some(Selection {
                    anchor,
                    head: target,
                });
            }
        }
    }

    /// When a key was last handed to the shell, in the window's own seconds.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn last_key_at(&self) -> Option<f64> {
        self.last_key_at
    }

    /// The text the user has picked out, if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selection(&self) -> Option<Selection> {
        self.pane().and_then(|pane| pane.selection)
    }

    /// The link that was just `Ctrl`-clicked, if one was.
    ///
    /// The panel knows nothing of the project or the browser, so it hands the link
    /// over and the application decides what to do with it.
    pub fn take_link(&mut self) -> Option<ide_term::link::Link> {
        self.link_clicked.take()
    }

    /// Records the link a `Ctrl`-click landed on, and shows the hand over one.
    ///
    /// Whether it is opened, and how, is the application's business: a URL belongs to
    /// the browser, a `file:line` to the editor beside it.
    fn follow_clicked_link(
        &mut self,
        ui: &Ui,
        hovered: &Option<(u16, u16, u16, ide_term::link::Link)>,
    ) {
        let Some((_, _, _, link)) = hovered else {
            return;
        };
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let clicked = ui.input(|input| {
            input.modifiers.ctrl && input.pointer.button_pressed(egui::PointerButton::Primary)
        });
        if clicked {
            self.link_clicked = Some(link.clone());
        }
    }

    /// The link under the pointer, and the columns it covers on its row.
    ///
    /// Links are read from the characters on the screen, so they are found in a
    /// shell's output as well as in a build tool's.
    fn link_under_pointer(
        &self,
        ui: &Ui,
        rect: egui::Rect,
        cell: egui::Vec2,
        cols: u16,
        rows: u16,
    ) -> Option<(u16, u16, u16, ide_term::link::Link)> {
        if !ui.input(|input| input.modifiers.ctrl) {
            return None;
        }
        let pos = ui.input(|input| input.pointer.hover_pos())?;
        if !rect.contains(pos) {
            return None;
        }
        let pane = self.pane()?;
        let (column, row) = Self::cell_at(rect, cell, pos, cols, rows);
        let found = pane.session.with_screen_scrolled(pane.view, |screen| {
            let text: String = (0..cols)
                .filter_map(|col| screen.cell(row, col))
                .map(|cell| cell.contents())
                .collect();
            ide_term::link::find(&text, column as usize)
        });
        let (range, link) = found?;
        Some((range.start as u16, range.end as u16, row, link))
    }

    /// The selected text, one line per row, trailing blanks cut.
    pub fn selected_text(&self) -> Option<String> {
        let pane = self.pane()?;
        let selection = pane.selection?;
        let session = &pane.session;
        let (start, end) = selection.ordered();
        let text = session.with_screen_scrolled(pane.view, |screen| {
            let (rows, cols) = screen.size();
            let (rows, cols) = (rows.get(), cols.get());
            let mut lines: Vec<String> = Vec::new();
            let last_row = end.1.min(rows.saturating_sub(1));
            for row in start.1..=last_row {
                let from = if row == start.1 { start.0 } else { 0 };
                let to = if row == end.1 {
                    end.0.min(cols.saturating_sub(1))
                } else {
                    cols.saturating_sub(1)
                };
                let mut line = String::new();
                for col in from..=to {
                    if let Some(cell) = screen.cell(row, col) {
                        line.push_str(cell.contents());
                    }
                }
                lines.push(line.trim_end().to_owned());
            }
            lines.join("\n")
        });
        (!text.is_empty()).then_some(text)
    }

    /// Puts the selection on the clipboard, where `Ctrl+Shift+C` asks for it.
    fn copy_selection(&self, ui: &Ui) {
        if let Some(text) = self.selected_text() {
            ui.ctx().copy_text(text);
        }
    }

    /// Sends the pointer to the program, if the program asked for it.
    ///
    /// A terminal has no mouse of its own: the program says `CSI ? 1000 h` and the
    /// terminal answers with reports, which is how `mc` picks the file under the
    /// pointer. Nothing is sent when the program asked for nothing, or a click would
    /// leave junk at a shell prompt.
    fn forward_mouse(&mut self, ui: &Ui, rect: egui::Rect, cell: egui::Vec2, cols: u16, rows: u16) {
        let Some(protocol) = self.pane().map(|pane| pane.session.mouse_protocol()) else {
            return;
        };
        // A program that does not track the mouse has no use for the wheel, and a shell prompt has
        // none at all: here the wheel is the person's and it walks the scrollback. A program that
        // does track it — `vim`, `less`, `mc` — scrolls itself, and moving the view under it would
        // show rows the program never wrote.
        if !protocol.enabled {
            self.wheel_the_view(ui, rect);
            if let Some(pane) = self.pane_mut() {
                pane.mouse_cell = None;
            }
            return;
        }
        let Some(pane) = self.pane_mut() else {
            return;
        };
        if cell.x <= 0.0 || cell.y <= 0.0 || cols == 0 || rows == 0 {
            pane.mouse_cell = None;
            return;
        }

        let (pos, modifiers, scroll) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.modifiers,
                input.smooth_scroll_delta.y,
            )
        });
        let mods = Mods {
            shift: modifiers.shift,
            alt: modifiers.alt,
            ctrl: modifiers.ctrl,
        };
        // With `Shift` held the mouse is the terminal's, not the program's: it picks
        // text. Programs know this as the shift override, and it is the only way to
        // copy a line out of `mc` or `vim`, which would otherwise eat every click.
        // `Ctrl` is the link key, and a program must not see that click either.
        if modifiers.shift || modifiers.ctrl {
            pane.mouse_cell = None;
            return;
        }
        let Some(pos) = pos.filter(|pos| rect.contains(*pos)) else {
            // Out there the pointer is not our business, and a report about a cell
            // it is not over would be a lie.
            pane.mouse_cell = None;
            return;
        };
        let (column, row) = Self::cell_at(rect, cell, pos, cols, rows);

        let mut events: Vec<mouse::Event> = Vec::new();
        let mut held: Option<mouse::Button> = None;
        for (button, pointer) in [
            (mouse::Button::Left, egui::PointerButton::Primary),
            (mouse::Button::Middle, egui::PointerButton::Middle),
            (mouse::Button::Right, egui::PointerButton::Secondary),
        ] {
            let (pressed, released, down) = ui.input(|input| {
                (
                    input.pointer.button_pressed(pointer),
                    input.pointer.button_released(pointer),
                    input.pointer.button_down(pointer),
                )
            });
            if down {
                held = Some(button);
            }
            if pressed {
                events.push(mouse::Event::new(
                    mouse::Action::Press(button),
                    column,
                    row,
                    mods,
                ));
            }
            if released && protocol.press_and_release {
                events.push(mouse::Event::new(
                    mouse::Action::Release(button),
                    column,
                    row,
                    mods,
                ));
            }
        }

        // Movement is reported where the program asked for it: on every cell change,
        // or while a button is held. Once per cell, not once per frame.
        let moved = pane.mouse_cell != Some((column, row));
        if moved {
            match held {
                Some(button) if protocol.drag => events.push(mouse::Event::new(
                    mouse::Action::Drag(button),
                    column,
                    row,
                    mods,
                )),
                Some(_) => {}
                None if protocol.motion => {
                    events.push(mouse::Event::new(mouse::Action::Motion, column, row, mods));
                }
                None => {}
            }
        }
        pane.mouse_cell = Some((column, row));

        if scroll != 0.0 {
            // Scrolling comes in pixels; the program wants notches, and the remainder
            // is kept so a slow wheel is not lost.
            const NOTCH: f32 = 40.0;
            pane.scroll_left += scroll;
            while pane.scroll_left.abs() >= NOTCH {
                let wheel = if pane.scroll_left > 0.0 {
                    mouse::Wheel::Up
                } else {
                    mouse::Wheel::Down
                };
                pane.scroll_left -= NOTCH * pane.scroll_left.signum();
                events.push(mouse::Event::new(
                    mouse::Action::Wheel(wheel),
                    column,
                    row,
                    mods,
                ));
            }
        }

        if events.is_empty() {
            return;
        }
        let mut bytes = Vec::new();
        for event in events {
            bytes.extend(mouse::bytes(event, protocol.encoding));
        }
        let _ = pane.session.write(&bytes);
    }

    /// Walks the scrollback with the wheel: three lines a notch, the remainder kept so a slow wheel
    /// is not lost. The session's own tab says `(scrolled)` while the view is up — the only thing on
    /// screen that tells a person the rows they are reading are not the living ones.
    fn wheel_the_view(&mut self, ui: &Ui, rect: egui::Rect) {
        const NOTCH: f32 = 40.0;
        const LINES: usize = 3;
        // The raw events, not `smooth_scroll_delta`: a terminal answers the wheel at once, and the
        // smoothed figure is spread over the frames that follow it.
        //
        // The unit is what a mouse and a touchpad disagree about, and reading it as points is what
        // made the wheel do nothing on a real mouse: a wheel reports **lines** (`delta.y` is ±1 for
        // one notch) while a touchpad reports **points** (tens of them per notch). One notch of the
        // wheel is `NOTCH` here, so a line counts as a notch and a point as a point.
        let (pointer, scroll) = ui.input(|input| {
            let scroll = input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Line => delta.y * NOTCH,
                        egui::MouseWheelUnit::Point => delta.y,
                        // A page is the height of what is scrolled, which is the pane's own screen.
                        egui::MouseWheelUnit::Page => delta.y * self.rows as f32 * NOTCH,
                    }),
                    _ => None,
                })
                .sum::<f32>();
            (input.pointer.interact_pos(), scroll)
        });
        let Some(pointer) = pointer else {
            return;
        };
        if !rect.contains(pointer) || scroll == 0.0 {
            return;
        }
        let kept = self.scrollback;
        let Some(pane) = self.pane_mut() else {
            return;
        };
        pane.scroll_left += scroll;
        let notches = (pane.scroll_left.abs() / NOTCH).floor();
        if notches < 1.0 {
            return;
        }
        let step = notches as usize * LINES;
        pane.view = if pane.scroll_left > 0.0 {
            (pane.view + step).min(kept)
        } else {
            pane.view.saturating_sub(step)
        };
        pane.scroll_left -= NOTCH * notches * pane.scroll_left.signum();
    }

    /// The wheel's place in the scrollback, for the checks.
    #[cfg(test)]
    pub fn view(&self) -> usize {
        self.pane().map(|pane| pane.view).unwrap_or(0)
    }

    /// Sends bytes to the shell in front, as typing does.
    pub fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        match self.pane_mut() {
            Some(pane) => pane.session.write(bytes),
            None => Ok(()),
        }
    }

    /// Ends every session; the next frame starts a fresh one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn restart(&mut self) {
        for mut pane in self.panes.drain(..) {
            pane.session.kill();
        }
        self.active = 0;
        self.error = None;
        self.focused = false;
        self.last_key_at = None;
    }

    /// Whether the program in the terminal asked for the application cursor keys.
    pub fn application_cursor(&self) -> bool {
        self.pane()
            .is_some_and(|pane| pane.session.application_cursor())
    }

    /// The grid the panel is drawing: columns and rows of cells.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Whether the program in the terminal asked for mouse reports.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn mouse_enabled(&self) -> bool {
        self.pane()
            .is_some_and(|pane| pane.session.mouse_protocol().enabled)
    }

    /// What the screen holds, for tests and for the clipboard.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn contents(&self) -> String {
        self.pane()
            .map(|pane| pane.session.contents())
            .unwrap_or_default()
    }

    /// The sessions, for the checks that ask whether the screen has someone to tell.
    #[cfg(test)]
    pub(crate) fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.panes.iter().map(|pane| &pane.session)
    }

    pub fn is_alive(&self) -> bool {
        self.pane().is_some_and(|pane| pane.session.is_alive())
    }
}

/// A terminal colour, with the palette the terminal world expects.
fn colour(value: vt100::Color) -> egui::Color32 {
    match value {
        vt100::Color::Default => egui::Color32::from_rgb(212, 214, 216),
        vt100::Color::Idx(index) => match index {
            0 => egui::Color32::from_rgb(24, 26, 30),
            1 => egui::Color32::from_rgb(224, 108, 117),
            2 => egui::Color32::from_rgb(152, 195, 121),
            3 => egui::Color32::from_rgb(229, 192, 123),
            4 => egui::Color32::from_rgb(97, 175, 239),
            5 => egui::Color32::from_rgb(198, 120, 221),
            6 => egui::Color32::from_rgb(86, 182, 194),
            7 => egui::Color32::from_rgb(200, 204, 208),
            8 => egui::Color32::from_rgb(90, 94, 100),
            9..=15 => {
                let base = colour(vt100::Color::Idx(index - 8));
                egui::Color32::from_rgb(
                    base.r().saturating_add(40),
                    base.g().saturating_add(40),
                    base.b().saturating_add(40),
                )
            }
            _ => {
                // The 256-colour cube: six steps per channel, then a grey ramp.
                if index >= 232 {
                    let level = 8 + (index - 232) * 10;
                    egui::Color32::from_rgb(level, level, level)
                } else {
                    let index = index - 16;
                    let steps = [0u8, 95, 135, 175, 215, 255];
                    egui::Color32::from_rgb(
                        steps[(index / 36) as usize],
                        steps[((index % 36) / 6) as usize],
                        steps[(index % 6) as usize],
                    )
                }
            }
        },
        vt100::Color::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
    }
}

/// Whether this key walks the lines kept above the screen rather than being typed into the program.
///
/// `Shift+PageUp` and `Shift+PageDown` are the terminal's own keys, the habit from every terminal emulator:
/// sent on to the shell they would ask a full-screen program to move a page, and a shell at a prompt would
/// ignore them. Andy: "в терминале не работает прокрутка по Shift-PageUp и Shift-PageDown. Добавь её".
pub fn walks_the_scrollback(key: Key, modifiers: Modifiers) -> bool {
    modifiers.shift && matches!(key, Key::PageUp | Key::PageDown)
}

/// Where the view stands after a page up or down.
///
/// A page is one screen less a line, so the line that was at the bottom stays in sight and the eye can follow
/// what it was reading. Zero is the live screen; the view never goes below it, and never above what is kept.
pub fn scrolled(view: usize, up: bool, rows: u16, kept: usize) -> usize {
    let page = (rows.saturating_sub(1)).max(1) as usize;
    if up {
        (view + page).min(kept)
    } else {
        view.saturating_sub(page)
    }
}

#[cfg(test)]
mod scrollback_tests {
    use super::*;

    #[test]
    fn a_raised_view_says_so_in_its_tab_and_loses_its_cursor() {
        assert_eq!(
            tab_label("Terminal 1", 0),
            "Terminal 1",
            "at the bottom, the name alone"
        );
        assert_eq!(
            tab_label("Terminal 1", 42),
            "Terminal 1 (scrolled)",
            "raised: the sign says the screen is not the end of the session"
        );

        assert!(
            cursor_is_drawn(false, 0),
            "at the bottom the cursor is drawn"
        );
        assert!(
            !cursor_is_drawn(false, 1),
            "one line up, and the living line is off screen: no cursor over history"
        );
        assert!(
            !cursor_is_drawn(true, 0),
            "a program that hides it is obeyed as before"
        );
    }

    #[test]
    fn a_page_key_with_shift_is_the_terminals_own_and_without_it_is_typed() {
        assert!(walks_the_scrollback(Key::PageUp, Modifiers::SHIFT));
        assert!(walks_the_scrollback(Key::PageDown, Modifiers::SHIFT));
        assert!(
            !walks_the_scrollback(Key::PageUp, Modifiers::NONE),
            "a plain PageUp belongs to the program in the terminal"
        );
        assert!(!walks_the_scrollback(Key::ArrowUp, Modifiers::SHIFT));
    }

    #[test]
    fn the_view_walks_a_page_at_a_time_and_stops_at_both_ends() {
        // A page keeps one line of the old view, so the eye does not lose what it was reading.
        assert_eq!(scrolled(0, true, 24, 2000), 23);
        assert_eq!(scrolled(23, true, 24, 2000), 46);
        assert_eq!(scrolled(46, false, 24, 2000), 23);
        assert_eq!(scrolled(23, false, 24, 2000), 0);
        assert_eq!(
            scrolled(0, false, 24, 2000),
            0,
            "the view went below the live screen"
        );
        assert_eq!(
            scrolled(1999, true, 24, 2000),
            2000,
            "the view went above the lines that are kept"
        );
        // A pane one row tall would otherwise have a page of zero and never move.
        assert_eq!(scrolled(0, true, 1, 2000), 1);
    }
}

/// The terminal input a key press stands for.
///
/// `app` is the terminal's application-cursor mode, read from the screen once per
/// frame: a full-screen program (`mc`, `vim`) turns it on and then matches `ESC O B`
/// for the arrows, where a shell at its prompt matches `ESC [ B`.
pub fn input_for_key(key: Key, modifiers: Modifiers, app: bool) -> Option<Input> {
    let mods = Mods {
        shift: modifiers.shift,
        alt: modifiers.alt,
        ctrl: modifiers.ctrl,
    };
    let input = match key {
        Key::Enter => Input::Enter,
        Key::Backspace => Input::Backspace,
        Key::Tab if modifiers.shift => Input::BackTab,
        Key::Tab => Input::Tab,
        Key::Escape => Input::Escape,
        Key::ArrowUp => Input::Arrow {
            direction: Direction::Up,
            app,
            mods,
        },
        Key::ArrowDown => Input::Arrow {
            direction: Direction::Down,
            app,
            mods,
        },
        Key::ArrowLeft => Input::Arrow {
            direction: Direction::Left,
            app,
            mods,
        },
        Key::ArrowRight => Input::Arrow {
            direction: Direction::Right,
            app,
            mods,
        },
        Key::Home => Input::Home { app, mods },
        Key::End => Input::End { app, mods },
        Key::PageUp => Input::PageUp,
        Key::PageDown => Input::PageDown,
        Key::Delete => Input::Delete,
        Key::Insert => Input::Insert,
        // Above F12 a key is not a function key any more but a macro the window
        // manager has already claimed; the terminal has nothing to say about it.
        Key::F1 => function(1, mods),
        Key::F2 => function(2, mods),
        Key::F3 => function(3, mods),
        Key::F4 => function(4, mods),
        Key::F5 => function(5, mods),
        Key::F6 => function(6, mods),
        Key::F7 => function(7, mods),
        Key::F8 => function(8, mods),
        Key::F9 => function(9, mods),
        Key::F10 => function(10, mods),
        Key::F11 => function(11, mods),
        Key::F12 => function(12, mods),
        _ => return None,
    };
    Some(input)
}

/// One numbered function key with its modifiers.
fn function(number: u8, mods: Mods) -> Input {
    Input::Function { number, mods }
}

/// The control byte a `Ctrl` key press stands for, if it has one.
pub fn control_letter(key: Key, modifiers: Modifiers) -> Option<u8> {
    if !modifiers.ctrl || modifiers.alt {
        return None;
    }
    let letter = match key {
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        _ => return None,
    };
    Some(letter as u8 - b'a' + 1)
}

/// What a session's tab says: its own name, and — when its view is raised — that what is on screen
/// is not the living end of the session.
///
/// A word rather than an arrow: the arrow a person would expect here is not in the fonts the window
/// carries, and a square where a sign should be is worse than no sign at all.
fn tab_label(title: &str, view: usize) -> String {
    if view == 0 {
        title.to_owned()
    } else {
        format!("{title} (scrolled)")
    }
}

/// What a chord the backend has already decided about means to a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    /// The platform's copy: the selection goes to the clipboard and nothing is typed.
    Copy,
    /// The terminal's own meaning, which is a byte of its own: 3 for `Ctrl`+`C` (the interrupt the
    /// terminal driver turns into `SIGINT`) and 24 for `Ctrl`+`X` (the cancel the driver acts on).
    ToTheShell,
}

/// Which of the two a `Copy` or `Cut` event is.
///
/// Those events carry no modifiers, and which chord sent them is the whole difference between
/// copying and stopping a program. The backend calls `Ctrl`+`C` a copy because it asks
/// `modifiers.command`, which on Linux and Windows is `Ctrl` — so a plain `Ctrl`+`C` here means the
/// interrupt and the copy is the chord holding `Shift`. On macOS the platform key is `Cmd`, which is
/// `mac_cmd`, and a plain `Ctrl`+`C` there arrives as a key of its own (read by `control_letter`).
pub fn chord_of(modifiers: Modifiers) -> Chord {
    if modifiers.shift || modifiers.mac_cmd {
        Chord::Copy
    } else {
        Chord::ToTheShell
    }
}

/// Whether the terminal's cursor belongs on screen.
///
/// The program may hide its own cursor; and with the view raised the cursor — which stands on the
/// living line at the bottom — is not among the rows on screen at all.
fn cursor_is_drawn(screen_hides_it: bool, view: usize) -> bool {
    !screen_hides_it && view == 0
}

#[cfg(test)]
mod chords {
    use super::{Chord, chord_of};
    use egui::Modifiers;

    #[test]
    fn the_plain_copy_chord_is_a_byte_of_its_own() {
        // `Ctrl`+`C` reaches the panel as a copy event, never as a key — the backend calls it a copy
        // because `modifiers.command` is `Ctrl` on Linux and Windows. A terminal that copied would
        // leave a running program unstoppable, so the plain chord has to mean byte 3.
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            chord_of(ctrl),
            Chord::ToTheShell,
            "Ctrl+C was read as a copy, so a stuck program cannot be stopped"
        );

        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            chord_of(ctrl_shift),
            Chord::Copy,
            "Ctrl+Shift+C is the copy chord of this window and was read as an interrupt"
        );

        // macOS: the platform's copy key is `Cmd`, and `Ctrl`+`C` there comes as a key of its own.
        let cmd = Modifiers {
            mac_cmd: true,
            command: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            chord_of(cmd),
            Chord::Copy,
            "Cmd+C on macOS was read as an interrupt"
        );
        assert_eq!(
            chord_of(Modifiers::NONE),
            Chord::ToTheShell,
            "the chord that comes with no modifier at all is not the interrupt"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_cell_is_the_cell_turned_over() {
        // The usual case: the program left both colours alone. The block is the ink of the text, and
        // the letter goes back onto the paper of the terminal — and the two must differ, or the
        // character under the cursor would be invisible.
        let (ink, paper) =
            cursor_ink_and_paper(Some(vt100::Color::Default), Some(vt100::Color::Default));
        assert_eq!(ink, colour(vt100::Color::Default));
        assert_eq!(paper, PAPER);
        assert_ne!(ink, paper, "the letter under the block must be readable");

        // A cell the program coloured itself is turned over in its own colours.
        let (ink, paper) =
            cursor_ink_and_paper(Some(vt100::Color::Idx(1)), Some(vt100::Color::Idx(4)));
        assert_eq!(ink, colour(vt100::Color::Idx(1)));
        assert_eq!(paper, colour(vt100::Color::Idx(4)));
    }

    #[test]
    fn the_cursor_blinks_where_the_keys_go_and_stands_still_where_they_do_not() {
        // The terminal that is taking the strokes: a block, and it blinks while the keys are quiet.
        assert_eq!(cursor_of(true, 0.1, false), Cursor::Block);
        assert_eq!(cursor_of(true, 0.7, false), Cursor::Nothing);
        // While a person is typing it is lit for the whole cycle — never caught mid-dark.
        assert_eq!(cursor_of(true, 0.1, true), Cursor::Block);
        assert_eq!(cursor_of(true, 0.7, true), Cursor::Block);
        // The terminal that is merely on screen: the hollow frame, and it does not blink — the frame
        // is what says "not here".
        assert_eq!(cursor_of(false, 0.1, false), Cursor::Hollow);
        assert_eq!(cursor_of(false, 0.7, true), Cursor::Hollow);
    }

    #[test]
    fn the_arrows_are_escape_sequences() {
        let up = input_for_key(Key::ArrowUp, Modifiers::NONE, false).unwrap();
        assert_eq!(up.bytes(), b"\x1b[A".to_vec());
    }

    #[test]
    fn a_program_that_asked_for_the_application_mode_gets_the_other_form() {
        // `mc` and `vim` say `smkx`; after that the arrows are `ESC O X`, and an
        // `ESC [ X` is silently ignored by them - the bug this test holds.
        let down = input_for_key(Key::ArrowDown, Modifiers::NONE, true).unwrap();
        assert_eq!(down.bytes(), b"\x1bOB".to_vec());
        let home = input_for_key(Key::Home, Modifiers::NONE, true).unwrap();
        assert_eq!(home.bytes(), b"\x1bOH".to_vec());
    }

    #[test]
    fn control_no_longer_means_the_application_mode() {
        // It never did: the mode belongs to the program, not to a modifier.
        let up = input_for_key(Key::ArrowUp, Modifiers::CTRL, false).unwrap();
        assert_eq!(up.bytes(), b"\x1b[1;5A".to_vec());
    }

    #[test]
    fn the_function_keys_reach_the_shell() {
        let f3 = input_for_key(Key::F3, Modifiers::NONE, true).unwrap();
        assert_eq!(f3.bytes(), b"\x1bOR".to_vec());
        let f10 = input_for_key(Key::F10, Modifiers::NONE, true).unwrap();
        assert_eq!(f10.bytes(), b"\x1b[21~".to_vec());
        let shift_f1 = input_for_key(Key::F1, Modifiers::SHIFT, true).unwrap();
        assert_eq!(shift_f1.bytes(), b"\x1b[1;2P".to_vec());
    }

    #[test]
    fn shift_tab_goes_backwards() {
        let back = input_for_key(Key::Tab, Modifiers::SHIFT, false).unwrap();
        assert_eq!(back.bytes(), b"\x1b[Z".to_vec());
        let forward = input_for_key(Key::Tab, Modifiers::NONE, false).unwrap();
        assert_eq!(forward, Input::Tab);
    }

    #[test]
    fn a_letter_with_control_is_a_control_byte() {
        assert_eq!(control_letter(Key::C, Modifiers::CTRL), Some(3));
        assert_eq!(control_letter(Key::D, Modifiers::CTRL), Some(4));
        assert_eq!(control_letter(Key::C, Modifiers::NONE), None);
        assert_eq!(
            control_letter(Key::C, Modifiers::CTRL | Modifiers::ALT),
            None
        );
        assert_eq!(control_letter(Key::Enter, Modifiers::CTRL), None);
    }

    #[test]
    fn a_plain_letter_is_not_a_key_press_the_terminal_maps() {
        assert!(input_for_key(Key::A, Modifiers::NONE, false).is_none());
    }
}

#[cfg(test)]
mod stealing {
    use super::Terminal;

    fn text_of(event: &egui::Event) -> Option<String> {
        match event {
            egui::Event::Text(text) => Some(text.clone()),
            _ => None,
        }
    }

    #[test]
    fn typing_and_keys_are_taken_from_the_queue() {
        let mut events = vec![
            egui::Event::Text("x".to_owned()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Paste("pasted".to_owned()),
            // A key release and anything that is not typing stay put.
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(egui::pos2(3.0, 4.0)),
        ];

        let taken = Terminal::split_terminal_events(&mut events);

        assert_eq!(
            taken.len(),
            3,
            "the terminal did not take its events: {taken:?}"
        );
        assert!(
            taken.iter().filter_map(text_of).any(|text| text == "x"),
            "the typed letter was not taken"
        );
        assert!(
            events.iter().filter_map(text_of).next().is_none(),
            "a typed letter was left for the editor"
        );
        assert_eq!(
            events.len(),
            2,
            "the editor lost its pointer events: {events:?}"
        );
    }

    #[test]
    fn nothing_typed_means_nothing_taken() {
        let mut events = vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))];
        let taken = Terminal::split_terminal_events(&mut events);
        assert!(taken.is_empty());
        assert_eq!(events.len(), 1);
    }
}

/// Who gets the keyboard after a press, if anyone changed hands this frame.
///
/// `None` means "leave it as it is": a press inside the terminal takes the keyboard,
/// a press anywhere else gives it back to whoever was clicked. Without the second half
/// the shell kept every keystroke after the pointer had left it, which is how typed
/// text ended up in the file being edited.
/// What the cursor looks like in one frame.
///
/// The one thing a person has to read off the cursor at a glance is where the keys go, so the shape
/// says it: a filled block where the shell is taking the strokes, a hollow rectangle where the
/// terminal is merely on screen. The block blinks, half a second on and half off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cursor {
    /// The terminal has the keys, and this is the lit half of the blink.
    Block,
    /// The terminal has the keys, and this is the dark half.
    Nothing,
    /// The keys are elsewhere: the terminal is here, and so is a still hollow frame.
    Hollow,
}

/// The two colours a cursor cell is drawn with: the block itself, and the letter put back over it.
///
/// The block takes the colour the letter had and the letter takes the colour that was behind it, so
/// the pair is a true inversion whichever way the program coloured the cell. `None` means the screen
/// has no cell there at all, and then the terminal's own two colours are used.
fn cursor_ink_and_paper(
    foreground: Option<vt100::Color>,
    background: Option<vt100::Color>,
) -> (egui::Color32, egui::Color32) {
    let ink = match foreground {
        Some(colour_of_cell) => colour(colour_of_cell),
        None => colour(vt100::Color::Default),
    };
    let paper = match background {
        Some(vt100::Color::Default) | None => PAPER,
        Some(other) => colour(other),
    };
    (ink, paper)
}

/// How long the block stays lit after the last key, in seconds.
///
/// Longer than one blink cycle on purpose: while a person is typing the cursor must not be caught
/// mid-dark, and the pause between two words is not "gone quiet" yet.
const STEADY_AFTER_A_KEY: f64 = 1.0;

/// The cursor of a frame.
///
/// `focused` is the terminal's own "these keys are mine" flag, `time` the seconds the window has been
/// open, and `working` whether a key has gone to the shell a moment ago. While the keys are being used
/// the block is simply lit; it blinks only once they have gone quiet.
fn cursor_of(focused: bool, time: f64, working: bool) -> Cursor {
    if !focused {
        return Cursor::Hollow;
    }
    if working {
        return Cursor::Block;
    }
    if time % 1.0 < 0.5 {
        Cursor::Block
    } else {
        Cursor::Nothing
    }
}

pub fn focus_after(rect: egui::Rect, pressed_at: Option<egui::Pos2>, inside: bool) -> Option<bool> {
    match pressed_at {
        Some(at) if rect.contains(at) => Some(true),
        Some(_) => Some(false),
        None if inside => Some(true),
        None => None,
    }
}

#[cfg(test)]
mod focus {
    use super::focus_after;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 50.0))
    }

    #[test]
    fn a_press_inside_takes_the_keyboard() {
        assert_eq!(
            focus_after(rect(), Some(egui::pos2(20.0, 20.0)), false),
            Some(true)
        );
    }

    #[test]
    fn a_press_outside_gives_it_back() {
        assert_eq!(
            focus_after(rect(), Some(egui::pos2(500.0, 500.0)), false),
            Some(false)
        );
    }

    #[test]
    fn a_frame_without_a_press_leaves_the_focus_alone() {
        assert_eq!(focus_after(rect(), None, false), None);
        assert_eq!(focus_after(rect(), None, true), Some(true));
    }
}

#[cfg(test)]
mod selection {
    use super::Selection;

    fn selection(anchor: (u16, u16), head: (u16, u16)) -> Selection {
        Selection { anchor, head }
    }

    #[test]
    fn the_ends_are_read_in_reading_order_whichever_way_the_drag_went() {
        let forwards = selection((2, 1), (7, 3));
        let backwards = selection((7, 3), (2, 1));
        assert_eq!(forwards.ordered(), ((2, 1), (7, 3)));
        assert_eq!(backwards.ordered(), forwards.ordered());
    }

    #[test]
    fn a_cell_is_inside_only_where_the_drag_went_over_it() {
        let picked = selection((4, 1), (2, 3));
        assert!(!picked.contains(3, 1), "the cell before the anchor");
        assert!(picked.contains(4, 1), "the anchor itself");
        assert!(picked.contains(0, 2), "the first cell of a middle row");
        assert!(picked.contains(2, 3), "the head itself");
        assert!(!picked.contains(3, 3), "the cell past the head");
        assert!(!picked.contains(0, 4), "the row after the head");
    }
}

#[cfg(test)]
mod panes {
    use super::Terminal;
    use std::time::{Duration, Instant};

    fn wait_for(terminal: &Terminal, needle: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if terminal.contents().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    }

    #[test]
    fn a_second_session_is_a_second_shell() {
        let mut terminal = Terminal::new();
        terminal.ensure_started(80, 24);
        terminal.write(b"echo FIRST-MARK\n").unwrap();
        assert!(
            wait_for(&terminal, "FIRST-MARK"),
            "the first shell said nothing"
        );

        terminal.new_pane();
        assert_eq!(terminal.pane_count(), 2);
        assert_eq!(terminal.active_pane(), 1);
        assert!(
            !terminal.contents().contains("FIRST-MARK"),
            "the second session shows the first one's screen"
        );
        terminal.write(b"echo SECOND-MARK\n").unwrap();
        assert!(
            wait_for(&terminal, "SECOND-MARK"),
            "the second shell said nothing"
        );

        // Going back finds the first session as it was left.
        terminal.close_pane(1);
        assert_eq!(terminal.pane_count(), 1);
        assert!(
            wait_for(&terminal, "FIRST-MARK"),
            "the first session lost its screen"
        );
        terminal.restart();
    }

    #[test]
    fn closing_the_last_session_leaves_the_panel_empty() {
        let mut terminal = Terminal::new();
        terminal.ensure_started(80, 24);
        assert_eq!(terminal.pane_count(), 1);
        terminal.close_pane(0);
        assert_eq!(terminal.pane_count(), 0);
        assert_eq!(terminal.contents(), "");
        terminal.restart();
    }
}
