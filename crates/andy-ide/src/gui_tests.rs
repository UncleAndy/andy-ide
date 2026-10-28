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
    _config: Option<tempfile::TempDir>,
}

/// Builds the real window on a project and a configuration directory.
///
/// A frame a quarter of a second long (the harness default) would put two clicks
/// 250 ms apart, outside egui's double-click window, so the clock here ticks at
/// 60 frames a second like a real window.
fn window(
    project: &Path,
    config: &Path,
    files: &[(&str, &str)],
    open: Option<&str>,
) -> Harness<'static, App> {
    for (name, text) in files {
        let path = project.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    let mut app = App::with_config_dir(config.to_path_buf(), Some(project.to_path_buf()), None);
    if let Some(name) = open {
        app.state.open_file(Path::new(name));
    }

    let mut harness = Harness::builder()
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(|ui, app: &mut App| app.draw(ui), app);
    harness.set_size(egui::vec2(1200.0, 800.0));
    harness.run();
    harness
}

impl Gui {
    fn new(files: &[(&str, &str)], open: Option<&str>) -> Self {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let harness = window(project.path(), config.path(), files, open);
        Self {
            harness,
            _project: project,
            _config: Some(config),
        }
    }

    /// A window on a configuration directory that an earlier window wrote: what
    /// the next launch of the editor sees.
    fn reopened(config: &Path, files: &[(&str, &str)], open: Option<&str>) -> Self {
        let project = tempfile::tempdir().unwrap();
        let harness = window(project.path(), config, files, open);
        Self {
            harness,
            _project: project,
            _config: None,
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
    let pos = egui::pos2(315.0, 125.0);
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
    click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
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
        click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
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

/// Clicking with Ctrl held asks for the definition at the pointer.
mod navigation_by_mouse {
    use super::*;

    fn ctrl_click_at(harness: &mut Harness<'static, App>, pos: egui::Pos2) {
        // The window layer sends the modifiers before the press, and egui keeps
        // them in the input state for the frame that handles the click.
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::CTRL));
        harness.event(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::CTRL,
            });
        }
        harness.step();
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        harness.step();
    }

    #[test]
    fn ctrl_click_asks_the_server_for_a_definition() {
        // A text file has no server, and that is the point: the click has to
        // reach the navigation code, which says so in the status bar.
        let mut gui = Gui::new(&[("notes.txt", "alpha\nbeta\n")], Some("notes.txt"));
        gui.settle();

        ctrl_click_at(&mut gui.harness, egui::pos2(315.0, 125.0));

        assert_eq!(
            gui.app().state.status,
            "no language server for notes.txt",
            "the Ctrl+Click did not ask for a definition"
        );
    }

    #[test]
    fn a_plain_click_still_only_moves_the_caret() {
        let mut gui = Gui::new(&[("notes.txt", "alpha\nbeta\n")], Some("notes.txt"));
        gui.settle();

        click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
        gui.settle();

        assert_eq!(
            gui.app().state.active_buffer().unwrap().line_col(),
            (0, 0),
            "the caret did not move"
        );
        assert!(
            !gui.app().state.status.contains("language server"),
            "a plain click asked the server: {:?}",
            gui.app().state.status
        );
    }
}

/// The list `Shift+F12` puts on screen.
mod the_reference_list {
    use super::*;
    use crate::ui::results::Hit;

    fn two_hits() -> Vec<Hit> {
        vec![
            Hit::reference(PathBuf::from("a.rs"), 2, 0, "three".to_owned()),
            Hit::reference(PathBuf::from("b.rs"), 1, 2, "beta".to_owned()),
        ]
    }

