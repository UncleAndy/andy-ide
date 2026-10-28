//! The wire format of LSP: a `Content-Length` header, a blank line, then that
//! many bytes of JSON.

use std::io::{self, BufRead, Write};

/// Writes one message with its header.
pub fn write_message(out: &mut impl Write, message: &serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(&body)?;
    out.flush()
}

/// Reads one message, or `None` at the end of the stream.
///
/// Header fields other than `Content-Length` (servers send `Content-Type` too)
/// are skipped, and a header line longer than [`MAX_HEADER`] is an error rather
/// than an endless allocation.
pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<serde_json::Value>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let read = input.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if line.len() > MAX_HEADER {
            return Err(io::Error::other("header line is too long"));
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }

    let length = length.ok_or_else(|| io::Error::other("message without Content-Length"))?;
    let mut body = vec![0u8; length];
    input.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body)
        .map_err(|error| io::Error::other(format!("message is not JSON: {error}")))?;
    Ok(Some(value))
}

/// A header line that long is not LSP, it is a broken pipe.
const MAX_HEADER: usize = 8 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_message_survives_a_round_trip() {
        let message =
            json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {"текст": "привет"}});
        let mut buffer = Vec::new();
        write_message(&mut buffer, &message).unwrap();

        let text = String::from_utf8(buffer.clone()).unwrap();
        assert!(text.starts_with("Content-Length: "), "got {text:?}");
        // The length counts bytes, not characters.
        let header = text.split("\r\n\r\n").next().unwrap();
        let declared: usize = header
            .trim_start_matches("Content-Length: ")
            .parse()
            .unwrap();
        assert_eq!(declared, buffer.len() - header.len() - 4);

        let mut reader = std::io::Cursor::new(buffer);
        let back = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(back, message);
        assert!(
            read_message(&mut reader).unwrap().is_none(),
            "one message only"
        );
    }

    #[test]
    fn other_headers_are_ignored() {
        let body = r#"{"id":7}"#;
        let raw = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}",
            body.len(),
            body
        );
        let mut reader = std::io::Cursor::new(raw.into_bytes());
        let message = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(message["id"], 7);
    }

    #[test]
    fn several_messages_are_read_in_order() {
        let mut buffer = Vec::new();
        for id in 0..3 {
            write_message(&mut buffer, &json!({"id": id})).unwrap();
        }
        let mut reader = std::io::Cursor::new(buffer);
        for id in 0..3 {
            let message = read_message(&mut reader).unwrap().unwrap();
            assert_eq!(message["id"], id);
        }
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn a_message_without_a_length_is_an_error() {
        let mut reader = std::io::Cursor::new(b"Content-Type: json\r\n\r\n{}".to_vec());
        let error = read_message(&mut reader).unwrap_err();
        assert!(error.to_string().contains("Content-Length"));
    }

    #[test]
    fn a_truncated_body_is_an_error() {
        let mut reader = std::io::Cursor::new(b"Content-Length: 20\r\n\r\n{}".to_vec());
        assert!(read_message(&mut reader).is_err());
    }
}
