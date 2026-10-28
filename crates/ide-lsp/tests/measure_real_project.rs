//! What the editor costs on its own sources.
//!
//! This is a measurement, not a check: it runs only when `ANDY_IDE_MEASURE=1` is set,
//! because a cold rust-analyzer on the whole workspace takes tens of seconds, and the
//! rest of the suite must stay quick.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ide_lsp::{LspSession, server_for};
use lsp_types::Position;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives in the workspace")
        .to_path_buf()
}

#[test]
fn the_server_answers_on_the_workspace_itself() {
    if std::env::var("ANDY_IDE_MEASURE").is_err() {
        println!("skipped: set ANDY_IDE_MEASURE=1 to measure");
        return;
    }
    let root = workspace_root();
    let file = root.join("crates/andy-ide/src/state.rs");
    let text = std::fs::read_to_string(&file).expect("the file is there");
    let server = server_for(Path::new("x.rs")).expect("rust-analyzer is known");
    let started = Instant::now();
    let mut session = LspSession::start(server, &root).expect("the server starts");
    session.did_open(&file, &text).expect("didOpen");
    println!("initialize + didOpen: {:?}", started.elapsed());

    // The place of `pub fn display_name`: inside its name, on its own line.
    let byte = text
        .find("pub fn display_name")
        .expect("the symbol is there")
        + "pub fn ".len();
    let line = text[..byte].matches('\n').count() as u32;
    let line_start = text[..byte].rfind('\n').map_or(0, |index| index + 1);
    let character = text[line_start..byte]
        .chars()
        .map(char::len_utf16)
        .sum::<usize>() as u32;
    let position = Position::new(line, character);

    let first = Instant::now();
    let mut definitions = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        definitions = session.definition(&file, position).unwrap_or_default();
        if !definitions.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("first definition: {:?}", first.elapsed());
    assert!(!definitions.is_empty(), "no definition came back");

    let outline = Instant::now();
    let symbols = session.document_symbols(&file).expect("an outline");
    println!(
        "outline of state.rs: {:?} ({} symbols)",
        outline.elapsed(),
        symbols.len()
    );

    // The server answers `content modified` while it is still settling after the
    // first questions; the editor retries it too.
    let uses = Instant::now();
    let mut references = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        match session.references(&file, position) {
            Ok(found) if !found.is_empty() => {
                references = found;
                break;
            }
            Ok(_) | Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    println!(
        "references: {:?} ({} places)",
        uses.elapsed(),
        references.len()
    );
    assert!(!references.is_empty(), "no references came back");
    session.shutdown();
}
