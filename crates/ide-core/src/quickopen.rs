//! A flat index of the project's files and fuzzy search over it.
//!
//! The index is built once when a project is opened and searched on every
//! keystroke in the quick-open field, so the matcher runs over a plain list of
//! relative path strings and nothing else.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use nucleo::Config;
use nucleo::Matcher;
use nucleo::Utf32Str;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Path relative to the project root.
    pub rel: PathBuf,
    /// Matcher score; higher is a better match. Zero for an empty query.
    pub score: u32,
}

/// Every file of a project, for quick-open.
#[derive(Debug, Clone, Default)]
pub struct FileIndex {
    /// `(path as display string, path relative to the root)`, sorted by path.
    files: Vec<(String, PathBuf)>,
}

impl FileIndex {
    /// Walks the project and indexes its files.
    ///
    /// Directories are not indexed, `.gitignore` is honoured even outside a git
    /// repository, and hidden entries are skipped unless `show_hidden` is set.
    pub fn build(root: &Path, show_hidden: bool) -> Result<FileIndex> {
        if !root.is_dir() {
            anyhow::bail!("{} is not a directory", root.display());
        }

        let mut builder = WalkBuilder::new(root);
        builder
            .hidden(!show_hidden)
            .git_ignore(true)
            .git_exclude(true)
            .require_git(false)
            .follow_links(false);

        let mut files = Vec::new();
        for item in builder.build() {
            let entry = item.with_context(|| format!("cannot walk {}", root.display()))?;
            if entry.depth() == 0 {
                continue;
            }
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.to_path_buf());
            files.push((rel.to_string_lossy().into_owned(), rel));
        }

        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(FileIndex { files })
    }

    /// Fuzzy-searches the index.
    ///
    /// An empty query returns the first `limit` files in path order. Otherwise
    /// the matcher runs in path mode, which favours matches inside the last path
    /// segment, and ties are broken by the shorter path.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        if limit == 0 {
            return Vec::new();
        }
        let query = query.trim();
        if query.is_empty() {
            return self
                .files
                .iter()
                .take(limit)
                .map(|(_, rel)| SearchHit {
                    rel: rel.clone(),
                    score: 0,
                })
                .collect();
        }

        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buffer = Vec::new();

        let mut scored: Vec<(usize, u32)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(index, (text, _))| {
                pattern
                    .score(Utf32Str::new(text, &mut buffer), &mut matcher)
                    .map(|score| (index, score))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| self.files[a.0].0.len().cmp(&self.files[b.0].0.len()))
                .then_with(|| self.files[a.0].0.cmp(&self.files[b.0].0))
        });

        scored
            .into_iter()
            .take(limit)
            .map(|(index, score)| SearchHit {
                rel: self.files[index].1.clone(),
                score,
            })
            .collect()
    }

    /// How many files the index holds.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}
