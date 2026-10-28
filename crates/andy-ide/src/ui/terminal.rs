//! The terminal panel: a real shell on a pseudo-terminal, drawn cell by cell.
//!
//! The session and the screen live in `ide-term`; this module only turns the screen
//! into rectangles and glyphs and turns egui's keys into the bytes a terminal expects.
//! A terminal is not a text field — `Ctrl+C` is byte 3, arrows are escape sequences —
//! so the mapping is a function of its own, tested without a window.

use egui::{Key, Modifiers, Ui};

use ide_term::Session;
use ide_term::keys::{Direction, Input};

/// A terminal tab: one session, or the reason it could not start.
pub struct Terminal {
    session: Option<Session>,
    pub error: Option<String>,
    cols: u16,
    rows: u16,
    /// Whether keys typed in this frame belong to the terminal.
    pub focused: bool,
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

    /// The bytes typed this frame, read from egui's events.
    pub fn read_keys(ui: &Ui) -> Vec<u8> {
        let mut bytes = Vec::new();
        ui.input(|input| {
            for event in &input.events {
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
                        if let Some(byte) = control_letter(*key, *modifiers) {
                            bytes.push(byte);
                        } else if let Some(input) = input_for_key(*key, *modifiers) {
                            bytes.extend_from_slice(&input.bytes());
                        }
                    }
                    egui::Event::Paste(text) => bytes.extend_from_slice(text.as_bytes()),
                    _ => {}
                }
            }
        });
        bytes
    }

    /// Draws the screen and lets the user type into it.
    pub fn show(&mut self, ui: &mut Ui) {
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
        if response.clicked() {
            self.focused = true;
        }
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
        session.with_screen(|screen| {
            for row in 0..rows {
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    let position = rect.left_top()
                        + egui::vec2(col as f32 * cell_width, row as f32 * cell_height);
                    let background = cell.bgcolor();
                    if background != vt100::Color::Default {
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

        if self.focused {
            let bytes = Self::read_keys(ui);
            if !bytes.is_empty() {
                let _ = self.write(&bytes);
            }
        }
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
/// `Ctrl` with a letter is a control byte; the arrows move the cursor in an
/// application (as `vim` asks for) when `Ctrl` is held, which is what xterm does.
pub fn input_for_key(key: Key, modifiers: Modifiers) -> Option<Input> {
    let app = modifiers.ctrl;
    let input = match key {
        Key::Enter => Input::Enter,
        Key::Backspace => Input::Backspace,
        Key::Tab if modifiers.shift => Input::BackTab,
        Key::Tab => Input::Tab,
        Key::Escape => Input::Escape,
        Key::ArrowUp => Input::Arrow {
            direction: Direction::Up,
            app,
        },
        Key::ArrowDown => Input::Arrow {
            direction: Direction::Down,
            app,
        },
        Key::ArrowLeft => Input::Arrow {
            direction: Direction::Left,
            app,
        },
        Key::ArrowRight => Input::Arrow {
            direction: Direction::Right,
            app,
        },
        Key::Home => Input::Home,
        Key::End => Input::End,
        Key::PageUp => Input::PageUp,
        Key::PageDown => Input::PageDown,
        Key::Delete => Input::Delete,
        Key::Insert => Input::Insert,
        _ => return None,
    };
    Some(input)
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
        let up = input_for_key(Key::ArrowUp, Modifiers::NONE).unwrap();
        assert_eq!(up.bytes(), b"\x1b[A".to_vec());
    }

    #[test]
    fn control_turns_an_arrow_into_the_application_variant() {
        let up = input_for_key(Key::ArrowUp, Modifiers::CTRL).unwrap();
        assert_eq!(up.bytes(), b"\x1bOA".to_vec());
    }

    #[test]
    fn shift_tab_goes_backwards() {
        let back = input_for_key(Key::Tab, Modifiers::SHIFT).unwrap();
        assert_eq!(back.bytes(), b"\x1b[Z".to_vec());
        let forward = input_for_key(Key::Tab, Modifiers::NONE).unwrap();
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
        assert!(input_for_key(Key::A, Modifiers::NONE).is_none());
    }
}
