//! Drawing and input for the text editor.
//!
//! Only the visible lines are laid out: the scroll area hands back the viewport,
//! the widget allocates the full content size for the scrollbars, and then
//! paints the slice of lines that intersects the viewport.

use std::sync::Arc;

use egui::text::{ByteIndex, CCursor, LayoutJob, LayoutSection};
use egui::{
    Color32, CornerRadius, Event, FontId, Galley, Key, Pos2, Rect, Sense, TextFormat, TextStyle,
    Ui, Vec2,
};
use ide_syntax::{HighlightSpan, TokenKind};

use super::EditorBuffer;
use super::INDENT_WIDTH as TAB_WIDTH;
use super::edit::Motion;

/// Caret width in points.
const CARET_WIDTH: f32 = 1.5;

const SELECTION_BG: Color32 = Color32::from_rgb(38, 79, 120);
const CARET: Color32 = Color32::from_rgb(220, 220, 220);
const GUTTER_BG: Color32 = Color32::from_rgb(30, 30, 30);
const GUTTER_FG: Color32 = Color32::from_rgb(110, 110, 110);
const GUTTER_FG_ACTIVE: Color32 = Color32::from_rgb(200, 200, 200);
const DEFAULT_FG: Color32 = Color32::from_rgb(212, 212, 212);

