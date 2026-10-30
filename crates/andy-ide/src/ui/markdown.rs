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

/// The room a cell keeps above and below its words.
///
/// A line between two rows is drawn halfway across the gap the table leaves between them
/// (`item_spacing.y`, three points), so without this room the line sits a point and a half
/// under the words of one row and over the words of the next — close enough to read as a
/// stroke through the text rather than a boundary under it.
const CELL_PAD: f32 = 2.0;

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

/// Draws a table: the columns a share of their own, a band on every other row, and lines
/// between the columns and the rows.
///
/// The columns are laid out here rather than by `egui::Grid`, and the reason is measured: a
/// grid measures a cell in the room that cell had *last* frame, and in the first frame that
/// room is `min_cell_size` — forty odd points wide. A cell with a sentence in it is therefore
/// measured forty points wide, wrapped into a pillar one word per line (a row 255 points tall,
/// measured), and the column keeps the narrow width it was born with.
///
/// So the room a column wants is remembered from the frame before: what the widest cell of
/// that column actually took when it was given its share. Until a table has been drawn once,
/// the columns share the room equally; from then on they are as wide as their words, and only
/// shrunk — in proportion — when together they want more than they have.
fn draw_table(ui: &mut Ui, cache: &mut CommonMarkCache, table: &Table) {
    let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let mut drawn_slots: Vec<egui::layers::ShapeIdx> = Vec::new();
    let mut drawn_cells: Vec<Vec<egui::Rect>> = Vec::new();
    let frame = egui::Frame::group(ui.style()).show(ui, |ui| {
        let gap = ui.spacing().item_spacing.x;
        let widths = column_widths(ui, columns, gap);

        // A place for each band is taken before the words are drawn, so the bands stay behind
        // the text — a band of every other row, the heading counted.
        let slots: Vec<egui::layers::ShapeIdx> = (0..table.rows.len() / 2)
            .map(|_| ui.painter().add(egui::Shape::Noop))
            .collect();

        let mut rows: Vec<Vec<egui::Rect>> = Vec::new();
        for (index, row) in table.rows.iter().enumerate() {
            let mut line: Vec<egui::Rect> = Vec::new();
            ui.horizontal_top(|ui| {
                for (column, width) in widths.iter().enumerate().take(columns) {
                    let cell = row.get(column).map(String::as_str).unwrap_or_default();
                    let width = *width;
                    let drawn = ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.push_id((index, column), |ui| {
                                // The words stand clear of the line above and the line below
                                // them: the cell is taller than its words by this much at each
                                // end, and the rows keep their distance from each other.
                                ui.add_space(CELL_PAD);
                                if cell.trim().is_empty() {
                                    ui.add_space(ui.text_style_height(&egui::TextStyle::Body));
                                } else if table.heading && index == 0 && plain(cell) {
                                    // A heading takes the room its words need and no more.
                                    bold_label(ui, cell, false);
                                } else {
                                    render(ui, cache, cell);
                                }
                                ui.add_space(CELL_PAD);
                            });
                        },
                    );
                    // What the cell did not use is left as room, so the columns of one row
                    // line up with the columns of the next.
                    if column + 1 < columns {
                        let used = drawn.response.rect.width().min(width);
                        let left = ui.available_width();
                        ui.add_space((width - used).min(left));
                    }
                    // Whatever a cell holds leaves a gap of its own after it — the space
                    // between two items of a ui (`item_spacing.y`, three points), left behind
                    // by the renderer, by a heading drawn as a label, by the room of an empty
                    // cell. Left in, the cell is taller than its words by that much at the
                    // bottom only, the room under the words of a row comes out half again the
                    // room above them, and the line between two rows sits low in its gap. The
                    // cell ends with its words and its own room at each end.
                    let rect = drawn.response.rect;
                    let tail = ui.spacing().item_spacing.y;
                    line.push(egui::Rect::from_min_max(
                        rect.min,
                        egui::pos2(rect.max.x, rect.max.y - tail),
                    ));
                }
            });
            rows.push(line);
        }
        remember_widths(ui, columns, &rows, &widths);
        drawn_slots = slots;
        drawn_cells = rows;
    });
    // The bands and the lines are drawn to the table's own frame, which is only known once
    // it has been drawn: a line that stops short of it leaves a gap on either side of every
    // row, and so does a band.
    bands_and_lines(ui, frame.response.rect, &drawn_slots, &drawn_cells);
}

/// How wide each column is to be drawn: what it wanted last frame, or an equal share of the
/// room when the table is drawn for the first time.
fn column_widths(ui: &Ui, columns: usize, gap: f32) -> Vec<f32> {
    let usable = (ui.available_width() - gap * columns.saturating_sub(1) as f32).max(1.0);
    let wanted: Vec<f32> = ui
        .ctx()
        .data(|data| data.get_temp(ui.id().with("column-widths")))
        .unwrap_or_default();
    if wanted.len() != columns {
        return vec![usable / columns as f32; columns];
    }
    let total: f32 = wanted.iter().sum();
    if total <= usable || total <= 0.0 {
        return wanted;
    }
    // Together they want more than there is: each keeps its share of what there is.
    wanted
        .iter()
        .map(|width| (width / total * usable).max(24.0))
        .collect()
}

