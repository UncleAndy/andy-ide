//! The andy-ide application: window, panels and the editor.

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

/// Application state. In this milestone it only remembers what was asked to open.
struct App {
    project: Option<PathBuf>,
    file: Option<PathBuf>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(format!("andy-ide {}", env!("CARGO_PKG_VERSION")));
            ui.separator();
            match &self.project {
                Some(path) => ui.label(format!("project: {}", path.display())),
                None => ui.label("project: none"),
            };
            if let Some(path) = &self.file {
                ui.label(format!("file: {}", path.display()));
            }
        });
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("andy-ide"),
        ..Default::default()
    };

    eframe::run_native(
        "andy-ide",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(App {
                project: args.project.clone(),
                file: args.file.clone(),
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::Args;
    use std::path::PathBuf;

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
