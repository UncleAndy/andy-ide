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
    markers: &[],
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

/// Подсказка по параметрам: ответ доехал, сервер назвал текущий параметр и получил контекст —
/// то, что спросили именно после «(». Заглушка отвечает отголоском триггера, и это единственное,
/// по чему видно, что контекст не потерялся по дороге: без него в метке будет "(none)", и
/// проверка обязана упасть.
#[test]
fn the_parameters_of_a_call_come_back() {
    let dir = project();
    let mut session = LspSession::start(&FAKE, dir.path()).expect("the server starts");
    let text = "// заголовок\nfn main() {}\n";
    let relative = std::path::Path::new("lib.rs");
    session
        .did_open(relative, text)
        .expect("the document is sent");

    let help = session
        .signature_help(
            relative,
            Position {
                line: 0,
                character: 0,
            },
            Some(ide_lsp::triggered_by('(')),
        )
        .expect("a signature help answer")
        .expect("some signature help");

    assert_eq!(
        help.active_parameter,
        Some(1),
        "the answer says which parameter is being written"
    );
    assert_eq!(help.signatures.len(), 1, "one signature came back");
    let parameters = help.signatures[0]
        .parameters
        .as_ref()
        .expect("the parameters come back with the signature");
    assert_eq!(parameters.len(), 2, "both parameters came back");
    assert!(
        help.signatures[0].label.ends_with("after ("),
        "the trigger reached the server: got {}",
        help.signatures[0].label
    );

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
        markers: &[],
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
fn a_server_that_leaves_before_answering_is_not_waited_out() {
    // A program that starts and goes at once is not a rare shape: a rustup proxy whose toolchain has no
    // `rust-analyzer` component is on `PATH`, starts, and exits without a word. The handshake used to wait
    // its whole patience — three minutes for `initialize` — for an answer that could not come, and a
    // release check spent exactly that long on it.
    static DEPARTING: ServerConfig = ServerConfig {
        name: "departing",
        command: "true",
        args: &[],
        language_id: "rust",
        extensions: &["rs"],
        markers: &[],
    };
    let dir = project();
    let started = Instant::now();
    let error = match LspSession::start(&DEPARTING, dir.path()) {
        Ok(_) => panic!("a server that has left cannot answer"),
        Err(error) => error,
    };
    let waited = started.elapsed();
    assert!(
        format!("{error:#}").contains("the server is gone"),
        "the handshake said {error:#} instead of naming the departed server"
    );
    assert!(
        waited < Duration::from_secs(10),
        "the handshake waited {waited:?} for a server that left at once"
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

#[test]
fn a_server_that_asks_a_question_before_answering_gets_an_answer() {
    // The fixture asks `workspace/configuration` the first time it is asked for
    // highlights, and waits for the reply. A client that builds the reply and forgets
    // to send it leaves the server waiting, and this test times out — which is what a
    // real `rust-analyzer` does under load, and how the bug was found.
    // A second configuration of the same fixture, started with the flag that makes it
    // ask a question first.
    static ASKING: ServerConfig = ServerConfig {
        name: "fake-lsp-asking",
        command: env!("CARGO_BIN_EXE_fake-lsp"),
        args: &["--ask"],
        language_id: "rust",
        extensions: &["rs"],
        markers: &[],
    };
    let dir = project();
    let mut session = LspSession::start(&ASKING, dir.path()).unwrap();
    // The fixture answers from the document it was given, so it has to be opened first.
    session
        .did_open(
            std::path::Path::new("lib.rs"),
            "// заголовок\nfn main() {}\n",
        )
        .expect("the document opened");
    let highlights = session
        .document_highlights(
            std::path::Path::new("lib.rs"),
            Position {
                line: 1,
                character: 3,
            },
        )
        .expect("the server answered");

    // Two `main` ranges and one marker that only a client which answered would get.
    assert_eq!(
        highlights.len(),
        2,
        "the server did not get its answer: {highlights:?}"
    );
    session.shutdown();
}

#[test]
fn implementations_come_back_as_a_list_of_places() {
    let dir = project();
    let mut session = LspSession::start(&FAKE, dir.path()).unwrap();
    let places = session
        .implementations(
            std::path::Path::new("lib.rs"),
            Position {
                line: 0,
                character: 0,
            },
        )
        .expect("implementations are answered");
    // The fixture puts them on the second line and the definition arm on the first, so a client
    // that asked for the wrong method is caught here rather than passing quietly.
    assert_eq!(places.len(), 1, "one place, got {places:?}");
    assert_eq!(
        places[0].range.start.line, 1,
        "the second line, not the first"
    );
    assert!(
        places[0].uri.as_str().ends_with("lib.rs"),
        "got {}",
        places[0].uri.as_str()
    );
}

#[test]
fn completion_comes_back_as_a_list_of_words() {
    let dir = project();
    let mut session = LspSession::start(&FAKE, dir.path()).unwrap();
    let items = session
        .completion(
            std::path::Path::new("lib.rs"),
            Position {
                line: 0,
                character: 0,
            },
        )
        .expect("completions are answered");
    // The fixture answers with two words inside a `CompletionList`; a client that asked for another
    // method gets that method's answer, which holds no items.
    assert_eq!(items.len(), 2, "two words, got {items:?}");
    assert_eq!(items[0].label, "alpha");
    assert_eq!(items[1].label, "beta");
    assert_eq!(
        items[1].detail.as_deref(),
        Some("u8"),
        "the detail the server sent did not survive"
    );
}

#[test]
fn incoming_calls_come_back_as_the_places_that_call_it() {
    let dir = project();
    let mut session = LspSession::start(&FAKE, dir.path()).unwrap();
    let places = session
        .incoming_calls(
            std::path::Path::new("lib.rs"),
            Position {
                line: 0,
                character: 0,
            },
        )
        .expect("the callers are answered");
    // Two call sites in another file, produced by the second question alone.
    assert_eq!(places.len(), 2, "two call sites, got {places:?}");
    assert!(
        places
            .iter()
            .all(|place| place.uri.as_str().ends_with("caller.rs")),
        "got {places:?}"
    );
    assert_eq!(places[0].range.start.line, 5);
    assert_eq!(places[1].range.start.line, 9);
}
