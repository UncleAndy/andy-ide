//! End-to-end: a real child process, the real transport, the real handshake.
//!
//! The server is `tests/support/fake_lsp.rs`, built by cargo as the `fake-lsp`
//! binary. It answers from the text it was given in `didOpen`, so a wrong
//! document synchronisation shows up as a wrong answer rather than as a passing
//! test.

use std::time::{Duration, Instant};

use ide_lsp::registry::ServerConfig;
use ide_lsp::{LspSession, position_to_byte};
use lsp_types::Position;

static FAKE: ServerConfig = ServerConfig {
    name: "fake-lsp",
    command: env!("CARGO_BIN_EXE_fake-lsp"),
    args: &[],
    language_id: "rust",
    extensions: &["rs"],
};

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "// заголовок\nfn main() {}\n").unwrap();
    dir
}

fn wait_for<T>(mut attempt: impl FnMut() -> Option<T>, what: &str) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(value) = attempt() {
            return value;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{what} never arrived");
}

#[test]
fn the_handshake_document_sync_and_a_definition_request() {
    let dir = project();
    let mut session = LspSession::start(&FAKE, dir.path()).expect("the server starts");
    assert!(session.is_alive());
    assert_eq!(session.name(), "fake-lsp");

    let text = "// заголовок\nfn main() {}\n";
    let path = dir.path().join("lib.rs");
    let relative = std::path::Path::new("lib.rs");
    session
        .did_open(relative, text)
        .expect("the document is sent");

    // The fixture publishes a diagnostic for every document it is given, which
    // proves didOpen arrived and the notification path works.
    let diagnostics = wait_for(
        || {
            let found = session.drain_diagnostics();
            (!found.is_empty()).then_some(found)
        },
        "diagnostics",
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].diagnostics[0].message,
        "the fixture noticed this line"
    );

    let answer = session
        .definition(
            relative,
            Position {
                line: 1,
                character: 3,
            },
        )
        .expect("a definition answer");
    assert_eq!(answer.len(), 1, "one location");
    // The range covers the first line of the document, which the fixture could
    // only know if the text arrived intact, cyrillic and all.
    assert_eq!(answer[0].range.start.line, 0);
    assert_eq!(
        answer[0].range.end.character as usize,
        "// заголовок".chars().count()
    );
    assert_eq!(
        ide_lsp::uri_to_path(&answer[0].uri).as_deref(),
        Some(path.as_path()),
        "the answer points at the file that was opened"
    );

    let hover = session
        .hover(
            relative,
            Position {
                line: 0,
                character: 0,
            },
        )
        .expect("a hover answer")
        .expect("some hover");
    let rendered = format!("{:?}", hover.contents);
    assert!(rendered.contains(&text.len().to_string()), "got {rendered}");

    session.shutdown();
}

#[test]
fn a_change_reaches_the_server() {
    let dir = project();
    let mut session = LspSession::start(&FAKE, dir.path()).expect("the server starts");
    let relative = std::path::Path::new("lib.rs");

    session.did_open(relative, "one\n").unwrap();
    session.did_change(relative, "// два\n").unwrap();

    // The fixture answers with the first line it currently holds.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = Vec::new();
    while Instant::now() < deadline {
        last = session
            .definition(
                relative,
                Position {
                    line: 0,
                    character: 0,
                },
            )
            .unwrap();
        if last
            .first()
            .map(|location| location.range.end.character == "// два".chars().count() as u32)
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        last[0].range.end.character,
        "// два".chars().count() as u32,
        "the server still holds the old text"
    );

    session.shutdown();
}

#[test]
fn a_server_that_is_not_there_is_an_error_not_a_panic() {
    static MISSING: ServerConfig = ServerConfig {
        name: "missing",
        command: "definitely-not-a-language-server-here",
        args: &[],
        language_id: "rust",
        extensions: &["rs"],
    };
    let dir = project();
    let error = match LspSession::start(&MISSING, dir.path()) {
        Ok(_) => panic!("a missing program cannot start"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("cannot start"),
        "got {error:#}"
    );
}

#[test]
fn an_unknown_server_request_does_not_stop_the_session() {
    let dir = project();
    let mut session = LspSession::start(&FAKE, dir.path()).unwrap();
    // The fixture answers unknown methods with an error; asking for one must not
    // leave the session unusable.
    let error = session
        .references(
            std::path::Path::new("lib.rs"),
            Position {
                line: 0,
                character: 0,
            },
        )
        .expect("references are answered");
    assert!(error.is_empty());

    let answer = session
        .definition(
            std::path::Path::new("lib.rs"),
            Position {
                line: 0,
                character: 0,
            },
        )
        .expect("the session still works");
    assert_eq!(answer.len(), 1);
    session.shutdown();
}

#[test]
fn positions_and_bytes_agree_on_a_real_document() {
    let text = "// заголовок\nfn main() {}\n";
    let byte = position_to_byte(
        text,
        Position {
            line: 1,
            character: 3,
        },
    );
    assert_eq!(&text[byte..byte + 4], "main");
}