/// Draws the buffer and handles its input. Returns true when the text changed.
pub fn ui(ui: &mut Ui, buffer: &mut EditorBuffer) -> bool {
    let font = TextStyle::Monospace.resolve(ui.style());
    let (char_w, row_h) = ui
        .ctx()
        .fonts_mut(|fonts| (fonts.glyph_width(&font, ' '), fonts.row_height(&font)));
    let char_w = char_w.max(1.0);
    let row_h = row_h.max(1.0);

    let mut changed = false;

    let line_count = buffer.line_count().max(1);
    let content = Vec2::new(
        ((buffer.max_line_chars() + 8) as f32) * char_w,
        (line_count as f32) * row_h,
    );

    egui::ScrollArea::both()
        .id_salt(("editor_scroll", buffer.path.clone()))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(
                    content.x.max(ui.available_width()),
                    content.y.max(ui.available_height()),
                ),
                Sense::click_and_drag(),
            );
            let origin = rect.min;
            let digits = ((line_count as f32).log10().floor() as usize + 1).max(2);
            let gutter_width = digits as f32 * char_w + 8.0;
            let text_origin = Pos2::new(origin.x + gutter_width, origin.y);

            if response.clicked() || response.dragged() {
                response.request_focus();
            }

            // egui turns arrow keys and Tab into focus navigation unless the
            // focused widget claims them, so the editor claims every key it
            // handles. Without this, Up/Down jump between panels.
            if response.has_focus() {
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        response.id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    );
                });
            }
            // Typing should work as soon as the editor is on screen.
            if !response.has_focus() && ui.memory(|memory| memory.focused().is_none()) {
                response.request_focus();
            }

            let painter = ui.painter_at(rect);

            let first_line = ((viewport.min.y / row_h).floor().max(0.0) as usize).saturating_sub(1);
            let last_line =
                (((viewport.max.y / row_h).ceil().max(0.0) as usize) + 2).min(line_count);

            let spans = buffer
                .highlighter
                .as_ref()
                .map(|highlighter| highlighter.spans_for_lines(first_line, last_line))
                .unwrap_or_default();

            paint_gutter(
                ui,
                &painter,
                buffer,
                rect,
                origin,
                gutter_width,
                row_h,
                first_line,
                last_line,
            );
            let layouts = LineLayouts::build(ui, buffer, &spans, &font, first_line, last_line);
            paint_selection(
                PaintCtx {
                    painter: &painter,
                    origin: text_origin,
                    char_w,
                    row_h,
                },
                buffer,
                &layouts,
                first_line,
                last_line,
            );
            paint_galleys(&painter, &layouts, text_origin, row_h);
            let (caret_line, _) = buffer.line_col();
            let caret_galley = layouts
                .galley(caret_line)
                .cloned()
                .or_else(|| line_galley(ui, buffer, &spans, &font, caret_line));
            let caret_pos = paint_caret(
                &painter,
                buffer,
                caret_galley.as_ref(),
                text_origin,
                char_w,
                row_h,
            );

            // Keep the caret on screen while it moves, and only then: scrolling
            // must never fight the user's wheel. The viewport is reported in
            // content coordinates, the caret is painted in screen coordinates,
            // so the caret is shifted into content space before comparing.
            let caret_content = Pos2::new(caret_pos.x - rect.min.x, caret_pos.y - rect.min.y);
            let tracked = response.id.with("last_caret");
            let previous: Option<usize> = ui.data(|data| data.get_temp(tracked));
            let caret = buffer.caret();
            let row = Rect::from_min_size(caret_content, Vec2::new(1.0, row_h));
            let (remember, scroll) = caret_follow(previous, caret, viewport, row);
            if remember {
                ui.data_mut(|data| data.insert_temp(tracked, caret));
            }

            // Pointer first: clicking must place the caret even though the very
            // same click gives the widget focus.
            let mut drag_scrolled = false;
            if (response.clicked() || response.dragged())
                && let Some(pos) = response.interact_pointer_pos()
            {
                let mut y = pos.y - rect.min.y;
                // Dragging past the edge scrolls one row at a time, so the
                // selection keeps growing while the button is held even though
                // the pointer has nowhere left to go. The scroll area takes its
                // delta negated: a negative y scrolls down.
                if response.dragged() {
                    let step = auto_scroll_step(y, viewport, row_h);
                    if step != 0.0 {
                        ui.scroll_with_delta_animation(
                            Vec2::new(0.0, step),
                            egui::style::ScrollAnimation::none(),
                        );
                        ui.ctx().request_repaint();
                        drag_scrolled = true;
                        // Aim one row past the edge that is being revealed.
                        y = if step < 0.0 {
                            viewport.max.y + row_h
                        } else {
                            viewport.min.y - row_h
                        };
                    }
                }
                let target = Pos2::new(pos.x, rect.min.y + y);
                let (line, col) =
                    position_to_line_col(buffer, &layouts, text_origin, target, row_h);
                let index = buffer.char_index(line, col);
                if response.dragged() {
                    buffer.set_drag_caret(index);
                } else {
                    buffer.set_caret(index);
                }
            }

            // While a drag scrolls by itself, following the caret as well would
            // move the view twice per frame and make the text jump.
            if scroll && !drag_scrolled {
                ui.scroll_to_rect(row.expand(3.0), None);
            }

            if response.has_focus() {
                changed |= handle_input(ui, buffer);
            }
        });

    changed
}

/// Decides whether the view follows the caret this frame.
///
/// Returns `(remember, scroll)`. The caret is followed only when it actually
/// moved: a wheel scroll leaves the caret where it is, and scrolling the view
/// back to it would make the wheel feel broken. Both rectangles are in content
/// coordinates — the scroll area reports the viewport that way, and the caller
/// shifts the caret there before asking.
fn caret_follow(previous: Option<usize>, caret: usize, viewport: Rect, row: Rect) -> (bool, bool) {
    if previous == Some(caret) {
        return (false, false);
    }
    (true, !viewport.contains_rect(row))
}