    #[test]
    fn the_list_shows_the_hits_and_enter_opens_the_selected_one() {
        let mut gui = Gui::new(
            &[("a.rs", "one\ntwo\nthree\n"), ("b.rs", "alpha\nbeta\n")],
            Some("a.rs"),
        );
        gui.settle();
        gui.app_mut().state.results.show_hits(two_hits());
        gui.settle();

        assert!(
            gui.harness.query_by_label_contains("a.rs:3").is_some(),
            "the first hit is not in the list"
        );
        assert!(
            gui.harness.query_by_label_contains("b.rs:2").is_some(),
            "the second hit is not in the list"
        );

        gui.harness.key_press(egui::Key::ArrowDown);
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();

        assert_eq!(
            gui.active_tab(),
            Some(PathBuf::from("b.rs")),
            "Enter did not open the second hit"
        );
        assert_eq!(
            gui.app().state.active_buffer().unwrap().line_col().0,
            1,
            "the caret is not on the line of the hit"
        );
        assert!(!gui.app().state.results.open, "the list stayed open");
    }

    #[test]
    fn clicking_a_hit_opens_it() {
        let mut gui = Gui::new(
            &[("a.rs", "one\ntwo\nthree\n"), ("b.rs", "alpha\nbeta\n")],
            Some("a.rs"),
        );
        gui.settle();
        gui.app_mut().state.results.show_hits(two_hits());
        gui.settle();

        gui.harness.get_by_label_contains("b.rs:2").click();
        gui.settle();

        assert_eq!(gui.active_tab(), Some(PathBuf::from("b.rs")));
        assert!(!gui.app().state.results.open);
    }

    #[test]
    fn escape_closes_the_list_without_opening_anything() {
        let mut gui = Gui::new(
            &[("a.rs", "one\ntwo\nthree\n"), ("b.rs", "alpha\nbeta\n")],
            Some("a.rs"),
        );
        gui.settle();
        gui.app_mut().state.results.show_hits(two_hits());
        gui.settle();

        gui.harness.key_press(egui::Key::Escape);
        gui.settle();

        assert!(!gui.app().state.results.open, "Escape left the list open");
        assert_eq!(
            gui.active_tab(),
            Some(PathBuf::from("a.rs")),
            "Escape opened something"
        );
    }
}

/// What happens after a jump: the code has to be on screen, and a definition
/// outside the project has to be readable.
mod jumps {
    use super::*;
    use crate::lsp::Outcome;
    use lsp_types::{Location, Position, Range};

    fn caret_on_screen(gui: &Gui) -> (egui::Pos2, egui::Rect) {
        gui.harness
            .ctx
            .data(|data| data.get_temp(egui::Id::new("andy-ide-caret-on-screen")))
            .expect("the editor did not publish where it drew the caret")
    }

    fn long_file(lines: usize) -> String {
        (0..lines).map(|n| format!("line {n}\n")).collect()
    }

    /// The caret has to be inside the view, with room around it: a caret on the
    /// very last row is easy to miss and the edge of the window can clip it.
    fn assert_in_sight(caret: egui::Pos2, view: egui::Rect) {
        let margin = view.height() / 6.0;
        assert!(
            caret.y > view.top() + margin && caret.y < view.bottom() - margin,
            "the caret at {caret:?} is on the edge of {view:?}: the jump did not centre it"
        );
    }

    #[test]
    fn a_jump_far_away_brings_the_caret_into_sight() {
        let mut gui = Gui::new(&[("a.rs", &long_file(400))], Some("a.rs"));
        gui.settle();
        let (before, _) = caret_on_screen(&gui);
        assert!(before.y < 160.0, "the caret started at {before:?}");

        // A definition on line 300: nothing was typed, nothing was clicked, the
        // application moved the caret.
        let uri = ide_lsp::path_to_uri(&gui._project.path().join("a.rs")).unwrap();
        gui.app_mut().state.apply_outcome(Outcome::Definitions {
            from: PathBuf::from("a.rs"),
            locations: vec![Location {
                uri,
                range: Range {
                    start: Position {
                        line: 300,
                        character: 0,
                    },
                    end: Position {
                        line: 300,
                        character: 4,
                    },
                },
            }],
        });
        gui.settle();

        let (caret, editor) = caret_on_screen(&gui);
        assert_eq!(
            gui.app().state.active_buffer().unwrap().line_col().0,
            300,
            "the caret did not move"
        );
        assert!(
            caret.y >= editor.top() && caret.y <= editor.bottom(),
            "the caret at {caret:?} is outside the editor {editor:?}: the view did not scroll"
        );
        assert_in_sight(caret, editor);
    }

