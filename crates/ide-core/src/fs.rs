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

    // The rights of the file being replaced are carried over to the new one: the rename would otherwise leave
    // it with whatever the umask gives, and a file whose rights were arranged so that a person and an agent can
    // both write it must not lose them because a save went through a temporary file. Andy: "редактор меняет
    // права на редактируемые файлы во время работы… права сразу настроены так, чтобы доступ был и у меня, и
    // у него".
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }

    std::fs::rename(&temp, path)
        .with_context(|| format!("cannot replace {} with {}", path.display(), temp.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_rights_of_the_file_that_is_replaced_are_kept() {
        let dir = tempfile::tempdir().unwrap();

        // What a brand-new file gets here is decided by the umask — and that is exactly what must not be
        // inherited by a file that already had its own rights. Reading it first is what makes this check
        // able to tell the two apart: without it the check passed even with the rights dropped, because on
        // this machine the umask happens to give the mode the check asked for.
        let probe = dir.path().join("probe.rs");
        write_atomic(&probe, b"probe").unwrap();
        let umask_mode = std::fs::metadata(&probe).unwrap().permissions().mode() & 0o777;
        let wanted = [0o640u32, 0o660, 0o606, 0o600]
            .into_iter()
            .find(|mode| *mode != umask_mode)
            .expect("some mode differs from the one the umask gives");

        // A file arranged for two people: a mode the umask would not give.
        let path = dir.path().join("shared.rs");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(wanted)).unwrap();

        write_atomic(&path, b"new").unwrap();

        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"new",
            "the new content did not arrive"
        );
        let got = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            got, wanted,
            "the rights were lost in the save: the file has the umask's {umask_mode:o} instead of {wanted:o}"
        );
    }

    #[test]
    fn a_file_that_did_not_exist_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.rs");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
    }
}
