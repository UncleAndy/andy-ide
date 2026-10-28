//! The input budget from the specification: typing in a large file must stay
//! interactive.
//!
//! The number is measured in a release build (`cargo test --release --test
//! performance`), because a debug build says nothing about what a user feels.
//! The assertion is deliberately generous — it guards against a change that
//! makes typing cost tens of milliseconds, not against a millisecond of drift;
//! the real figure is printed and recorded in `docs/M1_acceptance.md`.

use std::time::{Duration, Instant};

use ide_syntax::{Highlighter, Language};

/// Lines of the document under the cursor.
const LINES: usize = 10_000;
/// Lines the editor lays out: what fits a tall window, plus the margin the view
/// takes above and below.
const VISIBLE: usize = 60;
/// Edits to average over.
const EDITS: usize = 1_000;

fn document() -> String {
    let mut text = String::with_capacity(LINES * 40);
    for line in 0..LINES {
        match line % 5 {
            0 => text.push_str(&format!("fn function_{line}(value: usize) -> usize {{\n")),
            1 => text.push_str(&format!("    let total = value + {line};\n")),
            2 => text.push_str("    // считаем сумму\n"),
            3 => text.push_str("    total\n"),
            _ => text.push_str("}\n"),
        }
    }
    text
}

#[test]
fn typing_in_a_large_file_stays_interactive() {
    let mut text = document();
    let mut highlighter = Highlighter::new(Language::Rust);
    highlighter.set_text(&text);

    // The cursor sits in the middle, so the window is not at the start of the
    // document (where a naive implementation could get away with little work).
    let middle = text.len() / 2;
    let insert_at = text[..middle].rfind('\n').expect("a line break") + 1;

    let mut total = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for at in insert_at..insert_at + EDITS {
        // One character typed: the highlighter takes the same edit the buffer
        // would hand it, then the view asks for the lines it can show.
        text.insert(at, 'x');
        let started = Instant::now();
        highlighter.edit(at, at, at + 1, "x");
        let line = text[..at].matches('\n').count();
        let spans = highlighter.spans_for_lines(line.saturating_sub(1), line + VISIBLE);
        let took = started.elapsed();
        total += took;
        worst = worst.max(took);
        assert!(!spans.is_empty(), "the window must be highlighted");
    }

    let average = total / EDITS as u32;
    println!(
        "typing in {LINES} lines: average {average:?} per keystroke, worst {worst:?}, {} spans in the window",
        highlighter.spans_for_lines(0, VISIBLE).len()
    );
    assert!(
        average < Duration::from_millis(50),
        "a keystroke costs {average:?}, which is far beyond the 16 ms budget"
    );
}

#[test]
fn opening_a_large_file_is_not_the_bottleneck() {
    let text = document();
    let started = Instant::now();
    let mut highlighter = Highlighter::new(Language::Rust);
    highlighter.set_text(&text);
    let took = started.elapsed();
    println!("first parse of {LINES} lines: {took:?}");
    assert!(
        took < Duration::from_secs(1),
        "parsing {LINES} lines took {took:?}"
    );
}
