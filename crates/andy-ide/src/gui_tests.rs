//! Driving the real window in-process.
//!
//! There is no display in the sandbox, and a screenshot would not help anyway:
//! the check has to read the window as a tree of labelled widgets, click them and
//! type into them, then look at what the application did. `egui_kittest` runs the
//! real [`App`] with the real panels, the real dock layout and the real input
//! handling, so a broken frame, a panicking layout or a lost click shows up here.

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::app::App;
use crate::lsp::DiagnosticLine;
use crate::state::Prompt;

/// A window with a project in it.
struct Gui {
    harness: Harness<'static, App>,
    _project: tempfile::TempDir,
    _config: tempfile::TempDir,
}

impl Gui {
    fn new(files: &[(&str, &str)], open: Option<&str>) -> Self {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = project.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }

        let mut app = App::with_config_dir(
            config.path().to_path_buf(),
            Some(project.path().to_path_buf()),
            None,
        );
        if let Some(name) = open {
            app.state.open_file(Path::new(name));
        }

        // A frame a quarter of a second long (the default) would put two clicks
        // 250 ms apart, outside egui double-click window, so the clock here
        // ticks at 60 frames a second like a real window.
        let mut harness = Harness::builder()
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(|ui, app: &mut App| app.draw(ui), app);
        harness.set_size(egui::vec2(1200.0, 800.0));
        harness.run();
        Self {
            harness,
            _project: project,
            _config: config,
        }
    }

    /// Runs frames until the window settles.
    fn settle(&mut self) {
        self.harness.run();
    }

    fn app(&self) -> &App {
        self.harness.state()
    }

    fn app_mut(&mut self) -> &mut App {
        self.harness.state_mut()
    }

    /// The label of the tab that is open, if any.
    fn active_tab(&self) -> Option<PathBuf> {
        let state = &self.app().state;
        let index = state.tabs.active()?;
        Some(state.tabs.items().get(index)?.path.clone())
    }
}

#[test]
fn the_window_has_a_menu_a_project_tree_and_a_status_bar() {
    let gui = Gui::new(&[("a.rs", "fn a() {}\n"), ("notes.txt", "text\n")], None);

    // The menu bar.
    assert!(
        gui.harness.query_by_label("Project").is_some(),
        "no Project menu"
    );
    assert!(gui.harness.query_by_label("View").is_some(), "no View menu");

    // The project tree, read as labels.
    assert!(
        gui.harness.query_by_label("a.rs").is_some(),
        "a.rs is not in the tree"
    );
    assert!(
        gui.harness.query_by_label("notes.txt").is_some(),
        "notes.txt is not in the tree"
    );

    // The status bar says what is going on.
    assert!(
        gui.harness
            .query_by_label_contains("files indexed")
            .is_some(),
        "the index count is not in the status bar"
    );
}

#[test]
fn clicking_a_file_in_the_tree_opens_its_tab() {
    let mut gui = Gui::new(&[("a.rs", "fn a() {}\n"), ("notes.txt", "text\n")], None);
    assert_eq!(gui.app().state.tabs.len(), 0, "nothing is open yet");

    // The tree opens a file on a double click, as other IDEs do.
    gui.harness.get_by_label("notes.txt").click();
    gui.harness.get_by_label("notes.txt").click();
    gui.settle();

    assert_eq!(gui.app().state.tabs.len(), 1, "the click opened nothing");
    assert_eq!(
        gui.active_tab(),
        Some(PathBuf::from("notes.txt")),
        "the wrong file opened"
    );
    // The name is now in three places: the tree, the tab and the status bar.
    assert!(
        gui.harness.query_all_by_label_contains("notes.txt").count() >= 3,
        "the file is not named in the tab strip and the status bar"
    );
}

#[test]
fn ctrl_p_filters_files_and_enter_opens_the_pick() {
    let mut gui = Gui::new(
        &[("a.rs", "fn a() {}\n"), ("notes.txt", "text\n")],
        Some("a.rs"),
    );

    gui.harness
        .key_press_modifiers(egui::Modifiers::CTRL, egui::Key::P);
    gui.settle();
    assert!(gui.app().state.quick_open.open, "Ctrl+P did not open it");

    // The list is filtered by what is typed.
    gui.harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .focus();
    gui.harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .type_text("notes");
    gui.settle();
    assert!(
        gui.harness
            .query_all_by_label_contains("notes.txt")
            .next()
            .is_some(),
        "the pick list does not offer notes.txt"
    );

    gui.harness.key_press(egui::Key::Enter);
    gui.settle();
    assert!(!gui.app().state.quick_open.open, "the picker stayed open");
    assert_eq!(
        gui.active_tab(),
        Some(PathBuf::from("notes.txt")),
        "Enter opened something else"
    );
}

