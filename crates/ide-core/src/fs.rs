//! File writes that never leave a half-written file behind.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow};

/// Writes `bytes` to `path` atomically.
///
/// The data goes to a temporary file in the same directory, is flushed to disk,
/// and only then replaces the target by rename. A crash mid-write leaves the old
/// content intact.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create directory {}", parent.display()))?;

    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("path {} has no file name", path.display()))?;
    let temp = parent.join(format!(
        "{}.tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));

    {
        let mut file = std::fs::File::create(&temp)
            .with_context(|| format!("cannot create {}", temp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("cannot write {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("cannot flush {}", temp.display()))?;
    }

    std::fs::rename(&temp, path)
        .with_context(|| format!("cannot replace {} with {}", path.display(), temp.display()))?;
    Ok(())
}
