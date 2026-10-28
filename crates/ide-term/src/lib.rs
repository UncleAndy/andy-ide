//! PTY sessions and terminal screen emulation.
//!
//! This crate is UI-agnostic: it must never depend on `egui` or `eframe`. A session
//! owns a child process on a pseudo-terminal, a thread that reads what the process
//! writes, and a screen that the escape sequences are parsed into. The interface asks
//! the screen for cells and draws them; it never parses terminal output itself.
//!
//! Why a real PTY and not pipes: `vim`, `htop` and `tmux` ask the terminal what it
//! is, whether it is interactive and how wide it is. On a pipe they turn colours off,
//! refuse to start, or wrap at 80 columns forever.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// How many lines above the screen are kept: enough to scroll back through a test run,
/// small enough that a chatty program cannot eat memory.
pub const SCROLLBACK: usize = 2000;

/// One terminal session: a child process, its pty and the screen it painted.
pub struct Session {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    alive: Arc<AtomicBool>,
    size: PtySize,
}

impl Session {
    /// Starts `program` with `args` on a fresh pseudo-terminal.
    pub fn spawn(program: &str, args: &[String], cols: u16, rows: u16) -> Result<Self> {
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pty = native_pty_system()
            .openpty(size)
            .context("cannot open a pseudo-terminal")?;

        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.env("TERM", "xterm-256color");
        // A shell that thinks it is in a login session on an unknown host prints a
        // banner per tab; the editor is the terminal here.
        command.env("TERM_PROGRAM", "andy-ide");

        let child = pty
            .slave
            .spawn_command(command)
            .with_context(|| format!("cannot start {program}"))?;
        // The slave side is held by the child; keeping ours would stop the reader at
        // the moment the child exits, which is exactly what it must see.
        drop(pty.slave);

        let mut reader = pty
            .master
            .try_clone_reader()
            .context("cannot read the terminal")?;
        let writer = pty
            .master
            .take_writer()
            .context("cannot write to the terminal")?;

        let rows = std::num::NonZeroU16::new(size.rows).unwrap_or(std::num::NonZeroU16::MIN);
        let cols = std::num::NonZeroU16::new(size.cols).unwrap_or(std::num::NonZeroU16::MIN);
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
        let alive = Arc::new(AtomicBool::new(true));
        {
            let parser = Arc::clone(&parser);
            let alive = Arc::clone(&alive);
            std::thread::Builder::new()
                .name("term-reader".to_owned())
                .spawn(move || {
                    let mut buffer = [0u8; 8192];
                    loop {
                        match reader.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(count) => {
                                if let Ok(mut parser) = parser.lock() {
                                    parser.process(&buffer[..count]);
                                }
                            }
                        }
                    }
                    // A locked parser at this point means the screen is being drawn;
                    // the flag is what the interface watches, so it is set regardless.
                    alive.store(false, Ordering::Relaxed);
                })
                .context("cannot start the reader thread")?;
        }