    #[test]
    fn a_jump_into_an_external_file_scrolls_it_into_sight() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let outside = tempfile::tempdir().unwrap();
        let far = outside.path().join("far.rs");
        std::fs::write(&far, long_file(400)).unwrap();

        gui.app_mut().state.apply_outcome(Outcome::Definitions {
            from: PathBuf::from("a.rs"),
            locations: vec![Location {
                uri: ide_lsp::path_to_uri(&far).unwrap(),
                range: Range {
                    start: Position {
                        line: 300,
                        character: 2,
                    },
                    end: Position {
                        line: 300,
                        character: 6,
                    },
                },
            }],
        });
        gui.settle();

        assert_eq!(
            gui.active_tab(),
            Some(far),
            "the external file did not open"
        );
        assert_eq!(
            gui.app().state.active_buffer().unwrap().line_col(),
            (300, 2),
            "the caret is not on the definition"
        );
        let (caret, editor) = caret_on_screen(&gui);
        assert_in_sight(caret, editor);
    }

    #[test]
    fn a_definition_outside_the_project_opens_the_file() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let outside = tempfile::tempdir().unwrap();
        let far = outside.path().join("far.rs");
        std::fs::write(&far, "one\ntwo\nfn far () {}\nfour\n").unwrap();

        gui.app_mut().state.apply_outcome(Outcome::Definitions {
            from: PathBuf::from("a.rs"),
            locations: vec![Location {
                uri: ide_lsp::path_to_uri(&far).unwrap(),
                range: Range {
                    start: Position {
                        line: 2,
                        character: 3,
                    },
                    end: Position {
                        line: 2,
                        character: 9,
                    },
                },
            }],
        });
        gui.settle();

        assert_eq!(
            gui.active_tab(),
            Some(far.clone()),
            "the definition outside the project did not open"
        );
        let buffer = gui.app().state.active_buffer().expect("a buffer");
        assert_eq!(buffer.line_text(2), "fn far () {}", "the code is not there");
        assert_eq!(buffer.line_col(), (2, 3), "the caret is not on the symbol");
        assert!(
            gui.app().state.status.contains("outside the project"),
            "the status does not say the file is external: {:?}",
            gui.app().state.status
        );

        // The tab is named after the file, and the editor really draws it.
        assert!(
            gui.harness.query_by_label("far.rs").is_some(),
            "the tab or the tree does not name far.rs"
        );
    }

    #[test]
    fn an_external_file_saves_where_it_lives() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let outside = tempfile::tempdir().unwrap();
        let far = outside.path().join("far.rs");
        std::fs::write(&far, "one\n").unwrap();

        gui.app_mut().state.apply_outcome(Outcome::Definitions {
            from: PathBuf::from("a.rs"),
            locations: vec![Location {
                uri: ide_lsp::path_to_uri(&far).unwrap(),
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: 1,
                    },
                },
            }],
        });
        gui.settle();

        // Type one letter and save with the keyboard.
        click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
        gui.settle();
        gui.harness.event(egui::Event::Text("X".to_owned()));
        gui.settle();
        gui.harness
            .key_press_modifiers(egui::Modifiers::CTRL, egui::Key::S);
        gui.settle();

        assert_eq!(
            std::fs::read_to_string(&far).unwrap(),
            "Xone\n",
            "the external file was not written"
        );
        assert!(
            !gui._project.path().join("far.rs").exists(),
            "a copy was written into the project"
        );
    }
}

