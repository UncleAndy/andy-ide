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

/// A tmux server of this test's own, taken down again even when a check fails.
///
/// `C-b d` detaches, and a detached server is exactly the thing that stays behind holding a shell:
/// one per run, unnoticed until `tmux -L` sockets fill the shared runtime directory. Two things
/// keep that from happening. The server is told to put its socket in the test's own directory
/// (`TMUX_TMPDIR`), so nothing of this run is left in `/tmp/tmux-<uid>` — not even the socket file,
/// which tmux leaves behind after a server ends; both go with the directory. And the guard ends the
/// server on the way out, so a failed assertion does not leave one either.
struct TmuxServer {
    /// The test's own runtime directory; tmux keeps its socket here.
    tmpdir: PathBuf,
    socket: String,
}

impl TmuxServer {
    fn new(tmpdir: &std::path::Path, socket: String) -> Self {
        Self {
            tmpdir: tmpdir.to_path_buf(),
            socket,
        }
    }

    /// A `tmux` of this server's own: its socket lives in the test's directory, not the shared one.
    fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new("tmux");
        command
            .env("TMUX_TMPDIR", &self.tmpdir)
            .arg("-L")
            .arg(&self.socket);
        command
    }

    /// Whether a server answers on that socket.
    fn alive(&self) -> bool {
        self.command()
            .arg("ls")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        // The server is gone already if the check ended it by hand; then this changes nothing.
        let _ = self
            .command()
            .arg("kill-server")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
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
    let server = TmuxServer::new(&dir, format!("andy-ide-test-{}", std::process::id()));
    let socket = server.socket.clone();
    let mut session = Session::spawn(
        "bash",
        &[
            "-lc".to_owned(),
            format!(
                "cd {dir} && TMUX_TMPDIR={dir} exec tmux -L {socket} new-session -s main",
                dir = dir.display()
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
}

#[test]
fn the_tmux_server_of_a_check_does_not_outlive_it() {
    if !installed("tmux") {
        println!("tmux is not installed: the check was skipped");
        return;
    }
    let dir = scratch("tmux-server");
    let socket = format!("andy-ide-test-guard-{}", std::process::id());
    {
        let server = TmuxServer::new(&dir, socket.clone());
        let session = Session::spawn(
            "bash",
            &[
                "-lc".to_owned(),
                format!(
                    "cd {dir} && TMUX_TMPDIR={dir} exec tmux -L {socket} new-session -s main",
                    dir = dir.display()
                ),
            ],
            120,
            30,
        )
        .expect("tmux started");
        assert!(
            wait_for(&session, "main"),
            "the server did not come up: {:?}",
            session.contents()
        );
        assert!(
            server.alive(),
            "no server answered on the socket it was started with"
        );
    } // the guard goes out of scope here, as it does at the end of the check above

    assert!(
        !TmuxServer::new(&dir, socket.clone()).alive(),
        "a tmux server outlived the check that started it: {socket}"
    );
    // And the socket file tmux leaves behind — it does not remove it even when the server ends —
    // is inside the test's own directory, under the `tmux-<uid>` tmux makes there, so removing that
    // directory takes the crumb with it and the shared runtime directory gains nothing.
    let runtime = std::fs::read_dir(&dir)
        .expect("the directory is still there")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("tmux-"))
        })
        .expect("tmux kept its socket outside the directory of this check");
    assert!(
        runtime.join(&socket).exists(),
        "the socket of this run is not where tmux put its runtime directory: {}",
        runtime.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !dir.exists(),
        "the directory of the check outlived it: {}",
        dir.display()
    );
}
