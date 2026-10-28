//! The andy-ide application: window, panels and the editor.

mod app;
mod editor;
mod git;
#[cfg(test)]
mod gui_tests;
mod lsp;
mod rename;
mod state;
mod ui;

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

/// Command line of the application.
#[derive(Debug, Default)]
struct Args {
    project: Option<PathBuf>,
    file: Option<PathBuf>,
}

impl Args {
    /// Parses `--project <path>` and `--file <path>`; unknown arguments are ignored.
    fn parse<I: IntoIterator<Item = String>>(args: I) -> Self {
        let mut parsed = Args::default();
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--project" => parsed.project = iter.next().map(PathBuf::from),
                "--file" => parsed.file = iter.next().map(PathBuf::from),
                _ => {}
            }
        }
        parsed
    }
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
            .with_title("andy-ide")
            .with_maximized(config.window_maximized),
        ..Default::default()
    }
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("ANDY_IDE_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse(std::env::args().skip(1));
    tracing::info!(
        project = ?args.project,
        file = ?args.file,
        "starting andy-ide"
    );

    let config = ide_core::Config::load_or_default(&state::config_dir().join("config.toml"));
    let options = options(&config);

    eframe::run_native(
        "Andy IDE",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(app::App::new(
                args.project.clone(),
                args.file.clone(),
            )))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{Args, options};
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
    fn ignores_unknown_arguments() {
        let args = Args::parse(["--verbose".to_string()]);
        assert!(args.project.is_none());
        assert!(args.file.is_none());
    }
}
