//! Drawing an answer: its Markdown, tables and all.
//!
//! The renderer draws a table as `Grid::new(id).striped(true)` and nothing else, and a grid
//! whose columns have no ceiling does not wrap the text inside a cell at all — egui decides
//! by `Grid::wrap_text`, which is `max_cell_size.x.is_finite()` — so three cells of one
//! sentence each came out 240 px wider than the panel, and egui put a horizontal bar under a
//! table with nothing wide in it. `Grid::max_col_width` is that ceiling, and it is what
//! `draw_table` sets: the table is egui's own grid, striped as before, with every cell's
//! Markdown rendered inside its column and wrapping there. Only the table is drawn here;
//! everything else in an answer goes to the renderer as it did before.

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

/// Draws a table through egui's own grid, with a ceiling on its columns and lines between
/// them.
fn draw_table(ui: &mut Ui, cache: &mut CommonMarkCache, table: &Table) {
    let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let gap = ui.spacing().item_spacing.x;
        let room = ui.available_width();
        let share = ((room - gap * columns.saturating_sub(1) as f32) / columns as f32).max(1.0);
        // The id of the piece the table is drawn in: two tables in one answer are two
        // pieces, and a grid under a shared id is what egui warns about over the table.
        let cells = egui::Grid::new(ui.id())
            .striped(true)
            .num_columns(columns)
            .max_col_width(share)
            .show(ui, |ui| {
                let mut rows: Vec<Vec<egui::Rect>> = Vec::new();
                for (index, row) in table.rows.iter().enumerate() {
                    let mut line: Vec<egui::Rect> = Vec::new();
                    for column in 0..columns {
                        let cell = row.get(column).map(String::as_str).unwrap_or_default();
                        let drawn = ui.push_id((index, column), |ui| {
                            if cell.trim().is_empty() {
                                ui.add_space(ui.text_style_height(&egui::TextStyle::Body));
                            } else if table.heading && index == 0 && plain(cell) {
                                // A heading takes the room its words need and no more: a cell
                                // that fills its column is what makes a column of two short
                                // words as wide as the widest cell in the table.
                                bold_label(ui, cell, false);
                            } else {
                                render(ui, cache, cell);
                            }
                        });
                        line.push(drawn.response.rect);
                    }
                    rows.push(line);
                    ui.end_row();
                }
                rows
            })
            .inner;
        lines(ui, &cells, columns, gap);
    });
}

/// The lines of the table, drawn over the grid: a grid draws none of its own.
///
/// Where they go is measured, not counted. Every cell says how much room it took, the widest
/// cell of a column is that column's edge, and a line sits halfway into the gap between two
/// columns — drawn at a guessed x it would cross the words of one column or of the other.
fn lines(ui: &Ui, cells: &[Vec<egui::Rect>], columns: usize, gap: f32) {
    let top = cells
        .iter()
        .flatten()
        .map(|rect| rect.top())
        .fold(f32::INFINITY, f32::min);
    let bottom = cells
        .iter()
        .flatten()
        .map(|rect| rect.bottom())
        .fold(f32::NEG_INFINITY, f32::max);
    let left = cells
        .iter()
        .flatten()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);
    if !top.is_finite() || bottom <= top || !left.is_finite() {
        return;
    }
    let edge = |column: usize| {
        cells
            .iter()
            .filter_map(|row| row.get(column))
            .map(|rect| rect.right())
            .fold(f32::NEG_INFINITY, f32::max)
    };
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    let painter = ui.painter();
    for column in 0..columns.saturating_sub(1) {
        painter.vline(edge(column) + gap * 0.5, top..=bottom, stroke);
    }
    for row in cells.iter().take(cells.len().saturating_sub(1)) {
        let bottom_of_row = row
            .iter()
            .map(|rect| rect.bottom())
            .fold(f32::NEG_INFINITY, f32::max);
        painter.hline(left..=edge(columns - 1), bottom_of_row + gap * 0.5, stroke);
    }
}

/// Draws one line of words in bold — the words twice, a hair apart, as this window has no
/// bold face to set them in.
///
/// `fill` says whether the line takes the room it is offered or only the room its words need.
/// A block of its own fills the panel; a cell of a table must not, or a column of two short
/// words comes out as wide as the widest column of the table.
pub(super) fn bold_label(ui: &mut Ui, text: &str, fill: bool) {
    /// How far the second pass is shifted: enough to thicken, not enough to smear.
    const THICKEN: f32 = 0.6;

    let font = egui::TextStyle::Body.resolve(ui.style());
    let colour = ui.visuals().strong_text_color();
    let galley = ui
        .painter()
        .layout(text.to_owned(), font, colour, ui.available_width());
    let wanted = galley.size();
    let size = if fill {
        egui::vec2(ui.available_width(), wanted.y)
    } else {
        wanted
    };
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), text));
    ui.painter().galley(rect.left_top(), galley.clone(), colour);
    ui.painter()
        .galley(rect.left_top() + egui::vec2(THICKEN, 0.0), galley, colour);
}

/// Whether a cell is plain words: a heading is set in bold, which the renderer cannot draw,
/// and a cell carrying Markdown of its own is drawn by the renderer as it was written.
fn plain(cell: &str) -> bool {
    !cell.contains(['*', '`', '_', '[', ']', '\\'])
}