        Ok(Self {
            writer,
            master: pty.master,
            child,
            parser,
            alive,
            size,
        })
    }

    /// Starts the user's shell, or a sensible one when `SHELL` is unset.
    pub fn shell(cols: u16, rows: u16) -> Result<Self> {
        // The sandbox this was written in has no `/bin/sh` at all, so the shell is looked
        // for rather than assumed; a bare name goes through `PATH`.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| {
            for candidate in [
                "/run/current-system/sw/bin/bash",
                "/bin/bash",
                "/bin/sh",
                "bash",
                "sh",
            ] {
                if !candidate.starts_with('/') || std::path::Path::new(candidate).exists() {
                    return candidate.to_owned();
                }
            }
            "sh".to_owned()
        });
        let mut session = Self::spawn(&shell, &[], cols, rows)?;
        // Interactive and colourful, the way a person expects a terminal tab to open.
        session.write(b"")?;
        Ok(session)
    }

    /// Sends bytes as if typed.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("cannot write")?;
        self.writer.flush().context("cannot flush")
    }

    /// Tells the child how big its terminal now is.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.size.cols && rows == self.size.rows {
            return Ok(());
        }
        self.size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master
            .resize(self.size)
            .context("cannot resize the terminal")?;
        if let Ok(mut parser) = self.parser.lock() {
            let rows = std::num::NonZeroU16::new(rows).unwrap_or(std::num::NonZeroU16::MIN);
            let cols = std::num::NonZeroU16::new(cols).unwrap_or(std::num::NonZeroU16::MIN);
            parser.set_size(rows, cols);
        }
        Ok(())
    }

    /// The screen as it is now, for drawing or for a test.
    pub fn with_screen<T>(&self, read: impl FnOnce(&vt100::Screen) -> T) -> T {
        let parser = self
            .parser
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        read(parser.screen())
    }

    /// Whether the program in the terminal asked for the application cursor keys.
    ///
    /// A full-screen program turns this on with `smkx` and then expects `ESC O B`
    /// where a shell at its prompt expects `ESC [ B`. Sending the wrong one makes
    /// every arrow silently do nothing in `mc`, `vim` and friends.
    pub fn application_cursor(&self) -> bool {
        self.with_screen(|screen| screen.application_cursor())
    }

    /// What the program asked for from the mouse.
    ///
    /// Clicking a terminal does nothing unless the program in it has turned mouse
    /// reporting on with `CSI ? 1000 h` (press), `1002` (press and drag) or `1003`
    /// (every movement); `1006` asks for the SGR form of the reports. A shell at its
    /// prompt asks for nothing, and then a click belongs to the window, not the
    /// shell — which is also how the panel takes the keyboard.
    pub fn mouse_protocol(&self) -> mouse::Protocol {
        self.with_screen(|screen| {
            use vt100::MouseProtocolMode;
            let mode = screen.mouse_protocol_mode();
            mouse::Protocol {
                enabled: mode != MouseProtocolMode::None,
                press_and_release: matches!(
                    mode,
                    MouseProtocolMode::PressRelease
                        | MouseProtocolMode::ButtonMotion
                        | MouseProtocolMode::AnyMotion
                ),
                drag: matches!(
                    mode,
                    MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
                ),
                motion: mode == MouseProtocolMode::AnyMotion,
                encoding: match screen.mouse_protocol_encoding() {
                    vt100::MouseProtocolEncoding::Sgr => mouse::Encoding::Sgr,
                    vt100::MouseProtocolEncoding::Utf8 => mouse::Encoding::Utf8,
                    vt100::MouseProtocolEncoding::Default => mouse::Encoding::Default,
                },
            }
        })
    }

    /// Everything the screen holds, lines joined by `\n` and trailing blanks cut.
    pub fn contents(&self) -> String {
        let raw = self.with_screen(|screen| screen.contents());
        let lines: Vec<&str> = raw.lines().map(str::trim_end).collect();
        let last = lines
            .iter()
            .rposition(|line| !line.is_empty())
            .map_or(0, |index| index + 1);
        lines[..last].join("\n")
    }

    /// Whether the child is still running.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// The size the child was told about.
    pub fn size(&self) -> (u16, u16) {
        (self.size.cols, self.size.rows)
    }

    /// Ends the child and lets go of the terminal.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        self.alive.store(false, Ordering::Relaxed);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The bytes a key press turns into.
