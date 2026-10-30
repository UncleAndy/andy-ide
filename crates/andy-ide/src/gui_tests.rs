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
use crate::ui::Tab;

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
fn the_hint_stays_in_the_text_and_never_over_the_panel_below() {
    // The editor allocates a rect as tall as the whole document, so a pointer over the
    // agent panel below is, geometrically, "inside" the editor's rect — and asking about
    // the text there returned a symbol from a line that is not even on screen. The hint
    // belongs to the visible text: below the code there is no code.
    let text: String = (0..400).map(|line| format!("use thing{line};\n")).collect();
    let mut gui = Gui::new(&[("a.rs", &text)], Some("a.rs"));
    gui.settle();
    assert!(
        gui.app().state.editor_drawn,
        "the editor is not on screen: the check would pass for the wrong reason"
    );

    let window = gui.harness.ctx.viewport_rect();
    let over_panel = egui::pos2(window.center().x, window.top() + window.height() * 0.9);
    gui.harness.event(egui::Event::PointerMoved(over_panel));
    gui.settle();

    let spot = gui.app().state.hover_spot();
    assert!(
        spot.is_none(),
        "the pointer is over the panel below, not over the code, but the hint is about \
         {:?}",
        spot.map(|spot| (spot.line, spot.character))
    );
}

#[test]
fn a_click_in_the_editor_puts_the_caret_there() {
    let mut gui = Gui::new(&[("a.rs", "one\ntwo\nthree\n")], Some("a.rs"));
    gui.settle();

    // The editor is the large area to the right of the tree; the click is aimed
    // at the first character, wherever the rows above the text put it.
    // (315, 100) is on the first line of this window: the rows above the text
    // (tab strip, the Preview row when it is drawn) decide where it is.
    let pos = egui::pos2(315.0, 100.0);
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
    // (315, 100) is on the first line of this window: the rows above the text
    // (tab strip, the Preview row when it is drawn) decide where it is.
    let pos = egui::pos2(315.0, 100.0);
    click_at(&mut gui.harness, pos);
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
fn ctrl_shift_m_toggles_the_preview() {
    let mut gui = Gui::new(
        &[("notes.md", "# A heading\n\nsome body text\n")],
        Some("notes.md"),
    );
    gui.settle();

    // The rendered text is painted, not a widget, so it never reaches the tree: what is
    // checked here is the switch itself, and that both modes draw.
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
        "the second Ctrl+Shift+M did not bring the preview back"
    );
}

#[test]
fn the_preview_takes_the_whole_editing_room() {
    // The preview stands where the source was, across the whole of the editing room: beside
    // it, every line would appear twice and neither would have the width to be read in. The
    // heading of the preview is therefore painted near the left edge of the window — in a
    // split it sat at the middle — and the source is not painted at all.
    let long = "words that fill the width of the room and then some more of them to be sure";
    let mut gui = Gui::new(
        &[(
            "notes.md",
            &format!("# A heading\n\n{long} {long} {long}\n"),
        )],
        Some("notes.md"),
    );
    gui.settle();
    let width = gui.harness.ctx.viewport_rect().width();

    assert!(
        painted_text(&gui, "# A heading"),
        "the source is not painted before the preview is switched on"
    );

    gui.app_mut().state.preview_open = true;
    gui.settle();

    let heading = painted_at(&gui, "A heading").expect("the preview does not draw its heading");
    assert!(
        heading < width * 0.4,
        "the preview starts at {heading:.0} of {width:.0}: it is drawn beside the source, not in its place"
    );
    assert!(
        !painted_text(&gui, "# A heading"),
        "the source is still painted under the preview"
    );
    let right = painted_right(&gui).expect("nothing is painted at all");
    assert!(
        right > width * 0.6,
        "the preview stops at {right:.0} of {width:.0}: it does not use the room it has"
    );
}

#[test]
fn the_preview_does_not_repeat_the_word_preview() {
    // The switch above the page is already lit and says whose page it is: the page itself
    // says the word once, and once only.
    let mut gui = Gui::new(
        &[("notes.md", "# A heading\n\nsome body text\n")],
        Some("notes.md"),
    );
    gui.settle();
    gui.app_mut().state.preview_open = true;
    gui.settle();

    let painted = gui
        .harness
        .output()
        .shapes
        .iter()
        .filter(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.job.text == "Preview")
        })
        .count();
    assert_eq!(painted, 1, "the word Preview is painted {painted} times");
}

#[test]
fn a_table_in_the_preview_is_drawn_like_a_table_in_an_answer() {
    // A table in a file is a table in an answer: the same module draws it, so its columns are
    // measured and it stays inside the room rather than running off the edge — the renderer
    // lays a cell out by the room that cell had the frame before, which is how columns come
    // out narrow and the table wide.
    let sentence =
        "Every cell here is a phrase long enough to need wrapping somewhere in the line, twice";
    let md = format!(
        "| what | why | how |\n| --- | --- | --- |\n| {s} | {s} | {s} |\n",
        s = sentence
    );
    let mut gui = Gui::new(&[("notes.md", &md)], Some("notes.md"));
    gui.settle();
    gui.app_mut().state.preview_open = true;
    gui.settle();

    let width = gui.harness.ctx.viewport_rect().width();
    let mut cells = 0;
    let mut widest: f32 = 0.0;
    let mut columns: Vec<f32> = Vec::new();
    for clipped in &gui.harness.output().shapes {
        if let egui::Shape::Text(text) = &clipped.shape
            && text.galley.job.text.contains("Every cell")
        {
            cells += 1;
            columns.push(text.pos.x);
            widest = widest.max(text.pos.x + text.galley.size().x);
        }
    }
    assert!(cells >= 3, "the cells of the table are not drawn: {cells}");
    assert!(
        widest <= width,
        "the table runs past the window: {widest:.1} of a window {width:.1} wide"
    );
    columns.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(
        columns.len() >= 3 && columns[2] - columns[0] > 100.0,
        "the columns are not laid out: {columns:?}"
    );
}

/// Where a piece of painted text starts, if it is on screen.
fn painted_at(gui: &Gui, needle: &str) -> Option<f32> {
    gui.harness.output().shapes.iter().find_map(|clipped| {
        if let egui::Shape::Text(text) = &clipped.shape
            && text.galley.job.text == needle
        {
            return Some(text.pos.x);
        }
        None
    })
}

/// Whether some painted text says exactly this.
fn painted_text(gui: &Gui, needle: &str) -> bool {
    gui.harness.output().shapes.iter().any(|clipped| {
        matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.job.text == needle)
    })
}

/// How far to the right any painted text reaches.
fn painted_right(gui: &Gui) -> Option<f32> {
    gui.harness
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.pos.x + text.galley.size().x),
            _ => None,
        })
        .fold(None, |best: Option<f32>, right| {
            Some(best.map_or(right, |best| best.max(right)))
        })
}