/// The caret blinks so it can be found in the text; it is also the only way to
/// tell a caret that sits between two identical glyphs from no caret at all.
mod blink {
    use super::*;

    /// Reads whether the view lit the caret in the last frame it drew.
    fn caret_lit(gui: &Gui) -> bool {
        gui.harness
            .ctx
            .data(|data| data.get_temp::<bool>(egui::Id::new("andy-ide-caret-lit")))
            .expect("the editor did not publish whether the caret was lit")
    }

    /// Steps the harness for `seconds` of simulated time, 60 frames a second.
    fn advance(gui: &mut Gui, seconds: f32) {
        for _ in 0..(seconds * 60.0).round() as u32 {
            gui.harness.step();
        }
    }

    #[test]
    fn the_caret_blinks_and_comes_back() {
        let mut gui = Gui::new(&[("a.rs", "hello\n")], Some("a.rs"));
        click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
        gui.settle();
        assert!(caret_lit(&gui), "the caret is dark right after a click");

        advance(&mut gui, 0.7);
        assert!(
            !caret_lit(&gui),
            "the caret never goes dark: it does not blink"
        );

        advance(&mut gui, 0.6);
        assert!(
            caret_lit(&gui),
            "the caret never comes back after going dark"
        );
    }

    #[test]
    fn typing_lights_the_caret_again() {
        let mut gui = Gui::new(&[("a.rs", "hello\n")], Some("a.rs"));
        click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
        gui.settle();
        advance(&mut gui, 0.7);
        assert!(
            !caret_lit(&gui),
            "the caret is still lit after half a second"
        );

        // A key press has to make the caret visible at once, wherever the blink
        // was: otherwise a letter lands while the caret is dark and the user
        // cannot see where it went.
        gui.harness.event(egui::Event::Text("x".to_owned()));
        gui.harness.step();
        assert!(caret_lit(&gui), "the caret stayed dark after typing");
    }

    #[test]
    fn a_dark_caret_is_not_drawn() {
        let mut gui = Gui::new(&[("a.rs", "hello\n")], Some("a.rs"));
        click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
        gui.settle();
        advance(&mut gui, 0.7);
        assert!(!caret_lit(&gui));
        // The view still has to say where the caret is: the scroll follow reads
        // that, and a jump needs the position even while the caret is dark.
        let published = gui.harness.ctx.data(|data| {
            data.get_temp::<(egui::Pos2, egui::Rect)>(egui::Id::new("andy-ide-caret-on-screen"))
        });
        assert!(
            published.is_some(),
            "a dark caret is not drawn and its position is not published either"
        );
    }
}

/// A blinking caret in an idle window only works if the view asks for the next
/// frame itself: egui repaints on input or on request, and a caret that waits for
/// a key press would freeze in whatever half of the blink it was in.
mod blink_repaints {
    use super::*;

    #[test]
    fn a_still_caret_asks_for_the_next_frame() {
        let mut gui = Gui::new(&[("a.rs", "hello\n")], Some("a.rs"));
        click_at(&mut gui.harness, egui::pos2(315.0, 125.0));
        gui.settle();
        // Nothing moved in the last frame, so only the blink can want a repaint.
        gui.harness.step();
        let delay = gui
            .harness
            .output()
            .viewport_output
            .values()
            .map(|viewport| viewport.repaint_delay)
            .min()
            .expect("a viewport");
        assert!(
            delay > std::time::Duration::ZERO && delay <= std::time::Duration::from_millis(600),
            "an idle caret did not ask for a repaint in time to blink: {delay:?}, causes {:?}",
            gui.harness.ctx.repaint_causes()
        );
    }
}

/// The outline `Ctrl+F12` puts on screen.
mod the_symbol_list {
    use super::*;
    use crate::ui::results::SymbolKind;