#[test]
fn the_quit_prompt_appears_and_cancelling_it_keeps_the_window() {
    let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
    gui.app_mut().state.prompt = Some(Prompt::Quit);
    gui.settle();

    assert!(
        gui.harness.query_by_label_contains("not saved").is_some(),
        "the prompt is not on screen"
    );
    gui.harness.get_by_label("Cancel").click();
    gui.settle();

    assert_eq!(
        gui.app().state.prompt,
        None,
        "Cancel did not dismiss the prompt"
    );
    assert_eq!(gui.app().state.tabs.len(), 1, "the tab was closed anyway");
}

#[test]
fn a_problem_line_is_clickable_and_leads_to_its_file() {
    let mut gui = Gui::new(&[("a.rs", "one\ntwo\nthree\n")], None);
    gui.app_mut().state.diagnostics.insert(
        PathBuf::from("a.rs"),
        vec![DiagnosticLine {
            line: 2,
            message: "unused variable".to_owned(),
        }],
    );
    gui.settle();

    let line = gui
        .harness
        .query_all_by_label_contains("unused variable")
        .next()
        .expect("the problem is not in the panel");
    line.click();
    gui.settle();

    assert_eq!(
        gui.active_tab(),
        Some(PathBuf::from("a.rs")),
        "no jump to the file"
    );
    let buffer = gui.app().state.active_buffer().expect("a buffer");
    assert_eq!(
        buffer.line_col().0,
        2,
        "the caret is not on the reported line"
    );
}

#[test]
fn a_click_in_the_editor_puts_the_caret_there() {
    let mut gui = Gui::new(&[("a.rs", "one\ntwo\nthree\n")], Some("a.rs"));
    gui.settle();

    // The editor is the large area to the right of the tree, above the problems
    // panel; a click at its top-left corner lands on the first character.
    let pos = egui::pos2(320.0, 60.0);
    gui.harness.event(egui::Event::PointerMoved(pos));
    gui.harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    gui.harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    gui.settle();

    let buffer = gui.app().state.active_buffer().expect("a buffer");
    assert_eq!(
        buffer.line_col(),
        (0, 0),
        "the click did not move the caret to the first line"
    );
}

fn click_at(harness: &mut Harness<'static, App>, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
}

#[test]
fn typing_changes_the_buffer_and_marks_it_modified() {
    let mut gui = Gui::new(&[("a.rs", "one\ntwo\n")], Some("a.rs"));
    click_at(&mut gui.harness, egui::pos2(320.0, 60.0));
    gui.settle();

    gui.harness.event(egui::Event::Text("X".to_owned()));
    gui.settle();

    let buffer = gui.app().state.active_buffer().expect("a buffer");
    assert_eq!(
        buffer.text(),
        "Xone\ntwo\n",
        "the letter did not reach the editor"
    );
    assert!(buffer.is_dirty(), "the buffer does not know it changed");
    assert!(
        gui.harness.query_by_label_contains("modified").is_some(),
        "the status bar does not say the file changed"
    );
}

#[test]
fn ctrl_shift_m_toggles_the_preview_split() {
    let mut gui = Gui::new(
        &[("notes.md", "# A heading\n\nsome body text\n")],
        Some("notes.md"),
    );
    gui.settle();

    // The rendered text is painted, not a widget, so it never reaches the tree:
    // what is checked here is the split itself, and that both modes draw.
    let before = gui.app().state.preview_open;
    gui.harness
        .key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::M);
    gui.settle();
    assert_ne!(
        gui.app().state.preview_open,
        before,
        "Ctrl+Shift+M changed nothing"
    );

    gui.harness
        .key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::M);
    gui.settle();
    assert_eq!(
        gui.app().state.preview_open,
        before,
        "the second Ctrl+Shift+M did not bring the split back"
    );
}

/// The window's cross: the question it puts, and the answer that has to stick.
///
/// `ViewportCommand::Close` is delivered back to the application as a fresh close
/// request. An application that cancels every request while edits are unsaved
/// therefore cancels its own quit, and the question comes back for ever; these
/// checks exist because that is exactly what happened.
mod closing_the_window {
    use super::*;