/// Line numbers along the left edge, in a muted colour.
#[allow(clippy::too_many_arguments)]
fn paint_gutter(
    _ui: &Ui,
    painter: &egui::Painter,
    buffer: &EditorBuffer,
    rect: Rect,
    origin: Pos2,
    gutter_width: f32,
    row_h: f32,
    first_line: usize,
    last_line: usize,
) {
    let gutter = Rect::from_min_max(origin, Pos2::new(origin.x + gutter_width, rect.max.y));
    painter.rect_filled(gutter, CornerRadius::ZERO, GUTTER_BG);

    let (caret_line, _) = buffer.line_col();
    let font = FontId::monospace(row_h * 0.72);
    for line in first_line..last_line {
        let y = origin.y + (line as f32) * row_h;
        let color = if line == caret_line {
            GUTTER_FG_ACTIVE
        } else {
            GUTTER_FG
        };
        painter.text(
            Pos2::new(origin.x + gutter_width - 4.0, y),
            egui::Align2::RIGHT_TOP,
            (line + 1).to_string(),
            font.clone(),
            color,
        );
    }
}

/// Galleys of the visible lines, laid out once per frame and then used for the
/// text, the selection, the caret and hit testing alike.
///
/// The geometry comes from the galley rather than from `column * char_width`:
/// a monospace font falls back to other fonts for some scripts, and those
/// glyphs are not the same width, which used to leave the caret a few points
/// off the end of the line.
struct LineLayouts {
    first_line: usize,
    galleys: Vec<Option<Arc<Galley>>>,
}

impl LineLayouts {
    fn build(
        ui: &Ui,
        buffer: &EditorBuffer,
        spans: &[HighlightSpan],
        font: &FontId,
        first_line: usize,
        last_line: usize,
    ) -> Self {
        let galleys = (first_line..last_line)
            .map(|line| line_galley(ui, buffer, spans, font, line))
            .collect();
        Self {
            first_line,
            galleys,
        }
    }

    fn galley(&self, line: usize) -> Option<&Arc<Galley>> {
        let index = line.checked_sub(self.first_line)?;
        self.galleys.get(index).and_then(|galley| galley.as_ref())
    }
}

/// Everything the selection and the caret need to draw on the same grid.
struct PaintCtx<'a> {
    painter: &'a egui::Painter,
    origin: Pos2,
    char_w: f32,
    row_h: f32,
}

fn paint_selection(
    ctx: PaintCtx<'_>,
    buffer: &EditorBuffer,
    layouts: &LineLayouts,
    first_line: usize,
    last_line: usize,
) {
    let PaintCtx {
        painter,
        origin,
        char_w,
        row_h,
    } = ctx;
    if !buffer.has_selection() {
        return;
    }
    let range = buffer.selection();
    let (start_line, start_col) = buffer.doc.char_to_line_col(range.start);
    let (end_line, end_col) = buffer.doc.char_to_line_col(range.end);

    for line in first_line..last_line {
        if line < start_line || line > end_line {
            continue;
        }
        let text = buffer.line_text(line);
        let line_chars = text.chars().count();
        let from = if line == start_line {
            start_col.min(line_chars)
        } else {
            0
        };
        let to = if line == end_line {
            end_col.min(line_chars)
        } else {
            line_chars
        };

        let x0 = x_for_col(layouts, text, line, from, char_w);
        let mut x1 = x_for_col(layouts, text, line, to, char_w);
        if line != end_line && to == line_chars {
            // Show the line break as a small tail, otherwise the selection
            // seems to stop in mid-air.
            x1 += char_w * 0.5;
        }
        if x1 < x0 {
            continue;
        }
        let y = origin.y + (line as f32) * row_h;
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(origin.x + x0, y),
                Pos2::new(origin.x + x1.max(x0 + 2.0), y + row_h),
            ),
            CornerRadius::ZERO,
            SELECTION_BG,
        );
    }
}

