//! What a launch puts in front.
//!
//! A window started from a terminal in a directory opens that directory: a project is a place a
//! person names, and standing in it is naming it. A window started from the launcher has nothing
//! named at all — the desktop starts programs in the person's home — and a home is not a project:
//! opening one walks and watches an entire disk (a recursive inotify read of every directory, an
//! index of every file), which is minutes of work to show a tree nobody asked for.
//!
//! So a launch in the home asks the recent list for a project that no other window has open, and
//! keeps a project of its own for when none is left: `<home>/andy-ide/projects/default`, made on
//! demand. One project, one window: the lock that enforces it is in [`crate::lock`].

use std::path::{Path, PathBuf};

use crate::lock::{Locked, ProjectLock};

/// Where the project of a launch that was given none comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// Named on the command line.
    Given,
    /// The directory the program was started in.
    Here,
    /// The most recent project that no other window has open.
    Recent,
    /// The window's own project, made when there is nothing better to show.
    House,
}

/// What the launched window shows.
#[derive(Debug)]
pub enum Answer {
    /// A project to put in front, with the lock this window holds for as long as it lives.
    Show {
        project: PathBuf,
        how: How,
        /// `None` when the project cannot be locked at all — it is opened anyway, without the guard.
        lock: Option<ProjectLock>,
    },
    /// The project is open in another window, so this window must not open at all.
    Busy { project: PathBuf },
    /// Nothing to open: nothing was named, the directory the program stands in is the home, and
    /// there is neither a recent project nor a home to keep one in.
    Nothing,
}

/// The person's home, as the system says.
pub fn home() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
}

/// The project a launch with nothing to open keeps for itself: `<home>/andy-ide/projects/default`.
pub fn house(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|home| home.join("andy-ide").join("projects").join("default"))
}

/// Whether a directory may stand as a project at all.
///
/// The root of a disk and the person's own home are places to live in, not projects. Opening either
/// means walking and watching everything under it — minutes of work, and on a home that carries
/// other people's repositories, tens of thousands of watches.
pub fn may_be_a_project(dir: &Path, home: Option<&Path>) -> bool {
    // No parent means the root of a disk: `/` on Unix, `C:\` on Windows.
    if dir.parent().is_none() {
        return false;
    }
    match home {
        Some(home) => dir != home && !home.starts_with(dir),
        None => true,
    }
}

/// The project a launch puts in front, and the lock it holds on it.
///
/// `named` is `--project`, `here` the directory the program was started in, `recents` the list of
/// projects used before (newest first), and `house` the project of the window's own.
pub fn decide(
    named: Option<PathBuf>,
    here: Option<PathBuf>,
    recents: &[PathBuf],
    house: Option<&Path>,
    home: Option<&Path>,
) -> Answer {
    // What a person names wins over everything: an explicit word is not a guess to be improved on.
    if let Some(named) = named {
        return hold(named, How::Given);
    }
    // Standing in a directory names it — unless the directory is the home, which is where the
    // launcher leaves every program it starts.
    if let Some(here) = here.filter(|dir| may_be_a_project(dir, home)) {
        return hold(here, How::Here);
    }
    // The home itself, or a place too wide to be a project: ask the list of projects used before.
    for candidate in recents {
        if !candidate.is_dir() || !may_be_a_project(candidate, home) {
            // A directory that is gone, or the home as it was written down by an older build.
            continue;
        }
        match ProjectLock::take(candidate) {
            Ok(lock) => {
                return Answer::Show {
                    project: candidate.clone(),
                    how: How::Recent,
                    lock: Some(lock),
                };
            }
            // Another window has it: the next one in the list is the answer, not a refusal.
            Err(Locked::AnotherWindow) => continue,
            // A project nobody can lock is still a project to open.
            Err(Locked::Cannot(_)) => {
                return Answer::Show {
                    project: candidate.clone(),
                    how: How::Recent,
                    lock: None,
                };
            }
        }
    }
    match house {
        Some(house) => {
            // Made on demand: a person who has never had a project still gets a window with one.
            let _ = std::fs::create_dir_all(house);
            hold(house.to_path_buf(), How::House)
        }
        None => Answer::Nothing,
    }
}

