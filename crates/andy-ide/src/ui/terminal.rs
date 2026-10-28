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

/// A terminal tab: one session, or the reason it could not start.
pub struct Terminal {
    session: Option<Session>,
    pub error: Option<String>,
    cols: u16,
    rows: u16,
    /// Whether keys typed in this frame belong to the terminal.
    pub focused: bool,
    /// Whether the panel was drawn at all in the last frame.
    ///
    /// The panel is only drawn while its tab is visible, so this flag is the only way
    /// for the application to notice that the keyboard must be handed back: a hidden
    /// terminal that keeps focus would swallow every keystroke of the editor.
    pub shown: bool,
    /// Where the panel was last drawn, for the tests that click it.
    pub shown_rect: Option<egui::Rect>,
    /// The text the user has picked out with the mouse, if any.
    pub selection: Option<Selection>,
    /// Whether a drag is in progress, so a moving pointer keeps extending it.
    dragging: bool,
    /// The cell the pointer was over last frame, so a movement is reported once
    /// rather than sixty times a second.
    mouse_cell: Option<(u16, u16)>,
    /// Scrolling arrives in pixels and is reported in notches; the remainder is
    /// kept here so a slow wheel is not lost.
    scroll_left: f32,
}

/// The colour under selected cells.
const SELECTION: egui::Color32 = egui::Color32::from_rgb(38, 79, 120);

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
            session: None,
            error: None,
            cols: 80,
            rows: 24,
            focused: false,
            shown: false,
            shown_rect: None,
            selection: None,
            dragging: false,
            mouse_cell: None,
            scroll_left: 0.0,
        }
    }

    /// Starts the shell once, at the size the panel had room for.
    pub fn ensure_started(&mut self, cols: u16, rows: u16) {
        if self.session.is_some() || self.error.is_some() {
            return;
        }
        match Session::shell(cols, rows) {
            Ok(session) => {
                self.session = Some(session);
                self.cols = cols.max(1);
                self.rows = rows.max(1);
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    /// The bytes typed this frame, taken out of egui's events.
    ///
    /// The events are **removed**, not read: with the terminal focused the keys belong
    /// to the shell, and an editor that also saw them would type the same text into the
    /// open file — which is exactly what happened before this was a `take`.
    pub fn take_keys(&mut self, ui: &mut Ui) -> Vec<u8> {
        // Read once per frame, before anything is written: a program that has just
        // turned the mode on expects the very next arrow in the new form.
        let app = self.application_cursor();
        let events = ui.input_mut(|input| Self::split_terminal_events(&mut input.events));
        let mut bytes = Vec::new();
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
                        // `Ctrl+Shift+C` copies the selection: a terminal needs its
                        // own copy key, because `Ctrl+C` there means "stop".
                        if *key == egui::Key::C && modifiers.ctrl && modifiers.shift {
                            self.copy_selection(ui);
                            continue;
                        }
                        if let Some(byte) = control_letter(*key, *modifiers) {
                            bytes.push(byte);
                        } else if let Some(input) = input_for_key(*key, *modifiers, app) {
                            bytes.extend_from_slice(&input.bytes());
                        }
                    }
                    egui::Event::Paste(text) => bytes.extend_from_slice(text.as_bytes()),
                    _ => {}
                }
            }
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

    /// Draws the screen and lets the user type into it.
    pub fn show(&mut self, ui: &mut Ui) {
        self.shown = true;
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
        if (cols, rows) != (self.cols, self.rows) {
            if let Some(session) = self.session.as_mut() {
                let _ = session.resize(cols, rows);
            }
            self.cols = cols;
            self.rows = rows;
        }

        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        self.shown_rect = Some(rect);
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
        let cell_size = egui::vec2(cell_width, cell_height);
        self.track_selection(ui, rect, cell_size, cols, rows);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(16, 18, 20));
        // A shell that has finished is worth saying out loud; otherwise the tab looks
        // frozen rather than done.
        if self.session.is_some() && !self.is_alive() {
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
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let selection = self.selection;
        session.with_screen(|screen| {
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
            // The cursor is drawn last so that a block under a glyph is visible.
            if !screen.hide_cursor() {
                let (row, col) = screen.cursor_position();
                let position =
                    rect.left_top() + egui::vec2(col as f32 * cell_width, row as f32 * cell_height);
                painter.rect_stroke(
                    egui::Rect::from_min_size(position, egui::vec2(cell_width, cell_height)),
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::LIGHT_GRAY),
                    egui::StrokeKind::Inside,
                );
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
    ) {
        let (pos, shift, pressed, down, released) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.modifiers.shift,
                input.pointer.button_pressed(egui::PointerButton::Primary),
                input.pointer.button_down(egui::PointerButton::Primary),
                input.pointer.button_released(egui::PointerButton::Primary),
            )
        });
        let wants_mouse = self
            .session
            .as_ref()
            .is_some_and(|session| session.mouse_protocol().enabled);
        let selecting = shift || !wants_mouse;

        if released {
            self.dragging = false;
        }
        let Some(pos) = pos.filter(|pos| rect.contains(*pos)) else {
            return;
        };
        let target = Self::cell_at(rect, cell, pos, cols, rows);

        if pressed {
            if selecting {
                self.selection = Some(Selection {
                    anchor: target,
                    head: target,
                });
                self.dragging = true;
            } else {
                // A plain click belongs to the program; the old selection goes.
                self.selection = None;
                self.dragging = false;
            }
            return;
        }
        if self.dragging
            && down
            && let Some(selection) = self.selection.as_mut()
        {
            selection.head = target;
        }
    }

    /// The selected text, one line per row, trailing blanks cut.
    pub fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let session = self.session.as_ref()?;
        let (start, end) = selection.ordered();
        let text = session.with_screen(|screen| {
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
        let Some(protocol) = self.session.as_ref().map(Session::mouse_protocol) else {
            return;
        };
        if !protocol.enabled || cell.x <= 0.0 || cell.y <= 0.0 || cols == 0 || rows == 0 {
            self.mouse_cell = None;
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
        if modifiers.shift {
            self.mouse_cell = None;
            return;
        }
        let Some(pos) = pos.filter(|pos| rect.contains(*pos)) else {
            // Out there the pointer is not our business, and a report about a cell
            // it is not over would be a lie.
            self.mouse_cell = None;
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
        let moved = self.mouse_cell != Some((column, row));
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
        self.mouse_cell = Some((column, row));

        if scroll != 0.0 {
            // Scrolling comes in pixels; the program wants notches, and the remainder
            // is kept so a slow wheel is not lost.
            const NOTCH: f32 = 40.0;
            self.scroll_left += scroll;
            while self.scroll_left.abs() >= NOTCH {
                let wheel = if self.scroll_left > 0.0 {
                    mouse::Wheel::Up
                } else {
                    mouse::Wheel::Down
                };
                self.scroll_left -= NOTCH * self.scroll_left.signum();
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
        let _ = self.write(&bytes);
    }

    /// Sends bytes to the shell, as typing does.
    pub fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        match self.session.as_mut() {
            Some(session) => session.write(bytes),
            None => Ok(()),
        }
    }

    /// Ends the session; the next frame starts a new one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn restart(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.kill();
        }
        self.error = None;
        self.focused = false;
    }

    /// Whether the program in the terminal asked for the application cursor keys.
    pub fn application_cursor(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(ide_term::Session::application_cursor)
    }

    /// The grid the panel is drawing: columns and rows of cells.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Whether the program in the terminal asked for mouse reports.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn mouse_enabled(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.mouse_protocol().enabled)
    }

    /// What the screen holds, for tests and for the clipboard.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn contents(&self) -> String {
        self.session
            .as_ref()
            .map(Session::contents)
            .unwrap_or_default()
    }

    pub fn is_alive(&self) -> bool {
        self.session.as_ref().is_some_and(Session::is_alive)
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

#[cfg(test)]
mod tests {
    use super::*;

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
