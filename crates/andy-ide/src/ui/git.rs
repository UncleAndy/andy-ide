//! The Git panel: the branch, the two halves of `git status`, the commit field.
//!
//! A click selects a change, a double click opens the file, Enter shows the diff.
//! Staging and committing are buttons on purpose: they change the repository, and a
//! gesture that changes something should be one the user can see and aim at.

use egui::{Color32, RichText, Ui};
use ide_git::{Area, Branch, Change, Kind};

use crate::state::{Action, AppState};

use super::{diff, history};

/// The letters `git status` prints, in colours that read on both themes.
const ADDED: Color32 = Color32::from_rgb(78, 154, 92);
const MODIFIED: Color32 = Color32::from_rgb(206, 168, 78);
const DELETED: Color32 = Color32::from_rgb(196, 84, 84);
const RENAMED: Color32 = Color32::from_rgb(120, 160, 210);
const CONFLICTED: Color32 = Color32::from_rgb(224, 108, 117);
const UNTRACKED: Color32 = Color32::from_rgb(150, 152, 156);

/// What the panel was asked to do this frame, applied after the list is drawn.
enum Asked {
    StageSelected,
    UnstageSelected,
    StageAll,
    Commit,
    HideDiff,
    Switch(String),
    CreateBranch,
}

pub fn panel(ui: &mut Ui, state: &mut AppState) {
    state.git.refresh(false);

    let mut asked = None;
    ui.horizontal(|ui| {
        branch_menu(ui, state, &mut asked);
        ui.separator();
        let total = state.git.changes.len();
        ui.weak(format!(
            "{total} {}",
            if total == 1 { "change" } else { "changes" }
        ));
        // The two windows this panel has onto the repository, as a choice inside the tab and not
        // as tabs of their own. A panel that opened another panel would take the person away
        // from the list they were reading — and the list is where they were.
        if ui
            .selectable_label(!state.git.history, "Changes")
            .on_hover_text("What is staged and what is only on disk")
            .clicked()
        {
            state.git.history = false;
        }
        if ui
            .selectable_label(state.git.history, "History")
            .on_hover_text("The commits behind HEAD")
            .clicked()
        {
            state.git.history = true;
        }
        // The commands that change the repository stand in the corner of the row, away from the
        // switches that only change what is shown: beside a `Changes`, a `Fetch` read like one more
        // way to look at the list. The corner is far enough that a line between them is noise.
        if state.git.has_remote {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                remote_buttons(ui, state);
            });
        }
    });
    ui.separator();

    // The diff of what is picked, under the list: one change, line by line, without leaving the
    // panel that lists it. A fresh one opens the pane by itself, because a diff that was asked
    // for and not shown is a question that was answered with silence.
    if state.git.diff_open {
        egui::Panel::bottom("git_diff")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Diff");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Hide")
                            .on_hover_text("Keep the list to yourself for a while")
                            .clicked()
                        {
                            asked = Some(Asked::HideDiff);
                        }
                    });
                });
                diff::panel(ui, state);
            });
    }

    if state.git.history {
        history::body(ui, state);
    } else {
        changes(ui, state);
    }
    apply(state, asked);
}