#[test]
fn the_preview_switch_is_only_for_markdown_files() {
    // A file that cannot be previewed is not asked whether it wants one: the switch
    // is drawn for Markdown and the row is not drawn at all for anything else, so
    // the height of the row belongs to the text.
    let mut gui = Gui::new(&[("notes.md", "# A heading\n")], Some("notes.md"));
    gui.settle();
    assert!(
        gui.harness.query_by_label("Preview").is_some(),
        "the Markdown file has no Preview switch"
    );

    let mut gui = Gui::new(&[("main.rs", "fn main() {}\n")], Some("main.rs"));
    gui.settle();
    assert!(
        gui.harness.query_by_label("Preview").is_none(),
        "the plain file still offers a Preview switch"
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
        // (315, 100) is on the first line of this window: the rows above the text
        // (tab strip, the Preview row when it is drawn) decide where it is.
        let pos = egui::pos2(315.0, 100.0);
        click_at(&mut gui.harness, pos);
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

        // (315, 100) is on the first line of this window: the rows above the text
        // (tab strip, the Preview row when it is drawn) decide where it is.
        let pos = egui::pos2(315.0, 100.0);
        ctrl_click_at(&mut gui.harness, pos);

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

        // (315, 100) is on the first line of this window: the rows above the text
        // (tab strip, the Preview row when it is drawn) decide where it is.
        let pos = egui::pos2(315.0, 100.0);
        click_at(&mut gui.harness, pos);
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
        // The first line of the external file sits higher than the first line of the
        // first tab: the click finds it at 52 rather than 80.
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

        // Type one letter and save with the keyboard. The one line of the external
        // file sits higher than the first tab's: the click finds it at 52.
        let pos = egui::pos2(315.0, 52.0);
        click_at(&mut gui.harness, pos);
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
        // (315, 100) is on the first line of this window: the rows above the text
        // (tab strip, the Preview row when it is drawn) decide where it is.
        let pos = egui::pos2(315.0, 100.0);
        click_at(&mut gui.harness, pos);
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
        // (315, 100) is on the first line of this window: the rows above the text
        // (tab strip, the Preview row when it is drawn) decide where it is.
        let pos = egui::pos2(315.0, 100.0);
        click_at(&mut gui.harness, pos);
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
        // (315, 100) is on the first line of this window: the rows above the text
        // (tab strip, the Preview row when it is drawn) decide where it is.
        let pos = egui::pos2(315.0, 100.0);
        click_at(&mut gui.harness, pos);
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
        // (315, 100) is on the first line of this window: the rows above the text
        // (tab strip, the Preview row when it is drawn) decide where it is.
        let pos = egui::pos2(315.0, 100.0);
        click_at(&mut gui.harness, pos);
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
    use crate::ui::hover::{HOVER_DELAY, HOVER_LINGER};

    /// A window with a plain-text file: no language server is registered for it,
    /// so the test drives the answer itself instead of waiting for rust-analyzer.
    fn window() -> Gui {
        Gui::new(
            &[("notes.txt", "alpha beta\ngamma delta\n")],
            Some("notes.txt"),
        )
    }

    fn rest_on_text(gui: &mut Gui) {
        // Only some text has to be under the pointer: the line it is on does not
        // matter to these tests. (315, 80) is not over the text of this window —
        // it sits on the row the Preview switch used to take.)
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
    fn switched_off_hints_do_not_follow_the_pointer() {
        // With the switch off, a pointer that rests on a symbol is not watched at
        // all: the hint that was on screen goes when the switch goes, and an answer
        // about the next rested-upon spot never reaches a hint.
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\nalpha\n```\n---\n\nThe first letter.");
        assert!(
            hint_on_screen(&gui, "The first letter."),
            "the hint did not show before the switch went off"
        );

        gui.harness.get_by_label("View").click();
        gui.settle();
        gui.harness.get_by_label("Show hints").click();
        gui.settle();

        assert!(
            !hint_on_screen(&gui, "The first letter."),
            "the hint stayed after the switch went off"
        );
        assert!(
            gui.app().state.hover.text().is_none(),
            "the switch left an answer behind"
        );

        // A pointer that rests again is not watched: answering about it shows nothing.
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\nalpha\n```\n---\n\nThe first letter.");
        assert!(
            !hint_on_screen(&gui, "The first letter."),
            "a switched-off hint still followed the pointer"
        );
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
    fn the_hint_reads_its_prose_as_markdown() {
        // Servers document in Markdown. A hint that shows `**bold**` shows the source of
        // the documentation instead of the documentation.
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);

        answer(
            &mut gui,
            "```text\nalpha\n```\n---\n\nSome **bold** and `tick` text.",
        );

        assert!(
            gui.harness.query_by_label_contains("bold").is_some(),
            "the prose is not on screen"
        );
        assert!(
            gui.harness.query_by_label_contains("**").is_none(),
            "the Markdown is on screen as source"
        );
        assert!(hint_on_screen(&gui, "alpha"), "the signature is gone");
    }

    #[test]
    fn the_hint_waits_a_moment_before_it_goes_away() {
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\nalpha\n```");
        assert!(hint_on_screen(&gui, "alpha"));

        gui.harness.remove_cursor();
        // A frame in, the pointer has left and the hint is still there: the hand needs
        // the time to cross the gap into it.
        gui.harness.step();
        assert!(
            hint_on_screen(&gui, "alpha"),
            "the hint vanished the instant the pointer left"
        );

        run_for(&mut gui, HOVER_LINGER + 0.2);
        assert!(
            !hint_on_screen(&gui, "alpha"),
            "the hint stayed after the pointer left for good"
        );
    }

    #[test]
    fn the_hint_stays_while_the_pointer_is_on_it() {
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\nalpha\n```");
        assert!(hint_on_screen(&gui, "alpha"));

        let rect = gui
            .app()
            .state
            .hover
            .shown_rect()
            .expect("the hint is not on screen");

        // Into the hint — past it, past the linger, and it is still there to be used.
        gui.harness.hover_at(rect.center());
        run_for(&mut gui, HOVER_LINGER * 4.0);
        assert!(
            hint_on_screen(&gui, "alpha"),
            "the hint went away while the pointer was on it"
        );

        // Out of it, and the clock that was stopped starts again.
        gui.harness.remove_cursor();
        run_for(&mut gui, HOVER_LINGER + 0.2);
        assert!(
            !hint_on_screen(&gui, "alpha"),
            "the hint stayed after the pointer left it"
        );
    }

    #[test]
    fn a_hint_that_fits_the_window_is_shown_whole() {
        // Scrolling is a last resort: an answer shorter than the screen is shown in full,
        // and the scroll bar appears only when the window is genuinely too short for it.
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        let body: String = (0..20)
            .map(|line| format!("- line {line} of the documentation\n"))
            .collect();
        answer(&mut gui, &format!("```text\nalpha\n```\n---\n\n{body}"));

        let hint = gui
            .app()
            .state
            .hover
            .shown_rect()
            .expect("the hint is not on screen");
        let window = gui.harness.ctx.viewport_rect();
        assert!(
            hint.height() < window.height() - 20.0,
            "the hint takes the whole window for a short answer: {hint:?}"
        );

        // The last line of the answer is inside the hint: nothing is cut off.
        let last = gui
            .harness
            .query_by_label_contains("line 19 of")
            .expect("the last line is not there")
            .rect();
        assert!(
            last.bottom() <= hint.bottom() + 0.5 && last.top() >= hint.top(),
            "the last line is outside the hint: {last:?} in {hint:?}"
        );
    }

    #[test]
    fn a_tall_hint_moves_up_instead_of_running_off_the_window() {
        // A long answer, still shorter than the window: it must be shown whole, so the
        // hint is placed where it fits — above the pointer if it has to be — rather than
        // hanging off the bottom edge with its last lines out of reach.
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        let body: String = (0..40)
            .map(|line| format!("- line {line} of the documentation\n"))
            .collect();
        answer(&mut gui, &format!("```text\nalpha\n```\n---\n\n{body}"));
        // The first frame the hint is drawn it does not know its own height; the frame
        // after it is placed where it fits.
        run_for(&mut gui, 0.1);

        let hint = gui
            .app()
            .state
            .hover
            .shown_rect()
            .expect("the hint is not on screen");
        let window = gui.harness.ctx.viewport_rect();
        assert!(
            hint.height() < window.height() - 20.0,
            "the hint takes the whole window: {hint:?}"
        );
        assert!(
            hint.top() >= window.top() && hint.bottom() <= window.bottom(),
            "the hint is not inside the window: {hint:?} in {window:?}"
        );

        let last = gui
            .harness
            .query_by_label_contains("line 39 of")
            .expect("the last line is not there")
            .rect();
        assert!(
            last.bottom() <= hint.bottom() + 0.5 && last.top() >= hint.top(),
            "the last line is outside the hint: {last:?} in {hint:?}"
        );
    }

    #[test]
    fn a_hint_beside_the_bottom_of_the_window_stays_inside_it() {
        // The pointer near the bottom of the code: an answer taller than the space below
        // it is shown in full, so it has to be placed above the pointer.
        let text: String = (0..100)
            .map(|line| format!("alpha beta {line}\n"))
            .collect();
        let mut gui = Gui::new(&[("notes.txt", &text)], Some("notes.txt"));
        gui.settle();
        gui.harness.hover_at(egui::pos2(315.0, 500.0));
        gui.harness.step();
        run_for(&mut gui, HOVER_DELAY + 0.1);
        let body: String = (0..40)
            .map(|line| format!("- line {line} of the documentation\n"))
            .collect();
        answer(&mut gui, &format!("```text\nalpha\n```\n---\n\n{body}"));
        run_for(&mut gui, 0.1);

        let hint = gui
            .app()
            .state
            .hover
            .shown_rect()
            .expect("the hint is not on screen");
        let window = gui.harness.ctx.viewport_rect();
        assert!(
            hint.top() >= window.top() && hint.bottom() <= window.bottom(),
            "the hint is not inside the window: {hint:?} in {window:?}"
        );
        let last = gui
            .harness
            .query_by_label_contains("line 39 of")
            .expect("the last line is not there")
            .rect();
        assert!(
            last.bottom() <= hint.bottom() + 0.5,
            "the last line is cut off: {last:?} in {hint:?}"
        );
    }

    #[test]
    fn the_text_under_the_hint_is_not_asked_about() {
        // The hint floats over the code, so a pointer on the hint is geometrically on a
        // symbol it cannot see. Asking about that symbol replaced the answer being read
        // with an answer about something hidden behind the hint.
        let text: String = (0..100)
            .map(|line| format!("alpha beta {line}\n"))
            .collect();
        let mut gui = Gui::new(&[("notes.txt", &text)], Some("notes.txt"));
        gui.settle();

        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\nalpha\n```\n---\n\nfirst question");
        assert!(hint_on_screen(&gui, "first question"));

        // Onto the hint, and long enough for a question to be due.
        let rect = gui
            .app()
            .state
            .hover
            .shown_rect()
            .expect("the hint is not on screen");
        gui.harness.hover_at(rect.center());
        run_for(&mut gui, HOVER_DELAY * 3.0);

        // The answer to the symbol under the hint arrives: it must be dropped, because
        // nothing was asked about it.
        answer(&mut gui, "```text\nbeta\n```\n---\n\nsecond question");
        assert!(
            hint_on_screen(&gui, "first question"),
            "the hint being read was replaced while the pointer was on it"
        );
        assert!(
            !hint_on_screen(&gui, "second question"),
            "the text under the hint was asked about"
        );

        // Out of the hint and onto other code, and the mechanism works again.
        gui.harness.hover_at(egui::pos2(300.0, 300.0));
        run_for(&mut gui, HOVER_DELAY + 0.1);
        answer(&mut gui, "```text\ngamma\n```\n---\n\nthird question");
        assert!(
            hint_on_screen(&gui, "third question"),
            "nothing is asked about the text once the pointer has left the hint"
        );
    }

    #[test]
    fn a_hint_too_tall_for_the_window_is_scrolled() {
        // Forty lines of documentation do not fit beside the pointer: the hint is cut
        // to the window and scrolled, not left hanging off the bottom of the screen.
        let mut gui = window();
        rest_on_text(&mut gui);
        run_for(&mut gui, HOVER_DELAY + 0.1);
        let body: String = (0..80)
            .map(|line| format!("- line {line} of the documentation\n"))
            .collect();
        answer(&mut gui, &format!("```text\nalpha\n```\n---\n\n{body}"));

        let hint = gui
            .app()
            .state
            .hover
            .shown_rect()
            .expect("the hint is not on screen");
        let window = gui.harness.ctx.viewport_rect();
        assert!(
            hint.height() < window.height() - 20.0,
            "the hint is as tall as the window: {hint:?} in {window:?}"
        );
        assert!(
            hint.bottom() <= window.bottom(),
            "the hint runs off the bottom of the window: {hint:?} in {window:?}"
        );

        // The wheel over it scrolls what the answer hides.
        let first = gui
            .harness
            .query_by_label_contains("line 1 of")
            .expect("the first line is not there")
            .rect();
        gui.harness.hover_at(hint.center());
        gui.harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -200.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        gui.harness.step();
        gui.harness.step();

        let after = gui
            .harness
            .query_by_label_contains("line 1 of")
            .expect("the first line is gone")
            .rect();
        assert!(
            after.top() < first.top(),
            "the wheel did not scroll the hint: {first:?} then {after:?}"
        );
        assert!(
            hint_on_screen(&gui, "alpha"),
            "the hint closed when it was scrolled"
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

/// The dim background under the other uses of the symbol under the caret.
///
/// Painted rather than built from widgets, so the checks are: a frame with the
/// ranges drawn does not panic, and the ranges do not outlive the text they were
/// measured in.
mod the_occurrence_background {
    use super::*;

    fn set_occurrences(gui: &mut Gui, ranges: &[(usize, usize)]) {
        let buffer = gui
            .app_mut()
            .state
            .active_buffer_mut()
            .expect("no open buffer");
        buffer.occurrences = ranges.to_vec();
    }

    #[test]
    fn a_range_over_cyrillic_is_drawn_without_slicing_a_character() {
        let mut gui = Gui::new(&[("a.rs", "let привет = привет;\n")], Some("a.rs"));
        let text = gui.app().state.active_buffer().unwrap().text().to_owned();
        let first = text.find("привет").unwrap();
        let last = text.rfind("привет").unwrap();
        set_occurrences(
            &mut gui,
            &[
                (first, first + "привет".len()),
                (last, last + "привет".len()),
            ],
        );

        gui.settle();
        assert!(
            gui.app().state.active_buffer().unwrap().occurrences.len() == 2,
            "the ranges were lost"
        );
    }

    #[test]
    fn a_range_that_runs_past_its_line_is_clipped() {
        let mut gui = Gui::new(&[("a.rs", "one\ntwo\n")], Some("a.rs"));
        // A range from the first line into the second: a stale answer looks like
        // this, and painting it must not run off the end of a line.
        set_occurrences(&mut gui, &[(0, 7)]);
        gui.settle();
    }

    #[test]
    fn an_edit_clears_the_ranges() {
        let mut gui = Gui::new(&[("a.rs", "alpha beta\n")], Some("a.rs"));
        set_occurrences(&mut gui, &[(0, 5)]);
        gui.settle();

        gui.harness.event(egui::Event::Text("X".to_owned()));
        gui.settle();

        assert!(
            gui.app()
                .state
                .active_buffer()
                .unwrap()
                .occurrences
                .is_empty(),
            "the ranges still point into text that has changed"
        );
    }
}

/// The rename modal: the field, then the list of what would change.
mod the_rename_modal {
    use super::*;
    use crate::ui::rename::{FileChanges, Span};

    fn preview(gui: &mut Gui) {
        gui.app_mut().state.rename.preview(
            vec![FileChanges {
                path: PathBuf::from("a.rs"),
                spans: vec![
                    Span {
                        line: 0,
                        start: 3,
                        end: 8,
                    },
                    Span {
                        line: 2,
                        start: 30,
                        end: 35,
                    },
                ],
            }],
            Vec::new(),
        );
    }

    #[test]
    fn the_field_shows_the_name_and_closes_on_escape() {
        let mut gui = Gui::new(&[("a.rs", "fn alpha() {}\n")], Some("a.rs"));
        gui.app_mut().state.rename.begin("alpha");
        gui.settle();
        assert!(
            gui.harness.query_by_label("rename alpha").is_some(),
            "the field did not appear"
        );

        gui.harness.key_press(egui::Key::Escape);
        gui.settle();

        assert!(!gui.app().state.rename.open, "Escape did not close it");
        assert!(
            gui.harness.query_by_label("rename alpha").is_none(),
            "the modal is still on screen"
        );
    }

    #[test]
    fn the_preview_lists_the_places_before_anything_is_written() {
        let mut gui = Gui::new(&[("a.rs", "fn alpha() {}\n")], Some("a.rs"));
        gui.app_mut().state.rename.begin("alpha");
        gui.app_mut().state.rename.name = "beta".to_owned();
        preview(&mut gui);
        gui.settle();

        assert!(
            gui.harness.query_by_label("alpha → beta").is_some(),
            "the preview does not say what the rename would do"
        );
        assert!(
            gui.harness.query_by_label("2 uses in 1 file").is_some(),
            "the extent is not on screen"
        );
        assert!(
            gui.app()
                .state
                .active_buffer()
                .unwrap()
                .text()
                .contains("alpha"),
            "the text changed while the preview was on screen"
        );
    }

    #[test]
    fn escape_on_the_preview_writes_nothing() {
        let mut gui = Gui::new(&[("a.rs", "fn alpha() {}\n")], Some("a.rs"));
        gui.app_mut().state.rename.begin("alpha");
        gui.app_mut().state.rename.name = "beta".to_owned();
        preview(&mut gui);
        gui.settle();

        gui.harness.key_press(egui::Key::Escape);
        gui.settle();

        assert!(!gui.app().state.rename.open);
        assert!(
            gui.app()
                .state
                .active_buffer()
                .unwrap()
                .text()
                .contains("alpha"),
            "Escape renamed something"
        );
    }
}

/// The tab strip of a file that lives outside the project.
mod the_external_tab {
    use super::*;

    #[test]
    fn the_tab_says_which_mod_rs_it_is() {
        let mut gui = Gui::new(&[("a.rs", "one\n")], Some("a.rs"));
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("block")).unwrap();
        let top = outside.path().join("mod.rs");
        let nested = outside.path().join("block").join("mod.rs");
        std::fs::write(&top, "top\n").unwrap();
        std::fs::write(&nested, "nested\n").unwrap();

        gui.app_mut().state.open_file(&top);
        gui.app_mut().state.open_file(&nested);
        gui.settle();

        let labels: Vec<String> = [&top, &nested]
            .iter()
            .map(|path| {
                crate::state::tab_title(path, gui.app().state.project.as_ref().map(|p| p.root()))
            })
            .collect();
        assert_ne!(labels[0], labels[1]);
        for label in &labels {
            assert!(
                gui.harness.query_by_label(label).is_some(),
                "the tab {label} is not on screen"
            );
        }
    }
}

/// The terminal tab in the strip.
mod the_terminal_tab {
    use super::*;

    #[test]
    fn the_terminal_tab_is_in_the_layout_and_really_starts() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab is in the layout");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        gui.settle();

        assert_eq!(
            gui.app().state.terminal.error,
            None,
            "the tab drew but no shell started"
        );
        assert!(
            gui.app().state.terminal.is_alive(),
            "the shell behind the tab is not running"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// Typing with the terminal focused belongs to the shell and to nobody else.
mod the_terminal_owns_its_keys {
    use super::*;

    fn terminal_tab_active(gui: &mut Gui) {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab is in the layout");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        gui.settle();
        assert_eq!(gui.app().state.terminal.error, None);
    }

    #[test]
    fn the_terminal_takes_the_keys_the_moment_its_tab_comes_up() {
        // Coming to the terminal is coming to the keyboard: the tab that has just come to
        // the front holds the keys without being clicked first. A person who switches to a
        // terminal means to type in it, and a frame spent typing into a file instead is a
        // word in the wrong place.
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        terminal_tab_active(&mut gui);
        assert!(
            gui.app().state.terminal.focused,
            "the terminal came to the front and did not take the keyboard"
        );
    }

    #[test]
    fn a_letter_typed_there_does_not_appear_in_the_editor() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        terminal_tab_active(&mut gui);
        gui.app_mut().state.terminal.focused = true;

        gui.harness.event(egui::Event::Text("zzz".to_owned()));
        gui.settle();

        let editor = gui
            .app()
            .state
            .active_buffer()
            .expect("the editor still holds a.rs")
            .text()
            .to_owned();
        assert_eq!(
            editor, "fn a() {}\n",
            "the terminal's typing went into the editor as well"
        );
    }

    #[test]
    fn a_click_in_the_editor_gives_the_keyboard_back() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        terminal_tab_active(&mut gui);
        gui.app_mut().state.terminal.focused = true;
        gui.settle();
        assert!(gui.app().state.terminal.focused);

        // A press outside the terminal is a person going back to the code.
        let at = egui::pos2(400.0, 200.0);
        gui.harness.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        gui.settle();
        assert!(
            !gui.app().state.terminal.focused,
            "the terminal kept the keyboard after a click outside it"
        );

        gui.harness.event(egui::Event::Text("y".to_owned()));
        gui.settle();
        assert!(
            gui.app()
                .state
                .active_buffer()
                .unwrap()
                .text()
                .contains('y'),
            "the letter did not reach the editor: {:?}",
            gui.app().state.active_buffer().unwrap().text()
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn a_hidden_terminal_does_not_keep_the_keyboard() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        terminal_tab_active(&mut gui);
        gui.app_mut().state.terminal.focused = true;
        gui.settle();
        assert!(gui.app().state.terminal.focused);

        // The tab is taken out of the dock, so the panel is never drawn again.
        let terminal = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.remove_tab(terminal);
        gui.settle();
        gui.settle();

        assert!(
            !gui.app().state.terminal.focused,
            "a terminal that is not on screen kept the keyboard"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn a_command_typed_there_runs_in_the_shell() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        terminal_tab_active(&mut gui);
        gui.app_mut().state.terminal.focused = true;

        gui.harness
            .event(egui::Event::Text("echo tty-ok".to_owned()));
        gui.harness.key_press(egui::Key::Enter);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut seen = false;
        while std::time::Instant::now() < deadline {
            gui.settle();
            if gui.app().state.terminal.contents().contains("tty-ok") {
                seen = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(
            seen,
            "the shell never ran what was typed: {:?}",
            gui.app().state.terminal.contents()
        );
        assert_eq!(
            gui.app().state.active_buffer().unwrap().text(),
            "fn a() {}\n",
            "the command was typed into the editor too"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// The keyboard really changes hands when the pointer does.
mod the_terminal_click {
    use super::*;

    #[test]
    fn a_click_into_the_panel_puts_the_shell_in_charge() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        // The tab came up holding the keyboard by itself — that is what coming to the
        // terminal means (`the_terminal_owns_its_keys` walks that road). What the click
        // proves is the other half: a pointer landing inside the panel leaves the panel in
        // charge, and typing goes to the shell.
        assert!(
            gui.app().state.terminal.focused,
            "the terminal came to the front and did not take the keyboard"
        );

        gui.harness.event(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        gui.settle();
        assert!(
            gui.app().state.terminal.focused,
            "a click inside the terminal did not give it the keyboard"
        );

        gui.harness
            .event(egui::Event::Text("echo clicked-ok".to_owned()));
        gui.harness.key_press(egui::Key::Enter);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut seen = false;
        while std::time::Instant::now() < deadline {
            gui.settle();
            if gui.app().state.terminal.contents().contains("clicked-ok") {
                seen = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(seen, "the shell did not run the typed command");
        assert_eq!(
            gui.app().state.active_buffer().unwrap().text(),
            "fn a() {}\n",
            "the command was also typed into the editor"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// A program that asks for the application cursor keys and gets them.
///
/// This is the `mc` case end to end: egui's key events, the panel, the mapping and
/// the pseudo-terminal. The terminal driver echoes what reaches it, so the screen
/// says exactly which bytes were sent.
mod the_terminal_keys {
    use super::*;
    use std::time::{Duration, Instant};

    fn focused_terminal(gui: &mut Gui) {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        gui.harness.event(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        gui.settle();
        assert!(gui.app().state.terminal.focused, "the panel took no focus");
    }

    fn type_text(gui: &mut Gui, text: &str) {
        gui.harness.event(egui::Event::Text(text.to_owned()));
        gui.settle();
    }

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen: {:?}",
            gui.app().state.terminal.contents()
        );
    }

    /// Asks the terminal for the application cursor keys and hands the keyboard to
    /// `cat -v`, which makes the driver's echo the only thing on screen.
    fn ask_for_the_application_mode(gui: &mut Gui) {
        focused_terminal(gui);
        type_text(gui, "printf '\\033[?1h'; cat -v\n");
        wait_until(gui, "the program to ask for the mode", |gui| {
            gui.app().state.terminal.application_cursor()
        });
    }

    #[test]
    fn an_arrow_arrives_in_the_form_the_program_asked_for() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        ask_for_the_application_mode(&mut gui);

        gui.harness.key_press(egui::Key::ArrowDown);
        wait_until(&mut gui, "the arrow to be echoed", |gui| {
            gui.app().state.terminal.contents().contains("^[OB")
        });
        let screen = gui.app().state.terminal.contents();
        assert!(
            !screen.contains("^[[B"),
            "the plain form was sent as well: {screen:?}"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn a_function_key_arrives_as_its_sequence() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        ask_for_the_application_mode(&mut gui);

        gui.harness.key_press(egui::Key::F5);
        wait_until(&mut gui, "F5 to be echoed", |gui| {
            gui.app().state.terminal.contents().contains("^[[15~")
        });
        let screen = gui.app().state.terminal.contents();
        assert!(
            gui.app().state.active_buffer().unwrap().text() == "fn a() {}\n",
            "the function key was also typed into the file: {screen:?}"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// The bottom of the window is one strip of tabs, not two panes side by side.
mod the_bottom_strip_tabs {
    use super::*;

    fn terminal_path(gui: &Gui) -> egui_dock::TabPath {
        gui.app()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab")
    }

    #[test]
    fn the_panels_are_tabs_of_one_leaf() {
        let gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let problems = gui
            .app()
            .dock
            .find_tab(&crate::ui::Tab::Problems)
            .expect("the problems tab");
        assert_eq!(
            problems.node_path(),
            terminal_path(&gui).node_path(),
            "the panels are still in separate panes"
        );
    }

    #[test]
    fn only_the_tab_in_front_is_drawn() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        gui.settle();
        assert!(
            !gui.app().state.terminal.shown,
            "the terminal was drawn behind the problems"
        );

        let path = terminal_path(&gui);
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        gui.settle();
        assert!(
            gui.app().state.terminal.shown,
            "the terminal was not drawn after its tab came to the front"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn the_front_tab_uses_the_whole_width() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let path = terminal_path(&gui);
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        let window = gui.harness.ctx.viewport_rect();
        println!("terminal {:?} of window {:?}", rect, window);
        assert!(
            rect.width() > window.width() * 0.95,
            "the terminal is {:.0} of {:.0} wide: it still shares the strip",
            rect.width(),
            window.width()
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// A panel is opened by bringing its tab forward.
mod opening_a_panel {
    use super::*;

    #[test]
    fn alt_f12_brings_the_terminal_forward() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        gui.settle();
        assert!(
            !gui.app().state.terminal.shown,
            "the terminal was in front before the shortcut"
        );

        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::F12);
        gui.settle();
        gui.settle();
        assert!(
            gui.app().state.terminal.shown,
            "Alt+F12 did not bring the terminal tab forward"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn the_view_menu_switches_the_hints() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        gui.settle();
        assert!(gui.app().state.hints, "the hints start off");

        gui.harness.get_by_label("View").click();
        gui.settle();
        gui.harness.get_by_label("Show hints").click();
        gui.settle();

        assert!(
            !gui.app().state.hints,
            "the View menu switch did not turn the hints off"
        );
    }

    #[test]
    fn the_view_menu_names_both_panels() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        assert!(gui.harness.query_by_label("View").is_some(), "no View menu");

        // Every panel the window has is a line of the menu, on or off: a panel that is not
        // listed by name can only be reached by remembering its shortcut, and visibility that
        // is a shortcut for some and a blank for the rest is not a visibility at all.
        gui.harness.get_by_label("View").click();
        gui.settle();
        for tab in Tab::ALL {
            let label = match tab.shortcut() {
                Some(key) => format!("{}\t{key}", tab.title()),
                None => tab.title().to_owned(),
            };
            assert!(
                gui.harness.query_by_label(&label).is_some(),
                "the View menu has no line for {}",
                tab.title()
            );
        }
    }
}

/// Clicks belong to the program when the program asked for them.
mod the_terminal_mouse {
    use super::*;
    use std::time::{Duration, Instant};

    fn focus(gui: &mut Gui) -> egui::Rect {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        gui.harness.event(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        gui.harness.event(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        gui.settle();
        rect
    }

    fn click(gui: &mut Gui, pos: egui::Pos2) {
        for pressed in [true, false] {
            gui.harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        gui.settle();
    }

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen: {:?}",
            gui.app().state.terminal.contents()
        );
    }

    #[test]
    fn a_click_reaches_a_program_that_asked_for_the_mouse() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = focus(&mut gui);
        gui.harness.event(egui::Event::Text(
            "printf '\\033[?1000h\\033[?1006h'; cat -v\n".to_owned(),
        ));
        gui.settle();
        wait_until(&mut gui, "the program to ask for the mouse", |gui| {
            gui.app().state.terminal.mouse_enabled()
        });

        click(&mut gui, rect.center());
        wait_until(&mut gui, "the click to be reported", |gui| {
            gui.app().state.terminal.contents().contains("^[[<0;")
        });
        let screen = gui.app().state.terminal.contents();
        assert!(
            screen.contains("M"),
            "the report has no final letter: {screen:?}"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn a_click_at_a_plain_prompt_sends_nothing() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = focus(&mut gui);
        assert!(
            !gui.app().state.terminal.mouse_enabled(),
            "a bare shell asked for the mouse"
        );
        let before = gui.app().state.terminal.contents();
        click(&mut gui, rect.center());
        gui.settle();
        assert_eq!(
            gui.app().state.terminal.contents(),
            before,
            "a click changed the screen of a program that asked for nothing"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// `Shift` is the terminal's own mouse button: it picks text even out of a program
/// that wants the mouse for itself.
mod the_terminal_selection {
    use super::*;
    use std::time::{Duration, Instant};

    fn open_terminal(gui: &mut Gui) -> egui::Rect {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        gui.harness.event(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        gui.harness.event(egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        gui.settle();
        rect
    }

    fn type_text(gui: &mut Gui, text: &str) {
        gui.harness.event(egui::Event::Text(text.to_owned()));
        gui.settle();
    }

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen: {:?}",
            gui.app().state.terminal.contents()
        );
    }

    /// A point in the middle of a cell of the panel.
    fn at(gui: &Gui, rect: egui::Rect, column: u16, row: u16) -> egui::Pos2 {
        let (cols, rows) = gui.app().state.terminal.size();
        let cell = egui::vec2(rect.width() / cols as f32, rect.height() / rows as f32);
        rect.left_top() + egui::vec2((column as f32 + 0.5) * cell.x, (row as f32 + 0.5) * cell.y)
    }

    /// The row of the screen that holds a line, as `contents()` numbers them.
    fn row_of(gui: &Gui, needle: &str) -> u16 {
        gui.app()
            .state
            .terminal
            .contents()
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} is not on the screen")) as u16
    }

    fn drag(gui: &mut Gui, from: egui::Pos2, to: egui::Pos2, shift: bool) {
        let modifiers = if shift {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };
        // A real `Shift` is held down for the whole drag, so it is in the input state
        // of that frame and not only in the events.
        gui.harness.event(egui::Event::ModifiersChanged(modifiers));
        gui.harness.event(egui::Event::PointerMoved(from));
        gui.harness.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers,
        });
        gui.settle();
        gui.harness.event(egui::Event::PointerMoved(to));
        gui.settle();
        gui.harness.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        });
        gui.settle();
        gui.harness
            .event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    }

    #[test]
    fn a_shift_drag_picks_the_text_and_leaves_the_program_alone() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = open_terminal(&mut gui);
        type_text(&mut gui, "printf '\\033[?1000h\\033[?1006h'; cat -v\n");
        wait_until(&mut gui, "the program to ask for the mouse", |gui| {
            gui.app().state.terminal.mouse_enabled()
        });
        let before = gui.app().state.terminal.contents();

        let row = row_of(&gui, "cat -v");
        let (from, to) = (at(&gui, rect, 2, row), at(&gui, rect, 6, row));
        drag(&mut gui, from, to, true);
        assert!(
            gui.app().state.terminal.selection().is_some(),
            "shift+drag selected nothing"
        );
        assert_eq!(
            gui.app().state.terminal.contents(),
            before,
            "the click reached a program that was told to keep its hands off"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn a_drag_picks_a_line_and_ctrl_shift_c_copies_it() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = open_terminal(&mut gui);
        // The output differs from the command line itself, so waiting for it
        // cannot be satisfied by the echo of what was typed.
        type_text(
            &mut gui,
            "printf '\\none two\\nthree four\\n' | tr a-z A-Z\n",
        );
        wait_until(&mut gui, "the text to be on the screen", |gui| {
            gui.app().state.terminal.contents().contains("THREE FOUR")
        });

        let top = row_of(&gui, "ONE TWO");
        let (from, to) = (at(&gui, rect, 0, top), at(&gui, rect, 10, top + 1));
        drag(&mut gui, from, to, false);
        let text = gui
            .app()
            .state
            .terminal
            .selected_text()
            .expect("the drag picked nothing");
        assert_eq!(text, "ONE TWO\nTHREE FOUR", "picked the wrong cells");

        assert!(
            gui.app().state.terminal.focused,
            "the terminal did not have the keyboard"
        );
        gui.harness.event(egui::Event::Key {
            key: egui::Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        });
        // The clipboard command lives in the output of the frame that asked for it, and
        // every further frame would replace it.
        gui.harness.step();
        let copied = gui
            .harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            });
        assert_eq!(
            copied.as_deref(),
            Some("ONE TWO\nTHREE FOUR"),
            "Ctrl+Shift+C did not copy the selection"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// `Ctrl+Insert` copies in the terminal too, the same chord the editor uses.
mod the_terminal_copy_chord {
    use super::*;
    use std::time::{Duration, Instant};

    fn open_terminal(gui: &mut Gui) -> egui::Rect {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        for pressed in [true, false] {
            gui.harness.event(egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        gui.settle();
        rect
    }

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen: {:?}",
            gui.app().state.terminal.contents()
        );
    }

    fn copied(gui: &Gui) -> Option<String> {
        gui.harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
    }

    #[test]
    fn ctrl_insert_copies_the_selection_and_sends_nothing_to_the_shell() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = open_terminal(&mut gui);
        gui.harness.event(egui::Event::Text(
            "printf '\\none two\\nthree four\\n' | tr a-z A-Z\n".to_owned(),
        ));
        gui.settle();
        wait_until(&mut gui, "the text to be on the screen", |gui| {
            gui.app().state.terminal.contents().contains("THREE FOUR")
        });
        let (cols, rows) = gui.app().state.terminal.size();
        let cell = egui::vec2(rect.width() / cols as f32, rect.height() / rows as f32);
        let row = gui
            .app()
            .state
            .terminal
            .contents()
            .lines()
            .position(|line| line.contains("ONE TWO"))
            .expect("ONE TWO is on the screen") as u16;
        let point = |column: u16, row: u16| {
            rect.left_top()
                + egui::vec2((column as f32 + 0.5) * cell.x, (row as f32 + 0.5) * cell.y)
        };
        // A plain drag picks the text: no program here asked for the mouse.
        let press = |gui: &mut Gui, pos, pressed: bool| {
            gui.harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            gui.settle();
        };
        gui.harness.event(egui::Event::PointerMoved(point(0, row)));
        press(&mut gui, point(0, row), true);
        gui.harness
            .event(egui::Event::PointerMoved(point(10, row + 1)));
        gui.settle();
        press(&mut gui, point(10, row + 1), false);
        assert_eq!(
            gui.app().state.terminal.selected_text().as_deref(),
            Some("ONE TWO\nTHREE FOUR")
        );
        let before = gui.app().state.terminal.contents();

        gui.harness.input_mut().events.push(egui::Event::Key {
            key: egui::Key::Insert,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        });
        gui.harness.step();
        assert_eq!(
            copied(&gui).as_deref(),
            Some("ONE TWO\nTHREE FOUR"),
            "Ctrl+Insert did not copy the selection"
        );
        gui.settle();
        assert_eq!(
            gui.app().state.terminal.contents(),
            before,
            "the copy chord was typed into the shell"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// `Shift+Insert` puts the clipboard into the terminal, as it does in the editor.
mod the_terminal_paste_chord {
    use super::*;
    use std::time::{Duration, Instant};

    fn open_terminal(gui: &mut Gui) -> egui::Rect {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        for pressed in [true, false] {
            gui.harness.event(egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        gui.settle();
        rect
    }

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen: {:?}",
            gui.app().state.terminal.contents()
        );
    }

    #[test]
    fn shift_insert_asks_for_the_clipboard_and_types_it_into_the_shell() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        open_terminal(&mut gui);

        gui.harness.input_mut().events.push(egui::Event::Key {
            key: egui::Key::Insert,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        });
        gui.harness.step();
        let asked = gui
            .harness
            .output()
            .viewport_output
            .values()
            .flat_map(|viewport| viewport.commands.iter())
            .any(|command| matches!(command, egui::ViewportCommand::RequestPaste));
        assert!(
            asked,
            "Shift+Insert did not ask the backend for the clipboard"
        );
        // The shell writes its prompt on its own, so a whole-content comparison would be
        // comparing the shell with itself. What the chord must not leave behind is the
        // Insert key's own sequence (`ESC [ 2 ~`), which is what a key sent to the shell
        // would look like.
        assert!(
            !gui.app().state.terminal.contents().contains("[2~"),
            "the paste chord was typed into the shell as a key sequence"
        );

        // What the backend sends back once it has read the clipboard.
        gui.harness
            .input_mut()
            .events
            .push(egui::Event::Paste("echo $((6*7))\n".to_owned()));
        gui.harness.step();
        wait_until(&mut gui, "the pasted command to run", |gui| {
            gui.app().state.terminal.contents().contains("42")
        });
        gui.app_mut().state.terminal.restart();
    }
}

/// More than one shell in the panel: a strip of sessions above the screen.
mod the_terminal_sessions {
    use super::*;
    use std::time::{Duration, Instant};

    fn open_terminal(gui: &mut Gui) -> egui::Rect {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        click_grid(gui, rect);
        rect
    }

    /// A click in the middle of the screen, which also takes the keyboard.
    fn click_grid(gui: &mut Gui, rect: egui::Rect) {
        for pressed in [true, false] {
            gui.harness.event(egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        gui.settle();
    }

    fn run(gui: &mut Gui, command: &str) {
        gui.harness.event(egui::Event::Text(format!("{command}\n")));
        gui.settle();
    }

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen: {:?}",
            gui.app().state.terminal.contents()
        );
    }

    fn press(gui: &mut Gui, label: &str) {
        gui.harness.get_by_label(label).click();
        gui.settle();
    }

    #[test]
    fn the_plus_opens_a_second_shell_of_its_own() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = open_terminal(&mut gui);
        assert_eq!(gui.app().state.terminal.pane_count(), 1);
        run(&mut gui, "echo FIRST-MARK");
        wait_until(&mut gui, "the first shell to answer", |gui| {
            gui.app().state.terminal.contents().contains("FIRST-MARK")
        });

        press(&mut gui, "+");
        assert_eq!(
            gui.app().state.terminal.pane_count(),
            2,
            "the plus did not open a session"
        );
        assert!(
            gui.harness.query_by_label("Terminal 2").is_some(),
            "the second session has no tab"
        );
        assert!(
            !gui.app().state.terminal.contents().contains("FIRST-MARK"),
            "the new session shows the old one's screen"
        );

        // Its own shell, in its own state.
        click_grid(&mut gui, rect);
        run(&mut gui, "echo SECOND-MARK");
        wait_until(&mut gui, "the second shell to answer", |gui| {
            gui.app().state.terminal.contents().contains("SECOND-MARK")
        });

        press(&mut gui, "Terminal 1");
        assert_eq!(gui.app().state.terminal.active_pane(), 0);
        assert!(
            gui.app().state.terminal.contents().contains("FIRST-MARK"),
            "the first session lost its screen"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn the_cross_closes_a_session_and_its_shell() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        open_terminal(&mut gui);
        press(&mut gui, "+");
        assert_eq!(gui.app().state.terminal.pane_count(), 2);

        // The cross of the first session is the first one on the strip.
        let crosses = gui.harness.query_all_by_label("×").count();
        assert_eq!(crosses, 2, "every session needs its cross");
        gui.harness
            .query_all_by_label("×")
            .next()
            .expect("a cross")
            .click();
        gui.settle();
        assert_eq!(gui.app().state.terminal.pane_count(), 1);
        assert_eq!(gui.app().state.terminal.active_pane(), 0);
        assert!(
            gui.harness.query_by_label("Terminal 2").is_none(),
            "the closed session still has a tab"
        );
        assert!(
            gui.harness.query_by_label("Terminal 1").is_some(),
            "the remaining session lost its tab"
        );
        gui.app_mut().state.terminal.restart();
    }
}

/// A link in the terminal: `Ctrl+Click` opens it, a plain click does not.
mod the_terminal_links {
    use super::*;
    use std::time::{Duration, Instant};

    fn open_terminal(gui: &mut Gui) -> egui::Rect {
        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        for pressed in [true, false] {
            gui.harness.event(egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        gui.settle();
        rect
    }

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen: {:?}",
            gui.app().state.terminal.contents()
        );
    }

    /// The point in the middle of the cell where `needle` starts on its row.
    fn point_of(gui: &Gui, rect: egui::Rect, needle: &str) -> egui::Pos2 {
        let contents = gui.app().state.terminal.contents();
        // The last line, not the first: the shell echoes the command it ran, and the
        // output that matters comes below the echo.
        let (row, line) = contents
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains(needle))
            .last()
            .expect("the text is on the screen");
        let column = line.find(needle).expect("the text is on its line");
        let (cols, rows) = gui.app().state.terminal.size();
        let cell = egui::vec2(rect.width() / cols as f32, rect.height() / rows as f32);
        rect.left_top() + egui::vec2((column as f32 + 1.0) * cell.x, (row as f32 + 0.5) * cell.y)
    }

    fn ctrl_click(gui: &mut Gui, at: egui::Pos2) -> egui::FullOutput {
        gui.harness
            .event(egui::Event::ModifiersChanged(egui::Modifiers::CTRL));
        gui.harness.event(egui::Event::PointerMoved(at));
        gui.harness.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::CTRL,
        });
        gui.harness.step();
        let output = gui.harness.output().clone();
        gui.harness.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::CTRL,
        });
        gui.harness
            .event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        gui.settle();
        output
    }

    #[test]
    fn ctrl_click_opens_a_url_in_the_browser() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = open_terminal(&mut gui);
        gui.harness.event(egui::Event::Text(
            "printf '\\nsee https://example.com/docs\\n'\n".to_owned(),
        ));
        gui.settle();
        // Twice: once in the echoed command, once in what it printed.
        wait_until(&mut gui, "the url to be printed", |gui| {
            gui.app()
                .state
                .terminal
                .contents()
                .matches("https://example.com/docs")
                .count()
                >= 2
        });

        let at = point_of(&gui, rect, "https://example.com/docs");
        let output = ctrl_click(&mut gui, at);
        let opened = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::OpenUrl(url) => Some(url.url.clone()),
                _ => None,
            });
        assert_eq!(
            opened.as_deref(),
            Some("https://example.com/docs"),
            "Ctrl+Click did not open the link"
        );
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn ctrl_click_opens_the_file_on_the_line_it_names() {
        let mut gui = Gui::new(
            &[("src/main.rs", "first\nsecond\nthird\nfourth\n")],
            Some("src/main.rs"),
        );
        let rect = open_terminal(&mut gui);
        gui.harness.event(egui::Event::Text(
            "printf '\\nboom at src/main.rs:3\\n'\n".to_owned(),
        ));
        gui.settle();
        wait_until(&mut gui, "the file link to be printed", |gui| {
            gui.app()
                .state
                .terminal
                .contents()
                .matches("src/main.rs:3")
                .count()
                >= 2
        });

        let at = point_of(&gui, rect, "src/main.rs:3");
        ctrl_click(&mut gui, at);
        let state = &gui.app().state;
        let index = state.tabs.active().expect("an active tab");
        let active = &state.tabs.items()[index].path;
        assert_eq!(
            active,
            std::path::Path::new("src/main.rs"),
            "the link did not open the file"
        );
        let line = state
            .active_buffer()
            .map(|buffer| buffer.line_col().0)
            .expect("a buffer");
        assert_eq!(line, 2, "the caret is not on the third line");
        gui.app_mut().state.terminal.restart();
    }

    #[test]
    fn a_plain_click_does_not_open_a_link() {
        let mut gui = Gui::new(&[("a.rs", "fn a() {}\n")], Some("a.rs"));
        let rect = open_terminal(&mut gui);
        gui.harness.event(egui::Event::Text(
            "printf '\\nsee https://example.com/docs\\n'\n".to_owned(),
        ));
        gui.settle();
        wait_until(&mut gui, "the url to be printed", |gui| {
            gui.app()
                .state
                .terminal
                .contents()
                .contains("https://example.com/docs")
        });

        let at = point_of(&gui, rect, "https://example.com/docs");
        for pressed in [true, false] {
            gui.harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            gui.harness.step();
        }
        let opened = gui
            .harness
            .output()
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::OpenUrl(_)));
        assert!(!opened, "a plain click opened the link");
        gui.app_mut().state.terminal.restart();
    }
}

/// The Git panel: the branch, the two halves of the status, and a click that opens
/// the file.
mod the_panels {
    use super::*;

    /// A window on a project of its own.
    fn on_a_project() -> Gui {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let harness = window(
            project.path(),
            config.path(),
            &[("a.txt", "one\n")],
            Some("a.txt"),
        );
        Gui {
            harness,
            _project: project,
            _config: Some(config),
        }
    }

    /// Clicks the cross of a tab.
    ///
    /// The dock draws its tab bar itself: the titles are drawn and the crosses are painted, so
    /// there is no label to ask for and nothing in the tree of the window to click. What the dock
    /// does give every cross is an identifier of its own — `the tab's id`, then `"close-button"` —
    /// and egui keeps the rectangle of every widget under its identifier for a frame. The cross is
    /// therefore asked for by name rather than guessed at by arithmetic: the arithmetic was wrong
    /// by a padding per tab, which is how a click meant for one tab landed on the cross of the one
    /// before it.
    fn put_away_with_the_cross(gui: &mut Gui, tab: Tab) {
        gui.settle();
        let id = egui::Id::new("egui_dock::DockArea");
        let mut found = None;
        for (surface_index, surface) in gui.app().dock.iter_surfaces_indexed() {
            for ((node_index, tab_index), there) in surface.iter_all_tabs() {
                if *there != tab {
                    continue;
                }
                let cross = id
                    .with((surface_index, "surface"))
                    .with((node_index, "node"))
                    .with((tab_index, "tab"))
                    .with("close-button");
                found = gui.harness.ctx.read_response(cross).map(|it| it.rect);
            }
        }
        let rect =
            found.unwrap_or_else(|| panic!("the tab of {} has no cross on screen", tab.title()));
        click_at(&mut gui.harness, rect.center());
        // A click is events in a queue; the frame that reads them is what closes the tab, and the
        // frame after that is where the dock has tidied the leaf it left behind.
        gui.settle();
        gui.settle();
    }

    #[test]
    fn a_panel_put_away_keeps_everything_it_was_saying() {
        // The cross means "out of the window", never "forget it". A panel does not own its
        // thinking — the shell, the conversation, the repository all belong to the project —
        // so putting a panel away must not be able to lose a word of it. The agent panel is
        // the plainest proof: a half-written message and a conversation stay exactly as they
        // were, and come back with the panel.
        let mut gui = on_a_project();
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::You(
                "a question that was asked".to_owned(),
            ));
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::Agent(
                "an answer that was given".to_owned(),
            ));
        gui.app_mut().state.agent.input = "a half-written message".to_owned();
        gui.settle();

        put_away_with_the_cross(&mut gui, Tab::Agent);
        assert!(gui.app().dock.find_tab(&Tab::Agent).is_none());

        // The menu is the door it was put away from.
        gui.harness.get_by_label("View").click();
        gui.settle();
        gui.harness.get_by_label("Agent\tAlt+0").click();
        gui.settle();
        gui.harness.key_press(egui::Key::Escape);
        gui.settle();

        assert!(gui.app().dock.find_tab(&Tab::Agent).is_some());
        assert_eq!(
            gui.app().state.agent.input,
            "a half-written message",
            "the field came back empty"
        );
        assert_eq!(
            gui.app().state.agent.transcript.len(),
            2,
            "the conversation lost a line: {:?}",
            gui.app().state.agent.transcript
        );
        assert_eq!(
            gui.app().state.agent.transcript[1],
            crate::agent::Line::Agent("an answer that was given".to_owned())
        );
    }

    #[test]
    fn the_cross_puts_a_panel_away_instead_of_throwing_it_out() {
        // The cross of a panel means what its line in the menu means: the panel leaves the window
        // and everything behind it stays where it was. A panel is not a document, and a shell
        // that is still running is not the dock's to stop.
        let mut gui = on_a_project();
        put_away_with_the_cross(&mut gui, Tab::Terminal);
        assert!(
            gui.app().dock.find_tab(&Tab::Terminal).is_none(),
            "the cross left the panel in the dock"
        );
        assert!(
            gui.app().state.hidden.contains(&Tab::Terminal),
            "the cross did not write the panel down as put away"
        );

        // And the menu brings it back, which is the door it was put away from.
        gui.harness.get_by_label("View").click();
        gui.settle();
        gui.harness.get_by_label("Terminal\tAlt+F12").click();
        gui.settle();
        gui.harness.key_press(egui::Key::Escape);
        gui.settle();
        assert!(
            gui.app().dock.find_tab(&Tab::Terminal).is_some(),
            "the panel did not come back"
        );
        assert!(!gui.app().state.hidden.contains(&Tab::Terminal));
    }

    #[test]
    fn the_keys_call_a_panel_that_was_put_away() {
        // A shortcut is a way of asking for a panel, and asking for one that is put away must not
        // do nothing: a person pressing Alt+F12 wants the terminal, not an explanation.
        let mut gui = on_a_project();
        put_away_with_the_cross(&mut gui, Tab::Terminal);
        assert!(gui.app().dock.find_tab(&Tab::Terminal).is_none());

        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::F12);
        gui.settle();
        assert!(
            gui.app().dock.find_tab(&Tab::Terminal).is_some(),
            "the key did not bring the panel back"
        );
        assert!(!gui.app().state.hidden.contains(&Tab::Terminal));
    }

    #[test]
    fn where_a_person_dragged_a_panel_is_where_it_comes_back() {
        // Where a person dragged a panel is the layout, and the layout is what a project keeps.
        // Reading a layout back must therefore fill in what is missing and move nothing: a panel
        // put back where the defaults like it is the same as forgetting where it was.
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let moved;

        {
            let mut harness = window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            );
            let app = harness.state_mut();
            let projects = app.dock.find_tab(&Tab::Projects).expect("the projects tab");
            let projects_path = projects.node_path();
            let git = app.dock.find_tab(&Tab::Git).expect("the git tab");
            assert_ne!(
                projects_path,
                git.node_path(),
                "the drag would prove nothing"
            );
            app.dock
                .move_tab(git, (projects_path, egui_dock::TabInsert::Append));
            harness.run();
            let app = harness.state_mut();
            moved = app
                .dock
                .find_tab(&Tab::Git)
                .expect("the git tab")
                .node_path();
            // What the window writes on its way out, and every few seconds while it is open.
            let layout = serde_json::to_string(&app.dock).expect("the layout");
            app.state.save_workspace(Some(layout), 1000.0, true);
        }

        // A second launch on the same project.
        let harness = window(
            project.path(),
            config.path(),
            &[("a.txt", "one\n")],
            Some("a.txt"),
        );
        let dock = &harness.state().dock;
        assert!(
            dock.find_tab(&Tab::Terminal).is_some(),
            "the panels that were on screen did not all come back"
        );
        assert_eq!(
            dock.find_tab(&Tab::Git).map(|path| path.node_path()),
            Some(moved),
            "the panel was moved back where the defaults like it"
        );
        assert!(
            dock.find_tab(&Tab::Diff).is_none(),
            "a panel that was put away came back on its own"
        );
    }
}