/// Lays out one line with its highlight colours; `None` for an empty line.
fn line_galley(
    ui: &Ui,
    buffer: &EditorBuffer,
    spans: &[HighlightSpan],
    font: &FontId,
    line: usize,
) -> Option<Arc<Galley>> {
    if line >= buffer.line_count() {
        return None;
    }
    let text = buffer.line_text(line);
    let line_start = buffer.line_starts()[line];
    let line_end = line_start + text.len();

    let mut job = LayoutJob::default();
    let mut cursor = line_start;
    for span in spans {
        let start = span.start_byte.max(line_start).min(line_end);
        let end = span.end_byte.min(line_end);
        if end <= start {
            continue;
        }
        if start > cursor {
            push_section(&mut job, &buffer.text()[cursor..start], DEFAULT_FG, font);
        }
        push_section(
            &mut job,
            &buffer.text()[start..end],
            token_color(span.kind),
            font,
        );
        cursor = end;
    }
    if cursor < line_end {
        push_section(&mut job, &buffer.text()[cursor..line_end], DEFAULT_FG, font);
    }
    if job.text.is_empty() {
        return None;
    }
    Some(ui.ctx().fonts_mut(|fonts| fonts.layout_job(job)))
}

fn paint_galleys(painter: &egui::Painter, layouts: &LineLayouts, origin: Pos2, row_h: f32) {
    for (index, galley) in layouts.galleys.iter().enumerate() {
        let Some(galley) = galley else { continue };
        let line = layouts.first_line + index;
        let y = origin.y + (line as f32) * row_h;
        painter.galley(Pos2::new(origin.x, y), galley.clone(), DEFAULT_FG);
    }
}

fn paint_caret(
    painter: &egui::Painter,
    buffer: &EditorBuffer,
    galley: Option<&Arc<Galley>>,
    origin: Pos2,
    char_w: f32,
    row_h: f32,
) -> Pos2 {
    let (line, col) = buffer.line_col();
    let text = buffer.line_text(line);
    let x = match galley {
        Some(galley) => {
            galley
                .pos_from_cursor(CCursor::new(expanded_column(text, col)))
                .min
                .x
        }
        None => char_w * (col.min(text.chars().count()) as f32),
    };
    let y = origin.y + (line as f32) * row_h;
    let position = Pos2::new(origin.x + x, y);
    painter.rect_filled(
        Rect::from_min_size(position, Vec2::new(CARET_WIDTH, row_h)),
        CornerRadius::ZERO,
        CARET,
    );
    position
}

/// How far to scroll while a selection drag leaves the viewport.
///
/// The pointer is in content coordinates, like the viewport. A negative step
/// scrolls down (the scroll area takes its delta negated), so a pointer below
/// the viewport returns `-row_h`.
fn auto_scroll_step(pointer_y: f32, viewport: Rect, row_h: f32) -> f32 {
    if pointer_y > viewport.max.y {
        -row_h
    } else if pointer_y < viewport.min.y {
        row_h
    } else {
        0.0
    }
}

/// Absolute caret position from a pointer position.
fn position_to_line_col(
    buffer: &EditorBuffer,
    layouts: &LineLayouts,
    origin: Pos2,
    pos: Pos2,
    row_h: f32,
) -> (usize, usize) {
    let line = (((pos.y - origin.y) / row_h).floor().max(0.0) as usize)
        .min(buffer.line_count().saturating_sub(1));
    let text = buffer.line_text(line);
    let x = (pos.x - origin.x).max(0.0);
    match layouts.galley(line) {
        Some(galley) => {
            let shown = galley.cursor_from_pos(Vec2::new(x, 0.0)).index.0;
            (line, original_column(text, shown))
        }
        // An empty line has no galley, and there is nothing to hit.
        None => (line, 0),
    }
}

/// Where the caret of `col` is drawn, taken from the galley so that glyphs of a
/// fallback font, which are not exactly one cell wide, still land correctly.
fn x_for_col(layouts: &LineLayouts, text: &str, line: usize, col: usize, char_w: f32) -> f32 {
    match layouts.galley(line) {
        Some(galley) => {
            galley
                .pos_from_cursor(CCursor::new(expanded_column(text, col)))
                .min
                .x
        }
        None => char_w * (col.min(text.chars().count()) as f32),
    }
}

