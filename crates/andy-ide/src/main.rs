//! The andy-ide application: window, panels and the editor.

mod agent;
mod app;
mod credits;
mod editor;
mod git;
#[cfg(test)]
mod gui_tests;
mod log;
mod lsp;
mod notify;
mod rename;
mod state;
#[cfg(test)]
mod test_serial;
mod ui;

use std::path::PathBuf;

/// What `--help` prints.
///
/// A window is not the only thing a program is asked for: a package script asks which version it is,
/// and a person who has just unpacked an archive asks what to type. Both answers have to come without
/// a display and without a window.
const HELP: &str = "\
andy-ide — an editor in a window, with a terminal, git and an agent beside the text.

Usage: andy-ide [--project <dir>] [--file <path>]

  --project <dir>   the project to open; the directory the command was given in otherwise
  --file <path>     a file to open in the editor
  -h, --help        this text
  -V, --version     the version, and the commit the build was made from

The window keeps its settings in $XDG_CONFIG_HOME/andy-ide/config.toml (~/.config/andy-ide), the
project's own in <project>/.andy-ide/, and its log in $XDG_STATE_HOME/andy-ide/andy-ide.log.
";

/// The line `--version` prints: the number, and the commit the build was made from.
fn version_line() -> String {
    format!(
        "andy-ide {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("ANDY_IDE_COMMIT")
    )
}

/// Command line of the application.
#[derive(Debug, Default)]
struct Args {
    project: Option<PathBuf>,
    file: Option<PathBuf>,
    help: bool,
    version: bool,
}

impl Args {
    /// Parses `--project <path>`, `--file <path>`, `--help` and `--version`; unknowns are ignored.
    ///
    /// Ignoring an unknown argument is deliberate: the window is started from launchers, from menus and
    /// from scripts written for other editors, and a flag nobody recognises is a worse reason to refuse
    /// to open than a flag nobody needed.
    fn parse<I: IntoIterator<Item = String>>(args: I) -> Self {
        let mut parsed = Args::default();
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--project" => parsed.project = iter.next().map(PathBuf::from),
                "--file" => parsed.file = iter.next().map(PathBuf::from),
                "-h" | "--help" => parsed.help = true,
                "-V" | "--version" => parsed.version = true,
                _ => {}
            }
        }
        parsed
    }
}

/// The project to open: the one named, or the place the command was given in.
///
/// An editor is opened *in a place*. Naming nothing used to open whichever project was remembered from last
/// time, so `andy-ide` started inside one project opened another one. Andy: "даже если я нахожусь в другом
/// проекте и не указываю --project, у меня открывается проект RUST/andy-ide".
fn project_to_open(named: Option<PathBuf>, here: Option<PathBuf>) -> Option<PathBuf> {
    named.or(here)
}

/// The window the native backend is asked for.
///
/// Whether it opens maximized is read from the configuration before the window
/// exists: the window manager has to know on the first frame, and asking for it
/// afterwards means the window is seen at its old size first.
fn options(config: &ide_core::Config) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            // The title carries the build: which commit the window in front of a person was made from.
            // `ANDY_IDE_COMMIT` comes from `build.rs`; see the note there for why it is worth a title.
            .with_title(format!(
                "andy-ide {} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("ANDY_IDE_COMMIT")
            ))
            .with_maximized(config.window_maximized),
        ..Default::default()
    }
}

fn main() -> eframe::Result<()> {
    let args = Args::parse(std::env::args().skip(1));

    // Asked before anything is opened: a `--version` that starts a log file, a notification probe and a
    // window is a `--version` that hangs a packaging script.
    if args.help {
        print!("{HELP}");
        return Ok(());
    }
    if args.version {
        println!("{}", version_line());
        return Ok(());
    }

    // The log of the window: a file that is rotated at ten megabytes, and the standard error.
    log::init();

    if std::env::var_os("ANDY_IDE_NOTIFY_PROBE").is_some() {
        crate::notify::probe();
    }

    let project = project_to_open(args.project.clone(), std::env::current_dir().ok());
    // Who this window is and which desk it can reach, said once at the start. A notification the
    // desk takes and never shows is otherwise a mystery: the id it answers with says a daemon heard,
    // and only the address says whether that daemon is the one drawing the screen in front of the
    // person. When the two disagree, this line is where the disagreement is legible.
    tracing::info!(
        project = ?project,
        file = ?args.file,
        uid = %std::env::var("UID").unwrap_or_default(),
        user = %std::env::var("USER").unwrap_or_default(),
        bus = %std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
        display = %std::env::var("DISPLAY").unwrap_or_default(),
        wayland = %std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        "starting andy-ide"
    );

    let config = ide_core::Config::load_or_default(&state::config_dir().join("config.toml"));
    let options = options(&config);

    eframe::run_native(
        "Andy IDE",
        options,
        Box::new(move |_cc| Ok(Box::new(app::App::new(project.clone(), args.file.clone())))),
    )
}

#[cfg(test)]
mod tests {
    use super::{Args, options, project_to_open};
    use std::path::PathBuf;

    #[test]
    fn the_window_opens_the_way_it_was_left() {
        let mut config = ide_core::Config::default();
        assert_eq!(
            options(&config).viewport.maximized,
            Some(false),
            "a fresh install opens in a window"
        );

        config.window_maximized = true;
        assert_eq!(
            options(&config).viewport.maximized,
            Some(true),
            "the window was left maximized and the backend was not told"
        );
    }

    #[test]
    fn the_command_line_answers_what_and_which() {
        // The two flags a release is asked for: a package script reads `--version`, a person reads
        // `--help`, and neither may need a display.
        let version = Args::parse(["--version"].iter().map(|s| s.to_string()));
        assert!(version.version && !version.help);

        let help = Args::parse(["-h"].iter().map(|s| s.to_string()));
        assert!(help.help && !help.version);

        assert!(
            super::HELP.contains("--project") && super::HELP.contains("--file"),
            "the help text does not name the flags it has"
        );
        let line = super::version_line();
        assert!(
            line.starts_with("andy-ide ") && line.contains(env!("CARGO_PKG_VERSION")),
            "the version line does not carry the version: {line}"
        );
        assert!(
            line.contains('(') && line.ends_with(')'),
            "the version line does not carry the commit: {line}"
        );
    }

    #[test]
    fn parses_project_and_file() {
        let args = Args::parse(
            ["--project", "/tmp/p", "--file", "/tmp/p/a.rs"]
                .iter()
                .map(|s| s.to_string()),
        );
        assert_eq!(args.project, Some(PathBuf::from("/tmp/p")));
        assert_eq!(args.file, Some(PathBuf::from("/tmp/p/a.rs")));
    }

    #[test]
    fn a_named_project_wins_over_the_place_the_command_was_given_in() {
        assert_eq!(
            project_to_open(
                Some(PathBuf::from("/tmp/p")),
                Some(PathBuf::from("/tmp/here"))
            ),
            Some(PathBuf::from("/tmp/p"))
        );
    }

    #[test]
    fn without_a_name_the_place_the_command_was_given_in_is_the_project() {
        assert_eq!(
            project_to_open(None, Some(PathBuf::from("/tmp/here"))),
            Some(PathBuf::from("/tmp/here")),
            "nothing was named and the directory was ignored, so the remembered project would open"
        );
        assert_eq!(project_to_open(None, None), None);
    }

    #[test]
    fn ignores_unknown_arguments() {
        let args = Args::parse(["--verbose".to_string()]);
        assert!(args.project.is_none());
        assert!(args.file.is_none());
    }
}