mod the_git_panel {
    use super::*;

    /// A window on a project that is a repository with one commit in it.
    ///
    /// The repository has to exist before the window is built: the panel looks for
    /// one when it meets a project, not on every frame.
    pub(super) fn managed(files: &[(&str, &str)], open: Option<&str>) -> Gui {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        git(project.path(), &["init", "-q", "-b", "main"]);
        let mut gui = Gui {
            harness: window(project.path(), config.path(), files, open),
            _project: project,
            _config: Some(config),
        };
        commit_all(gui._project.path(), "first");
        gui.settle();
        gui
    }

    pub(super) fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git ran");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit_all(root: &Path, message: &str) {
        // The panel commits with plain `git commit`, which reads the identity from the
        // repository's config: a temp directory has none of its own, and a developer's
        // machine does.
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", message]);
    }

    /// `Alt+9`, the way a user opens the panel.
    pub(super) fn show_panel(gui: &mut Gui) {
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num9);
        gui.settle();
        gui.settle();
    }

    #[test]
    fn the_panel_does_its_work_without_leaving_its_own_tab() {
        // The diff and the history used to be panels of their own, and looking at a change moved
        // the person out of the list they were reading: the list is where they were, and the
        // answer belongs beside the question.
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("a.txt"), "one\ntwo\n").unwrap();
        show_panel(&mut gui);
        let before: Vec<Tab> = gui
            .app()
            .dock
            .iter_all_tabs()
            .map(|(_, tab)| *tab)
            .collect();

        gui.harness.get_by_label("M  a.txt").click();
        gui.settle();
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();
        assert!(
            gui.app().state.git.diff_open,
            "the diff was asked for and not opened"
        );
        assert!(
            gui.harness.query_by_label_contains("+two").is_some(),
            "the diff is not drawn in the panel that listed the change"
        );

        // The history is the other window of the same tab.
        gui.harness.get_by_label("History").click();
        gui.settle();
        assert!(
            gui.app().state.git.history,
            "the panel did not turn to its history"
        );
        assert!(
            gui.harness.query_by_label_contains("first").is_some(),
            "the commits are not drawn"
        );

        let after: Vec<Tab> = gui
            .app()
            .dock
            .iter_all_tabs()
            .map(|(_, tab)| *tab)
            .collect();
        assert_eq!(before, after, "the panel opened a tab of its own");
        assert_eq!(
            gui.app_mut()
                .dock
                .find_active_focused()
                .map(|(_, tab)| *tab),
            Some(Tab::Git),
            "the front of the window stopped being the panel that was being read"
        );
    }

    #[test]
    fn a_click_selects_and_a_double_click_opens() {
        let mut gui = managed(&[("a.txt", "one\n"), ("b.txt", "two\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("b.txt"), "two\nedited\n").unwrap();

        show_panel(&mut gui);
        gui.harness.get_by_label("M  b.txt").click();
        gui.settle();
        assert_eq!(
            gui.app().state.git.selected,
            Some(0),
            "a click did not select the change"
        );
        assert_eq!(
            gui.active_tab(),
            Some(PathBuf::from("a.txt")),
            "a single click moved the keyboard to the editor"
        );

        // A second click inside the double-click window opens the file.
        gui.harness.get_by_label("M  b.txt").click();
        gui.settle();
        assert_eq!(
            gui.active_tab(),
            Some(PathBuf::from("b.txt")),
            "a double click did not open the file"
        );
    }

    #[test]
    fn enter_shows_the_diff_of_the_selected_change() {
        let mut gui = managed(&[("a.txt", "one\ntwo\nthree\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("a.txt"), "one\nTWO\nthree\n").unwrap();

        show_panel(&mut gui);
        gui.harness.get_by_label("M  a.txt").click();
        gui.settle();
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();
        gui.settle();

        let diff = gui
            .app()
            .state
            .git
            .diff
            .clone()
            .expect("Enter did not read a diff");
        assert!(
            diff.text.contains("-two") && diff.text.contains("+TWO"),
            "the diff is wrong: {}",
            diff.text
        );
        assert!(
            gui.harness.query_by_label("+TWO").is_some(),
            "the diff panel is not on screen"
        );
        assert!(
            gui.harness.query_by_label("-two").is_some(),
            "the removed line is not on screen"
        );
    }

    #[test]
    fn a_half_staged_file_appears_in_both_halves() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        let root = gui._project.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "one\nstaged\n").unwrap();
        git(&root, &["add", "a.txt"]);
        std::fs::write(root.join("a.txt"), "one\nstaged\nthen edited\n").unwrap();

        show_panel(&mut gui);

        let halves: Vec<String> = gui
            .app()
            .state
            .git
            .changes
            .iter()
            .map(|change| {
                let area = match change.area {
                    ide_git::Area::Staged => "index",
                    ide_git::Area::Worktree => "worktree",
                };
                format!("{area}:{}", change.path.display())
            })
            .collect();
        assert_eq!(
            halves,
            vec!["index:a.txt".to_owned(), "worktree:a.txt".to_owned()],
            "the two halves were not told apart"
        );
        assert!(
            gui.harness.query_by_label("In the index").is_some()
                && gui.harness.query_by_label("In the work tree").is_some(),
            "the panel does not name the two halves"
        );
    }

    #[test]
    fn stage_and_commit_take_the_changes_away() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("a.txt"), "one\ntwo\n").unwrap();

        show_panel(&mut gui);
        gui.harness.get_by_label("Stage all").click();
        gui.settle();
        assert!(
            gui.app()
                .state
                .git
                .changes
                .iter()
                .all(|change| change.area == ide_git::Area::Staged),
            "Stage all left something in the work tree: {:?}",
            gui.app().state.git.changes
        );

        gui.app_mut().state.git.message = "second".to_owned();
        gui.settle();
        gui.harness.get_by_label("Commit").click();
        gui.settle();

        assert!(
            gui.app().state.git.changes.is_empty(),
            "the panel still lists changes after a commit: {:?}",
            gui.app().state.git.changes
        );
        assert!(
            gui.app().state.git.message.is_empty(),
            "the message field kept the text of the commit it just made"
        );
        let log = std::process::Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(gui._project.path())
            .output()
            .expect("git ran");
        assert_eq!(
            String::from_utf8_lossy(&log.stdout).trim(),
            "second",
            "the commit did not carry the message"
        );
        assert!(
            gui.harness
                .query_by_label("Nothing has changed since the last commit.")
                .is_some(),
            "the panel does not say the repository is clean"
        );
    }

    #[test]
    fn stage_puts_one_selected_file_into_the_index() {
        let mut gui = managed(&[("a.txt", "one\n"), ("b.txt", "two\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("a.txt"), "one\nedited\n").unwrap();
        std::fs::write(gui._project.path().join("b.txt"), "two\nedited\n").unwrap();

        show_panel(&mut gui);
        gui.harness.get_by_label("M  a.txt").click();
        gui.settle();
        gui.harness.get_by_label("Stage").click();
        gui.settle();

        let staged: Vec<String> = gui
            .app()
            .state
            .git
            .staged()
            .map(|change| change.path.display().to_string())
            .collect();
        let worktree: Vec<String> = gui
            .app()
            .state
            .git
            .worktree()
            .map(|change| change.path.display().to_string())
            .collect();
        assert_eq!(staged, vec!["a.txt"], "the wrong file was staged");
        assert_eq!(worktree, vec!["b.txt"], "the other file was staged too");
    }

    #[test]
    fn a_changed_line_is_marked_in_the_margin() {
        let mut gui = managed(&[("a.txt", "one\ntwo\nthree\n")], Some("a.txt"));
        assert!(
            gui.app_mut().state.git.marks(Path::new("a.txt")).is_empty(),
            "a clean file has marks"
        );

        // The margin compares the index with the file on disk, so writing the file is
        // enough: no save, no editor, exactly what happens when another program
        // touches the file.
        std::fs::write(gui._project.path().join("a.txt"), "one\nTWO\nthree\n").unwrap();

        let marks = gui.app_mut().state.git.marks(Path::new("a.txt")).to_vec();
        assert_eq!(
            marks,
            vec![ide_git::LineMark {
                line: 1,
                kind: ide_git::MarkKind::Modified
            }],
            "the changed line is not marked"
        );
    }

    #[test]
    fn staging_takes_the_marks_away_only_when_the_index_catches_up() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        let file = gui._project.path().join("a.txt");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        assert_eq!(
            gui.app_mut().state.git.marks(Path::new("a.txt")).to_vec(),
            vec![ide_git::LineMark {
                line: 1,
                kind: ide_git::MarkKind::Added
            }],
            "the added line is not marked"
        );

        // Staging makes the file and the index agree, so the margin goes quiet even
        // though nothing is committed yet.
        show_panel(&mut gui);
        gui.harness.get_by_label("Stage all").click();
        gui.settle();

        assert!(
            gui.app_mut().state.git.marks(Path::new("a.txt")).is_empty(),
            "the marks outlived the staging"
        );
    }

    /// The branch names the panel knows, in the order the menu shows them.
    fn branch_names(gui: &Gui) -> Vec<String> {
        gui.app()
            .state
            .git
            .branches
            .iter()
            .map(|branch| branch.name.clone())
            .collect()
    }

    #[test]
    fn the_branches_menu_switches_head() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        git(gui._project.path(), &["branch", "feature"]);
        show_panel(&mut gui);

        assert_eq!(
            branch_names(&gui),
            vec!["main".to_owned(), "feature".to_owned()],
            "the panel does not know the branches"
        );
        assert!(
            gui.harness.query_by_label("main").is_some(),
            "the checked-out branch is not in the panel"
        );

        gui.harness.get_by_label("Branches").click();
        gui.settle();
        gui.harness.get_by_label("feature").click();
        gui.settle();
        gui.settle();

        assert_eq!(
            gui.app().state.git.branch().as_deref(),
            Some("feature"),
            "the switch did not move HEAD"
        );
        assert_eq!(
            gui.app()
                .state
                .git
                .branches
                .first()
                .map(|branch| branch.name.clone()),
            Some("feature".to_owned()),
            "the panel still lists the old branch first"
        );
        assert!(
            gui.harness.query_by_label("feature").is_some(),
            "the new branch is not named in the panel"
        );
    }

    #[test]
    fn a_new_branch_is_created_from_the_panel() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        show_panel(&mut gui);

        gui.app_mut().state.git.branch_name = "topic".to_owned();
        gui.settle();
        gui.harness.get_by_label("Branches").click();
        gui.settle();
        gui.harness.get_by_label("New branch").click();
        gui.settle();
        gui.settle();

        assert_eq!(
            gui.app().state.git.branch().as_deref(),
            Some("topic"),
            "the new branch was not checked out"
        );
        assert!(
            branch_names(&gui).contains(&"topic".to_owned()),
            "the new branch is not in the list: {:?}",
            branch_names(&gui)
        );
        assert!(
            gui.app().state.git.branch_name.is_empty(),
            "the field kept the name it just used"
        );
    }

    #[test]
    fn switching_branches_leaves_the_files_alone() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        git(gui._project.path(), &["branch", "feature"]);
        let file = gui._project.path().join("a.txt");
        std::fs::write(&file, "one\nwork in progress\n").unwrap();
        show_panel(&mut gui);

        gui.harness.get_by_label("Branches").click();
        gui.settle();
        gui.harness.get_by_label("feature").click();
        gui.settle();
        gui.settle();

        assert_eq!(gui.app().state.git.branch().as_deref(), Some("feature"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "one\nwork in progress\n",
            "the switch touched the working file"
        );
    }

    #[test]
    fn typing_in_the_message_field_and_pressing_enter_commits() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("a.txt"), "one\ntwo\n").unwrap();
        show_panel(&mut gui);
        gui.harness.get_by_label("Stage all").click();
        gui.settle();

        let field = gui.harness.get_by_role(egui::accesskit::Role::TextInput);
        field.focus();
        field.type_text("typed in the field");
        gui.settle();
        assert_eq!(
            gui.app().state.git.message,
            "typed in the field",
            "the text did not reach the field"
        );

        gui.harness.key_press(egui::Key::Enter);
        gui.settle();

        assert!(
            gui.app().state.git.changes.is_empty(),
            "Enter in the field did not commit: {:?}",
            gui.app().state.git.changes
        );
        let log = std::process::Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(gui._project.path())
            .output()
            .expect("git ran");
        assert_eq!(
            String::from_utf8_lossy(&log.stdout).trim(),
            "typed in the field",
            "the commit did not carry the typed message"
        );
    }

    #[test]
    fn the_history_lists_the_commits_and_shows_what_one_changed() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("a.txt"), "one\ntwo\n").unwrap();
        commit_all(gui._project.path(), "second");
        gui.settle();

        show_panel(&mut gui);
        // The history is the panel's other window, and it is turned to inside the panel: looking
        // at what came before is not a reason to leave the list of what is here now.
        gui.harness.get_by_label("History").click();
        gui.settle();
        gui.settle();

        let summaries: Vec<String> = gui
            .app()
            .state
            .git
            .commits
            .iter()
            .map(|commit| commit.summary.clone())
            .collect();
        assert_eq!(
            summaries,
            vec!["second".to_owned(), "first".to_owned()],
            "the history is not newest first"
        );
        assert!(
            gui.harness
                .query_all_by_label_contains("second")
                .next()
                .is_some(),
            "the commits are not on screen"
        );

        // The newest commit is the first row.
        let row = gui
            .app()
            .state
            .git
            .commits
            .iter()
            .position(|commit| commit.summary == "second")
            .expect("the commit");
        let label = format!(
            "{}  {}  ({})",
            gui.app().state.git.commits[row].short,
            "second",
            "Test"
        );
        gui.harness.get_by_label(&label).click();
        gui.settle();
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();
        gui.settle();

        let diff = gui
            .app()
            .state
            .git
            .diff
            .clone()
            .expect("Enter did not read the commit's diff");
        assert!(
            diff.text.contains("+two") && !diff.text.contains("-one"),
            "the commit diff is wrong: {}",
            diff.text
        );
        assert!(
            gui.harness.query_by_label("+two").is_some(),
            "the diff of the commit is not on screen"
        );
        assert!(
            gui.app().dock.find_tab(&Tab::History).is_none(),
            "reading a commit opened a tab of its own"
        );
    }

    #[test]
    fn a_project_that_is_not_a_repository_says_so() {
        let mut gui = Gui::new(&[("a.txt", "one\n")], Some("a.txt"));
        show_panel(&mut gui);
        assert!(
            gui.harness
                .query_by_label("This project is not a repository.")
                .is_some(),
            "the panel said nothing instead of explaining"
        );
    }

    #[test]
    fn the_blame_names_the_commit_of_the_line_under_the_caret() {
        let mut gui = managed(&[("a.txt", "one\ntwo\n")], Some("a.txt"));
        // A second commit that only touches the second line, by another author.
        std::fs::write(gui._project.path().join("a.txt"), "one\nsecond\n").unwrap();
        let output = std::process::Command::new("git")
            .args(["commit", "-q", "-am", "a later hand"])
            .current_dir(gui._project.path())
            .env("GIT_AUTHOR_NAME", "Second")
            .env("GIT_AUTHOR_EMAIL", "second@example.com")
            .env("GIT_COMMITTER_NAME", "Second")
            .env("GIT_COMMITTER_EMAIL", "second@example.com")
            .output()
            .expect("git ran");
        assert!(output.status.success(), "the second commit failed");
        gui.settle();

        // The caret goes to the second line, and the blame is asked for.
        gui.app_mut().state.place_caret(Path::new("a.txt"), 4);
        gui.settle();
        gui.harness
            .key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::B);
        gui.settle();

        let line = gui
            .app()
            .state
            .blame_at_caret()
            .cloned()
            .expect("the blame was not shown for the line under the caret");
        assert_eq!(line.author, "Second");
        assert_eq!(line.summary, "a later hand");
        assert_eq!(line.line, 1, "the blame points at another line");
        assert!(
            gui.harness
                .query_by_label_contains(&format!("{} Second", line.short))
                .is_some(),
            "the status bar does not show the blame"
        );

        // Asking again hides it.
        gui.harness
            .key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::B);
        gui.settle();
        assert!(
            gui.app().state.blame.is_none(),
            "the blame stayed after being asked to go"
        );
    }

    #[test]
    fn the_blame_of_an_uncommitted_file_says_there_is_none() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("new.txt"), "fresh\n").unwrap();
        gui.app_mut().state.open_file(Path::new("new.txt"));
        gui.settle();

        gui.harness
            .key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::B);
        gui.settle();

        assert!(
            gui.app().state.blame.is_none(),
            "an uncommitted file was given a blame"
        );
        assert!(
            gui.app().state.status.contains("not committed"),
            "the panel does not explain why there is no blame: {:?}",
            gui.app().state.status
        );
    }

    #[test]
    fn an_edited_buffer_hides_the_blame() {
        let mut gui = managed(&[("a.txt", "one\ntwo\n")], Some("a.txt"));
        gui.harness
            .key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::B);
        gui.settle();
        assert!(
            gui.app().state.blame_at_caret().is_some(),
            "there is no blame to hide"
        );

        gui.harness.event(egui::Event::Text("X".to_owned()));
        gui.settle();
        assert!(
            gui.app().state.blame_at_caret().is_none(),
            "the blame stayed on a buffer that has been edited"
        );
    }

    #[test]
    fn the_panel_can_start_a_repository() {
        let mut gui = Gui::new(&[("a.txt", "one\n")], Some("a.txt"));
        show_panel(&mut gui);
        gui.harness.get_by_label("Initialise a repository").click();
        gui.settle();

        assert!(
            gui._project.path().join(".git").is_dir(),
            "the repository was not started"
        );
        assert!(
            gui.app().state.git.repo.is_some(),
            "the panel did not notice the repository it just made"
        );
        assert_eq!(
            gui.app().state.git.branch().as_deref(),
            Some("main"),
            "the first branch is not main"
        );
        assert!(
            gui.app()
                .state
                .git
                .changes
                .iter()
                .any(|change| change.path == Path::new("a.txt")),
            "the file that is already there is not listed as new"
        );
        assert!(
            gui.app().state.git.error.is_none(),
            "the panel still complains that this is not a repository"
        );
    }

    #[test]
    fn alt_9_shows_the_branch_and_what_changed() {
        let mut gui = managed(&[("a.txt", "one\n"), ("b.txt", "two\n")], Some("a.txt"));
        std::fs::write(gui._project.path().join("a.txt"), "one\nedited\n").unwrap();
        std::fs::write(gui._project.path().join("new.txt"), "fresh\n").unwrap();

        show_panel(&mut gui);

        assert_eq!(
            gui.app().state.git.branch().as_deref(),
            Some("main"),
            "the panel does not know the branch"
        );
        let changes: Vec<String> = gui
            .app()
            .state
            .git
            .changes
            .iter()
            .map(|change| format!("{} {}", change.kind.letter(), change.path.display()))
            .collect();
        assert_eq!(
            changes,
            vec!["M a.txt".to_owned(), "? new.txt".to_owned()],
            "the panel lists the wrong changes"
        );
        assert!(
            gui.harness.query_by_label("M  a.txt").is_some(),
            "the changed file is not on screen"
        );
    }
}

