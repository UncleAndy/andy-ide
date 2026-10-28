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
        /// An arrow, with `app` meaning the cursor-moving variant (`Ctrl`).
        Arrow { direction: Direction, app: bool },
        /// `Home`.
        Home,
        /// `End`.
        End,
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
                Self::Arrow { direction, app } => {
                    let final_byte = match direction {
                        Direction::Up => 'A',
                        Direction::Down => 'B',
                        Direction::Right => 'C',
                        Direction::Left => 'D',
                    };
                    let introducer = if *app { "\x1bO" } else { "\x1b[" };
                    format!("{introducer}{final_byte}").into_bytes()
                }
                Self::Home => b"\x1b[H".to_vec(),
                Self::End => b"\x1b[F".to_vec(),
                Self::PageUp => b"\x1b[5~".to_vec(),
                Self::PageDown => b"\x1b[6~".to_vec(),
                Self::Delete => b"\x1b[3~".to_vec(),
                Self::Insert => b"\x1b[2~".to_vec(),
                Self::Control(byte) => vec![*byte],
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Direction, Input};

        #[test]
        fn control_c_is_one_byte_not_a_letter() {
            assert_eq!(Input::Control(3).bytes(), vec![3]);
        }

        #[test]
        fn the_arrows_are_escape_sequences() {
            let up = Input::Arrow {
                direction: Direction::Up,
                app: false,
            };
            assert_eq!(up.bytes(), b"\x1b[A".to_vec());
            let left = Input::Arrow {
                direction: Direction::Left,
                app: true,
            };
            assert_eq!(left.bytes(), b"\x1bOD".to_vec());
        }

        #[test]
        fn enter_is_a_carriage_return_and_backspace_is_delete() {
            assert_eq!(Input::Enter.bytes(), b"\r".to_vec());
            assert_eq!(Input::Backspace.bytes(), vec![0x7f]);
            assert_eq!(Input::BackTab.bytes(), b"\x1b[Z".to_vec());
        }
    }
}