/// Column in the laid-out text, where a tab occupies `TAB_WIDTH` cells.
fn expanded_column(text: &str, col: usize) -> usize {
    let mut shown = 0;
    for (index, ch) in text.chars().enumerate() {
        if index >= col {
            break;
        }
        shown += if ch == '\t' { TAB_WIDTH } else { 1 };
    }
    shown
}

/// Column of the source text nearest to a column of the laid-out text.
fn original_column(text: &str, shown: usize) -> usize {
    let mut cells = 0;
    let mut col = 0;
    for ch in text.chars() {
        let width = if ch == '\t' { TAB_WIDTH } else { 1 };
        if cells + width > shown {
            // Inside this character: pick whichever edge is closer.
            return if shown - cells <= width / 2 {
                col
            } else {
                col + 1
            };
        }
        cells += width;
        col += 1;
    }
    col
}

fn push_section(job: &mut LayoutJob, text: &str, color: Color32, font: &FontId) {
    if text.is_empty() {
        return;
    }
    let expanded: String = text.replace('\t', &" ".repeat(TAB_WIDTH));
    let start = job.text.len();
    job.text.push_str(&expanded);
    job.sections.push(LayoutSection {
        leading_space: 0.0,
        byte_range: ByteIndex(start)..ByteIndex(job.text.len()),
        format: TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    });
}

fn token_color(kind: TokenKind) -> Color32 {
    match kind {
        TokenKind::Keyword => Color32::from_rgb(197, 134, 192),
        TokenKind::Type => Color32::from_rgb(78, 201, 176),
        TokenKind::Function | TokenKind::Method => Color32::from_rgb(220, 220, 170),
        TokenKind::Variable => Color32::from_rgb(156, 220, 254),
        TokenKind::Constant => Color32::from_rgb(79, 193, 255),
        TokenKind::String => Color32::from_rgb(206, 145, 120),
        TokenKind::Escape => Color32::from_rgb(215, 186, 125),
        TokenKind::Number => Color32::from_rgb(181, 206, 168),
        TokenKind::Comment | TokenKind::DocComment => Color32::from_rgb(106, 153, 85),
        TokenKind::Attribute => Color32::from_rgb(156, 220, 254),
        TokenKind::Operator => Color32::from_rgb(212, 212, 212),
        TokenKind::Punctuation => Color32::from_rgb(128, 128, 128),
        TokenKind::Error => Color32::from_rgb(244, 71, 71),
    }
}