/// The commands that talk to a remote, against a remote that lives on this disk.
#[cfg(test)]
mod the_remote {
    use super::the_git_panel::{git, managed, show_panel};
    use super::*;
    use std::time::{Duration, Instant};

    /// A bare repository to push to, and `origin` in the project pointed at it.
    fn remote_for(gui: &Gui) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote.git");
        git(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", "remote.git"],
        );
        git(
            gui._project.path(),
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        // The directory is handed back, so the remote lives exactly as long as the test
        // does and no two tests share one.
        (dir, remote)
    }

    fn head_of(path: &Path) -> String {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .expect("git ran");
        assert!(output.status.success(), "rev-parse failed");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn wait(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            gui.settle();
            if check(gui) {
                return;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        panic!(
            "{what} did not happen; the panel says {:?}",
            gui.app().state.git.outcome
        );
    }

    #[test]
    fn a_project_without_a_remote_offers_no_fetch_pull_or_push() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        show_panel(&mut gui);
        assert!(
            !gui.app().state.git.has_remote,
            "a project with no remote believes it has one"
        );
        assert!(
            gui.harness.query_by_label("Push").is_none(),
            "there is a Push button for a project with nowhere to push"
        );
    }

    #[test]
    fn push_sends_the_branch_and_sets_up_the_upstream() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        let (_remote_dir, remote) = remote_for(&gui);
        // The panel learns about a remote when it re-reads, which it is asked to do
        // here: the test does not want to wait out the timer.
        gui.app_mut().state.git.refresh(true);
        show_panel(&mut gui);

        gui.harness.get_by_label("Push").click();
        wait(&mut gui, "the push finishing", |gui| {
            gui.app().state.git.network_busy.is_none()
        });

        let there = head_of(&remote);
        let here = head_of(gui._project.path());
        assert_eq!(there, here, "the remote does not hold the same commit");
        let said = gui.app().state.git.outcome.clone().unwrap_or_default();
        assert!(
            !said.trim().is_empty(),
            "the panel says nothing about what git answered"
        );
        assert!(
            gui.app().state.git.error.is_none(),
            "the panel reports an error after a push that worked: {:?}",
            gui.app().state.git.error
        );
    }

    #[test]
    fn a_push_to_a_remote_that_is_not_there_is_reported_and_changes_nothing() {
        let mut gui = managed(&[("a.txt", "one\n")], Some("a.txt"));
        let before = head_of(gui._project.path());
        git(
            gui._project.path(),
            &["remote", "add", "origin", "/nowhere/at/all.git"],
        );
        gui.app_mut().state.git.refresh(true);
        show_panel(&mut gui);

        gui.harness.get_by_label("Push").click();
        wait(&mut gui, "the push finishing", |gui| {
            gui.app().state.git.network_busy.is_none()
        });

        let said = gui.app().state.git.outcome.clone().unwrap_or_default();
        assert!(
            said.contains("nowhere") || said.contains("does not appear"),
            "git's own complaint did not reach the panel: {said:?}"
        );
        assert_eq!(
            head_of(gui._project.path()),
            before,
            "the refusal moved HEAD"
        );
    }
}

/// The conversation panel, driven by the stand-in agent the crate ships for its tests.
#[cfg(test)]
mod the_agent_panel {
    use super::*;
    use std::time::{Duration, Instant};

    /// The stand-in agent, wherever cargo put it.
    ///
    /// `CARGO_BIN_EXE_…` is only defined for the package that owns the binary, so the
    /// path is built from the target directory instead: `cargo test --workspace` builds
    /// that binary, and a run of this package alone does not — which is why the tests
    /// say they were skipped rather than failing on a missing fixture.
    fn stand_in() -> Option<PathBuf> {
        let profile = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"));
        let path = profile.join("debug").join("acp_test_agent");
        path.is_file().then_some(path)
    }

    fn with_agent() -> Option<Gui> {
        with_agent_env(&[])
    }