/// The changes of the repository: the two halves of `git status`, the commit field, and the
/// gestures the list answers to.
fn changes(ui: &mut Ui, state: &mut AppState) {
    let mut asked = None;

    if let Some(error) = &state.git.error {
        ui.add_space(6.0);
        ui.weak(error.clone());
        // The one remedy worth offering here: a project that is not a repository can
        // become one, and the alternative is telling the user to go and type it.
        if state.git.repo.is_none()
            && ui
                .button("Initialise a repository")
                .on_hover_text("git init -b main, in the project root")
                .clicked()
        {
            state.git.init_repository();
        }
        return;
    }

    if !state.git.changes.is_empty() {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(state.git.selected.is_some(), egui::Button::new("Stage"))
                .on_hover_text("Put the selected file into the index")
                .clicked()
            {
                asked = Some(Asked::StageSelected);
            }
            if ui
                .add_enabled(state.git.selected.is_some(), egui::Button::new("Unstage"))
                .on_hover_text("Take the selected file out of the index")
                .clicked()
            {
                asked = Some(Asked::UnstageSelected);
            }
            if ui
                .button("Stage all")
                .on_hover_text("Put every change into the index")
                .clicked()
            {
                asked = Some(Asked::StageAll);
            }
        });
    }

    let mut enter_used = false;
    ui.horizontal(|ui| {
        let field = egui::TextEdit::singleline(&mut state.git.message)
            .hint_text("Commit message")
            .desired_width(ui.available_width() - 90.0);
        let response = ui.add(field);
        // Enter in the field commits: the field has the keyboard, so Enter cannot
        // mean "show the diff" here, and a second key to commit would be a mystery.
        // `lost_focus` and not `has_focus`: a single-line field gives the focus up
        // when Enter is pressed, so on that frame it no longer has it — the frame
        // that carries the Enter is the frame it lost the focus in.
        let typed_enter =
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        enter_used = typed_enter;
        if ui
            .add_enabled(
                !state.git.message.trim().is_empty(),
                egui::Button::new("Commit"),
            )
            .clicked()
            || typed_enter
        {
            asked = Some(Asked::Commit);
        }
    });

    if let Some(outcome) = &state.git.outcome {
        ui.add_space(2.0);
        ui.weak(outcome.clone());
    }
    ui.separator();

    if state.git.changes.is_empty() {
        ui.add_space(6.0);
        ui.weak("Nothing has changed since the last commit.");
        // The buttons above are live even then: History is exactly what a person
        // looks at when the work tree is clean, and a commit is what makes it so.
        apply(state, asked);
        return;
    }

    // The panel is in front, so the keyboard is not the editor's: Enter on the
    // selected change is the shortest way to its diff.
    if !enter_used && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        state.git.open_selected_diff();
    }

    let mut clicked = None;
    let mut opened = None;
    let mut selected = state.git.selected;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut index = 0;
            for (area, title) in [
                (Area::Staged, "In the index"),
                (Area::Worktree, "In the work tree"),
            ] {
                let changes: Vec<&Change> = match area {
                    Area::Staged => state.git.staged().collect(),
                    Area::Worktree => state.git.worktree().collect(),
                };
                if changes.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong(title);
                    ui.weak(changes.len().to_string());
                });
                for change in changes {
                    let response = change_row(ui, Some(index) == selected, change);
                    if response.clicked() {
                        clicked = Some(index);
                    }
                    // A single click only selects: it must not move the keyboard to
                    // the editor, or Enter would go to the text and not to the diff.
                    if response.double_clicked() {
                        opened = Some(change.path.clone());
                    }
                    response
                        .on_hover_text("Click to select, double-click to open, Enter for the diff");
                    index += 1;
                }
            }
        });

    if let Some(index) = clicked {
        selected = Some(index);
        // The diff belongs to the row that is selected now; an old one would be
        // answering a question nobody asked.
        state.git.diff = None;
        state.git.diff_error = None;
    }
    state.git.selected = selected;
    if let Some(path) = opened {
        state.pending.push(Action::OpenFile(path));
    }

    apply(state, asked);
}

/// One row of the list: the letter `git status` prints, in the colour of what happened, then the
/// path — the directories dimmed, the name of the file plain.
///
/// The whole row is the click, not just the letters: a list where only the text answers is a list
/// a person misses. The row is one row by construction, so a path too long for the panel is cut
/// where it ends instead of pushing the next change down while the list is being read.
fn change_row(ui: &mut Ui, selected: bool, change: &Change) -> egui::Response {
    let height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if selected {
        ui.painter().rect_filled(
            rect,
            2.0,
            ui.visuals().selection.bg_fill.gamma_multiply(0.35),
        );
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }

    let room = rect.shrink2(egui::vec2(6.0, 0.0));
    let mut job = change_text(change, ui.style());
    job.wrap = egui::text::TextWrapping {
        max_width: room.width(),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(room)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            // Not selectable: a label that takes the pointer for text selection would
            // swallow the click the row is for.
            ui.add(egui::Label::new(job).selectable(false));
        },
    );
    response
}

/// The text of a row, in three parts that read as one: the letter, then the path with its
/// directories dimmed.
///
/// The repository-relative path is whole, because in a repository the directory is the half of the
/// answer that tells two `mod.rs` apart, and `git status` prints it for the same reason.
fn change_text(change: &Change, style: &egui::Style) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("{}  ", change.kind.letter()),
        0.0,
        egui::TextFormat {
            font_id: egui::TextStyle::Monospace.resolve(style),
            color: letter_colour(change.kind),
            ..Default::default()
        },
    );
    let path = change.path.display().to_string();
    let plain = egui::TextFormat {
        font_id: egui::TextStyle::Body.resolve(style),
        color: style.visuals.strong_text_color(),
        ..Default::default()
    };
    let dimmed = egui::TextFormat {
        color: style.visuals.weak_text_color(),
        ..plain.clone()
    };
    match path.rfind('/') {
        Some(at) => {
            job.append(&path[..=at], 0.0, dimmed);
            job.append(&path[at + 1..], 0.0, plain);
        }
        None => job.append(&path, 0.0, plain),
    }
    job
}

