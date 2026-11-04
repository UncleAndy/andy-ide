//! Asking the desktop for a directory: the dialog a project is opened from.
//!
//! The dialog is the desktop's, not this window's — the portal draws it, the person picks a folder
//! and the answer comes back here. It is asked on a thread of its own so the window keeps drawing
//! while it is up: a window that stops answering because a dialog is open reads as a window that
//! has died, and the person has no way to tell the two apart.

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::mpsc;

/// Who is asked for a directory.
pub trait Chooser: Send + Sync {
    /// Blocks until the person picks a folder or gives up.
    fn choose(&self, start: Option<&Path>) -> Option<PathBuf>;
}

/// The desktop's own dialog, through the portal.
pub struct Desktop;

impl Chooser for Desktop {
    fn choose(&self, start: Option<&Path>) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title("Open a project");
        if let Some(start) = start
            && start.is_dir()
        {
            dialog = dialog.set_directory(start);
        }
        dialog.pick_folder()
    }
}

/// A chooser that answers from a list: what a check uses in place of a person.
///
/// Only a check constructs one: the window always asks the desktop.
///
/// It also remembers what it was asked, because half of the dialog is the question: a folder
/// offered as the starting place is the folder the person was looking at.
#[cfg(test)]
#[derive(Default)]
pub struct Written {
    answers: Mutex<std::collections::VecDeque<Option<PathBuf>>>,
    asked: Mutex<Vec<Option<PathBuf>>>,
}

#[cfg(test)]
impl Written {
    /// A chooser that answers with these, in this order; `None` is a person giving up.
    pub fn answering(answers: Vec<Option<PathBuf>>) -> Written {
        let written = Written::default();
        *written.answers.lock().expect("an answer list") = answers.into();
        written
    }

    /// What the dialog was asked, one entry per question.
    pub fn asked(&self) -> Vec<Option<PathBuf>> {
        self.asked.lock().expect("an answer list").clone()
    }
}

#[cfg(test)]
impl Chooser for Written {
    fn choose(&self, start: Option<&Path>) -> Option<PathBuf> {
        self.asked
            .lock()
            .expect("an answer list")
            .push(start.map(Path::to_path_buf));
        self.answers
            .lock()
            .expect("an answer list")
            .pop_front()
            .flatten()
    }
}

/// The channel a dialog answers on.
pub type Answer = mpsc::Receiver<Option<PathBuf>>;