    /// The same window, with settings for the stand-in agent it starts: how many answers it
    /// offers, how many models it has.
    fn with_agent_env(env: &[(&str, &str)]) -> Option<Gui> {
        let agent = stand_in()?;
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".andy-ide")).unwrap();
        let mut file = format!(
            "[[agent]]\nname = \"stand-in\"\ncommand = {:?}\nargs = []\n",
            agent.to_string_lossy()
        );
        if !env.is_empty() {
            file.push_str("\n[agent.env]\n");
            for (name, value) in env {
                file.push_str(&format!("{name} = {value:?}\n"));
            }
        }
        std::fs::write(project.path().join(".andy-ide").join("agents.toml"), file).unwrap();
        let mut gui = Gui {
            harness: window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            ),
            _project: project,
            _config: Some(config),
        };
        gui.settle();
        Some(gui)
    }

    /// Starts the panel's stand-in agent and asks it for a choice, up to the point where the
    /// card is on screen and the agent is waiting.
    fn ask_for_a_choice(gui: &mut Gui) {
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        gui.app_mut().state.agent.input = "choose".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(gui, "the variants", |gui| {
            gui.app().state.agent.choice.is_some()
        });
    }

    fn wait(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            gui.settle();
            if check(gui) {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "{what} did not happen; the panel says {:?} with {:?}",
            gui.app().state.agent.status,
            gui.app().state.agent.transcript
        );
    }

    /// Waits while the panel itself keeps asking for frames.
    ///
    /// `settle` runs until nothing more asks to be drawn, and a panel waiting to
    /// reconnect asks on a timer: once the timer is due the request is immediate and
    /// `settle` never finishes. Here one frame is drawn at a time instead.
    fn wait_stepping(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            gui.harness.step();
            if check(gui) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "{what} did not happen; the panel says {:?} with {:?}",
            gui.app().state.agent.status,
            gui.app().state.agent.transcript
        );
    }

    /// What egui drew to warn about two widgets sharing an id: a red box, or a line of text
    /// marked with a flame.
    fn warnings_of(gui: &Gui) -> Vec<String> {
        let mut found = Vec::new();
        for clipped in &gui.harness.output().shapes {
            match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.job.text.starts_with('\u{1f525}')
                        || text.galley.job.text.contains("Second use") =>
                {
                    found.push(text.galley.job.text.clone());
                }
                egui::Shape::Rect(rect)
                    if rect.stroke.color == egui::Color32::from_rgb(255, 0, 0) =>
                {
                    found.push(format!("a red box at {:?}", rect.rect));
                }
                _ => {}
            }
        }
        found
    }

    #[test]
    fn the_foots_button_stops_the_answer_and_not_the_agent() {
        // If the answer is not worth waiting for, the answer is what is stopped. The agent
        // stays running: a session is not thrown away because one turn was.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        // The stand-in thinks for five seconds on this word: a turn with time to be stopped.
        gui.app_mut().state.agent.input = "slow".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer being written", |gui| {
            gui.app().state.agent.busy
        });

        gui.settle();
        assert!(
            gui.harness.query_by_label("Cancel").is_some(),
            "the foot offers no way to stop the answer"
        );
        gui.harness.get_by_label("Cancel").click();
        wait(&mut gui, "the answer stopping", |gui| {
            !gui.app().state.agent.busy
        });
        assert!(
            gui.app().state.agent.running(),
            "stopping the answer stopped the agent as well"
        );
        assert!(
            gui.app().state.agent.status.contains("cancelled"),
            "the panel does not say the turn was stopped: {:?}",
            gui.app().state.agent.status
        );
        gui.app_mut().state.agent.stop();
    }

    #[test]
    fn a_restored_session_that_is_still_working_offers_to_stop_it() {
        // The agent a person stopped may have been in the middle of a turn: the session
        // comes back writing, and nothing will announce the end of a turn this panel never
        // started. The words are the whole account of the work — so the foot's button and
        // its clock follow the words, and the silence is what ends it. A button that said
        // "Send" over an answer being written is an offer to talk over it.
        let Some(mut gui) = with_agent_env(&[("ACP_TEST_RESUME_WORKS", "1")]) else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        // Stopped and started again: the restoration a person does by hand.
        gui.harness.get_by_label("Stop").click();
        wait(&mut gui, "the agent stopping", |gui| {
            !gui.app().state.agent.running()
        });
        gui.harness.get_by_label("Start").click();

        // The restored work is writing: the foot offers to stop it, and the clock runs.
        wait(&mut gui, "the restored work", |gui| {
            gui.app().state.agent.working()
        });
        gui.settle();
        assert!(
            gui.harness.query_by_label("Cancel").is_some(),
            "the restored work cannot be stopped from the foot"
        );
        let running = gui
            .app()
            .state
            .agent
            .elapsed()
            .expect("no clock for the restored work");
        std::thread::sleep(Duration::from_millis(250));
        gui.settle();
        let later = gui.app().state.agent.elapsed().expect("the clock");
        assert!(
            later > running,
            "the clock of the restored work is standing still: {running:?} then {later:?}"
        );

        // Quiet means the answer is over: the figure is kept and the foot sends again.
        wait(&mut gui, "the foot going back to Send", |gui| {
            gui.harness.query_by_label("Send").is_some()
                && gui.harness.query_by_label("Cancel").is_none()
        });
        let done = gui
            .app()
            .state
            .agent
            .elapsed()
            .expect("no figure for the finished work");
        std::thread::sleep(Duration::from_millis(300));
        gui.settle();
        assert_eq!(
            gui.app().state.agent.elapsed().expect("the figure"),
            done,
            "the clock of a finished turn kept running"
        );
        gui.app_mut().state.agent.stop();
    }

    fn answer(gui: &Gui) -> String {
        gui.app()
            .state
            .agent
            .transcript
            .iter()
            .filter_map(|line| match line {
                crate::agent::Line::Agent(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// What the agent said last: the line an answer about the session is, and not the whole
    /// replayed history the panel may have come back with.
    fn last_answer(gui: &Gui) -> String {
        gui.app()
            .state
            .agent
            .transcript
            .iter()
            .rev()
            .find_map(|line| match line {
                crate::agent::Line::Agent(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// What the panel said about a switch of model or level, in the order it said it.
    fn switches(gui: &Gui) -> Vec<String> {
        gui.app()
            .state
            .agent
            .transcript
            .iter()
            .filter_map(|line| match line {
                crate::agent::Line::Note(text)
                    if text.starts_with("model: ") || text.starts_with("thinking: ") =>
                {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_project_keeps_the_agent_that_was_on_duty_and_opens_it_again() {
        // Who was working belongs to the project, like the layout: the next launch opens
        // that agent as though the person had pressed Start the moment the panel was in
        // front of them. An agent the file no longer knows is left alone.
        let Some(agent) = stand_in() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let place = project.path().join(".andy-ide");
        std::fs::create_dir_all(&place).unwrap();
        std::fs::write(
            place.join("agents.toml"),
            format!(
                "[[agent]]\nname = \"stand-in\"\ncommand = {:?}\nargs = []\n",
                agent.to_string_lossy()
            ),
        )
        .unwrap();

        let config = tempfile::tempdir().unwrap();
        let mut first = Gui {
            harness: window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            ),
            _project: tempfile::tempdir().unwrap(),
            _config: Some(config),
        };
        first.settle();
        first
            .harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        first.settle();
        first.harness.get_by_label("Start").click();
        wait(&mut first, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        // The window closes: the workspace is written on close, and it says who was on duty.
        first.app_mut().state.save_workspace(None, 1000.0, true);
        drop(first);

        // The next launch: the agent is up before a finger is lifted, and it is running
        // the way the button would be told to.
        let config = tempfile::tempdir().unwrap();
        let mut second = Gui {
            harness: window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            ),
            _project: tempfile::tempdir().unwrap(),
            _config: Some(config),
        };
        second
            .harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        wait_stepping(&mut second, "the agent opening on its own", |gui| {
            gui.app().state.agent.running() && gui.app().state.agent.status.starts_with("ready")
        });
        assert!(
            second.harness.query_by_label("Stop").is_some(),
            "the agent did not open on its own"
        );
    }

    #[test]
    fn the_project_keeps_the_conversation_and_picks_it_up_next_time() {
        // A conversation is worth keeping: an agent that has read the project once knows
        // it. The id goes into `.andy-ide`, beside the layout, and the next launch asks
        // the agent to pick it up instead of starting from nothing.
        let Some(agent) = stand_in() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let place = project.path().join(".andy-ide");
        std::fs::create_dir_all(&place).unwrap();
        std::fs::write(
            place.join("agents.toml"),
            format!(
                "[[agent]]\nname = \"stand-in\"\ncommand = {:?}\nargs = []\n",
                agent.to_string_lossy()
            ),
        )
        .unwrap();

        let mut first = {
            let config = tempfile::tempdir().unwrap();
            let mut gui = Gui {
                harness: window(
                    project.path(),
                    config.path(),
                    &[("a.txt", "one\n")],
                    Some("a.txt"),
                ),
                _project: tempfile::tempdir().unwrap(),
                _config: Some(config),
            };
            gui.settle();
            gui
        };
        first
            .harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        first.settle();
        first.harness.get_by_label("Start").click();
        wait(&mut first, "the session", |gui| {
            gui.app().state.agent.session.is_some()
        });

        let kept = std::fs::read_to_string(place.join("agent-session.json"))
            .expect("the project kept no conversation");
        assert!(
            kept.contains("stand-in-session"),
            "the id is not in the file: {kept}"
        );
        assert!(
            kept.contains("stand-in"),
            "the file does not say whose conversation it is: {kept}"
        );

        // The window closes — the agent process goes with it — and the project is opened
        // again, as on the next launch. The workspace is written on close, so the closing
        // window says who was on duty.
        first.app_mut().state.save_workspace(None, 1000.0, true);
        drop(first);

        let mut second = {
            let config = tempfile::tempdir().unwrap();
            let mut gui = Gui {
                harness: window(
                    project.path(),
                    config.path(),
                    &[("a.txt", "one\n")],
                    Some("a.txt"),
                ),
                _project: tempfile::tempdir().unwrap(),
                _config: Some(config),
            };
            gui.settle();
            gui
        };
        second
            .harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        // Nothing is pressed: the agent that was on duty when the window closed opens on
        // its own, as though the person had pressed Start the moment the panel was in
        // front of them, and the conversation it kept comes with it.
        wait(
            &mut second,
            "the conversation coming back on its own",
            |gui| gui.app().state.agent.status.contains("resumed"),
        );
        assert!(
            second.harness.query_by_label("Stop").is_some(),
            "the reopened window did not open its agent by itself"
        );

        // And the agent itself agrees: this is the conversation it kept, not a new one.
        second.app_mut().state.agent.input = "session".to_owned();
        second.settle();
        second.harness.get_by_label("Send").click();
        wait(&mut second, "the answer about the session", |gui| {
            answer(gui).contains("loaded")
        });
        assert_eq!(last_answer(&second), "stand-in-session loaded");
    }

    #[test]
    fn the_setup_row_makes_room_for_the_conversation_while_the_agent_runs() {
        // The row of the name and the command is the desk the agent is set up at; a running
        // agent is not being set up there, and the strip is only a row the conversation did
        // not ask for. It comes back when the agent stops, because the next setup happens
        // then.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        assert!(
            gui.harness.query_by_label("Name").is_some(),
            "the setup row is not there before the start"
        );
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        assert!(
            gui.harness.query_by_label("Name").is_none(),
            "the setup row is still there while the agent runs"
        );
        gui.harness.get_by_label("Stop").click();
        wait(&mut gui, "the agent stopping", |gui| {
            !gui.app().state.agent.running()
        });
        gui.settle();
        assert!(
            gui.harness.query_by_label("Name").is_some(),
            "the setup row did not come back after the stop"
        );
    }

    #[test]
    fn the_connection_says_what_it_is_doing() {
        // A start that is slow is a start the panel has to speak for: between "Start" and
        // "ready" it says which step it is at, so a person watching does not have to guess
        // whether the agent is coming up or never going to.
        let Some(mut gui) = with_agent_env(&[("ACP_TEST_INIT_DELAY_MS", "600")]) else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait_stepping(&mut gui, "the step about the agent", |gui| {
            gui.harness
                .query_by_label_contains("Getting agent parameters")
                .is_some()
        });
        // The step is being held: the agent deliberately answers slowly, and the panel is
        // still saying it waits for the parameters.
        gui.settle();
        assert!(
            gui.harness
                .query_by_label_contains("Getting agent parameters")
                .is_some(),
            "the step vanished while the agent was still slow: {:?}",
            gui.app().state.agent.status
        );
        wait(&mut gui, "the agent coming up", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        gui.settle();
        assert!(
            gui.harness
                .query_by_label_contains("Getting agent parameters")
                .is_none(),
            "the step stayed after the start finished"
        );
    }

    #[test]
    fn a_restored_session_brings_back_the_words_of_both_sides() {
        // On `session/load` an agent replays the conversation it kept, and the replay carries
        // the person's own words as well as its answers — one chunk per message with the role,
        // like Hermes' (`acp_adapter/server.py::_history_replay_updates`). Drawn into the same
        // line, the words of both sides would read as a conversation that was all answers, and
        // a person opening the panel would find half of what they said gone.
        let Some(agent) = stand_in() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let place = project.path().join(".andy-ide");
        std::fs::create_dir_all(&place).unwrap();
        std::fs::write(
            place.join("agents.toml"),
            format!(
                "[[agent]]\nname = \"stand-in\"\ncommand = {:?}\nargs = []\n",
                agent.to_string_lossy()
            ),
        )
        .unwrap();
        std::fs::write(
            place.join("agent-session.json"),
            "{\"agent\":\"stand-in\",\"session\":\"kept-42\"}\n",
        )
        .unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui {
            harness: window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            ),
            _project: tempfile::tempdir().unwrap(),
            _config: Some(config),
        };
        gui.settle();
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the conversation coming back", |gui| {
            gui.app().state.agent.status.contains("resumed")
        });

        let lines = gui.app().state.agent.transcript.clone();
        assert!(
            lines.iter().any(|line| matches!(
                line,
                crate::agent::Line::You(text) if text == "why is the sky blue?"
            )),
            "the person's own words did not come back: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| matches!(
                line,
                crate::agent::Line::Agent(text) if text == "Because air scatters blue light."
            )),
            "the answer did not come back in a line of its own: {lines:?}"
        );

        // And a fresh answer after the restore starts a line of its own, instead of growing
        // into the replayed answer as though both were one message.
        gui.app_mut().state.agent.input = "session".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer about the session", |gui| {
            last_answer(gui).contains("loaded")
        });
        assert_eq!(last_answer(&gui), "kept-42 loaded");
    }

    #[test]
    fn a_new_session_does_not_pick_up_the_conversation_the_project_kept() {
        // `Start` resumes what the project saved. "New session" must end that, including
        // the copy on disk: otherwise the next launch brings the old conversation back.
        let Some(agent) = stand_in() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let place = project.path().join(".andy-ide");
        std::fs::create_dir_all(&place).unwrap();
        std::fs::write(
            place.join("agents.toml"),
            format!(
                "[[agent]]\nname = \"stand-in\"\ncommand = {:?}\nargs = []\n",
                agent.to_string_lossy()
            ),
        )
        .unwrap();
        // A conversation an earlier run left behind.
        std::fs::write(
            place.join("agent-session.json"),
            "{\"agent\":\"stand-in\",\"session\":\"kept-42\"}\n",
        )
        .unwrap();

        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui {
            harness: window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            ),
            _project: tempfile::tempdir().unwrap(),
            _config: Some(config),
        };
        gui.settle();
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the kept conversation coming back", |gui| {
            gui.app().state.agent.status.contains("resumed")
        });
        gui.app_mut().state.agent.input = "session".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer about the session", |gui| {
            answer(gui).contains("loaded")
        });
        assert_eq!(last_answer(&gui), "kept-42 loaded");

        gui.harness.get_by_label("New session").click();
        wait(&mut gui, "the new conversation", |gui| {
            gui.app()
                .state
                .agent
                .status
                .contains("ready, session stand-in-session")
        });
        assert!(
            !gui.app().state.agent.status.contains("resumed"),
            "the fresh session was announced as a resumed one: {}",
            gui.app().state.agent.status
        );

        gui.app_mut().state.agent.input = "session".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer about the new session", |gui| {
            answer(gui).contains("new")
        });
        assert_eq!(answer(&gui), "stand-in-session new");

        let kept = std::fs::read_to_string(place.join("agent-session.json"))
            .expect("the project kept no conversation");
        assert!(
            kept.contains("stand-in-session") && !kept.contains("kept-42"),
            "the old conversation is still the one on disk: {kept}"
        );
    }

    #[test]
    fn folding_a_provider_does_not_dismiss_the_model_list() {
        // The list is grouped by provider, and each group folds. Folding one is how a
        // person looks past it at another, so a click on a group's own header must leave
        // the menu open — egui closes a menu on any click inside it by default.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the models", |gui| {
            gui.app().state.agent.models.len() == 4
        });

        gui.harness.get_by_label_contains("Small ▾").click();
        gui.settle();
        assert!(
            gui.harness.query_by_label_contains("Hermes 4").is_some(),
            "the list did not open"
        );
        // The provider is on the group's own line, not on every row under it.
        assert!(
            gui.harness.query_by_label("Hermes 4").is_some(),
            "the model row is not named by the model alone"
        );
        assert!(
            gui.harness.query_by_label_contains("Nous \u{b7}").is_none(),
            "the provider is repeated on every model row"
        );

        // Fold the "nous" group: its model goes, the list and the other provider stay.
        gui.harness.get_by_label("nous").click();
        gui.settle();
        assert!(
            gui.harness.query_by_label_contains("Hermes 4").is_none(),
            "the group did not fold"
        );
        assert!(
            gui.harness.query_by_label_contains("GPT-5").is_some(),
            "the list closed when a provider group was folded"
        );
        assert!(
            gui.harness.query_by_label_contains("Large").is_some(),
            "the list closed when a provider group was folded"
        );

        // Unfolding is the same click, and it must not dismiss the list either.
        gui.harness.get_by_label("nous").click();
        gui.settle();
        assert!(
            gui.harness.query_by_label_contains("Hermes 4").is_some(),
            "the group did not unfold, or the list closed"
        );

        // And a model that is actually chosen still ends the visit.
        gui.harness.get_by_label_contains("GPT-5").click();
        wait(&mut gui, "the model switching", |gui| {
            gui.app().state.agent.current_model.as_deref() == Some("openai/gpt-5")
        });
        assert!(
            gui.harness.query_by_label_contains("Hermes 4").is_none(),
            "the list stayed open after a model was chosen"
        );
    }

    #[test]
    fn the_model_list_scrolls_when_it_is_longer_than_the_window() {
        // A long list is why the grouping exists at all, and a list that cannot be
        // scrolled leaves the providers at the bottom unreachable.
        let Some(agent) = stand_in() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let place = project.path().join(".andy-ide");
        std::fs::create_dir_all(&place).unwrap();
        // The stand-in offers thirty models when it is told to, which is longer than the
        // box the panel gives the list.
        std::fs::write(
            place.join("agents.toml"),
            format!(
                "[[agent]]\nname = \"stand-in\"\ncommand = {:?}\nargs = []\n\n\
                 [agent.env]\nACP_TEST_MANY_MODELS = \"1\"\n",
                agent.to_string_lossy()
            ),
        )
        .unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui {
            harness: window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            ),
            _project: tempfile::tempdir().unwrap(),
            _config: Some(config),
        };
        gui.settle();
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the long list of models", |gui| {
            gui.app().state.agent.models.len() == 30
        });

        gui.harness.get_by_label_contains("Small ▾").click();
        gui.settle();
        let before = gui
            .harness
            .query_by_label_contains("Large")
            .expect("the list did not open")
            .rect()
            .min
            .y;

        // The pointer has to be over the list: egui only sends the wheel to what is
        // under it.
        gui.harness.get_by_label("Nous 1").hover();
        gui.harness.step();
        for _ in 0..6 {
            gui.harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -60.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::default(),
            });
            gui.harness.step();
        }

        // The entries moved up — or the first ones went out of view entirely, which is the
        // same thing said by a list that scrolled.
        let moved = match gui.harness.query_by_label_contains("Large") {
            Some(node) => node.rect().min.y < before,
            None => true,
        };
        assert!(moved, "the wheel did not move the model list");
        assert!(
            gui.app().state.agent.models_open,
            "the list closed while it was scrolled"
        );
    }

    #[test]
    fn a_change_of_model_or_thinking_is_said_in_the_conversation() {
        // A switch changes what answers and how hard it thinks. Nothing else in the panel
        // says so in the conversation, and an answer written by a model a person did not
        // ask for reads as a mistake.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the models and settings", |gui| {
            gui.app().state.agent.models.len() == 4 && gui.app().state.agent.options.len() == 2
        });
        assert!(
            switches(&gui).is_empty(),
            "a switch was announced before one happened: {:?}",
            switches(&gui)
        );

        gui.harness.get_by_label_contains("Small ▾").click();
        gui.settle();
        gui.harness.get_by_label_contains("GPT-5").click();
        wait(&mut gui, "the model switching", |gui| {
            gui.app().state.agent.current_model.as_deref() == Some("openai/gpt-5")
        });
        assert_eq!(
            switches(&gui),
            vec!["model: OpenAI \u{b7} GPT-5".to_owned()],
            "the model switch was not said in the conversation"
        );

        // The level of thinking: a setting of its own, with the levels the agent names.
        gui.harness.get_by_label_contains("default ▾").click();
        gui.settle();
        gui.harness.get_by_label("high").click();
        wait(&mut gui, "the level changing", |gui| {
            gui.app()
                .state
                .agent
                .options
                .iter()
                .find(|option| option.id == "reasoning_effort")
                .and_then(|option| option.current.as_deref())
                == Some("high")
        });
        assert_eq!(
            switches(&gui),
            vec![
                "model: OpenAI \u{b7} GPT-5".to_owned(),
                "thinking: high".to_owned()
            ],
            "the level was not said in the conversation"
        );
    }

    #[test]
    fn an_agent_that_offers_no_level_says_so_instead_of_offering_one() {
        // Hermes 0.22.5 offers the edit policy over ACP and nothing else: there the level
        // belongs to the model in the Hermes profile. A panel that offered its own levels
        // anyway would offer levels the agent has never heard of.
        let Some(agent) = stand_in() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let place = project.path().join(".andy-ide");
        std::fs::create_dir_all(&place).unwrap();
        std::fs::write(
            place.join("agents.toml"),
            format!(
                "[[agent]]\nname = \"stand-in\"\ncommand = {:?}\nargs = [\"--no-levels\"]\n",
                agent.to_string_lossy()
            ),
        )
        .unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui {
            harness: window(
                project.path(),
                config.path(),
                &[("a.txt", "one\n")],
                Some("a.txt"),
            ),
            _project: tempfile::tempdir().unwrap(),
            _config: Some(config),
        };
        gui.settle();
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the models and the edit policy", |gui| {
            gui.app().state.agent.models.len() == 4 && gui.app().state.agent.options.len() == 1
        });
        assert!(
            crate::agent::thought_level(&gui.app().state.agent.options).is_none(),
            "a level of thinking was read out of an agent that offers none"
        );

        // The line is there, and it is not a menu: pressing it offers nothing, because
        // there is nothing to offer.
        gui.harness.get_by_label_contains("thinking —").click();
        gui.settle();
        assert!(
            gui.harness.query_by_label("high").is_none(),
            "the panel offered levels the agent does not have"
        );

        // What the agent does offer is still a menu: the edit policy, which is a mode.
        gui.harness.get_by_label_contains("Default ▾").click();
        gui.settle();
        assert!(
            gui.harness.query_by_label_contains("Don't Ask").is_some(),
            "the modes the agent offers are not offered"
        );
    }

    #[test]
    fn alt_0_shows_the_panel_with_the_agents_from_the_file() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();

        let names: Vec<String> = gui
            .app()
            .state
            .agent
            .registry
            .agents
            .iter()
            .map(|agent| agent.name.clone())
            .collect();
        assert_eq!(
            names,
            vec!["stand-in".to_owned()],
            "the panel did not read agents.toml"
        );
        assert_eq!(gui.app().state.agent.chosen.as_deref(), Some("stand-in"));
        assert!(
            gui.harness.query_by_label("Start").is_some(),
            "there is no way to start the agent"
        );
    }

    #[test]
    fn a_command_typed_into_the_field_is_what_runs() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();

        // A command that is in no file anywhere: typed, and started as typed.
        let typed = format!("{:?} --not-an-argument-it-knows", stand_in().unwrap());
        gui.app_mut().state.agent.command = typed.clone();
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the typed agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "hello".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer from the typed agent", |gui| {
            !answer(gui).is_empty()
        });
        assert_eq!(answer(&gui), "Hello");

        // An unreadable command is refused in words, before anything is spawned.
        gui.app_mut().state.agent.stop();
        gui.app_mut().state.agent.command = "agent \"unfinished".to_owned();
        gui.settle();
        gui.harness.get_by_label("Start").click();
        gui.settle();
        assert!(
            !gui.app().state.agent.running(),
            "an unreadable command was started"
        );
        assert!(
            gui.app().state.agent.status.contains("never closed"),
            "the refusal is not in words: {:?}",
            gui.app().state.agent.status
        );
    }

    #[test]
    fn remember_writes_the_typed_command_into_the_file() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();

        let typed = format!("{:?}", stand_in().unwrap());
        gui.app_mut().state.agent.command = typed.clone();
        gui.settle();
        gui.harness.get_by_label("Remember").click();
        gui.settle();

        let place = gui._project.path().join(".andy-ide").join("agents.toml");
        let text = std::fs::read_to_string(&place).expect("the file");
        assert!(
            text.contains("acp_test_agent"),
            "the typed command is not in the file: {text}"
        );
        let read = ide_agents::Registry::load(&place).expect("reading it back");
        // The name is the one in the field, not one guessed from the program: the entry
        // that was there keeps its name and takes the new command.
        let saved = read
            .get("stand-in")
            .unwrap_or_else(|| panic!("the agent did not come back: {:?}", read.agents));
        assert_eq!(
            saved.command,
            stand_in().unwrap().to_string_lossy(),
            "the command was not saved"
        );
    }

    #[test]
    fn enter_in_the_agent_field_stays_out_of_the_editor() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        let before = gui.app().state.active_buffer().unwrap().text().to_owned();
        // The last text field in the panel is the one that talks to the agent: the
        // command field is drawn above it.
        let fields = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput);
        let last = fields.last().expect("the message field");
        last.click();
        gui.settle();
        gui.harness.event(egui::Event::Text("hi".to_owned()));
        gui.settle();
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();

        assert_eq!(
            gui.app().state.active_buffer().unwrap().text(),
            before,
            "the Enter meant for the agent put a newline in the editor"
        );
        assert!(
            gui.app()
                .state
                .agent
                .transcript
                .iter()
                .any(|line| matches!(line, crate::agent::Line::You(text) if text == "hi")),
            "the message was not sent: {:?}",
            gui.app().state.agent.transcript
        );
    }

    #[test]
    fn a_question_gets_an_answer_that_streams_in() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "hello".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer", |gui| !answer(gui).is_empty());

        assert_eq!(answer(&gui), "Hello", "the pieces did not add up");
        assert!(
            gui.harness.query_by_label_contains("Hello").is_some(),
            "the answer is not on screen"
        );
        wait(&mut gui, "the turn ending", |gui| {
            !gui.app().state.agent.busy
        });
    }

    #[test]
    fn an_answer_is_read_as_markdown_while_it_arrives() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "markdown".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer", |gui| !answer(gui).is_empty());

        // The text keeps its Markdown, the screen does not show the markers: what is
        // drawn is the rendered form, not the source.
        assert_eq!(answer(&gui), "**bold** and `code`");
        assert!(
            gui.harness.query_by_label_contains("**bold**").is_none(),
            "the answer is on screen as Markdown source, not as Markdown"
        );
        assert!(
            gui.harness.query_by_label_contains("bold").is_some(),
            "the bold word is not on screen at all"
        );
    }

    #[test]
    fn the_keyboard_stays_in_the_field_after_a_message_is_sent() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        let before = gui.app().state.active_buffer().unwrap().text().to_owned();
        let fields = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput);
        fields.last().expect("the message field").click();
        gui.settle();
        gui.harness.event(egui::Event::Text("one".to_owned()));
        gui.settle();
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();

        // The next thing typed, with no click in between, is a message and not code.
        gui.harness.event(egui::Event::Text("next".to_owned()));
        gui.settle();
        assert_eq!(
            gui.app().state.agent.input,
            "next",
            "the field lost the keyboard after sending: focus={:?} editor={:?} status={:?}",
            gui.harness.ctx.memory(|memory| memory.focused()),
            gui.app()
                .state
                .active_buffer()
                .map(|buffer| buffer.text().to_owned()),
            gui.app().state.agent.status
        );
        assert_eq!(
            gui.app().state.active_buffer().unwrap().text(),
            before,
            "the next message was typed into the file"
        );

        // The keyboard is still the field's own: nothing else has taken it between the
        // send and now.
        assert_eq!(
            gui.harness.ctx.memory(|memory| memory.focused()),
            Some(egui::Id::new("agent-message")),
            "the field does not hold the keyboard after sending"
        );
    }

    #[test]
    fn a_message_typed_mid_answer_is_queued_and_then_sent() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "slow".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        gui.settle();
        assert!(gui.app().state.agent.busy, "the agent is not answering yet");

        // Written while the answer is still coming: kept, and the field keeps the keys.
        let fields = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput);
        fields.last().expect("the message field").click();
        gui.settle();
        gui.harness.event(egui::Event::Text("next".to_owned()));
        gui.settle();
        assert_eq!(gui.app().state.agent.input, "next");
        gui.harness.key_press(egui::Key::Enter);
        gui.settle();
        assert!(
            gui.app().state.agent.status.contains("queued"),
            "the message was not queued: {:?}",
            gui.app().state.agent.status
        );

        // And it goes out by itself when the answer ends — nobody has to press Enter twice.
        wait(&mut gui, "the queued message going out", |gui| {
            gui.app()
                .state
                .agent
                .transcript
                .iter()
                .any(|line| matches!(line, crate::agent::Line::You(text) if text == "next"))
        });
    }

    #[test]
    fn the_status_line_names_the_model_the_level_and_the_context() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        // What it offers arrived with the session, before anything was asked: the models,
        // the edit modes, and the settings — of which the level of thinking is one.
        wait(&mut gui, "the models and modes", |gui| {
            gui.app().state.agent.models.len() == 4
                && gui.app().state.agent.modes.len() == 3
                && gui.app().state.agent.options.len() == 2
        });
        assert_eq!(
            gui.app()
                .state
                .agent
                .models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Small",
                "Large",
                "Nous \u{b7} Hermes 4",
                "OpenAI \u{b7} GPT-5"
            ]
        );
        assert_eq!(
            gui.app().state.agent.current_model.as_deref(),
            Some("small")
        );
        assert!(
            gui.harness.query_by_label_contains("⏱").is_some(),
            "the status line is not drawn at all"
        );
        assert!(
            gui.harness.query_by_label_contains("Small").is_some(),
            "the model is not on the status line"
        );
        assert!(
            gui.harness.query_by_label_contains("default").is_some(),
            "the level of thinking is not on the status line"
        );

        // Choosing another model asks the agent, and the line follows the answer.
        gui.harness.get_by_label_contains("Small ▾").click();
        gui.settle();
        gui.harness.get_by_label_contains("Large").click();
        wait(&mut gui, "the model switching", |gui| {
            gui.app().state.agent.current_model.as_deref() == Some("large")
        });

        // A turn: the timer runs, and the context figure arrives with the answer.
        gui.app_mut().state.agent.input = "model".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer and its figure", |gui| {
            gui.app().state.agent.usage.is_some()
        });
        // The mode the agent reports is its edit policy, which is what ACP modes carry.
        assert_eq!(answer(&gui), "large / ask", "the switch did not arrive");
        assert!(
            gui.app().state.agent.elapsed().is_some(),
            "the turn left no time behind"
        );
        assert!(
            gui.harness.query_by_label_contains("42% of 100k").is_some(),
            "the context figure is not on the status line: {:?}",
            gui.app().state.agent.usage
        );
        assert!(
            gui.harness.query_by_label_contains("⏱").is_some(),
            "the timer is not on the status line"
        );
    }

    #[test]
    fn the_name_given_by_hand_is_the_one_the_list_shows() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();

        // A name of its own: two agents run from one binary are two entries to a person,
        // and a name taken from the program cannot tell them apart.
        gui.app_mut().state.agent.name = "Fast stand-in".to_owned();
        gui.settle();
        gui.harness.get_by_label("Remember").click();
        gui.settle();

        let place = gui._project.path().join(".andy-ide").join("agents.toml");
        let text = std::fs::read_to_string(&place).expect("the file");
        assert!(
            text.contains("Fast stand-in"),
            "the name is not in the file: {text}"
        );
        let read = ide_agents::Registry::load(&place).expect("reading it back");
        assert!(
            read.get("Fast stand-in").is_some(),
            "the agent did not come back under its name: {:?}",
            read.agents
        );
    }

    #[test]
    fn the_name_and_the_command_are_on_one_row() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();

        let fields = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::TextInput);
        // The message field is the field of several lines, and it is the last row of all.
        let message = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .last()
            .expect("the message field")
            .rect();
        let rows: Vec<egui::Rect> = fields.map(|field| field.rect()).collect();
        assert!(
            rows.len() >= 2,
            "the name and the command are not both on screen: {rows:?}"
        );
        let (name, command) = (rows[0], rows[1]);
        let middle = |rect: egui::Rect| rect.center().y;
        assert!(
            (middle(name) - middle(command)).abs() < 1.0,
            "the fields are on two rows: {name:?} and {command:?}"
        );
        assert!(
            name.right() <= command.left(),
            "the fields overlap: {name:?} and {command:?}"
        );
        assert!(
            message.top() > command.bottom(),
            "the message field is not on a row of its own: {message:?}"
        );
    }

    #[test]
    fn a_selection_in_the_panel_is_copied_with_ctrl_insert() {
        // egui copies a label selection only when a copy *event* arrives, and the keyboard
        // backend makes one out of Ctrl+C alone. The editor holds the keyboard when
        // nothing else does, so the shortcut has to be recognised for the panel first.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        gui.app_mut().state.agent.input = "hello".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the answer", |gui| !answer(gui).is_empty());
        gui.settle();

        // Select the words the agent said, the way a hand does: press, drag, release.
        let label = gui
            .harness
            .query_by_label_contains("Hello")
            .expect("the answer is not on screen")
            .rect();
        let from = egui::pos2(label.left() + 1.0, label.center().y);
        let to = egui::pos2(label.right() - 1.0, label.center().y);
        gui.harness.event(egui::Event::PointerMoved(from));
        gui.harness.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        gui.harness.step();
        gui.harness.event(egui::Event::PointerMoved(to));
        gui.harness.step();
        gui.harness.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        gui.harness.step();

        gui.harness
            .event(egui::Event::ModifiersChanged(egui::Modifiers::CTRL));
        gui.harness.event(egui::Event::Key {
            key: egui::Key::Insert,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        });
        gui.harness.step();
        let output = gui.harness.output().clone();
        gui.harness
            .event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));

        let copied = output.platform_output.commands.iter().find_map(|command| {
            if let egui::OutputCommand::CopyText(text) = command {
                Some(text.clone())
            } else {
                None
            }
        });
        assert_eq!(
            copied.as_deref(),
            Some("Hello"),
            "Ctrl+Insert copied {copied:?} instead of the selected words"
        );
    }

    #[test]
    fn a_permission_is_a_question_with_buttons_and_the_turn_waits() {
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "ask".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the question", |gui| {
            gui.app().state.agent.asking.is_some()
        });

        let (_, request) = gui.app().state.agent.asking.clone().expect("a question");
        assert_eq!(request.title(), "Run the tests");
        assert!(
            gui.harness.query_by_label("Allow").is_some(),
            "the question is not on screen with its options"
        );

        gui.harness.get_by_label("Allow").click();
        wait(&mut gui, "the answer to the question", |gui| {
            answer(gui) == "allowed"
        });
        assert!(
            gui.app().state.agent.asking.is_none(),
            "the question stayed after it was answered"
        );
    }

    #[test]
    fn the_agent_can_ask_the_person_to_choose_a_variant() {
        // ACP's elicitation: the agent cannot decide which way to go, offers variants,
        // and waits. The panel draws them as buttons and sends back the value, not the
        // title — the title is for the person, the value is for the agent.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "choose".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the variants", |gui| {
            gui.app().state.agent.choice.is_some()
        });
        let (_, request) = gui.app().state.agent.choice.clone().expect("a choice");
        assert_eq!(request.title(), "Which variant should I write?");
        assert!(
            gui.harness
                .query_by_label("Readable \u{2014} a few lines, with the reasoning behind them")
                .is_some(),
            "the variants are not on screen"
        );

        gui.harness
            .get_by_label("Readable \u{2014} a few lines, with the reasoning behind them")
            .click();
        wait(&mut gui, "the chosen variant coming back", |gui| {
            answer(gui).contains("readable")
        });
        assert_eq!(answer(&gui), "readable");
        assert!(
            gui.app().state.agent.choice.is_none(),
            "the choice stayed after it was answered"
        );
    }

    #[test]
    fn the_variants_are_inside_the_panel_whatever_the_window() {
        // The answer to a question has to be on screen. Laid out to what the dock *could*
        // give rather than to what is drawn, a card comes out wider than the panel and its
        // refusal is cut away; grown to its own height, its offers go past the bottom edge,
        // where egui clips them and they take no click — the question is there, the answers
        // are not, and the agent waits for an answer nobody can give.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "choose".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the variants", |gui| {
            gui.app().state.agent.choice.is_some()
        });

        let inside = |gui: &Gui, label: &str| {
            let panel = gui
                .app()
                .state
                .agent
                .shown_rect
                .expect("the panel is drawn");
            let node = gui
                .harness
                .query_by_label(label)
                .unwrap_or_else(|| panic!("{label} is not on screen at all"));
            let rect = node.rect();
            assert!(
                panel.x_range().contains(rect.left())
                    && panel.x_range().contains(rect.right() - 1.0),
                "{label} runs out of the panel: {rect:?} in {panel:?}"
            );
            assert!(
                panel.y_range().contains(rect.top())
                    && panel.y_range().contains(rect.bottom() - 1.0),
                "{label} is past the panel's edge: {rect:?} in {panel:?}"
            );
        };

        // A card that has to scroll keeps a scroll bar, and a scroll bar fades in and out:
        // `settle` runs until nothing asks to be drawn, which a fading bar never does. A
        // handful of frames is enough to lay the panel out.
        let lay_out = |gui: &mut Gui| {
            for _ in 0..10 {
                gui.harness.step();
            }
        };

        for size in [
            egui::vec2(1200.0, 800.0),
            egui::vec2(1000.0, 620.0),
            egui::vec2(820.0, 560.0),
        ] {
            gui.harness.set_size(size);
            lay_out(&mut gui);
            // The first answer: the part that has to be on screen for a person to answer
            // at all, in any panel that can hold it. The question above it is drawn with it
            // — the card scrolls as one piece — and the rest of the answers are a scroll
            // away rather than off the panel.
            inside(&gui, "Compact \u{2014} one line, no detail");
        }

        // In a panel with room, nothing is left to a scroll: every variant is on screen.
        gui.harness.set_size(egui::vec2(1200.0, 800.0));
        lay_out(&mut gui);
        inside(&gui, "Compact \u{2014} one line, no detail");
        inside(
            &gui,
            "Readable \u{2014} a few lines, with the reasoning behind them",
        );
        inside(&gui, "Formal \u{2014} a full report in the usual structure");
    }

    #[test]
    fn no_two_widgets_share_an_id_in_the_panel() {
        // egui warns about two widgets under one id by drawing a red box and a line marked
        // with a flame over them — but only in a debug build, which is how the editor is run
        // while it is being written. Two scroll areas are the easy pair to fall into: an
        // unnamed one takes its id from the ui it is drawn in, and the panel draws the
        // conversation and the question card in the same ui.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        ask_for_a_choice(&mut gui);

        let warnings = |gui: &Gui| {
            let mut found = Vec::new();
            for clipped in &gui.harness.output().shapes {
                match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.job.text.starts_with('\u{1f525}') => {
                        found.push(text.galley.job.text.clone());
                    }
                    egui::Shape::Rect(rect)
                        if rect.stroke.color == egui::Color32::from_rgb(255, 0, 0) =>
                    {
                        found.push(format!("a red box at {:?}", rect.rect));
                    }
                    _ => {}
                }
            }
            found
        };

        // With the card up — the card holds a scroll area of its own — and after it is gone.
        assert!(
            warnings(&gui).is_empty(),
            "egui is warning about the panel: {:?}",
            warnings(&gui)
        );
        gui.harness.get_by_label("None of these").click();
        wait(&mut gui, "the refusal coming back", |gui| {
            answer(gui).contains("none")
        });
        gui.harness.step();
        assert!(
            warnings(&gui).is_empty(),
            "egui is warning about the panel: {:?}",
            warnings(&gui)
        );
    }

    #[test]
    fn two_thoughts_of_one_length_do_not_share_an_id() {
        // Every block of thinking is named "Thinking…", and their id used to be built from
        // the length of what was thought. Two thoughts of equal length therefore landed on
        // one id, and egui drew its warning box over them — the red frames that covered the
        // whole transcript the moment a session was restored. A thought being written also
        // changed length with every chunk, which is the box that flickers while an answer
        // streams.
        let mut gui = Gui::new(&[("a.txt", "one\n")], Some("a.txt"));
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        // Two thoughts of one length and different in their words: that is what a restored
        // conversation holds, and what one id for both would put a warning box over.
        let words = [
            "first", "thought", "about", "the", "parser", "in", "the", "file",
        ];
        let mut other = words;
        other[0] = "other";
        let first = words.join(" ");
        let second = other.join(" ");
        assert_eq!(
            first.len(),
            second.len(),
            "the two thoughts of this check have to be one length"
        );
        let transcript = &mut gui.app_mut().state.agent.transcript;
        transcript.push(crate::agent::Line::Thinking(first.to_owned()));
        transcript.push(crate::agent::Line::Thinking(second.to_owned()));
        gui.settle();

        assert!(
            warnings_of(&gui).is_empty(),
            "egui is warning about the thinking blocks: {:?}",
            warnings_of(&gui)
        );
    }

    #[test]
    fn a_table_of_long_cells_wraps_inside_the_panel() {
        // A cell of a table is a share of the panel wide and wraps inside it. The renderer
        // puts a cell in a container that never wraps, and three cells of a sentence each
        // made the table 240 px wider than the panel — a horizontal bar under a table with
        // nothing wide in it. A table that fits by dropping what it cannot draw would be no
        // better, so the cells are looked for as well as measured.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        // A sentence long enough that three of them in a row want more room than the
        // panel has: with the renderer drawing the cells, this table came out 240 px too
        // wide.
        let sentence =
            "Every cell here is a phrase long enough to need wrapping somewhere in the line";
        let table = format!(
            "| what | why | how |\n| --- | --- | --- |\n| {s} | {s} | {s} |\n",
            s = sentence
        );
        gui.app_mut().state.agent.transcript.clear();
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::Agent(table));
        gui.settle();

        let panel = gui.app().state.agent.shown_rect.expect("the panel");
        let mut columns: Vec<f32> = Vec::new();
        let mut widest: f32 = 0.0;
        for clipped in &gui.harness.output().shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                let inside = text.pos.x >= panel.left() - 40.0
                    && text.pos.y >= panel.top()
                    && text.pos.y <= panel.bottom();
                if !inside {
                    continue;
                }
                if text.galley.job.text.contains("Every cell") {
                    columns.push(text.pos.x);
                }
                widest = widest.max(text.pos.x + text.galley.size().x);
            }
        }
        columns.sort_by(|a, b| a.partial_cmp(b).unwrap());
        columns.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        assert!(
            columns.len() >= 3,
            "the cells of the table are not drawn: {} of them are on screen",
            columns.len()
        );
        assert!(
            widest <= panel.right() + 1.0,
            "the table is wider than the panel: {widest:.1} against {:.1}",
            panel.right()
        );
        // A cell that wraps still keeps its column: the share is what holds the columns
        // apart, whether the cell fills it or wraps inside it.
        let share = panel.width() / columns.len() as f32;
        for pair in columns.windows(2) {
            assert!(
                pair[1] - pair[0] > share * 0.6,
                "two columns are squeezed together: {:.1} apart, a share is {share:.1}",
                pair[1] - pair[0]
            );
        }
        gui.app_mut().state.agent.stop();
    }

    #[test]
    fn a_line_without_spaces_stays_inside_the_panel() {
        // Code and inline code carry lines with nowhere to break — a path, a hash, a URL —
        // and the answer is read in a panel, not in a page that scrolls sideways. What the
        // panel draws has to be no wider than the panel.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        for (name, body) in [
            ("a block of code", format!("```\n{}\n```", "x".repeat(300))),
            ("inline code", format!("see `{}` here", "y".repeat(300))),
        ] {
            gui.app_mut().state.agent.transcript.clear();
            gui.app_mut()
                .state
                .agent
                .transcript
                .push(crate::agent::Line::Agent(body));
            gui.settle();
            let panel = gui.app().state.agent.shown_rect.expect("the panel");
            let mut widest: f32 = 0.0;
            for clipped in &gui.harness.output().shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    if text.pos.x < panel.left()
                        || text.pos.y < panel.top()
                        || text.pos.y > panel.bottom()
                    {
                        continue;
                    }
                    widest = widest.max(text.pos.x + text.galley.size().x);
                }
            }
            assert!(
                widest <= panel.right() + 1.0,
                "{name} is wider than the panel: {widest:.1} against {:.1}",
                panel.right()
            );
        }
        gui.app_mut().state.agent.stop();
    }

    /// Where a piece of text of the panel was drawn, and how wide it is.
    fn drawn_at(gui: &Gui, needle: &str) -> Option<(f32, f32)> {
        let panel = gui.app().state.agent.shown_rect?;
        for clipped in &gui.harness.output().shapes {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.job.text == needle
                && text.pos.x >= panel.left()
                && text.pos.y >= panel.top()
                && text.pos.y <= panel.bottom()
            {
                return Some((text.pos.x, text.pos.x + text.galley.size().x));
            }
        }
        None
    }

    /// The vertical lines of the panel between two x positions.
    fn vertical_lines_between(gui: &Gui, from: f32, to: f32) -> usize {
        gui.harness
            .output()
            .shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                egui::Shape::LineSegment { points, .. } => {
                    (points[0].x - points[1].x).abs() < 0.5
                        && (points[0].y - points[1].y).abs() > 10.0
                        && points[0].x > from
                        && points[0].x < to
                }
                _ => false,
            })
            .count()
    }

    /// The horizontal lines of the panel between two y positions.
    fn horizontal_lines_between(gui: &Gui, from: f32, to: f32) -> usize {
        gui.harness
            .output()
            .shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                egui::Shape::LineSegment { points, .. } => {
                    (points[0].y - points[1].y).abs() < 0.5
                        && (points[0].x - points[1].x).abs() > 10.0
                        && points[0].y > from
                        && points[0].y < to
                }
                _ => false,
            })
            .count()
    }

    /// What a table in the panel is made of: the words, the horizontal lines and the bands
    /// of its rows, all inside the panel.
    struct TableParts {
        /// The table's own frame: its left and right uprights.
        frame: (f32, f32),
        words: Vec<(f32, f32, f32, f32, String)>,
        /// Every horizontal line of the panel: where it is, and where it runs.
        lines: Vec<(f32, f32, f32)>,
        bands: Vec<(f32, f32, f32, f32)>,
    }

    fn table_parts(gui: &Gui) -> TableParts {
        let panel = gui.app().state.agent.shown_rect.expect("the panel");
        let inside = |x: f32, y: f32| {
            x >= panel.left()
                && x <= panel.left() + panel.width() * 0.9
                && y >= panel.top()
                && y <= panel.bottom()
        };
        let mut frames: Vec<egui::Rect> = Vec::new();
        let mut parts = TableParts {
            frame: (f32::NAN, f32::NAN),
            words: Vec::new(),
            lines: Vec::new(),
            bands: Vec::new(),
        };
        for clipped in &gui.harness.output().shapes {
            match &clipped.shape {
                egui::Shape::Text(text) => {
                    let y = text.pos.y;
                    let (x0, x1) = (text.pos.x, text.pos.x + text.galley.size().x);
                    if inside(text.pos.x, y) {
                        parts.words.push((
                            y,
                            y + text.galley.size().y,
                            x0,
                            x1,
                            text.galley.job.text.clone(),
                        ));
                    }
                }
                egui::Shape::LineSegment { points, .. } => {
                    let (x0, x1) = (points[0].x.min(points[1].x), points[0].x.max(points[1].x));
                    if (points[0].y - points[1].y).abs() < 0.5
                        && inside(x0, points[0].y)
                        && x1 - x0 > 60.0
                    {
                        parts.lines.push((points[0].y, x0, x1));
                    }
                }
                egui::Shape::Rect(rect) => {
                    // A band of a row is looked for by what it does — a strip where the words
                    // of a row are — rather than by its colour: the theme in hand may paint
                    // `faint_bg_color` see-through, and a band that is not seen is drawn in the
                    // same place. The table's own frame is one strip as tall as the whole
                    // table, a row's band is a row tall.
                    let tall = rect.rect.height() > 8.0 && rect.rect.height() < 70.0;
                    if tall && rect.rect.width() > 60.0 && inside(rect.rect.left(), rect.rect.top())
                    {
                        parts.bands.push((
                            rect.rect.top(),
                            rect.rect.bottom(),
                            rect.rect.left(),
                            rect.rect.right(),
                        ));
                    }
                    // The frame is drawn as one stroked rectangle around the whole table, and
                    // it is told apart by covering every word of it.
                    if rect.stroke.width > 0.0
                        && rect.rect.width() > 200.0
                        && rect.rect.height() > 60.0
                        && inside(rect.rect.left(), rect.rect.top())
                    {
                        frames.push(rect.rect);
                    }
                }
                _ => {}
            }
        }
        // The frame of the table: the stroked rectangle that holds the most of its words.
        // The panel has stroked rectangles of its own (the field below it, for one), and the
        // panel has words of its own too (its tabs), so the frame is chosen by how much of the
        // table it covers.
        let covers = |frame: &egui::Rect| {
            parts
                .words
                .iter()
                .filter(|(y0, y1, x0, x1, _)| {
                    *x0 >= frame.left() - 1.0
                        && *x1 <= frame.right() + 1.0
                        && *y0 >= frame.top() - 1.0
                        && *y1 <= frame.bottom() + 1.0
                })
                .count()
        };
        if let Some(frame) = frames
            .into_iter()
            .filter(|frame| covers(frame) >= 4)
            .max_by_key(|frame| covers(frame))
        {
            parts.frame = (frame.left(), frame.right());
        }
        parts
    }

    /// The words of a table, told apart from the words of the panel around it by what they say:
    /// the panel has words of its own (its tabs, its buttons).
    fn only_the_tables_words(parts: &mut TableParts) {
        let mine = |word: &str| word == "what" || word == "why" || word.contains("row");
        parts.words.retain(|(_, _, _, _, word)| mine(word));
    }

    /// Whether a y is within the rows of the table, give or take the room around them.
    /// Whether a piece of the panel — a line, a band — belongs to this table.
    ///
    /// A part of the table runs inside the table's own frame; the panel's own lines (a rule
    /// above its foot, the edge of a card) run past it, and one of them crossing the words of
    /// a table is not a defect of the table.
    fn a_part_of_the_table(parts: &TableParts, x0: f32, x1: f32) -> bool {
        let (left, right) = parts.frame;
        x1 - x0 > 60.0 && x0 >= left - 2.0 && x1 <= right + 2.0
    }

    fn within_the_rows(parts: &TableParts, y: f32) -> bool {
        let first = parts
            .words
            .iter()
            .map(|(y, ..)| *y)
            .fold(f32::INFINITY, f32::min);
        let last = parts
            .words
            .iter()
            .map(|(_, y, ..)| *y)
            .fold(f32::NEG_INFINITY, f32::max);
        y > first - 30.0 && y < last + 30.0
    }

    #[test]
    fn a_tables_lines_and_bands_reach_its_frame() {
        // A line that stops at the widest cell leaves a gap at either side of every row, and
        // a band does the same: both run from one side of the table's frame to the other.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        let table = "| what | why |\n| --- | --- |\n| first row | a cell with enough words in it to wrap onto two lines and a bit more |\n| second row | two |\n| third row | another cell that wraps as well somewhere here indeed |\n";
        gui.app_mut().state.agent.transcript.clear();
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::Agent(table.to_owned()));
        gui.settle();

        let mut parts = table_parts(&gui);
        only_the_tables_words(&mut parts);
        let (left, right) = parts.frame;
        assert!(
            left.is_finite() && right > left,
            "the table has no frame: {left:?}..{right:?}"
        );
        let (words_left, words_right) = parts.words.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(l, r), (_, _, x0, x1, _)| (l.min(*x0), r.max(*x1)),
        );
        assert!(
            words_left >= left - 2.0 && words_right <= right + 2.0,
            "the words run {words_left:.1}..{words_right:.1}, and the frame is {left:.1}..{right:.1}"
        );
        let mut lines = 0;
        for (y, x0, x1) in &parts.lines {
            if !a_part_of_the_table(&parts, *x0, *x1) {
                continue;
            }
            lines += 1;
            assert!(
                *x0 <= left + 2.0 && *x1 >= right - 2.0,
                "a line at y {y:.1} runs {x0:.1}..{x1:.1}, and the frame is {left:.1}..{right:.1}"
            );
        }
        assert!(lines >= 2, "the table has {lines} lines of its own");
        // A band of this table is bounded by two of its lines; the band of a button of the
        // panel around it is bounded by nothing, and a button is a hundred points wide.
        let of_the_table: Vec<f32> = parts
            .lines
            .iter()
            .filter(|(_, x0, x1)| *x0 >= left - 2.0 && *x1 <= right + 2.0)
            .map(|(y, ..)| *y)
            .collect();
        let mut bands = 0;
        for (top, bottom, x0, x1) in &parts.bands {
            let bounded = |y: f32| of_the_table.iter().any(|line| (*line - y).abs() < 1.5);
            if !bounded(*top) || !bounded(*bottom) {
                continue;
            }
            bands += 1;
            assert!(
                *x0 <= left + 2.0 && *x1 >= right - 2.0,
                "a band from y {top:.1} to {bottom:.1} runs {x0:.1}..{x1:.1}, and the frame is {left:.1}..{right:.1}"
            );
        }
        assert!(bands >= 1, "the table has no bands of its own");
        gui.app_mut().state.agent.stop();
    }

    #[test]
    fn a_tables_lines_keep_their_distance_from_the_words() {
        // A line between two rows is drawn halfway across the gap the table leaves between
        // them, and that gap is three points: a point and a half from the words of the row
        // above and the row below, close enough to read as a stroke through the text rather
        // than a boundary between two rows. Each cell therefore keeps room of its own at both
        // ends, and the line stands clear of every word it passes.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        let table = "| what | why |\n| --- | --- |\n| first row | a cell with enough words in it to wrap onto two lines and a bit more of them for certain |\n| second row | two |\n| third row | another cell long enough that it wraps as well, twice over and then once more |\n";
        gui.app_mut().state.agent.transcript.clear();
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::Agent(table.to_owned()));
        gui.settle();

        let mut parts = table_parts(&gui);
        let mine = |word: &str| word == "what" || word == "why" || word.contains("row");
        parts.words.retain(|(_, _, _, _, word)| mine(word));
        let (frame_left, frame_right) = parts.frame;
        // The lines of this table, told apart from the lines of the panel around it by running
        // from one upright of the table's frame to the other.
        let lines: Vec<f32> = parts
            .lines
            .iter()
            .filter(|(_, left, right)| {
                (*left - frame_left).abs() <= 2.0 && (*right - frame_right).abs() <= 2.0
            })
            .map(|(y, _, _)| *y)
            .collect();
        assert!(
            parts.words.len() >= 6 && lines.len() >= 3,
            "the table is not drawn: {} pieces of text, {} lines",
            parts.words.len(),
            lines.len()
        );

        // How close any line of the table comes to any word of it.
        let mut closest = f32::INFINITY;
        for line in &lines {
            for (top, bottom, ..) in &parts.words {
                if *bottom <= *line {
                    closest = closest.min(line - bottom);
                } else if *top >= *line {
                    closest = closest.min(top - line);
                }
            }
        }
        assert!(
            closest >= 3.0,
            "a line of the table stands {closest:.1} points from the words beside it"
        );
    }

    #[test]
    fn a_tables_lines_and_bands_do_not_cross_its_words() {
        // The line under a row and the edge of a band belong in the empty room between two
        // rows. Counted from `item_spacing`, the line landed inside the words of the next row
        // (y 616 while that row began at y 615) and the bands of the grid came from the row
        // heights of the frame before. Both are placed from what the cells took now, and
        // neither may touch a word.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        let table = "| what | why |\n| --- | --- |\n| first row | a cell with enough words in it to wrap onto two lines and a bit more of them for certain |\n| second row | two |\n| third row | another cell long enough that it wraps as well, twice over and then once more |\n";
        gui.app_mut().state.agent.transcript.clear();
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::Agent(table.to_owned()));
        gui.settle();

        // The words of this table, told apart from the words of the panel around it by what
        // they say: a band is looked for where these words are, and nowhere else — the panel
        // has bands of its own (its tabs, its buttons).
        let mut parts = table_parts(&gui);
        let mine = |word: &str| word == "what" || word == "why" || word.contains("row");
        parts.words.retain(|(_, _, _, _, word)| mine(word));
        // A band of this table runs from one side of its frame to the other — which is what
        // tells it apart from the bands of the panel around it (a tab, a button).
        let (frame_left, frame_right) = parts.frame;
        parts.bands.retain(|(_, _, left, right)| {
            (*left - frame_left).abs() <= 2.0 && (*right - frame_right).abs() <= 2.0
        });
        assert!(
            parts.words.len() >= 6,
            "the table is not drawn: {} pieces of text",
            parts.words.len()
        );
        assert!(
            parts
                .lines
                .iter()
                .filter(|(y, ..)| within_the_rows(&parts, *y))
                .count()
                >= 3,
            "the table has no lines"
        );
        assert!(!parts.bands.is_empty(), "the table has no bands");
        // No allowance: a line one point inside a word is a line drawn across it, which is
        // exactly what counting the gap from `item_spacing` produced.
        let crossing = |y: f32| {
            parts
                .words
                .iter()
                .any(|(top, bottom, _, _, _)| y > *top && y < *bottom)
        };
        for (y, x0, x1) in &parts.lines {
            if a_part_of_the_table(&parts, *x0, *x1) {
                assert!(
                    !crossing(*y),
                    "a line at {y:.1} is drawn across the words of a row"
                );
            }
        }
        for (top, bottom, _, _) in &parts.bands {
            assert!(
                !crossing(*top) && !crossing(*bottom),
                "a band runs from {top:.1} to {bottom:.1}, across the words of a row"
            );
            assert!(
                parts.lines.iter().any(|(y, ..)| (*y - top).abs() < 1.5),
                "the band starting at {top:.1} is not bounded by a line"
            );
        }
        gui.app_mut().state.agent.stop();
    }

    #[test]
    fn a_table_takes_only_the_room_its_words_need() {
        // A cell of a table takes the room its words need, not the whole column it could
        // have: a heading drawn across its whole column is what makes a column of two short
        // words as wide as the widest column of the table. The row still lines up under the
        // heading, and the columns stay apart.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        let table = "| what | why | how |\n| --- | --- | --- |\n| first cell | second cell | third cell |\n";
        gui.app_mut().state.agent.transcript.clear();
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::Agent(table.to_owned()));
        gui.settle();

        let panel = gui.app().state.agent.shown_rect.expect("the panel");
        let heading = drawn_at(&gui, "what").expect("the heading is not drawn");
        let first = drawn_at(&gui, "first cell").expect("the first column is not drawn");
        let second = drawn_at(&gui, "second cell").expect("the second column is not drawn");
        let third = drawn_at(&gui, "third cell").expect("the third column is not drawn");
        assert!(
            (first.0 - heading.0).abs() < 8.0,
            "the row does not line up under the heading: {:.1} against {:.1}",
            first.0,
            heading.0
        );
        assert!(
            second.0 >= first.1,
            "the second column starts inside the first: {:.1} against {:.1}",
            second.0,
            first.1
        );
        assert!(
            third.1 < panel.left() + panel.width() * 0.4,
            "a table of three short cells takes {:.0} of the panel's {:.0} points",
            third.1 - panel.left(),
            panel.width()
        );

        // And the columns are drawn apart, not only laid out apart.
        assert!(
            vertical_lines_between(&gui, first.1, second.0) >= 1,
            "no line between the first column and the second"
        );
        assert!(
            vertical_lines_between(&gui, second.1, third.0) >= 1,
            "no line between the second column and the third"
        );
        assert!(
            horizontal_lines_between(&gui, heading.0.max(first.0), heading.0 + 30.0) >= 1
                || horizontal_lines_between(&gui, heading.0, heading.0 + 60.0) >= 1,
            "no line between the heading and the row"
        );
        gui.app_mut().state.agent.stop();
    }

    #[test]
    fn two_tables_in_one_answer_do_not_share_a_grid_id() {
        // An answer may carry more than one table, and each is a grid of its own: one id for
        // both is what egui answers with "Second use of GRID ID …" over the block, and the
        // rows of one table shuffle into the other.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        let answer = "| what | why |\n| --- | --- |\n| first table | one |\n\n| how | when |\n| --- | --- |\n| second table | two |\n";
        gui.app_mut().state.agent.transcript.clear();
        gui.app_mut()
            .state
            .agent
            .transcript
            .push(crate::agent::Line::Agent(answer.to_owned()));
        gui.settle();

        assert!(
            warnings_of(&gui).is_empty(),
            "egui is warning about the tables: {:?}",
            warnings_of(&gui)
        );
        let drawn = |needle: &str| {
            gui.harness.output().shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text)
                    if text.galley.job.text.contains(needle))
            })
        };
        assert!(drawn("first table"), "the first table is not drawn");
        assert!(drawn("second table"), "the second table is not drawn");
        gui.app_mut().state.agent.stop();
    }

    #[test]
    fn two_answers_with_a_table_do_not_share_a_grid_id() {
        // An answer's Markdown is drawn by a renderer that puts a Grid under the panel's
        // ui, and every answer in a conversation shares that ui: two answers carrying a
        // table landed on one Grid id, and egui answered "Second use of GRID ID …" over the
        // block. An answer's id is its place in the conversation now.
        let mut gui = Gui::new(&[("a.txt", "one\n")], Some("a.txt"));
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        let table = "| a | b |\n| --- | --- |\n| 1 | 2 |";
        let transcript = &mut gui.app_mut().state.agent.transcript;
        transcript.push(crate::agent::Line::Agent(table.to_owned()));
        transcript.push(crate::agent::Line::Agent(table.to_owned()));
        gui.settle();

        assert!(
            warnings_of(&gui).is_empty(),
            "egui is warning about the answers: {:?}",
            warnings_of(&gui)
        );
    }

    #[test]
    fn the_card_is_as_tall_as_its_answers_and_never_the_whole_panel() {
        // The card grows with its answers and stops at a share of the panel: a question that
        // covers the whole panel takes the conversation with it, and the conversation is what
        // a person reads to decide which answer is right.
        let Some(mut few) = with_agent_env(&[("ACP_TEST_VARIANTS", "2")]) else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        ask_for_a_choice(&mut few);
        // A window tall enough that the panel has room: the card is its content there, so a
        // card with two answers and a card with fourteen differ by what is in them.
        for gui in [&mut few] {
            gui.harness.set_size(egui::vec2(1200.0, 1400.0));
            for _ in 0..10 {
                gui.harness.step();
            }
        }
        let panel = few
            .app()
            .state
            .agent
            .shown_rect
            .expect("the panel is drawn");
        let short = few
            .app()
            .state
            .agent
            .card_rect
            .expect("the card is drawn")
            .height();

        let Some(mut many) = with_agent_env(&[("ACP_TEST_VARIANTS", "14")]) else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        ask_for_a_choice(&mut many);
        for gui in [&mut many] {
            gui.harness.set_size(egui::vec2(1200.0, 1400.0));
            for _ in 0..10 {
                gui.harness.step();
            }
        }
        let tall = many
            .app()
            .state
            .agent
            .card_rect
            .expect("the card is drawn")
            .height();

        assert!(
            tall > short,
            "the card does not grow with its answers: {tall} against {short}"
        );
        for (height, answers) in [(short, 2), (tall, 14)] {
            assert!(
                height <= panel.height() * 0.75 + 2.0,
                "a card with {answers} answers took {height} of the panel's {}",
                panel.height()
            );
        }
    }

    #[test]
    fn a_question_with_several_answers_shows_them_and_sends_the_ones_picked() {
        // Hermes asks for several answers by making the property an array of the same
        // strings. A client that only knew the flat shape drew the question and none of its
        // answers: a dialog with nothing to click. And one click cannot be the whole answer
        // to such a question, so the answers are switches and the answer is sent when it is
        // complete.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        gui.app_mut().state.agent.input = "choose-many".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the question", |gui| {
            gui.app().state.agent.choice.is_some()
        });

        let tests = "Tests \u{2014} the whole suite";
        let docs = "Docs \u{2014} the reference page";
        let release = "Release \u{2014} a version of its own";
        for answer in [tests, docs, release] {
            assert!(
                gui.harness.query_by_label(answer).is_some(),
                "an answer of a question of several is not on screen: {answer}"
            );
        }

        // Two of them, and the question is still waiting: what goes back is the set.
        gui.harness.get_by_label(tests).click();
        gui.harness.step();
        gui.harness.get_by_label(docs).click();
        gui.harness.step();
        assert!(
            gui.app().state.agent.choice.is_some(),
            "one click answered a question of several"
        );
        assert_eq!(
            gui.app().state.agent.picked,
            vec![tests.to_owned(), docs.to_owned()],
            "the answers picked are not the ones switched on"
        );
        assert!(
            gui.harness.query_by_label(release).is_some(),
            "the answers went off screen while they were being chosen"
        );

        gui.harness.get_by_label("Answer").click();
        wait(&mut gui, "the answers coming back", |gui| {
            answer(gui).contains("chose:")
        });
        assert_eq!(
            answer(&gui),
            format!("chose: {tests}, {docs}"),
            "the agent was not told both of the answers picked"
        );
        assert!(
            gui.app().state.agent.choice.is_none(),
            "the question stayed after it was answered"
        );
    }

    #[test]
    fn the_commands_the_agent_offers_are_offered_back_when_a_slash_is_typed() {
        // The agent names its commands and says what they do (`available_commands_update`).
        // The panel offers them with the agent's own words while a slash is typed, and the
        // keyboard completes the one it is on — a command typed by hand is one typo away
        // from a prompt the agent reads as plain text.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the commands", |gui| {
            gui.app().state.agent.commands.len() == 9
        });

        // Nothing is offered until a slash is typed.
        gui.settle();
        assert!(
            gui.harness
                .query_by_label_contains("List available commands")
                .is_none(),
            "the commands are offered before they are asked for"
        );

        let fields = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput);
        fields.last().expect("the message field").click();
        gui.settle();
        gui.harness.event(egui::Event::Text("/".to_owned()));
        for _ in 0..6 {
            gui.harness.step();
        }
        assert!(
            gui.harness.query_by_label_contains("/help").is_some(),
            "the commands are not offered: the field says {:?}",
            gui.app().state.agent.input
        );
        assert!(
            gui.harness
                .query_by_label_contains("List available commands")
                .is_some(),
            "the commands are offered without what they do"
        );

        // What is typed narrows the list, and `Enter` completes the command it is on rather
        // than sending half of one.
        gui.harness.event(egui::Event::Text("mo".to_owned()));
        for _ in 0..4 {
            gui.harness.step();
        }
        assert!(
            gui.harness.query_by_label_contains("/model").is_some(),
            "the list did not narrow to what was typed"
        );
        assert!(
            gui.harness.query_by_label_contains("/help").is_none(),
            "the list kept the commands that do not match"
        );
        gui.harness.key_press(egui::Key::Enter);
        for _ in 0..4 {
            gui.harness.step();
        }
        assert_eq!(
            gui.app().state.agent.input,
            "/model ",
            "the command was not completed"
        );
        assert!(
            gui.app().state.agent.transcript.is_empty(),
            "completing a command sent it"
        );

        // And a command sent as a prompt is a prompt: the agent hears it.
        gui.harness.key_press(egui::Key::Enter);
        wait(&mut gui, "the command reaching the agent", |gui| {
            !gui.app().state.agent.transcript.is_empty()
        });
        assert!(
            gui.app()
                .state
                .agent
                .transcript
                .iter()
                .any(|line| matches!(line, crate::agent::Line::You(text) if text == "/model")),
            "the command was not sent as it was completed: {:?}",
            gui.app().state.agent.transcript
        );
    }

    #[test]
    fn the_field_is_three_lines_and_grows_with_shift_enter() {
        // A message to an agent is often more than one line, and a field of one line makes
        // every such message a single unreadable one. `Enter` sends — it is how a message is
        // sent everywhere else — and `Shift`+`Enter` is what makes a new line.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        let field = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .last()
            .expect("the message field");
        let rect = field.rect();
        assert!(
            rect.height() > 40.0,
            "the field is not three lines tall: {rect:?}"
        );
        let send = gui
            .harness
            .query_by_label("Send")
            .expect("the button that sends")
            .rect();
        assert!(
            send.top() >= rect.bottom() - 1.0,
            "the button is not under the field: {send:?} against {rect:?}"
        );

        field.click();
        gui.settle();
        gui.harness.event(egui::Event::Text("one".to_owned()));
        gui.harness.step();
        assert_eq!(
            gui.app().state.agent.input,
            "one",
            "the field did not take what was typed"
        );
        gui.harness
            .key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
        gui.harness.step();
        assert_eq!(
            gui.app().state.agent.input,
            "one\n",
            "Shift+Enter did not add a line"
        );
        gui.harness.event(egui::Event::Text("two".to_owned()));
        gui.harness.step();
        assert_eq!(
            gui.app().state.agent.input,
            "one\ntwo",
            "the line after Shift+Enter went somewhere else"
        );
        assert!(
            gui.app().state.agent.transcript.is_empty(),
            "Shift+Enter sent the message"
        );

        // Three lines are what it starts with, so a fourth is where it makes room: the field
        // is not a box of three lines with the rest hidden.
        for line in ["three", "four"] {
            gui.harness
                .key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
            gui.harness.step();
            gui.harness.event(egui::Event::Text(line.to_owned()));
            gui.harness.step();
        }
        assert_eq!(
            gui.app().state.agent.input,
            "one\ntwo\nthree\nfour",
            "the field stopped taking lines"
        );
        let taller = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .last()
            .expect("the message field")
            .rect();
        assert!(
            taller.height() > rect.height(),
            "the field did not grow with the lines in it: {taller:?} against {rect:?}"
        );

        gui.harness.key_press(egui::Key::Enter);
        wait(&mut gui, "the message going out", |gui| {
            !gui.app().state.agent.transcript.is_empty()
        });
    }

    #[test]
    fn a_request_is_drawn_in_a_block_of_its_own() {
        // An answer is read against the question it answers. The question is set in a block
        // of its own, so that it can be found again by looking at the conversation rather
        // than by reading all of it.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        gui.app_mut().state.agent.input = "hello".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the request on screen", |gui| {
            gui.harness.query_by_label("hello").is_some()
        });

        let asked = gui
            .harness
            .query_by_label("hello")
            .expect("the request is not on screen")
            .rect();
        let block = gui
            .harness
            .output()
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == egui::Color32::from_rgb(34, 38, 45) => {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("the request is not drawn in a block of its own");
        assert!(
            block.contains_rect(asked),
            "the request is outside the block: {block:?} against {asked:?}"
        );
    }

    #[test]
    fn a_question_with_nothing_to_pick_is_answered_by_writing() {
        // Hermes asks for free text with a bare string and nothing to choose between
        // (`acp_adapter/elicitation.py`). A card with a heading and a refusal is a question
        // nobody can answer, so such a question is drawn with somewhere to write the answer.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "ask-text".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the question", |gui| {
            gui.app().state.agent.choice.is_some()
        });
        assert!(
            gui.app()
                .state
                .agent
                .choice
                .as_ref()
                .is_some_and(|(_, request)| request.free_text),
            "a question with nothing to pick was not read as one to write"
        );

        // The field to write in is there, and the keyboard is in it: the question is what the
        // person has to deal with, not the message they might send next.
        let fields = gui
            .harness
            .get_all_by_role(egui::accesskit::Role::TextInput);
        let written = fields.last().expect("nowhere to write the answer");
        assert!(
            written.is_focused(),
            "the keyboard did not go to the card: {:?} is focused",
            gui.harness.ctx.memory(|memory| memory.focused())
        );

        gui.harness
            .event(egui::Event::Text("a name of my own".to_owned()));
        gui.harness.step();
        gui.harness.key_press(egui::Key::Enter);
        wait(&mut gui, "the answer reaching the agent", |gui| {
            answer(gui).contains("wrote:")
        });
        assert_eq!(
            answer(&gui),
            "wrote: a name of my own",
            "the agent heard something other than what was written"
        );
        assert!(
            gui.app().state.agent.choice.is_none(),
            "the question stayed after it was answered"
        );
        // The only thing sent as a message is the prompt that asked the question: the answer
        // went back as an answer, not as another prompt.
        assert_eq!(
            gui.app()
                .state
                .agent
                .transcript
                .iter()
                .filter(|line| matches!(line, crate::agent::Line::You(_)))
                .count(),
            1,
            "the answer was sent as a message as well: {:?}",
            gui.app().state.agent.transcript
        );
    }

    #[test]
    fn the_facts_under_the_field_are_one_row_in_a_narrow_panel() {
        // The foot of the panel is a field, a button and a row of facts. A row that wraps is
        // two rows, and the second one is a line the conversation loses — and it appears the
        // moment a figure in it gains a character, which the timer does several times a
        // second: the panel is repainted, the row wraps differently, and the foot grows while
        // the mouse is being moved and nothing else is happening.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        // A narrow dock, and the longest figures the panel ever shows: a model, a level, a
        // mode, and a context figure of six digits.
        gui.harness.set_size(egui::vec2(320.0, 700.0));
        // The longest facts the panel can be asked to show: a long model name and a context
        // figure of six digits, in a panel too narrow for all of them at once.
        gui.app_mut().state.agent.current_model = Some("a-model-id-that-is-long".to_owned());
        gui.app_mut().state.agent.models = vec![ide_agents::Model {
            id: "a-model-id-that-is-long".to_owned(),
            name: "a very long model name that takes a good deal of the row".to_owned(),
        }];
        gui.app_mut().state.agent.usage = Some(ide_agents::Usage {
            used: 42_000,
            window: Some(100_000),
        });
        gui.settle();
        let row = gui
            .app()
            .state
            .agent
            .status_rect
            .expect("the row of facts is not on screen");
        assert!(
            row.height() <= 24.0,
            "the row of facts wrapped: it is {:.1} px tall at {:.0} px wide (the row is {:?})",
            row.height(),
            row.width(),
            row
        );
    }

    #[test]
    fn the_foot_of_the_panel_is_the_field_and_one_row() {
        // What the field, the button and the facts take together is the height of the field plus
        // one row — the field grows a line at a time and the row does not move. Anything else in
        // it is a line the conversation is owed: a foot that is measured against the drawing
        // above it is a foot the conversation can push down.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        // The longest facts the panel can be asked to show: a long model name and a context
        // figure of six digits, in a panel too narrow for all of them at once.
        gui.app_mut().state.agent.current_model = Some("a-model-id-that-is-long".to_owned());
        gui.app_mut().state.agent.models = vec![ide_agents::Model {
            id: "a-model-id-that-is-long".to_owned(),
            name: "a very long model name that takes a good deal of the row".to_owned(),
        }];
        gui.app_mut().state.agent.usage = Some(ide_agents::Usage {
            used: 42_000,
            window: Some(100_000),
        });

        // A conversation long enough that the transcript takes what is left rather than what it
        // would like, in a narrow panel tall enough for the whole foot to be inside it.
        gui.harness.set_size(egui::vec2(320.0, 1400.0));
        for index in 0..40 {
            gui.app_mut()
                .state
                .agent
                .transcript
                .push(crate::agent::Line::Agent(format!(
                    "line {index}: some words that take a whole row of the panel by themselves"
                )));
        }
        gui.settle();
        let field = gui
            .app()
            .state
            .agent
            .field_rect
            .expect("the field is not on screen");
        let foot = gui.app().state.agent.below;
        // The field, and one row under it with its gaps: 34 px of room for a row of 20 and the
        // spacing around it. Measured to the bottom of the panel the same foot came to 65 px,
        // most of it the panel's own padding and a whole extra row for the mark of a fact left
        // out — the padding is the panel's business, and the mark belongs in the row.
        assert!(
            foot <= field.height() + 34.0,
            "the foot is {foot:.1} px tall for a field of {:.1} px",
            field.height()
        );

        // And it does not move when the pointer does: the panel is repainted then and at no
        // other time, so a foot that is measured against the conversation grows on that frame
        // and stays grown.
        let before = (field, foot);
        for step in 0..8 {
            gui.harness.event(egui::Event::PointerMoved(egui::pos2(
                40.0 + step as f32,
                60.0,
            )));
            gui.harness.step();
        }
        let after = (
            gui.app().state.agent.field_rect.expect("the field"),
            gui.app().state.agent.below,
        );
        assert_eq!(
            (before.0.top(), before.0.height()),
            (after.0.top(), after.0.height()),
            "the field moved when the pointer did: {before:?} became {after:?}"
        );
        assert!(
            (before.1 - after.1).abs() < 0.5,
            "the foot changed height when the pointer moved: {:.1} px became {:.1} px",
            before.1,
            after.1
        );
    }

    #[test]
    fn the_send_button_stands_at_the_right_edge() {
        // The button belongs to the field above it and to the edge the panel ends at: at the
        // left it looks like the beginning of another row of facts.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        let panel = gui
            .app()
            .state
            .agent
            .shown_rect
            .expect("the panel is not on screen");
        let send = gui.harness.get_by_label("Send").rect();
        assert!(
            (panel.right() - send.right()).abs() <= 12.0,
            "Send stands {:.1} px from the right edge of the panel (panel {:?}, button {:?})",
            panel.right() - send.right(),
            panel,
            send
        );
        assert!(
            send.left() > panel.center().x,
            "Send is not on the right half of the panel: {:?}",
            send
        );
    }

    #[test]
    fn a_panel_put_away_from_the_menu_keeps_what_was_in_it() {
        // The menu is the door for a panel that is not on screen at all, and putting a panel away
        // is not throwing it out: the conversation is still there when it comes back, because the
        // conversation was never the dock's.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        gui.app_mut().state.agent.input = "hello".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        // The answer, and not the question: a transcript that only holds the request is a turn
        // still running, and a turn still running adds to the conversation while the panel is
        // away — the snapshot has to be of a finished turn. `settle` then runs the frames until
        // nothing asks for another one, which is what the last chunk of an answer does.
        wait(&mut gui, "the greeting", |gui| !answer(gui).is_empty());
        gui.settle();
        let said = gui.app().state.agent.transcript.clone();
        assert!(
            said.iter()
                .any(|line| matches!(line, crate::agent::Line::You(_))),
            "the request is not part of the conversation: {said:?}"
        );

        // The panel's own line in the menu, unchecked.
        gui.harness.get_by_label("View").click();
        gui.settle();
        gui.harness.get_by_label("Agent\tAlt+0").click();
        gui.settle();
        gui.harness.key_press(egui::Key::Escape);
        gui.settle();
        assert!(
            gui.app().dock.find_tab(&Tab::Agent).is_none(),
            "the panel is still in the dock"
        );
        assert!(
            gui.app().state.hidden.contains(&Tab::Agent),
            "the workspace did not remember that the panel was put away"
        );
        assert_eq!(
            gui.app().state.agent.transcript,
            said,
            "the conversation went away with the panel"
        );

        // And back it comes, with everything that was in it.
        gui.harness.get_by_label("View").click();
        gui.settle();
        gui.harness.get_by_label("Agent\tAlt+0").click();
        gui.settle();
        gui.harness.key_press(egui::Key::Escape);
        gui.settle();
        assert!(
            gui.app().dock.find_tab(&Tab::Agent).is_some(),
            "the panel did not come back"
        );
        assert!(!gui.app().state.hidden.contains(&Tab::Agent));
        assert_eq!(gui.app().state.agent.transcript, said);
    }

    #[test]
    fn a_variant_can_be_refused() {
        // A question the agent is waiting on has to have a way out: an agent that hears
        // nothing waits forever, and a person who wants none of the variants has to be
        // able to say so.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });

        gui.app_mut().state.agent.input = "choose".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait(&mut gui, "the variants", |gui| {
            gui.app().state.agent.choice.is_some()
        });
        assert!(
            gui.harness.query_by_label("None of these").is_some(),
            "there is no way to refuse the variants"
        );

        gui.harness.get_by_label("None of these").click();
        wait(&mut gui, "the refusal coming back", |gui| {
            answer(gui).contains("none")
        });
        assert_eq!(
            answer(&gui),
            "none",
            "the agent was not told of the refusal"
        );
        assert!(
            gui.app().state.agent.choice.is_none(),
            "the choice stayed after it was refused"
        );
    }

    #[test]
    fn a_crashed_agent_is_started_again_with_the_conversation_restored() {
        // A process dying is not the conversation ending. The panel starts another one,
        // asks for the same session, and clears what was on screen first: the restored
        // agent replays its history, and keeping the old lines would draw them twice.
        let Some(mut gui) = with_agent() else {
            println!("the stand-in agent is not built: the check was skipped");
            return;
        };
        gui.harness
            .key_press_modifiers(egui::Modifiers::ALT, egui::Key::Num0);
        gui.settle();
        gui.harness.get_by_label("Start").click();
        wait(&mut gui, "the agent starting", |gui| {
            gui.app().state.agent.status.starts_with("ready")
        });
        let session = gui.app().state.agent.session.clone().expect("a session");

        // A short wait so the test can see the panel waiting, and not just the restart.
        gui.app_mut().state.agent.reconnect_delay = Duration::from_millis(300);
        gui.app_mut().state.agent.input = "die".to_owned();
        gui.settle();
        gui.harness.get_by_label("Send").click();
        wait_stepping(&mut gui, "the crash being noticed", |gui| {
            gui.app().state.agent.waiting_to_reconnect()
        });
        assert!(
            !gui.app().state.agent.running(),
            "the panel still believes the dead agent is running"
        );

        wait_stepping(&mut gui, "the agent coming back", |gui| {
            gui.app().state.agent.running() && gui.app().state.agent.status.starts_with("ready")
        });
        assert_eq!(
            gui.app().state.agent.session.as_deref(),
            Some(session.as_str()),
            "the restarted agent was not asked for the same conversation"
        );
        // The restored session replays what it kept, and the replay carries the words of both
        // sides: the panel comes back to its conversation, not to a blank wall.
        let lines = gui.app().state.agent.transcript.clone();
        assert!(
            lines.iter().any(|line| matches!(
                line,
                crate::agent::Line::You(text) if text == "why is the sky blue?"
            )),
            "the person's words were not restored after the restart: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| matches!(
                line,
                crate::agent::Line::Agent(text) if text == "Because air scatters blue light."
            )),
            "the answer was not restored after the restart: {lines:?}"
        );
        assert!(
            gui.app().state.agent.status.contains("resumed"),
            "the restarted agent did not pick the conversation up: {:?}",
            gui.app().state.agent.status
        );
    }
}