    fn outline() -> Vec<crate::ui::results::Hit> {
        use crate::ui::results::Hit;
        vec![
            Hit::symbol("a.rs".into(), 0, 4, "App".to_owned(), 0, SymbolKind::Type),
            Hit::symbol(
                "a.rs".into(),
                4,
                8,
                "draw".to_owned(),
                1,
                SymbolKind::Method,
            ),
            Hit::symbol(
                "a.rs".into(),
                9,
                0,
                "run".to_owned(),
                0,
                SymbolKind::Function,
            ),
        ]
    }

    #[test]
    fn the_outline_shows_the_tree_and_enter_jumps_to_a_symbol() {
        let text: String = (0..10)
            .map(|n| format!("abcdefghijklmnopqrst{n}\n"))
            .collect();
        let mut gui = Gui::new(&[("a.rs", &text)], Some("a.rs"));
        gui.settle();
        gui.app_mut()
            .state
            .results
            .show_symbols("Symbols in a.rs: 3".to_owned(), outline());
        gui.settle();

        assert!(
            gui.harness
                .query_by_label_contains("Symbols in a.rs: 3")
                .is_some(),
            "the outline has no title"
        );
        assert!(
            gui.harness.query_by_label_contains("type App").is_some(),
            "the first symbol is not in the list"
        );
        assert!(
            gui.harness
                .query_by_label_contains("  method draw")
                .is_some(),
            "a nested symbol is not indented under its parent"
        );

        gui.harness.key_press(egui::Key::ArrowDown);
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();

        let buffer = gui.app().state.active_buffer().expect("a buffer");
        assert_eq!(buffer.line_col().0, 4, "Enter did not jump to the method");
        assert_eq!(buffer.line_col().1, 8, "the caret is not on the name");
        assert!(!gui.app().state.results.open, "the outline stayed open");
    }
}

/// The F12 chords, against the real input state: `consume_key` ignores a Shift it
/// was not asked about, so the order the chords are read in decides who wins.
mod f12_chords {

    /// Reads the chords in the order `App::handle_keys` reads them, against the
    /// real input state, and says which one fired.
    fn fired(key: egui::Key, modifiers: egui::Modifiers) -> Vec<&'static str> {
        let mut fired = Vec::new();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let mut output = egui::Context::default().run_ui(input, |ui| {
            let project = ui.ctx().input_mut(|input| {
                input.consume_key(
                    egui::Modifiers::CTRL
                        .plus(egui::Modifiers::SHIFT)
                        .plus(egui::Modifiers::ALT),
                    egui::Key::N,
                )
            });
            if project {
                fired.push("project symbols");
            }
            let file = ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F12));
            if file {
                fired.push("file symbols");
            }
            let references = ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::F12));
            if references {
                fired.push("references");
            }
            let definition = ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F12));
            if definition {
                fired.push("definition");
            }
        });
        // The frame produced a font texture that nobody renders here; dropping it
        // without saying so panics in `epaint`.
        output.textures_delta.clear();
        fired
    }

    #[test]
    fn each_chord_asks_for_exactly_one_thing() {
        assert_eq!(
            fired(
                egui::Key::N,
                egui::Modifiers::CTRL
                    .plus(egui::Modifiers::SHIFT)
                    .plus(egui::Modifiers::ALT)
            ),
            vec!["project symbols"],
            "Ctrl+Alt+Shift+N was taken by a less specific chord"
        );
        assert_eq!(
            fired(egui::Key::F12, egui::Modifiers::CTRL),
            vec!["file symbols"]
        );
        // Shift+F12 before the bare key: `consume_key` ignores a Shift it was
        // not asked about, so plain F12 first would swallow the reference list.
        assert_eq!(
            fired(egui::Key::F12, egui::Modifiers::SHIFT),
            vec!["references"]
        );
        assert_eq!(
            fired(egui::Key::F12, egui::Modifiers::NONE),
            vec!["definition"]
        );
    }
}

