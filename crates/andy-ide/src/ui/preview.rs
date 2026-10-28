//! Markdown preview beside the source.
//!
//! Rendering parses the whole document, so the preview does not follow every
//! keystroke: the text it shows changes at most every [`PREVIEW_DELAY`], and
//! only when it actually changed.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::Ui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

/// How long the source may keep changing before the preview catches up.
pub const PREVIEW_DELAY: Duration = Duration::from_millis(300);

/// Whether this file is meant to be read as Markdown.
pub fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("md" | "markdown" | "mdx")
    )
}

/// The preview of one document: what it last showed, and when.
#[derive(Debug, Default)]
pub struct Preview {
    shown: String,
    last_render: Option<Instant>,
}

impl Preview {
    pub fn new() -> Self {
        Self::default()
    }

    /// The text the preview would draw right now.
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// Whether the preview should take the new text: it changed, and the delay
    /// since the last update has passed.
    pub fn should_render(&mut self, text: &str, now: Instant) -> bool {
        if text == self.shown {
            return false;
        }
        match self.last_render {
            Some(last) if now.saturating_duration_since(last) < PREVIEW_DELAY => false,
            _ => {
                self.shown = text.to_owned();
                self.last_render = Some(now);
                true
            }
        }
    }

    /// Feeds the current source in, respecting the delay.
    pub fn update(&mut self, text: &str, now: Instant) {
        self.should_render(text, now);
    }

    /// Draws the last accepted text.
    pub fn show(&mut self, ui: &mut Ui, cache: &mut CommonMarkCache) {
        egui::ScrollArea::vertical()
            .id_salt("markdown-preview")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                CommonMarkViewer::new().show(ui, cache, &self.shown);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_waits_for_the_typing_to_pause() {
        let mut preview = Preview::new();
        let start = Instant::now();

        assert!(
            preview.should_render("# a", start),
            "the first text renders"
        );
        assert!(
            !preview.should_render("# a", start),
            "the same text is not work"
        );

        let soon = start + Duration::from_millis(100);
        assert!(
            !preview.should_render("# ab", soon),
            "a keystroke is not worth a re-parse"
        );
        assert_eq!(preview.shown(), "# a", "the old text is still shown");

        let later = start + PREVIEW_DELAY + Duration::from_millis(50);
        assert!(preview.should_render("# ab", later));
        assert_eq!(preview.shown(), "# ab");
    }

    #[test]
    fn the_delay_counts_from_the_last_render() {
        let mut preview = Preview::new();
        let start = Instant::now();
        assert!(preview.should_render("one", start));

        let mid = start + Duration::from_millis(200);
        assert!(!preview.should_render("two", mid));

        // 350 ms after the first render, though only 150 after the attempt.
        let after = start + Duration::from_millis(350);
        assert!(preview.should_render("two", after));
    }

    #[test]
    fn only_markdown_files_are_previewed() {
        assert!(is_markdown(Path::new("README.md")));
        assert!(is_markdown(Path::new("docs/plan.markdown")));
        assert!(is_markdown(Path::new("x.mdx")));
        assert!(!is_markdown(Path::new("main.rs")));
        assert!(!is_markdown(Path::new("Makefile")));
    }
}