    /// The commands of one frame, in which the cross was clicked.
    fn frame_with_the_cross(gui: &mut Gui) -> Vec<egui::ViewportCommand> {
        gui.harness.input_mut().viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                events: vec![egui::ViewportEvent::Close],
                ..Default::default()
            },
        );
        gui.harness.step();
        commands(&gui.harness)
    }

    /// The commands of one frame, in which a button of the question was clicked.
    fn frame_with_the_button(gui: &mut Gui, label: &str) -> Vec<egui::ViewportCommand> {
        gui.harness.get_by_label(label).click();
        gui.harness.step();
        commands(&gui.harness)
    }

    fn commands(harness: &Harness<'static, App>) -> Vec<egui::ViewportCommand> {
        harness
            .output()
            .viewport_output
            .values()
            .flat_map(|viewport| viewport.commands.clone())
            .collect()
    }

    fn has(commands: &[egui::ViewportCommand], wanted: fn(&egui::ViewportCommand) -> bool) -> bool {
        commands.iter().any(wanted)
    }

    fn dirty_window() -> Gui {
        let mut gui = Gui::new(&[("a.rs", "one\ntwo\n")], Some("a.rs"));
        click_at(&mut gui.harness, egui::pos2(320.0, 60.0));
        gui.settle();
        gui.harness.event(egui::Event::Text("X".to_owned()));
        gui.settle();
        assert!(
            gui.app().state.has_unsaved(),
            "the window has nothing to ask about"
        );
        gui
    }

    #[test]
    fn the_cross_asks_once_and_the_answer_closes_the_window() {
        let mut gui = dirty_window();

        let asked = frame_with_the_cross(&mut gui);
        // The question is put at the end of that frame; the widgets of the modal
        // need a settled frame before a click can land.
        gui.settle();
        assert_eq!(
            gui.app().state.prompt,
            Some(Prompt::Quit),
            "the cross did not ask"
        );
        assert!(
            has(&asked, |command| matches!(
                command,
                egui::ViewportCommand::CancelClose
            )),
            "the close was not cancelled, the edits would be lost: {asked:?}"
        );

        // "Quit without saving" is a real button on the screen.
        let answered = frame_with_the_button(&mut gui, "Quit without saving");
        assert!(gui.app().state.prompt.is_none(), "the question stayed");
        assert!(
            has(&answered, |command| matches!(
                command,
                egui::ViewportCommand::Close
            )),
            "the window was not told to close: {answered:?}"
        );

        // eframe turns that Close into a close request on the next frame. It is
        // the same decision, not a new question: nothing may cancel it again.
        let again = frame_with_the_cross(&mut gui);
        assert!(
            !has(&again, |command| matches!(
                command,
                egui::ViewportCommand::CancelClose
            )),
            "the window cancelled its own quit: {again:?}"
        );
        assert!(gui.app().state.prompt.is_none(), "the question came back");
    }

    #[test]
    fn cancel_keeps_the_window_and_the_next_click_asks_again() {
        let mut gui = dirty_window();
        frame_with_the_cross(&mut gui);
        gui.settle();

        let answered = frame_with_the_button(&mut gui, "Cancel");
        assert!(gui.app().state.prompt.is_none(), "Cancel left the question");
        assert!(
            !has(&answered, |command| matches!(
                command,
                egui::ViewportCommand::Close
            )),
            "Cancel closed the window: {answered:?}"
        );
        assert!(gui.app().state.has_unsaved(), "the edits were dropped");

        // The request is over; a second click is a new question.
        gui.harness.step();
        frame_with_the_cross(&mut gui);
        assert_eq!(
            gui.app().state.prompt,
            Some(Prompt::Quit),
            "the second click on the cross did not ask"
        );
    }

    #[test]
    fn save_all_and_quit_writes_the_file_and_closes() {
        let mut gui = dirty_window();
        frame_with_the_cross(&mut gui);
        gui.settle();

        let answered = frame_with_the_button(&mut gui, "Save all and quit");

        assert_eq!(
            std::fs::read_to_string(gui._project.path().join("a.rs")).unwrap(),
            "Xone\ntwo\n",
            "the file was not written"
        );
        assert!(!gui.app().state.has_unsaved(), "the buffer is still dirty");
        assert!(
            has(&answered, |command| matches!(
                command,
                egui::ViewportCommand::Close
            )),
            "the window was not told to close: {answered:?}"
        );
    }
}
