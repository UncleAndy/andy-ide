//! The real thing: `rust-analyzer` on a small crate.
//!
//! Nothing here is faked — a real server is started, the real handshake runs,
//! and a definition is asked for through the real transport. When the server is
//! not installed the test says so and passes: the crate must build and test on a
//! machine without it, and the acceptance of M2 is done on a machine with it.

use std::time::{Duration, Instant};

use ide_lsp::registry::{ServerConfig, is_installed};
use ide_lsp::{LspSession, position_to_byte};
use lsp_types::Position;

static RUST_ANALYZER: ServerConfig = ServerConfig {
    name: "rust-analyzer",
    command: "rust-analyzer",
    args: &[],
    language_id: "rust",
    extensions: &["rs"],
};

const SOURCE: &str = "fn helper() -> u8 {\n    7\n}\n\nfn main() {\n    let value = helper();\n    println!(\"{value}\");\n}\n";

fn crate_with_source() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), SOURCE).unwrap();
    dir
}

#[test]
fn rust_analyzer_answers_a_definition() {
    if !is_installed(&RUST_ANALYZER) {
        println!("rust-analyzer is not installed here: the end-to-end check was skipped");
        return;
    }

    let dir = crate_with_source();
    let mut session = LspSession::start(&RUST_ANALYZER, dir.path()).expect("rust-analyzer starts");

    let relative = std::path::Path::new("src/main.rs");
    session.did_open(relative, SOURCE).expect("didOpen");

    // The call of `helper()` on line 5, at the `helper` identifier.
    let call = SOURCE.find("helper()").expect("the call") + 2;
    let position = ide_lsp::byte_to_position(SOURCE, call);

    // rust-analyzer answers once it has loaded the crate; it may need a while.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last_error;
    loop {
        match session.definition(relative, position) {
            Ok(locations) if !locations.is_empty() => {
                let location = &locations[0];
                let path = ide_lsp::uri_to_path(&location.uri).expect("a file uri");
                assert!(
                    path.ends_with("src/main.rs"),
                    "the definition is in {}",
                    path.display()
                );
                assert_eq!(
                    location.range.start.line, 0,
                    "`helper` is declared on the first line"
                );
                let text = std::fs::read_to_string(&path).unwrap();
                let byte = position_to_byte(
                    &text,
                    Position {
                        line: location.range.start.line,
                        character: location.range.start.character,
                    },
                );
                assert!(
                    text[byte..].starts_with("helper"),
                    "the range points at {:?}",
                    &text[byte..byte.min(text.len() - byte) + 6]
                );
                session.shutdown();
                return;
            }
            Ok(_) => last_error = "the answer was empty".to_owned(),
            Err(error) => last_error = format!("{error:#}"),
        }
        if Instant::now() > deadline {
            panic!("rust-analyzer never answered a definition: {last_error}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
