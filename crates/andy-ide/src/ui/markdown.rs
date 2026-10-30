//! Drawing an answer: its Markdown, tables and all.
//!
//! The renderer draws a table by putting every cell in a horizontal container, and a
//! horizontal container never wraps: three cells of one sentence each came out 240 px wider
//! than the panel, and egui put a horizontal bar under a table with nothing wide in it. A
//! table is drawn here instead — every cell takes a share of the panel's width and the cell's
//! own Markdown is drawn inside that share, so a line wraps and the table is as wide as the
//! panel and no wider.

use egui::Ui;
use egui_commonmark::CommonMarkCache;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Draws the Markdown of one answer, under an id of its own.
pub fn answer(ui: &mut Ui, cache: &mut CommonMarkCache, id: egui::Id, text: &str) {
    for (index, part) in split(text).into_iter().enumerate() {
        ui.push_id((id, index), |ui| match part {
            Part::Markdown(markdown) => render(ui, cache, &markdown),
            Part::Table(table) => draw_table(ui, cache, &table),
        });
    }
}

/// One piece of an answer: Markdown for the renderer, or a table drawn here.
enum Part {
    Markdown(String),
    Table(Table),
}

/// A table as it was written, each cell in its own Markdown.
struct Table {
    rows: Vec<Vec<String>>,
    /// Whether the first row came from the table's heading.
    heading: bool,
}

/// Cuts an answer into the pieces it is made of, the tables apart from the rest.
fn split(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut plain_from = 0;
    let mut table: Option<Table> = None;
    let mut row: Vec<String> = Vec::new();
    let mut cell: Option<(usize, usize)> = None;

    for (event, span) in Parser::new_ext(text, options()).into_offset_iter() {
        // A cell is its own source, from where it starts to the end of the last thing written
        // in it, bars left out: the cell's Markdown is drawn as it was written.
        if let Some((start, end)) = cell
            && !matches!(
                event,
                Event::Start(Tag::TableCell) | Event::End(TagEnd::TableCell)
            )
        {
            cell = Some((start, end.max(span.end)));
        }
        match event {
            Event::Start(Tag::Table(_)) => {
                keep_markdown(&mut parts, text, plain_from, span.start);
                row.clear();
                cell = None;
                table = Some(Table {
                    rows: Vec::new(),
                    heading: false,
                });
            }
            Event::Start(Tag::TableHead) => row.clear(),
            Event::End(TagEnd::TableHead) => {
                if let Some(made) = &mut table {
                    made.heading = true;
                    made.rows.push(std::mem::take(&mut row));
                }
            }
            Event::Start(Tag::TableRow) => row.clear(),
            Event::End(TagEnd::TableRow) => {
                if let Some(made) = &mut table {
                    made.rows.push(std::mem::take(&mut row));
                }
            }
            Event::Start(Tag::TableCell) => cell = Some((span.start, span.start)),
            Event::End(TagEnd::TableCell) => {
                if let Some((start, end)) = cell.take() {
                    let written = text.get(start..end).unwrap_or_default();
                    row.push(written.trim().to_owned());
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(made) = table.take() {
                    parts.push(Part::Table(made));
                }
                plain_from = span.end;
            }
            _ => {}
        }
    }
    keep_markdown(&mut parts, text, plain_from, text.len());
    parts
}

/// Keeps the Markdown between two tables, if there is any of it.
fn keep_markdown(parts: &mut Vec<Part>, text: &str, from: usize, to: usize) {
    if let Some(markdown) = text.get(from..to)
        && !markdown.trim().is_empty()
    {
        parts.push(Part::Markdown(markdown.to_owned()));
    }
}

/// The same options the renderer reads the rest of an answer with.
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}

fn render(ui: &mut Ui, cache: &mut CommonMarkCache, markdown: &str) {
    egui_commonmark::CommonMarkViewer::new().show(ui, cache, markdown);
}

/// Draws a table: a share of the panel's width to each cell, wrapping inside it.
fn draw_table(ui: &mut Ui, cache: &mut CommonMarkCache, table: &Table) {
    let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        // The room inside the frame, which is what the cells have to share: taken after the
        // frame's own margin, the table comes out exactly as wide as the panel.
        let gap = ui.spacing().item_spacing.x;
        let room = ui.available_width();
        let share = ((room - gap * columns.saturating_sub(1) as f32) / columns as f32).max(1.0);
        for (index, row) in table.rows.iter().enumerate() {
            ui.horizontal_top(|ui| {
                for column in 0..columns {
                    ui.allocate_ui_with_layout(
                        egui::vec2(share, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            // An id per cell: several cells share the ui's id, and a Grid
                            // under a shared id is what egui warns about over the block.
                            ui.push_id((index, column), |ui| {
                                let cell = row.get(column).map(String::as_str).unwrap_or_default();
                                if cell.trim().is_empty() {
                                    // An empty cell keeps its place and draws nothing.
                                    ui.add_space(ui.text_style_height(&egui::TextStyle::Body));
                                } else if table.heading && index == 0 && plain(cell) {
                                    super::agent::bold_label(ui, cell);
                                } else {
                                    render(ui, cache, cell);
                                }
                            });
                        },
                    );
                }
            });
            if index + 1 < table.rows.len() {
                ui.separator();
            }
        }
    });
}

/// Whether a cell is plain words: a heading is set in bold, which the renderer cannot draw,
/// and a cell carrying Markdown of its own is drawn by the renderer as it was written.
fn plain(cell: &str) -> bool {
    !cell.contains(['*', '`', '_', '[', ']', '\\'])
}
