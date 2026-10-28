//! Project root and lazy directory listing.
//!
//! A project is a directory on disk. Every path the editor works with is kept
//! relative to that root, so a project can be moved or renamed without touching
//! the files it points at.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;

/// What a node of the project tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Dir,
}

impl EntryKind {
    /// Sort key: directories come before files.
    fn rank(self) -> u8 {
        match self {
            EntryKind::Dir => 0,
            EntryKind::File => 1,
        }
    }
}

/// One node of the project tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path relative to the project root.
    pub rel: PathBuf,
    /// Last component of the path, for display.
    pub name: String,
    pub kind: EntryKind,
}

/// How a directory should be listed.
#[derive(Debug, Clone, Copy, Default)]
pub struct TreeOptions {
    /// Show dotfiles. `.gitignore` rules apply either way.
    pub show_hidden: bool,
}

/// An opened project.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
}

impl Project {
    /// Opens an existing directory as a project.
    ///
    /// The root is canonicalised when the platform allows it; a project opened
    /// through a symlink still strips paths correctly, because the walk starts
    /// from the same root value.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            bail!(
                "project root {} is not an existing directory",
                root.display()
            );
        }
        let root = root.canonicalize().unwrap_or(root);
        Ok(Self { root })
    }

    /// The absolute root of the project.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Lists one level of the project tree.
    ///
    /// Directories come first, then files; within each group names are sorted
    /// case-insensitively. Hidden entries and anything matched by `.gitignore`
    /// are skipped unless [`TreeOptions::show_hidden`] asks for the former.
    pub fn list_dir(&self, rel: &Path, opts: &TreeOptions) -> Result<Vec<Entry>> {
        let dir = if rel.as_os_str().is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        };
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }

        let mut builder = WalkBuilder::new(&dir);
        builder
            .max_depth(Some(1))
            .hidden(!opts.show_hidden)
            .git_ignore(true)
            .git_exclude(true)
            .require_git(false)
            .follow_links(false);

        let mut entries = Vec::new();
        for item in builder.build() {
            let dent = item.with_context(|| format!("cannot read directory {}", dir.display()))?;
            if dent.depth() == 0 {
                // The directory itself, not one of its children.
                continue;
            }
            let path = dent.path();
            let name = dent.file_name().to_string_lossy().into_owned();
            let kind = match dent.file_type() {
                Some(ft) if ft.is_dir() => EntryKind::Dir,
                _ => EntryKind::File,
            };
            let rel = path
                .strip_prefix(&self.root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| PathBuf::from(&name));
            entries.push(Entry { rel, name, kind });
        }

        entries.sort_by(|a, b| {
            a.kind
                .rank()
                .cmp(&b.kind.rank())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }
}