/// Remembers how wide the columns turned out, for the next frame to lay out.
///
/// A frame is asked for when what the columns took differs from what they were given: the
/// table is then drawn again with the measured widths, and until it does the first frame's
/// equal shares are what is on screen.
fn remember_widths(ui: &Ui, columns: usize, rows: &[Vec<egui::Rect>], given: &[f32]) {
    let wanted: Vec<f32> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|rect| rect.width())
                .fold(0.0, f32::max)
        })
        .collect();
    let settling = wanted
        .iter()
        .zip(given)
        .any(|(wanted, given)| (wanted - given).abs() > 0.5);
    ui.ctx()
        .data_mut(|data| data.insert_temp(ui.id().with("column-widths"), wanted));
    if settling {
        ui.ctx().request_repaint();
    }
}

/// The bands and the lines of a table, placed from what the cells actually took.
///
/// A column's edge is its widest cell and a row's is its tallest, and both a line and the edge
/// of a band sit halfway into the room the grid left between two neighbours — the room it left,
/// not the room `item_spacing` suggests. Counted from the spacing, the line under a row lands
/// inside the next row as soon as a cell wraps, and it is drawn there.
fn bands_and_lines(
    ui: &Ui,
    frame: egui::Rect,
    slots: &[egui::layers::ShapeIdx],
    cells: &[Vec<egui::Rect>],
) {
    if cells.is_empty() || cells.iter().all(Vec::is_empty) {
        return;
    }
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let row_top = |row: &[egui::Rect]| {
        row.iter()
            .map(|rect| rect.top())
            .fold(f32::INFINITY, f32::min)
    };
    let row_bottom = |row: &[egui::Rect]| {
        row.iter()
            .map(|rect| rect.bottom())
            .fold(f32::NEG_INFINITY, f32::max)
    };
    let column_left = |column: usize| {
        cells
            .iter()
            .filter_map(|row| row.get(column))
            .map(|rect| rect.left())
            .fold(f32::INFINITY, f32::min)
    };
    let column_right = |column: usize| {
        cells
            .iter()
            .filter_map(|row| row.get(column))
            .map(|rect| rect.right())
            .fold(f32::NEG_INFINITY, f32::max)
    };

    let top_of_rows = row_top(&cells[0]);
    let bottom_of_rows = row_bottom(&cells[cells.len() - 1]);
    if !top_of_rows.is_finite() || bottom_of_rows <= top_of_rows {
        return;
    }
    // A hair inside the frame, so that a line and the frame do not draw over each other.
    let left = frame.left() + 0.5;
    let right = frame.right() - 0.5;
    let top = frame.top() + 0.5;
    let bottom = frame.bottom() - 0.5;
    // Where one row ends and the next begins, and where the table begins and ends.
    let mut edge: Vec<f32> = Vec::with_capacity(cells.len() + 1);
    edge.push(top_of_rows);
    for index in 0..cells.len().saturating_sub(1) {
        edge.push((row_bottom(&cells[index]) + row_top(&cells[index + 1])) * 0.5);
    }
    edge.push(bottom_of_rows);

    // A band on every other row, from one edge to the next: the same edges the lines are
    // drawn on, so a band never reaches past the line into the words of the row below it —
    // except at the foot of the table, where there is no line but the frame itself. A frame
    // keeps a margin of its own around the rows, so a band of the last row that stopped at
    // the last cell left a strip of bare background between itself and the frame.
    for (slot, index) in slots.iter().zip((1..cells.len()).step_by(2)) {
        let to = if index + 1 == cells.len() {
            bottom
        } else {
            edge[index + 1]
        };
        ui.painter().set(
            *slot,
            egui::Shape::rect_filled(
                egui::Rect::from_min_max(egui::pos2(left, edge[index]), egui::pos2(right, to)),
                0.0,
                ui.visuals().faint_bg_color,
            ),
        );
    }

    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    let painter = ui.painter();
    for column in 0..columns.saturating_sub(1) {
        painter.vline(
            (column_right(column) + column_left(column + 1)) * 0.5,
            top..=bottom,
            stroke,
        );
    }
    for y in edge.iter().take(edge.len().saturating_sub(1)).skip(1) {
        painter.hline(left..=right, *y, stroke);
    }
}

/// Draws one line of words in bold — the words twice, a hair apart, as this window has no
/// bold face to set them in.
///
/// `fill` says whether the line takes the room it is offered or only the room its words need.
/// A block of its own fills the panel; a cell of a table must not, or a column of two short
/// words comes out as wide as the widest column of the table.
pub(super) fn bold_label(ui: &mut Ui, text: &str, fill: bool) {
    let colour = ui.visuals().strong_text_color();
    bold_label_in(ui, text, fill, colour);
}

/// The same line of bold words, in a colour of the caller's choosing: the words of a person are
/// not the window's own text, and the colour is how the block says so.
pub(super) fn bold_label_in(ui: &mut Ui, text: &str, fill: bool, colour: egui::Color32) {
    /// How far the second pass is shifted: enough to thicken, not enough to smear.
    const THICKEN: f32 = 0.6;

    let font = egui::TextStyle::Body.resolve(ui.style());
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