/// The hint belongs to the pointer's place in the text, and nowhere else.
#[cfg(test)]
mod the_hint_and_the_panels {
    use super::*;

    #[test]
    fn a_frame_that_draws_the_editor_says_so() {
        let mut gui = Gui::new(&[("a.rs", "use std::fmt;\n")], Some("a.rs"));
        gui.settle();
        assert!(
            gui.app().state.editor_drawn,
            "the editor did not mark itself drawn"
        );
    }

    #[test]
    fn the_hint_is_nothing_when_the_editor_did_not_draw() {
        let mut gui = Gui::new(&[("a.rs", "use std::fmt;\n")], Some("a.rs"));
        gui.settle();
        // A place under the pointer, as the view would have left it.
        gui.app_mut()
            .state
            .set_hover_for_test(0, 4, egui::pos2(300.0, 200.0));
        assert!(
            gui.app().state.hover_spot().is_some(),
            "the spot is not published while the editor is on screen"
        );

        // The next frame draws something else in its place: the spot means nothing, and
        // a hint left over from it would follow the pointer into another panel.
        gui.app_mut().state.editor_drawn = false;
        assert!(
            gui.app().state.hover_spot().is_none(),
            "the hint outlived the editor not being drawn"
        );
    }
}

/// The directory a shell opens in, and the line the agent panel keeps under the
/// conversation.
#[cfg(test)]
mod the_project_directory {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_until(gui: &mut Gui, what: &str, check: impl Fn(&Gui) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if check(gui) {
                return;
            }
            gui.settle();
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!("{what} did not happen");
    }