///
/// A terminal is not a text field: `Ctrl+C` is byte 3, not the letter `c` with a
/// modifier, and the arrow keys are escape sequences. The mapping lives here, in the
/// UI-free crate, so it can be tested without a window.
pub mod keys {
    /// What a key press means, already translated to terminal input.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Input {
        /// Text to send as it stands.
        Text(String),
        /// `Enter`.
        Enter,
        /// `Backspace`.
        Backspace,
        /// `Tab`.
        Tab,
        /// `Shift+Tab`.
        BackTab,
        /// `Escape`.
        Escape,
        /// An arrow.
        ///
        /// `app` is the terminal's *application cursor* mode, which a program turns
        /// on with `CSI ? 1 h` (`smkx`). In that mode the arrows and `Home`/`End`
        /// are `ESC O` sequences, not `ESC [`: a full-screen program such as `mc`
        /// matches what its terminfo entry says, and `ESC [` is not in it.
        Arrow {
            direction: Direction,
            app: bool,
            mods: Mods,
        },
        /// `Home`.
        Home { app: bool, mods: Mods },
        /// `End`.
        End { app: bool, mods: Mods },
        /// A numbered function key, `F1`..`F12`.
        Function { number: u8, mods: Mods },
        /// `PageUp`.
        PageUp,
        /// `PageDown`.
        PageDown,
        /// `Delete`.
        Delete,
        /// `Insert`.
        Insert,
        /// `Ctrl` with a letter: `1..=26` besides the named ones.
        Control(u8),
    }

    /// Which arrow.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Direction {
        Up,
        Down,
        Right,
        Left,
    }

    /// The modifier keys a terminal can tell apart, and the number they come to.
    ///
    /// xterm writes them as one parameter: `1` plus `1` for `Shift`, `2` for `Alt`
    /// and `4` for `Ctrl` — `ESC [ 1 ; 5 A` is `Ctrl+Up`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Mods {
        pub shift: bool,
        pub alt: bool,
        pub ctrl: bool,
    }

    impl Mods {
        /// Whether any of them is held: a plain key must not gain a parameter, or
        /// its form would stop matching what terminfo says.
        pub fn any(&self) -> bool {
            self.shift || self.alt || self.ctrl
        }

        /// The single parameter xterm uses for these modifiers.
        pub fn code(&self) -> u8 {
            1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
        }
    }

    impl Input {
        /// The bytes this input sends to the child.
        pub fn bytes(&self) -> Vec<u8> {
            match self {
                Self::Text(text) => text.as_bytes().to_vec(),
                Self::Enter => b"\r".to_vec(),
                Self::Backspace => vec![0x7f],
                Self::Tab => b"\t".to_vec(),
                // `CSI Z` is what xterm sends for Shift+Tab.
                Self::BackTab => b"\x1b[Z".to_vec(),
                Self::Escape => vec![0x1b],
                Self::Arrow {
                    direction,
                    app,
                    mods,
                } => {
                    let final_byte = match direction {
                        Direction::Up => 'A',
                        Direction::Down => 'B',
                        Direction::Right => 'C',
                        Direction::Left => 'D',
                    };
                    cursor_key(final_byte, *app, *mods)
                }
                Self::Home { app, mods } => cursor_key('H', *app, *mods),
                Self::End { app, mods } => cursor_key('F', *app, *mods),
                Self::Function { number, mods } => function_key(*number, *mods),
                Self::PageUp => b"\x1b[5~".to_vec(),
                Self::PageDown => b"\x1b[6~".to_vec(),
                Self::Delete => b"\x1b[3~".to_vec(),
                Self::Insert => b"\x1b[2~".to_vec(),
                Self::Control(byte) => vec![*byte],
            }
        }
    }

    /// An arrow, `Home` or `End`, in whichever of the two forms the terminal is in.
    ///
    /// `ESC O X` is the application-cursor form that terminfo's `kcud1` and friends
    /// name, and it is what full-screen programs expect once they have asked for it
    /// with `smkx`. With modifiers xterm switches to the `CSI 1 ; n X` form in both
    /// modes, because the application form has nowhere to put the parameter.
    fn cursor_key(final_byte: char, app: bool, mods: Mods) -> Vec<u8> {
        if mods.any() {
            format!("\x1b[1;{}{final_byte}", mods.code()).into_bytes()
        } else if app {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    }

    /// A function key, numbered as xterm numbers them.
    ///
    /// `F1`..`F4` are the letters `P`..`S` after `ESC O`; `F5` and up are numbered
    /// `CSI 15 ~`, `17 ~`, `18 ~`, `19 ~`, `20 ~`, `21 ~`, `23 ~`, `24 ~` — the
    /// numbers skip 16 and 22 for reasons lost with the terminals that had them.
    fn function_key(number: u8, mods: Mods) -> Vec<u8> {
        let code = mods.code();
        let has_mods = mods.any();
        match number {
            1..=4 => {
                let letter = ['P', 'Q', 'R', 'S'][(number - 1) as usize];
                if has_mods {
                    format!("\x1b[1;{code}{letter}").into_bytes()
                } else {
                    format!("\x1bO{letter}").into_bytes()
                }
            }
            5..=12 => {
                let base = match number {
                    5 => 15,
                    6 => 17,
                    7 => 18,
                    8 => 19,
                    9 => 20,
                    10 => 21,
                    11 => 23,
                    _ => 24,
                };
                if has_mods {
                    format!("\x1b[{base};{code}~").into_bytes()
                } else {
                    format!("\x1b[{base}~").into_bytes()
                }
            }
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Direction, Input, Mods};

        #[test]
        fn control_c_is_one_byte_not_a_letter() {
            assert_eq!(Input::Control(3).bytes(), vec![3]);
        }

        fn arrow(direction: Direction, app: bool) -> Input {
            Input::Arrow {
                direction,
                app,
                mods: Mods::default(),
            }
        }

        #[test]
        fn the_arrows_are_escape_sequences() {
            assert_eq!(arrow(Direction::Up, false).bytes(), b"\x1b[A".to_vec());
            assert_eq!(arrow(Direction::Left, false).bytes(), b"\x1b[D".to_vec());
        }

        #[test]
        fn in_the_application_mode_the_arrows_lose_the_bracket() {
            // `mc`, `vim` and every other ncurses program asks for this mode with
            // `smkx` and then matches `kcud1` from terminfo, which is `ESC O B`.
            assert_eq!(arrow(Direction::Down, true).bytes(), b"\x1bOB".to_vec());
            assert_eq!(arrow(Direction::Right, true).bytes(), b"\x1bOC".to_vec());
            assert_eq!(
                Input::Home {
                    app: true,
                    mods: Mods::default()
                }
                .bytes(),
                b"\x1bOH".to_vec()
            );
            assert_eq!(
                Input::End {
                    app: false,
                    mods: Mods::default()
                }
                .bytes(),
                b"\x1b[F".to_vec()
            );
        }

        #[test]
        fn a_modified_arrow_carries_its_modifiers() {
            let ctrl_up = Input::Arrow {
                direction: Direction::Up,
                app: true,
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
            };
            assert_eq!(ctrl_up.bytes(), b"\x1b[1;5A".to_vec());
            let shift_left = Input::Arrow {
                direction: Direction::Left,
                app: false,
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
            };
            assert_eq!(shift_left.bytes(), b"\x1b[1;2D".to_vec());
        }

        #[test]
        fn the_function_keys_are_what_xterm_sends() {
            let plain = Mods::default();
            for (number, expected) in [
                (1u8, b"\x1bOP".as_slice()),
                (2, b"\x1bOQ"),
                (3, b"\x1bOR"),
                (4, b"\x1bOS"),
                (5, b"\x1b[15~"),
                (6, b"\x1b[17~"),
                (7, b"\x1b[18~"),
                (8, b"\x1b[19~"),
                (9, b"\x1b[20~"),
                (10, b"\x1b[21~"),
                (11, b"\x1b[23~"),
                (12, b"\x1b[24~"),
            ] {
                assert_eq!(
                    Input::Function {
                        number,
                        mods: plain
                    }
                    .bytes(),
                    expected.to_vec(),
                    "F{number}"
                );
            }
            assert_eq!(
                Input::Function {
                    number: 1,
                    mods: Mods {
                        shift: true,
                        ..Mods::default()
                    }
                }
                .bytes(),
                b"\x1b[1;2P".to_vec()
            );
            assert_eq!(
                Input::Function {
                    number: 5,
                    mods: Mods {
                        ctrl: true,
                        ..Mods::default()
                    }
                }
                .bytes(),
                b"\x1b[15;5~".to_vec()
            );
        }

        #[test]
        fn enter_is_a_carriage_return_and_backspace_is_delete() {
            assert_eq!(Input::Enter.bytes(), b"\r".to_vec());
            assert_eq!(Input::Backspace.bytes(), vec![0x7f]);
            assert_eq!(Input::BackTab.bytes(), b"\x1b[Z".to_vec());
        }
    }
}

