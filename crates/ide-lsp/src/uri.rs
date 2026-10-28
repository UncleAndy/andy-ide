//! `file://` addresses.
//!
//! `lsp-types` 0.97 keeps URIs as `fluent_uri::Uri`, which has no conversion
//! from a path, so the two directions live here: paths are percent-encoded as
//! UTF-8 and decoded the same way.

use std::path::{Path, PathBuf};

use lsp_types::Uri;

/// `file:///path/to/file`, with everything outside the unreserved set escaped.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let text = path.to_str()?;
    let mut encoded = String::with_capacity(text.len() + 8);
    encoded.push_str("file://");
    for byte in text.bytes() {
        match byte {
            b'/' | b'-' | b'_' | b'.' | b'~' => encoded.push(byte as char),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => encoded.push(byte as char),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded.parse::<Uri>().ok()
}

/// The path behind a `file://` address, with escapes decoded.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let text = uri.as_str();
    let rest = text.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[index + 1..index + 3]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_path_round_trips() {
        let path = Path::new("/home/andy/projects/andy-ide/src/main.rs");
        let uri = path_to_uri(path).expect("a uri");
        assert_eq!(
            uri.as_str(),
            "file:///home/andy/projects/andy-ide/src/main.rs"
        );
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn spaces_cyrillic_and_hash_are_escaped() {
        let path = Path::new("/home/andy/Проект с пробелом/a#b.rs");
        let uri = path_to_uri(path).expect("a uri");
        assert!(!uri.as_str().contains(' '), "got {}", uri.as_str());
        assert!(!uri.as_str().contains('#'), "got {}", uri.as_str());
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn a_non_file_uri_has_no_path() {
        let uri: Uri = "https://example.com/x".parse().unwrap();
        assert!(uri_to_path(&uri).is_none());
    }
}