/// Opens `project` unless another window holds it.
fn hold(project: PathBuf, how: How) -> Answer {
    match ProjectLock::take(&project) {
        Ok(lock) => Answer::Show {
            project,
            how,
            lock: Some(lock),
        },
        Err(Locked::AnotherWindow) => Answer::Busy { project },
        Err(Locked::Cannot(_)) => Answer::Show {
            project,
            how,
            lock: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Answer, How, decide, house, may_be_a_project};
    use crate::lock::ProjectLock;
    use std::path::{Path, PathBuf};

    /// A temp directory standing for a home, with `names` directories in it.
    fn home_with(names: &[&str]) -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_path_buf();
        let mut made = Vec::new();
        for name in names {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            made.push(dir);
        }
        (home, root, made)
    }

    fn shown(answer: Answer) -> (PathBuf, How) {
        match answer {
            Answer::Show { project, how, .. } => (project, how),
            other => panic!("the launch did not open a project: {other:?}"),
        }
    }

    #[test]
    fn a_named_project_wins_over_the_directory_and_the_list() {
        // `--project` is a word, not a guess: nothing else may overrule it.
        let (_home, root, dirs) = home_with(&["a", "b"]);
        let (project, how) = shown(decide(
            Some(dirs[1].clone()),
            Some(dirs[0].clone()),
            &[dirs[0].clone()],
            Some(&root.join("andy-ide/projects/default")),
            Some(&root),
        ));
        assert_eq!(project, dirs[1]);
        assert_eq!(how, How::Given);
    }

    #[test]
    fn the_directory_the_program_stands_in_is_the_project() {
        // Started from a terminal inside a project, the window opens that project — the personal
        // list of what was used before has no say.
        let (_home, root, dirs) = home_with(&["project", "elsewhere"]);
        let (project, how) = shown(decide(
            None,
            Some(dirs[0].clone()),
            &[dirs[1].clone()],
            Some(&root.join("andy-ide/projects/default")),
            Some(&root),
        ));
        assert_eq!(project, dirs[0]);
        assert_eq!(how, How::Here);
    }

    #[test]
    fn a_launch_in_the_home_opens_the_most_recent_project() {
        // The launcher's own case: the directory is the home, so the home is not the answer.
        let (_home, root, dirs) = home_with(&["work", "older"]);
        let (project, how) = shown(decide(
            None,
            Some(root.clone()),
            &[dirs[0].clone(), dirs[1].clone()],
            Some(&root.join("andy-ide/projects/default")),
            Some(&root),
        ));
        assert_eq!(project, dirs[0], "the newest usable project was not opened");
        assert_eq!(how, How::Recent);
    }

    #[test]
    fn a_project_another_window_holds_is_passed_over() {
        // The second press of the launcher must not land on the project already on screen: it
        // takes the next one in the list.
        let (_home, root, dirs) = home_with(&["busy", "free"]);
        let held = ProjectLock::take(&dirs[0]).unwrap();
        let (project, how) = shown(decide(
            None,
            Some(root.clone()),
            &[dirs[0].clone(), dirs[1].clone()],
            Some(&root.join("andy-ide/projects/default")),
            Some(&root),
        ));
        assert_eq!(
            project, dirs[1],
            "a project with a window of its own was opened again"
        );
        assert_eq!(how, How::Recent);
        drop(held);
    }

    #[test]
    fn the_home_in_the_list_is_never_a_candidate() {
        // An older build wrote the home into the list; reading it back would open the whole disk
        // again, which is the thing this whole rule exists to prevent.
        let (_home, root, dirs) = home_with(&["work"]);
        let (project, how) = shown(decide(
            None,
            Some(root.clone()),
            &[root.clone(), dirs[0].clone()],
            Some(&root.join("andy-ide/projects/default")),
            Some(&root),
        ));
        assert_eq!(project, dirs[0]);
        assert_eq!(how, How::Recent);
    }

    #[test]
    fn a_directory_that_is_gone_is_passed_over() {
        let (_home, root, dirs) = home_with(&["work"]);
        let gone = root.join("was-deleted");
        let (project, how) = shown(decide(
            None,
            Some(root.clone()),
            &[gone, dirs[0].clone()],
            Some(&root.join("andy-ide/projects/default")),
            Some(&root),
        ));
        assert_eq!(project, dirs[0]);
        assert_eq!(how, How::Recent);
    }

    #[test]
    fn when_everything_is_open_the_project_of_the_window_itself_is_made_and_opened() {
        // Nothing free is left: the window opens the project it keeps for exactly this, and makes
        // it if a person has never had one.
        let (_home, root, dirs) = home_with(&["busy"]);
        let held = ProjectLock::take(&dirs[0]).unwrap();
        let house_dir = root.join("andy-ide/projects/default");
        let (project, how) = shown(decide(
            None,
            Some(root.clone()),
            &[dirs[0].clone()],
            Some(&house_dir),
            Some(&root),
        ));
        assert_eq!(project, house_dir);
        assert_eq!(how, How::House);
        assert!(project.is_dir(), "the project of its own was not made");
        drop(held);
    }

    #[test]
    fn a_named_project_a_second_window_holds_is_refused() {
        // No fallback here: a person named this project, and opening another one instead would be
        // answering a question nobody asked.
        let (_home, root, dirs) = home_with(&["work"]);
        let held = ProjectLock::take(&dirs[0]).unwrap();
        assert!(matches!(
            decide(
                Some(dirs[0].clone()),
                None,
                &[],
                Some(&root.join("andy-ide/projects/default")),
                Some(&root),
            ),
            Answer::Busy { .. }
        ));
        drop(held);
    }

    #[test]
    fn the_project_of_the_window_itself_is_refused_too_when_it_is_open() {
        // "Everything is open" ends in a message, not in a second window on the same project.
        let (_home, root, _) = home_with(&[]);
        let house_dir = root.join("andy-ide/projects/default");
        std::fs::create_dir_all(&house_dir).unwrap();
        let held = ProjectLock::take(&house_dir).unwrap();
        assert!(matches!(
            decide(None, Some(root.clone()), &[], Some(&house_dir), Some(&root)),
            Answer::Busy { .. }
        ));
        drop(held);
        assert!(matches!(
            decide(None, Some(root.clone()), &[], Some(&house_dir), Some(&root)),
            Answer::Show {
                how: How::House,
                ..
            }
        ));
    }

    #[test]
    fn the_home_and_the_root_are_not_projects() {
        let home = Path::new("/home/person");
        assert!(may_be_a_project(Path::new("/home/person/work"), Some(home)));
        assert!(
            !may_be_a_project(home, Some(home)),
            "the home was taken for a project"
        );
        assert!(
            !may_be_a_project(Path::new("/home"), Some(home)),
            "an ancestor of the home was taken for a project"
        );
        assert!(
            !may_be_a_project(Path::new("/"), Some(home)),
            "the root was taken for a project"
        );
    }

    #[test]
    fn the_window_keeps_its_project_under_the_home() {
        let home = Path::new("/home/person");
        assert_eq!(
            house(Some(home)),
            Some(PathBuf::from("/home/person/andy-ide/projects/default"))
        );
        assert_eq!(house(None), None);
    }
}