/// Mouse reports: what a click in the panel means to the program in it.
///
/// A terminal does not have a mouse. The program asks for reports with
/// `CSI ? 1000 h` and friends, and the terminal answers with escape sequences that
/// carry the button, the modifiers and the cell. `mc` selects the file under the
/// pointer that way, and `vim` moves its cursor.
pub mod mouse {
    use super::keys::Mods;

    /// What the program asked for when it turned reporting on.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Protocol {
        /// Whether clicks are reported at all (`1000`, `1002`, `1003`).
        pub enabled: bool,
        /// Whether letting a button go is reported (`1002`, `1003`); the X10 mode
        /// only reports presses.
        pub press_and_release: bool,
        /// Whether moving while a button is held is reported (`1002`, `1003`).
        pub drag: bool,
        /// Whether every movement is reported, held button or not (`1003`).
        pub motion: bool,
        /// Which shape the reports take (`1006` asks for SGR).
        pub encoding: Encoding,
    }

    /// The shape of a report.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum Encoding {
        /// `ESC [ M` and three bytes, the original xterm form.
        #[default]
        Default,
        /// The same, with the three values written as UTF-8 (xterm's `1005`).
        Utf8,
        /// `ESC [ < b ; x ; y M`, which has no ceiling on the numbers (xterm's `1006`).
        Sgr,
    }

    /// Which button.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Button {
        Left,
        Middle,
        Right,
    }

    /// Which way the wheel turned.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Wheel {
        Up,
        Down,
    }

    /// What happened to the pointer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Action {
        Press(Button),
        Release(Button),
        Drag(Button),
        Motion,
        Wheel(Wheel),
    }

    /// One report, already in cells of the screen rather than pixels.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Event {
        pub action: Action,
        /// Zero-based column and row of the cell under the pointer.
        pub column: u16,
        pub row: u16,
        pub mods: Mods,
    }

    impl Event {
        pub fn new(action: Action, column: u16, row: u16, mods: Mods) -> Self {
            Self {
                action,
                column,
                row,
                mods,
            }
        }
    }

    impl Action {
        /// The number xterm gives this action, modifiers aside.
        fn code(self) -> u8 {
            let base = match self {
                Action::Press(Button::Left)
                | Action::Release(Button::Left)
                | Action::Drag(Button::Left) => 0,
                Action::Press(Button::Middle)
                | Action::Release(Button::Middle)
                | Action::Drag(Button::Middle) => 1,
                Action::Press(Button::Right)
                | Action::Release(Button::Right)
                | Action::Drag(Button::Right) => 2,
                // 35 is "moved with nothing held": there is no such button, so the
                // number has no other use.
                Action::Motion => 35,
                Action::Wheel(Wheel::Up) => 64,
                Action::Wheel(Wheel::Down) => 65,
            };
            // A drag is a press of the same button with this bit set.
            base + if matches!(self, Action::Drag(_)) {
                32
            } else {
                0
            }
        }

        fn is_release(self) -> bool {
            matches!(self, Action::Release(_))
        }
    }

    /// The number for a button in the original encoding, where a release says
    /// nothing about which button it was.
    const RELEASE_CODE: u8 = 3;

    /// The bytes one report turns into.
    pub fn bytes(event: Event, encoding: Encoding) -> Vec<u8> {
        let mods = u8::from(event.mods.shift) * 4
            + u8::from(event.mods.alt) * 8
            + u8::from(event.mods.ctrl) * 16;
        let code = if event.action.is_release() && encoding != Encoding::Sgr {
            RELEASE_CODE
        } else {
            event.action.code()
        } + mods;
        let column = event.column as u32 + 1;
        let row = event.row as u32 + 1;

        match encoding {
            Encoding::Sgr => {
                let final_byte = if event.action.is_release() { 'm' } else { 'M' };
                format!("\x1b[<{code};{column};{row}{final_byte}").into_bytes()
            }
            Encoding::Default | Encoding::Utf8 => {
                let mut bytes = b"\x1b[M".to_vec();
                for value in [code as u32 + 32, column + 32, row + 32] {
                    if encoding == Encoding::Utf8 {
                        // UTF-8 carries anything above 127 in more than one byte,
                        // which is the whole reason this encoding exists.
                        let character = char::from_u32(value).unwrap_or('\u{fffd}');
                        let mut buffer = [0u8; 4];
                        bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                    } else {
                        bytes.push(value as u8);
                    }
                }
                bytes
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Action, Button, Encoding, Event, Wheel, bytes};
        use crate::keys::Mods;

        fn event(action: Action) -> Event {
            Event::new(action, 4, 2, Mods::default())
        }

        #[test]
        fn a_click_in_the_sgr_form_names_the_button_and_the_cell() {
            assert_eq!(
                bytes(event(Action::Press(Button::Left)), Encoding::Sgr),
                b"\x1b[<0;5;3M".to_vec()
            );
            assert_eq!(
                bytes(event(Action::Release(Button::Left)), Encoding::Sgr),
                b"\x1b[<0;5;3m".to_vec(),
                "a release is the same number with a small final letter"
            );
            assert_eq!(
                bytes(event(Action::Press(Button::Right)), Encoding::Sgr),
                b"\x1b[<2;5;3M".to_vec()
            );
        }

        #[test]
        fn the_wheel_and_the_drag_have_their_own_numbers() {
            assert_eq!(
                bytes(event(Action::Wheel(Wheel::Up)), Encoding::Sgr),
                b"\x1b[<64;5;3M".to_vec()
            );
            assert_eq!(
                bytes(event(Action::Wheel(Wheel::Down)), Encoding::Sgr),
                b"\x1b[<65;5;3M".to_vec()
            );
            assert_eq!(
                bytes(event(Action::Drag(Button::Left)), Encoding::Sgr),
                b"\x1b[<32;5;3M".to_vec()
            );
            assert_eq!(
                bytes(event(Action::Motion), Encoding::Sgr),
                b"\x1b[<35;5;3M".to_vec()
            );
        }

        #[test]
        fn a_modifier_is_one_byte_of_the_number() {
            let ctrl = Event::new(
                Action::Press(Button::Left),
                0,
                0,
                Mods {
                    ctrl: true,
                    ..Mods::default()
                },
            );
            assert_eq!(bytes(ctrl, Encoding::Sgr), b"\x1b[<16;1;1M".to_vec());
            let shift_alt = Event::new(
                Action::Press(Button::Left),
                0,
                0,
                Mods {
                    shift: true,
                    alt: true,
                    ctrl: false,
                },
            );
            assert_eq!(bytes(shift_alt, Encoding::Sgr), b"\x1b[<12;1;1M".to_vec());
        }

        #[test]
        fn the_old_form_carries_the_numbers_as_single_bytes() {
            // `ESC [ M` and three bytes, each 32 above the number; a release has no
            // button in this form and says 3.
            assert_eq!(
                bytes(event(Action::Press(Button::Left)), Encoding::Default),
                vec![0x1b, b'[', b'M', 32, 37, 35]
            );
            assert_eq!(
                bytes(event(Action::Release(Button::Right)), Encoding::Default),
                vec![0x1b, b'[', b'M', 32 + 3, 37, 35]
            );
        }

        #[test]
        fn the_utf8_form_writes_the_same_numbers_as_characters() {
            let far = Event::new(Action::Press(Button::Left), 300, 0, Mods::default());
            let bytes = bytes(far, Encoding::Utf8);
            assert_eq!(&bytes[..3], b"\x1b[M");
            // The button comes first, then 0x20 + 301 = 333, which UTF-8 writes as
            // two bytes: 0xC5 0x8D.
            assert_eq!(bytes[3], 32);
            assert_eq!(&bytes[4..6], "\u{14d}".as_bytes());
        }
    }
}
