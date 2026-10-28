//! Real full-screen programs, of the kind the plan asks to accept by hand.
//!
//! `vim` and `tmux` are here because they are the honest test of a terminal: they
//! switch the screen to the alternate buffer, ask for the application cursor keys,
//! paint with colours and take the keyboard raw. A terminal that only works with
//! `echo` passes none of this. `htop` is not installed in this environment, so its
//! column is covered by `mc`, which is the same kind of ncurses program.
//!
//! Every check needs the program in `PATH`; without it the check says so and passes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ide_term::{Session, keys::Input};

fn installed(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// A directory of this test's own: two runs side by side must not share one.
fn scratch(prefix: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "andy-ide-{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn wait_for(session: &Session, needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if session.contents().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn wait_until_gone(session: &Session) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if !session.is_alive() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn vim_takes_the_keys_and_writes_the_file() {
    if !installed("vim") {
        println!("vim is not installed: the check was skipped");
        return;
    }
    let dir = scratch("vim");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "first line\n").unwrap();

    let mut session = Session::spawn(
        "bash",
        &[
            "-lc".to_owned(),
            format!("cd {} && exec vim notes.txt", dir.display()),
        ],
        100,
        30,
    )
    .expect("vim started");
    assert!(
        wait_for(&session, "notes.txt"),
        "vim did not draw its status line: {:?}",
        session.contents()
    );
    assert!(
        session.application_cursor(),
        "vim did not ask for the application cursor keys"
    );

    // Insert mode, text, leave and save: every one of those is a different key path.
    session
        .write(b"isecond line\x1b:wq\r")
        .expect("keys reached vim");
    assert!(
        wait_until_gone(&session),
        "vim did not leave: {:?}",
        session.contents()
    );
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(
        saved.contains("second line"),
        "vim did not save what was typed: {saved:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tmux_runs_a_shell_of_its_own_and_lets_go() {
    if !installed("tmux") {
        println!("tmux is not installed: the check was skipped");
        return;
    }
    let dir = scratch("tmux");
    let socket = format!("andy-ide-test-{}", std::process::id());
    let mut session = Session::spawn(
        "bash",
        &[
            "-lc".to_owned(),
            format!(
                "cd {} && exec tmux -L {socket} new-session -s main",
                dir.display()
            ),
        ],
        120,
        30,
    )
    .expect("tmux started");
    assert!(
        wait_for(&session, "main"),
        "tmux did not draw its status bar: {:?}",
        session.contents()
    );

    // Typing goes through the client into the pane's shell.
    session
        .write(b"echo TMUX-MARK\r")
        .expect("keys reached tmux");
    assert!(
        wait_for(&session, "TMUX-MARK"),
        "the command did not run inside tmux: {:?}",
        session.contents()
    );

    // `C-b d` detaches, and the client exits: the keys really are tmux's.
    session
        .write(&Input::Control(0x02).bytes())
        .expect("the prefix reached tmux");
    std::thread::sleep(Duration::from_millis(200));
    session.write(b"d").expect("the detach key reached tmux");
    assert!(
        wait_until_gone(&session),
        "tmux did not detach: {:?}",
        session.contents()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