/// The colour of a letter, by what happened to the file.
fn letter_colour(kind: Kind) -> Color32 {
    match kind {
        Kind::Added => ADDED,
        Kind::Modified | Kind::TypeChanged => MODIFIED,
        Kind::Deleted => DELETED,
        Kind::Renamed => RENAMED,
        Kind::Conflicted => CONFLICTED,
        Kind::Untracked => UNTRACKED,
    }
}

/// Carries out what the buttons were asked to do, once the list has been drawn.
fn apply(state: &mut AppState, asked: Option<Asked>) {
    match asked {
        Some(Asked::StageSelected) => state.git.stage_selected(),
        Some(Asked::UnstageSelected) => state.git.unstage_selected(),
        Some(Asked::StageAll) => state.git.stage_all(),
        Some(Asked::Commit) => state.git.commit(),
        Some(Asked::HideDiff) => state.git.diff_open = false,
        Some(Asked::Switch(name)) => state.git.switch_to(&name),
        Some(Asked::CreateBranch) => state.git.create_branch(),
        None => {}
    }
}

/// The branch in front, and the menu that changes it: the branches with the current one marked,
/// and the field that starts a new one.
///
/// The indicator is the door, and there is no second button for the same thing: switching is a
/// choice made against the state of the world, and the state of the world is the name that is
/// already on screen.
fn branch_menu(ui: &mut Ui, state: &mut AppState, asked: &mut Option<Asked>) {
    let Some(head) = state.git.branch() else {
        ui.strong("Git");
        return;
    };
    // The menu reads the list while the field is edited, so the field is taken out
    // for the duration and put back after: two mutable borrows of the same state
    // would not compile, and one of them being a text buffer is no reason to clone.
    let branches: Vec<Branch> = state.git.branches.clone();
    let mut name = std::mem::take(&mut state.git.branch_name);
    let mut chosen = None;
    let mut create = false;

    // The caret is the same `▾` the agent panel puts on its own menus: a name in a row of
    // labels is a name, and a name with a caret is a door.
    let menu = ui.menu_button(RichText::new(format!("{head} ▾")).strong(), |ui| {
        ui.set_min_width(170.0);
        for branch in &branches {
            let label = if branch.is_head {
                format!("• {}", branch.name)
            } else {
                branch.name.clone()
            };
            if ui.button(label).clicked() {
                chosen = Some(branch.name.clone());
                ui.close();
            }
        }
        if !branches.is_empty() {
            ui.separator();
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut name)
                    .hint_text("new branch")
                    .desired_width(110.0),
            );
            if ui.button("New branch").clicked() {
                create = true;
                ui.close();
            }
        });
    });
    menu.response
        .on_hover_text("Switch to another branch, or start a new one");

    state.git.branch_name = name;
    if let Some(chosen) = chosen {
        *asked = Some(Asked::Switch(chosen));
    }
    if create {
        *asked = Some(Asked::CreateBranch);
    }
}

/// Fetch, pull and push — the commands that talk to a remote.
///
/// They run on their own thread, so the whole row is disabled while one is in flight
/// and the panel says which: a second `fetch` on top of the first would ask the same
/// question twice and race for the answer. Added right to left, because the caller puts them in
/// the corner of the row: the first one added is the one on the right, and the row still reads
/// `Fetch Pull Push` from the left.
fn remote_buttons(ui: &mut Ui, state: &mut AppState) {
    let busy = state.git.network_busy;
    if ui
        .add_enabled(busy.is_none(), egui::Button::new("Push"))
        .clicked()
    {
        state.git.push();
    }
    if ui
        .add_enabled(busy.is_none(), egui::Button::new("Pull"))
        .clicked()
    {
        state.git.pull();
    }
    if ui
        .add_enabled(busy.is_none(), egui::Button::new("Fetch"))
        .clicked()
    {
        state.git.fetch();
    }
    if let Some(doing) = busy {
        ui.weak(format!("{doing}…"));
    }
}
