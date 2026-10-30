//! The Git panel: the branch, the two halves of `git status`, the commit field.
//!
//! A click selects a change, a double click opens the file, Enter shows the diff.
//! Staging and committing are buttons on purpose: they change the repository, and a
//! gesture that changes something should be one the user can see and aim at.

use egui::Ui;
use ide_git::{Area, Branch, Change};

use crate::state::{Action, AppState};

use super::{diff, history};

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
        match state.git.branch() {
            Some(branch) => {
                ui.strong(branch);
                let total = state.git.changes.len();
                ui.separator();
                ui.weak(format!(
                    "{total} {}",
                    if total == 1 { "change" } else { "changes" }
                ));
            }
            None => {
                ui.strong("Git");
            }
        }
        ui.separator();
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
        branches_menu(ui, state, &mut asked);
        remote_buttons(ui, state);
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
                ui.add_space(4.0);
                ui.weak(title);
                for change in changes {
                    // The whole repository-relative path: in a repository the
                    // directory is the half of the answer that tells two `mod.rs`
                    // apart, and `git status` prints it for the same reason.
                    let text = format!("{}  {}", change.kind.letter(), change.path.display());
                    let response = ui.add(
                        egui::Label::new(text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    if Some(index) == selected {
                        ui.painter().rect_stroke(
                            response.rect.expand(2.0),
                            2.0,
                            ui.visuals().selection.stroke,
                            egui::StrokeKind::Inside,
                        );
                    }
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

/// The branches, and the field that starts a new one.
///
/// Switching is a menu rather than a keystroke: it moves `HEAD`, and a list with the
/// current branch marked is the state of the world the choice is made against.
fn branches_menu(ui: &mut Ui, state: &mut AppState, asked: &mut Option<Asked>) {
    // The menu reads the list while the field is edited, so the field is taken out
    // for the duration and put back after: two mutable borrows of the same state
    // would not compile, and one of them being a text buffer is no reason to clone.
    let branches: Vec<Branch> = state.git.branches.clone();
    let mut name = std::mem::take(&mut state.git.branch_name);

    ui.menu_button("Branches", |ui| {
        for branch in &branches {
            let label = if branch.is_head {
                format!("• {}", branch.name)
            } else {
                branch.name.clone()
            };
            if ui.button(label).clicked() {
                *asked = Some(Asked::Switch(branch.name.clone()));
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
                    .desired_width(140.0),
            );
            if ui.button("New branch").clicked() {
                *asked = Some(Asked::CreateBranch);
                ui.close();
            }
        });
    });

    state.git.branch_name = name;
}

/// Fetch, pull and push — the commands that talk to a remote.
///
/// They run on their own thread, so the whole row is disabled while one is in flight
/// and the panel says which: a second `fetch` on top of the first would ask the same
/// question twice and race for the answer.
fn remote_buttons(ui: &mut Ui, state: &mut AppState) {
    if !state.git.has_remote {
        return;
    }
    let busy = state.git.network_busy;
    ui.horizontal(|ui| {
        let fetch = ui.add_enabled(busy.is_none(), egui::Button::new("Fetch"));
        if fetch.clicked() {
            state.git.fetch();
        }
        let pull = ui.add_enabled(busy.is_none(), egui::Button::new("Pull"));
        if pull.clicked() {
            state.git.pull();
        }
        let push = ui.add_enabled(busy.is_none(), egui::Button::new("Push"));
        if push.clicked() {
            state.git.push();
        }
        if let Some(doing) = busy {
            ui.weak(format!("{doing}…"));
        }
    });
}