/// The project-symbol search `Ctrl+Alt+Shift+N` opens.
mod the_symbol_search {
    use super::*;
    use crate::ui::results::{Hit, SymbolKind};

    fn text() -> String {
        (0..10)
            .map(|n| format!("abcdefghijklmnopqrst{n}\n"))
            .collect()
    }

    fn hits() -> Vec<Hit> {
        vec![
            Hit::symbol(
                "a.rs".into(),
                2,
                4,
                "draw".to_owned(),
                0,
                SymbolKind::Function,
            ),
            Hit::symbol(
                "a.rs".into(),
                7,
                6,
                "run".to_owned(),
                0,
                SymbolKind::Function,
            ),
        ]
    }

    #[test]
    fn the_field_lists_what_the_server_found_and_enter_jumps() {
        let text = text();
        let mut gui = Gui::new(&[("a.rs", &text)], Some("a.rs"));
        gui.settle();

        gui.app_mut().state.symbols.open();
        gui.app_mut().state.symbols.query = "r".to_owned();
        gui.app_mut().state.symbols.apply("r", hits());
        gui.settle();

        assert!(
            gui.harness.query_by_label_contains("fn draw").is_some(),
            "the first match is not in the list"
        );
        assert!(
            gui.harness.query_by_label_contains("fn run").is_some(),
            "the second match is not in the list"
        );

        gui.harness.key_press(egui::Key::ArrowDown);
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();

        let buffer = gui.app().state.active_buffer().expect("a buffer");
        assert_eq!(
            buffer.line_col(),
            (7, 6),
            "Enter did not open the second match"
        );
        assert!(!gui.app().state.symbols.open, "the field stayed open");
    }

    #[test]
    fn escape_closes_the_field_and_typing_reaches_the_query() {
        let text = text();
        let mut gui = Gui::new(&[("a.rs", &text)], Some("a.rs"));
        gui.settle();

        gui.app_mut().state.symbols.open();
        gui.settle();
        assert!(
            gui.harness.query_by_label_contains("type a name").is_some(),
            "the field does not say what to do"
        );

        gui.harness.event(egui::Event::Text("run".to_owned()));
        gui.settle();
        assert_eq!(
            gui.app().state.symbols.query,
            "run",
            "typing did not reach the query"
        );

        gui.harness.key_press(egui::Key::Escape);
        gui.settle();
        assert!(!gui.app().state.symbols.open, "Escape left the field open");
    }
}

/// The zoom of the interface, and whether it is still there after a restart.
mod the_zoom {
    use super::*;
    use crate::state::{DEFAULT_FONT_SIZE, ZOOM_SAVE_DELAY};

    fn config_text(config: &Path) -> String {
        std::fs::read_to_string(config.join("config.toml")).unwrap_or_default()
    }

    /// Steps the window for a while, the way a real window ticks after a key.
    fn run_for(gui: &mut Gui, seconds: f64) {
        let frames = (seconds * 60.0).ceil() as usize;
        for _ in 0..frames {
            gui.harness.step();
        }
    }

    #[test]
    fn ctrl_plus_makes_everything_larger_and_clearly_so() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let before = gui.harness.output().pixels_per_point;

        gui.harness
            .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
        run_for(&mut gui, 1.0 / 60.0);