/// Applies keyboard and clipboard events. Returns true when the text changed.
fn handle_input(ui: &mut Ui, buffer: &mut EditorBuffer) -> bool {
    let mut changed = false;
    let events = ui.input(|input| input.events.clone());

    for event in events {
        match event {
            Event::Text(text) => {
                buffer.insert_text(&text);
                changed = true;
            }
            Event::Paste(text) => {
                buffer.insert_text(&text);
                changed = true;
            }
            Event::Copy => {
                let selection = buffer.selected_text();
                if !selection.is_empty() {
                    ui.ctx().copy_text(selection);
                }
            }
            Event::Cut => {
                let selection = buffer.selected_text();
                if !selection.is_empty() {
                    ui.ctx().copy_text(selection);
                    buffer.insert_text("");
                    changed = true;
                }
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let extend = modifiers.shift;
                match key {
                    Key::ArrowLeft if modifiers.ctrl => buffer.move_caret(Motion::WordLeft, extend),
                    Key::ArrowLeft => buffer.move_caret(Motion::Left, extend),
                    Key::ArrowRight if modifiers.ctrl => {
                        buffer.move_caret(Motion::WordRight, extend)
                    }
                    Key::ArrowRight => buffer.move_caret(Motion::Right, extend),
                    Key::ArrowUp => buffer.move_caret(Motion::Up, extend),
                    Key::ArrowDown => buffer.move_caret(Motion::Down, extend),
                    Key::Home if modifiers.ctrl => buffer.move_caret(Motion::DocStart, extend),
                    Key::Home => buffer.move_caret(Motion::Home, extend),
                    Key::End if modifiers.ctrl => buffer.move_caret(Motion::DocEnd, extend),
                    Key::End => buffer.move_caret(Motion::End, extend),
                    Key::PageUp => buffer.move_caret(Motion::PageUp(20), extend),
                    Key::PageDown => buffer.move_caret(Motion::PageDown(20), extend),
                    Key::Backspace => {
                        buffer.backspace();
                        changed = true;
                    }
                    Key::Delete => {
                        buffer.delete_forward();
                        changed = true;
                    }
                    Key::Enter => {
                        buffer.insert_text("\n");
                        changed = true;
                    }
                    Key::Tab if modifiers.shift => {
                        buffer.outdent();
                        changed = true;
                    }
                    Key::Tab => {
                        buffer.indent();
                        changed = true;
                    }
                    Key::A if modifiers.ctrl => buffer.select_all(),
                    Key::Z if modifiers.ctrl && modifiers.shift => {
                        changed |= buffer.redo();
                    }
                    Key::Z if modifiers.ctrl => {
                        changed |= buffer.undo();
                    }
                    Key::Y if modifiers.ctrl => {
                        changed |= buffer.redo();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::{auto_scroll_step, caret_follow};
    use egui::{Rect, pos2};

    fn viewport() -> Rect {
        Rect::from_min_max(pos2(0.0, 200.0), pos2(600.0, 400.0))
    }

    fn row(y: f32) -> Rect {
        Rect::from_min_size(pos2(12.0, y), egui::vec2(1.0, 18.0))
    }

    #[test]
    fn a_still_caret_is_never_chased() {
        // The caret sits above the viewport after a wheel scroll, but it did not
        // move: the view must stay where the user put it.
        assert_eq!(
            caret_follow(Some(0), 0, viewport(), row(0.0)),
            (false, false)
        );
    }

    #[test]
    fn moving_the_caret_inside_the_viewport_only_remembers_it() {
        assert_eq!(
            caret_follow(Some(0), 7, viewport(), row(300.0)),
            (true, false)
        );
    }

    #[test]
    fn moving_the_caret_out_of_the_viewport_scrolls() {
        assert_eq!(
            caret_follow(Some(7), 40, viewport(), row(900.0)),
            (true, true)
        );
    }

    /// Dragging below the viewport scrolls down (the scroll area negates the
    /// delta), and dragging above it scrolls up.
    #[test]
    fn a_drag_past_the_edge_scrolls_towards_the_pointer() {
        assert_eq!(auto_scroll_step(450.0, viewport(), 18.0), -18.0);
        assert_eq!(auto_scroll_step(150.0, viewport(), 18.0), 18.0);
        assert_eq!(auto_scroll_step(300.0, viewport(), 18.0), 0.0);
    }

    #[test]
    fn the_first_frame_does_not_scroll_when_the_caret_is_visible() {
        assert_eq!(caret_follow(None, 0, viewport(), row(200.0)), (true, false));
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::{expanded_column, original_column};

    #[test]
    fn a_tab_takes_four_cells() {
        let text = "\tab";
        assert_eq!(expanded_column(text, 0), 0);
        assert_eq!(expanded_column(text, 1), 4);
        assert_eq!(expanded_column(text, 3), 6);
    }

    #[test]
    fn a_click_inside_a_tab_picks_the_nearer_edge() {
        let text = "\tab";
        assert_eq!(original_column(text, 1), 0);
        assert_eq!(original_column(text, 2), 0);
        assert_eq!(original_column(text, 3), 1);
        assert_eq!(original_column(text, 4), 1);
    }

    #[test]
    fn columns_round_trip_for_plain_text() {
        let text = "fn main() {";
        for col in 0..=text.chars().count() {
            assert_eq!(original_column(text, expanded_column(text, col)), col);
        }
    }
}