    #[test]
    fn a_terminal_opens_in_the_project_and_not_where_the_editor_was_started() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.txt"), "one\n").unwrap();
        let root = project.path().canonicalize().unwrap();
        let dir = project.path().to_path_buf();
        let config = tempfile::tempdir().unwrap();
        let mut gui = Gui {
            harness: window(&dir, config.path(), &[("a.txt", "one\n")], Some("a.txt")),
            _project: project,
            _config: Some(config),
        };
        gui.settle();
        assert_eq!(
            gui.app().state.terminal.cwd.as_deref(),
            Some(root.as_path()),
            "the terminal was told nothing about the project: {:?}",
            gui.app()
                .state
                .project
                .as_ref()
                .map(|project| project.root().to_path_buf())
        );

        let path = gui
            .app_mut()
            .dock
            .find_tab(&crate::ui::Tab::Terminal)
            .expect("the terminal tab");
        gui.app_mut().dock.set_active_tab(path).unwrap();
        gui.settle();
        let rect = gui
            .app()
            .state
            .terminal
            .shown_rect
            .expect("the terminal was drawn");
        for pressed in [true, false] {
            gui.harness.event(egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        gui.settle();
        gui.harness.event(egui::Event::Text("pwd\n".to_owned()));
        gui.settle();

        let expected = root.to_string_lossy().into_owned();
        wait_until(&mut gui, "the shell to say where it is", |gui| {
            gui.app().state.terminal.contents().contains(&expected)
        });
    }
}