        let after = gui.harness.output().pixels_per_point;
        assert!(
            after > before,
            "Ctrl+Plus did not enlarge anything: {before} -> {after}"
        );
        assert!(
            (gui.app().state.config.font_size - DEFAULT_FONT_SIZE * 1.1).abs() < 0.01,
            "the zoom was not read back from the window: {}",
            gui.app().state.config.font_size
        );
    }

    #[test]
    fn the_zoom_is_written_once_the_keys_go_quiet() {
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui::reopened(config.path(), &[("a.rs", "fn a() {}\n")], Some("a.rs"));

        gui.harness
            .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
        run_for(&mut gui, 1.0 / 60.0);
        assert!(
            !config_text(config.path()).contains("font_size"),
            "the config was written before the keys went quiet"
        );

        run_for(&mut gui, ZOOM_SAVE_DELAY + 0.1);
        assert!(
            config_text(config.path()).contains("font_size = 15.4"),
            "the zoom was not written: {}",
            config_text(config.path())
        );
    }

    #[test]
    fn the_zoom_comes_back_on_the_next_launch() {
        let config = tempfile::tempdir().unwrap();
        let saved = {
            let mut gui = Gui::reopened(config.path(), &[("a.rs", "fn a() {}\n")], Some("a.rs"));
            gui.harness
                .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
            gui.harness
                .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
            run_for(&mut gui, ZOOM_SAVE_DELAY + 0.3);
            gui.harness.output().pixels_per_point
        };

        let gui = Gui::reopened(config.path(), &[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let now = gui.harness.output().pixels_per_point;
        assert!(
            (now - saved).abs() < 0.001,
            "the next launch opened at {now} instead of {saved}"
        );
        assert!(
            (gui.app().state.config.font_size - DEFAULT_FONT_SIZE * 1.2).abs() < 0.01,
            "the remembered size is {} instead of {}",
            gui.app().state.config.font_size,
            DEFAULT_FONT_SIZE * 1.2
        );
    }

    #[test]
    fn ctrl_0_puts_the_zoom_back_and_remembers_that() {
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui::reopened(config.path(), &[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let original = gui.harness.output().pixels_per_point;

        gui.harness
            .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
        gui.harness
            .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
        run_for(&mut gui, ZOOM_SAVE_DELAY + 0.1);
        assert!(config_text(config.path()).contains("font_size = 16.8"));

        gui.harness
            .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num0);
        run_for(&mut gui, ZOOM_SAVE_DELAY + 0.1);

        assert!(
            (gui.harness.output().pixels_per_point - original).abs() < 0.001,
            "Ctrl+0 did not put the zoom back"
        );
        assert!(
            config_text(config.path()).contains("font_size = 14"),
            "the reset was not written: {}",
            config_text(config.path())
        );
    }
}

/// A zoom that is still waiting for its delay when the window closes would be
/// lost: no frame comes to write it.
mod a_zoom_written_on_the_way_out {
    use super::*;

    fn config_text(config: &Path) -> String {
        std::fs::read_to_string(config.join("config.toml")).unwrap_or_default()
    }

    #[test]
    fn closing_the_window_writes_a_zoom_that_was_still_waiting() {
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui::reopened(config.path(), &[], None);

        gui.harness
            .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
        gui.harness.step();
        gui.harness.step();
        assert!(
            !config_text(config.path()).contains("font_size"),
            "the delay was skipped, so this test proves nothing"
        );

        gui.harness.input_mut().viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                events: vec![egui::ViewportEvent::Close],
                ..Default::default()
            },
        );
        gui.harness.step();

        assert!(
            config_text(config.path()).contains("font_size = 15.4"),
            "the zoom died with the window: {}",
            config_text(config.path())
        );
    }
}

/// A maximized window has to come back maximized.
mod the_maximized_window {
    use super::*;

    fn config_text(config: &Path) -> String {
        std::fs::read_to_string(config.join("config.toml")).unwrap_or_default()
    }

