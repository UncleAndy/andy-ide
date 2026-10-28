//! What `mc` — a full-screen ncurses program — does with the keys we send.
//!
//! `mc` asks for the application cursor keys (`smkx`) and then matches the names in
//! its terminfo entry, where `kcud1` is `ESC O B`. Sending `ESC [ B`, which is right
//! at a shell prompt, is silently ignored there: that is the bug these tests hold.
//!
//! The checks need `mc` installed; without it they say so and pass.

use std::time::{Duration, Instant};

use ide_term::keys::{Direction, Input, Mods};

fn mc_is_installed() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join("mc").is_file()))
}

fn with_files() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("andy-ide-mc");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["alpha.rs", "beta.rs", "gamma.rs"] {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    dir
}

fn wait_for(session: &ide_term::Session, needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if session.contents().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// The row `mc` has selected in the left panel.
///
/// The selection is the row whose background differs from the rest of the panel:
/// the text is the same either way, so the colour is the only witness.
fn selected_row(session: &ide_term::Session) -> Option<u16> {
    session.with_screen(|screen| {
        let (rows, cols) = screen.size();
        let rows = rows.get();
        let cols = cols.get();
        let mut name_rows: Vec<(u16, vt100::Color)> = Vec::new();
        // The listing starts under the header and ends at its first blank row; the
        // function-key bar at the bottom of the screen belongs to no panel.
        for row in 3..rows.saturating_sub(2) {
            let has_name = (1..(cols / 2).min(12))
                .filter_map(|col| screen.cell(row, col))
                .any(|cell| !cell.contents().trim().is_empty());
            if !has_name {
                break;
            }
            if let Some(cell) = screen.cell(row, 1) {
                name_rows.push((row, cell.bgcolor()));
            }
        }
        let panel_colour = name_rows
            .iter()
            .map(|(_, colour)| *colour)
            .max_by_key(|colour| {
                name_rows
                    .iter()
                    .filter(|(_, other)| other == colour)
                    .count()
            })?;
        let selected: Vec<u16> = name_rows
            .iter()
            .filter(|(_, colour)| *colour != panel_colour)
            .map(|(row, _)| *row)
            .collect();
        (selected.len() == 1).then(|| selected[0])
    })
}

/// Starts `mc` on a directory of files and waits for its panel to be drawn.
fn start_mc() -> ide_term::Session {
    let dir = with_files();
    let args = vec!["-lc".to_owned(), format!("cd {} && exec mc", dir.display())];
    let session = ide_term::Session::spawn("bash", &args, 100, 30).expect("mc started");
    assert!(wait_for(&session, "alpha.rs"), "mc did not draw its panel");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        session.application_cursor(),
        "mc did not ask for the application cursor keys"
    );
    session
}

fn down(app: bool) -> Vec<u8> {
    Input::Arrow {
        direction: Direction::Down,
        app,
        mods: Mods::default(),
    }
    .bytes()
}

#[test]
fn the_application_form_moves_the_selection() {
    if !mc_is_installed() {
        println!("mc is not installed: the check was skipped");
        return;
    }
    let mut session = start_mc();
    let before = selected_row(&session).expect("mc marked one row");
    session.write(&down(true)).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        selected_row(&session),
        Some(before + 1),
        "the selection did not move down"
    );
    session.kill();
}

#[test]
fn the_plain_form_is_ignored_by_mc() {
    if !mc_is_installed() {
        println!("mc is not installed: the check was skipped");
        return;
    }
    let mut session = start_mc();
    let before = selected_row(&session).expect("mc marked one row");
    session.write(&down(false)).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        selected_row(&session),
        Some(before),
        "mc accepted the plain form after asking for the application one"
    );
    session.kill();
}

#[test]
fn f10_leaves_mc() {
    if !mc_is_installed() {
        println!("mc is not installed: the check was skipped");
        return;
    }
    let mut session = start_mc();
    session
        .write(
            &Input::Function {
                number: 10,
                mods: Mods::default(),
            }
            .bytes(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && session.is_alive() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!session.is_alive(), "F10 did not leave mc");
}