    /// Says what the window manager says about the window.
    fn maximized(gui: &mut Gui, maximized: bool) {
        gui.harness.input_mut().viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                maximized: Some(maximized),
                ..Default::default()
            },
        );
        gui.harness.step();
    }

    #[test]
    fn maximizing_the_window_is_remembered_at_once() {
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui::reopened(config.path(), &[], None);
        assert!(!gui.app().state.config.window_maximized);

        maximized(&mut gui, true);

        assert!(
            gui.app().state.config.window_maximized,
            "the window did not notice it was maximized"
        );
        assert!(
            config_text(config.path()).contains("window_maximized = true"),
            "the state was not written: {}",
            config_text(config.path())
        );
    }

    #[test]
    fn restoring_the_window_is_remembered_too() {
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui::reopened(config.path(), &[], None);
        maximized(&mut gui, true);
        maximized(&mut gui, false);

        assert!(!gui.app().state.config.window_maximized);
        assert!(
            config_text(config.path()).contains("window_maximized = false"),
            "restoring was not written: {}",
            config_text(config.path())
        );
    }

    #[test]
    fn a_backend_that_does_not_say_leaves_the_value_alone() {
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui::reopened(config.path(), &[], None);
        maximized(&mut gui, true);

        // `maximized: None` is what a backend that cannot tell reports.
        gui.harness.input_mut().viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                maximized: None,
                ..Default::default()
            },
        );
        gui.harness.step();

        assert!(
            gui.app().state.config.window_maximized,
            "silence from the window manager was taken for a restored window"
        );
    }
}

/// The hint that follows the pointer.
mod the_hint {
    use super::*;
    use crate::lsp::Outcome;
    use crate::ui::hover::HOVER_DELAY;

    /// A window with a plain-text file: no language server is registered for it,
    /// so the test drives the answer itself instead of waiting for rust-analyzer.
    fn window() -> Gui {
        Gui::new(
            &[("notes.txt", "alpha beta\ngamma delta\n")],
            Some("notes.txt"),
        )
    }

    fn rest_on_text(gui: &mut Gui) {
        gui.harness.hover_at(egui::pos2(315.0, 125.0));
        gui.harness.step();
    }

    fn run_for(gui: &mut Gui, seconds: f64) {
        let frames = (seconds * 60.0).ceil() as usize;
        for _ in 0..frames {
            gui.harness.step();
        }
    }

    fn hint_on_screen(gui: &Gui, text: &str) -> bool {
        gui.harness.query_by_label(text).is_some()
    }

    /// Answers the question the window has asked about the spot it is on.
    fn answer(gui: &mut Gui, text: &str) {
        let spot = gui
            .app()
            .state
            .hover_spot()
            .expect("the pointer is not on the text");
        gui.app_mut().state.apply_outcome(Outcome::Hover {
            path: spot.path,
            line: spot.line,
            character: spot.character,
            text: Some(text.to_owned()),
        });
        gui.settle();
    }

    #[test]
    fn nothing_is_shown_before_the_pointer_rests() {
        let mut gui = window();
        rest_on_text(&mut gui);
        answer(&mut gui, "```text\nalpha\n```");
        assert!(
            gui.app().state.hover.text().is_none(),
            "a hint was taken for a pointer that only just arrived"
        );
    }

    #[test]
    fn a_resting_pointer_shows_what_the_server_said() {
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);

        answer(&mut gui, "```text\nalpha\n```\n---\n\nThe first letter.");

        assert!(
            hint_on_screen(&gui, "alpha"),
            "the signature is not on screen"
        );
        assert!(
            hint_on_screen(&gui, "The first letter."),
            "the documentation is not on screen"
        );
    }

    #[test]
    fn the_hint_goes_away_when_the_pointer_leaves() {
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\nalpha\n```");
        assert!(hint_on_screen(&gui, "alpha"));

        gui.harness.remove_cursor();
        gui.harness.step();

        assert!(
            !hint_on_screen(&gui, "alpha"),
            "the hint stayed after the pointer left"
        );
    }

    #[test]
    fn a_modal_takes_the_hint_away() {
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\nalpha\n```");
        assert!(hint_on_screen(&gui, "alpha"));

        gui.app_mut().state.results.open = true;
        gui.harness.step();

        assert!(
            !hint_on_screen(&gui, "alpha"),
            "the hint hangs over a modal"
        );
    }
}
