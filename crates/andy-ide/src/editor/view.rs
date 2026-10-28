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
use ide_core::Theme;
use ide_syntax::{HighlightSpan, TokenKind};

use super::EditorBuffer;
use super::INDENT_WIDTH as TAB_WIDTH;
use super::edit::Motion;

/// Caret width in points.
const CARET_WIDTH: f32 = 1.5;

/// Colours of the text area for one theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub selection: Color32,
    pub caret: Color32,
    pub gutter_bg: Color32,
    pub gutter_fg: Color32,
    pub gutter_fg_active: Color32,
    pub foreground: Color32,
}

impl Palette {
    pub const DARK: Palette = Palette {
        selection: Color32::from_rgb(38, 79, 120),
        caret: Color32::from_rgb(220, 220, 220),
        gutter_bg: Color32::from_rgb(30, 30, 30),
        gutter_fg: Color32::from_rgb(110, 110, 110),
        gutter_fg_active: Color32::from_rgb(200, 200, 200),
        foreground: Color32::from_rgb(212, 212, 212),
    };

    pub const LIGHT: Palette = Palette {
        selection: Color32::from_rgb(181, 213, 255),
        caret: Color32::from_rgb(20, 20, 20),
        gutter_bg: Color32::from_rgb(240, 240, 240),
        gutter_fg: Color32::from_rgb(140, 140, 140),
        gutter_fg_active: Color32::from_rgb(40, 40, 40),
        foreground: Color32::from_rgb(30, 30, 30),
    };

    pub fn for_theme(theme: Theme) -> Palette {
        match theme {
            Theme::Dark => Palette::DARK,
            Theme::Light => Palette::LIGHT,
        }
    }

    /// Colour of one token; the light scheme has to stay readable on white.
    pub fn token(self, kind: TokenKind) -> Color32 {
        match (self.foreground, kind) {
            _ if self == Palette::LIGHT => match kind {
                TokenKind::Keyword => Color32::from_rgb(160, 32, 160),
                TokenKind::Type => Color32::from_rgb(0, 110, 110),
                TokenKind::Function | TokenKind::Method => Color32::from_rgb(110, 80, 0),
                TokenKind::Variable => Color32::from_rgb(20, 60, 140),
                TokenKind::Constant => Color32::from_rgb(0, 90, 160),
                TokenKind::String => Color32::from_rgb(170, 60, 20),
                TokenKind::Escape => Color32::from_rgb(140, 90, 0),
                TokenKind::Number => Color32::from_rgb(60, 120, 20),
                TokenKind::Comment | TokenKind::DocComment => Color32::from_rgb(90, 130, 80),
                TokenKind::Attribute => Color32::from_rgb(20, 60, 140),
                TokenKind::Operator => Color32::from_rgb(30, 30, 30),
                TokenKind::Punctuation => Color32::from_rgb(90, 90, 90),
                TokenKind::Error => Color32::from_rgb(200, 30, 30),
            },
            _ => match kind {
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
            },
        }
    }
}

/// Draws the buffer and handles its input. Returns true when the text changed.
pub fn ui(ui: &mut Ui, buffer: &mut EditorBuffer, palette: Palette) -> bool {
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
            let layouts =
                LineLayouts::build(ui, buffer, &spans, &font, palette, first_line, last_line);

            // Input comes before painting: the caret this frame is the one the
            // key press asked for, so it is drawn where it belongs and the view
            // can follow it in the same frame.
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

            let mut navigated = false;
            if response.has_focus() {
                let (text_changed, moved) = handle_input(ui, buffer);
                changed |= text_changed;
                navigated = moved;
            }

            paint_gutter(
                ui,
                &painter,
                palette,
                buffer,
                rect,
                origin,
                gutter_width,
                row_h,
                first_line,
                last_line,
            );
            paint_selection(
                PaintCtx {
                    painter: &painter,
                    palette,
                    origin: text_origin,
                    char_w,
                    row_h,
                },
                buffer,
                &layouts,
                first_line,
                last_line,
            );
            paint_galleys(&painter, palette, &layouts, text_origin, row_h);
            let (caret_line, _) = buffer.line_col();
            let caret_galley = layouts
                .galley(caret_line)
                .cloned()
                .or_else(|| line_galley(ui, buffer, &spans, &font, palette, caret_line));
            let caret_pos = paint_caret(
                &painter,
                palette,
                buffer,
                caret_galley.as_ref(),
                text_origin,
                char_w,
                row_h,
            );

            // The view followed the caret in the same frame, so the caret is where
            // it was asked to be, and the scroll target is in screen coordinates:
            // the scroll area compares it with its clip rect.
            let caret_content = Pos2::new(caret_pos.x - rect.min.x, caret_pos.y - rect.min.y);
            let tracked = response.id.with("last_caret");
            let previous: Option<usize> = ui.data(|data| data.get_temp(tracked));
            let caret = buffer.caret();
            let row = Rect::from_min_size(caret_content, Vec2::new(1.0, row_h));
            let (remember, outside) = caret_follow(previous, caret, viewport, row);
            if remember {
                ui.data_mut(|data| data.insert_temp(tracked, caret));
            }
            // A wheel scroll moves the view and leaves the caret alone: chasing
            // it then would fight the wheel. Only a caret move or an explicit
            // navigation key pulls the view.
            let pull = outside && (navigated || previous.is_none());
            if pull && !drag_scrolled {
                // Instant, like an editor: a smooth animation would leave the
                // caret off screen for a moment after Ctrl+Home.
                ui.scroll_to_rect_animation(
                    Rect::from_min_size(caret_pos, Vec2::new(1.0, row_h)).expand(3.0),
                    None,
                    egui::style::ScrollAnimation::none(),
                );
            }
        });

    changed
}

/// Decides whether the view should follow the caret this frame.
///
/// Returns `(remember, outside)`: whether the caret of this frame has to be
/// stored for the next one, and whether it sits outside the viewport. Both
/// rectangles are in content coordinates — the scroll area reports the viewport
/// that way, and the caller shifts the caret there before asking.
///
/// The caller scrolls only when the caret is outside *and* the user asked for a
/// caret motion: a wheel scroll leaves the caret where it is, and chasing it
/// then would fight the wheel.
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
    palette: Palette,
    buffer: &EditorBuffer,
    rect: Rect,
    origin: Pos2,
    gutter_width: f32,
    row_h: f32,
    first_line: usize,
    last_line: usize,
) {
    let gutter = Rect::from_min_max(origin, Pos2::new(origin.x + gutter_width, rect.max.y));
    painter.rect_filled(gutter, CornerRadius::ZERO, palette.gutter_bg);

    let (caret_line, _) = buffer.line_col();
    let font = FontId::monospace(row_h * 0.72);
    for line in first_line..last_line {
        let y = origin.y + (line as f32) * row_h;
        let color = if line == caret_line {
            palette.gutter_fg_active
        } else {
            palette.gutter_fg
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
        palette: Palette,
        first_line: usize,
        last_line: usize,
    ) -> Self {
        let galleys = (first_line..last_line)
            .map(|line| line_galley(ui, buffer, spans, font, palette, line))
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
    palette: Palette,
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
        palette,
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
            palette.selection,
        );
    }
}

/// Lays out one line with its highlight colours; `None` for an empty line.
fn line_galley(
    ui: &Ui,
    buffer: &EditorBuffer,
    spans: &[HighlightSpan],
    font: &FontId,
    palette: Palette,
    line: usize,
) -> Option<Arc<Galley>> {
    let job = line_job(buffer, spans, font, palette, line);
    if job.text.is_empty() {
        return None;
    }
    Some(ui.ctx().fonts_mut(|fonts| fonts.layout_job(job)))
}

/// One line as a layout job: every byte of the line lands in exactly one
/// section, so nothing is drawn twice and nothing is left uncoloured.
///
/// The spans are a partition of the text (see `Highlighter::spans_for_lines`);
/// the walk below still clips to the cursor, so a caller that hands over
/// overlapping spans cannot duplicate text.
fn line_job(
    buffer: &EditorBuffer,
    spans: &[HighlightSpan],
    font: &FontId,
    palette: Palette,
    line: usize,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    if line >= buffer.line_count() {
        return job;
    }
    let text = buffer.line_text(line);
    let line_start = buffer.line_starts()[line];
    let line_end = line_start + text.len();
    let mut cursor = line_start;

    for span in spans {
        let start = span.start_byte.max(line_start).min(line_end).max(cursor);
        let end = span.end_byte.min(line_end);
        if end <= start {
            continue;
        }
        let plain = text_slice(buffer.text(), cursor, start);
        push_section(&mut job, plain, palette.foreground, font);
        let coloured = text_slice(buffer.text(), start, end);
        push_section(&mut job, coloured, palette.token(span.kind), font);
        cursor = end;
    }
    let rest = text_slice(buffer.text(), cursor, line_end);
    push_section(&mut job, rest, palette.foreground, font);
    job
}

/// Slices text by byte range, moving both ends back onto characters.
///
/// Highlight spans are aligned to characters already, but a rendering path must
/// not be able to panic. Both ends move *inwards*: a half letter is skipped
/// rather than drawn twice by two neighbouring slices.
fn text_slice(text: &str, start: usize, end: usize) -> &str {
    let mut start = floor_boundary(text, start.min(text.len()));
    let end = floor_boundary(text, end.clamp(start, text.len())).max(start);
    start = start.min(end);
    &text[start..end]
}

fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn paint_galleys(
    painter: &egui::Painter,
    palette: Palette,
    layouts: &LineLayouts,
    origin: Pos2,
    row_h: f32,
) {
    for (index, galley) in layouts.galleys.iter().enumerate() {
        let Some(galley) = galley else { continue };
        let line = layouts.first_line + index;
        let y = origin.y + (line as f32) * row_h;
        painter.galley(Pos2::new(origin.x, y), galley.clone(), palette.foreground);
    }
}

fn paint_caret(
    painter: &egui::Painter,
    palette: Palette,
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
        palette.caret,
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

/// Applies keyboard and clipboard events.
///
/// Returns `(text changed, caret moved)`: the second flag tells the view that
/// the user asked for a caret motion, even if the caret could not move.
/// The text a copy takes: the selection, or the line under the caret.
///
/// An editor without a selection copies the whole line, terminator included,
/// which is what every editor people come from does.
fn text_to_copy(buffer: &EditorBuffer) -> String {
    if buffer.has_selection() {
        return buffer.selected_text();
    }
    let (line, _) = buffer.line_col();
    let starts = buffer.line_starts();
    let start = starts[line];
    let end = starts
        .get(line + 1)
        .copied()
        .unwrap_or_else(|| buffer.text().len());
    buffer.text()[start..end].to_owned()
}

fn copy_to_clipboard(ui: &Ui, buffer: &EditorBuffer) {
    let text = text_to_copy(buffer);
    if !text.is_empty() {
        ui.ctx().copy_text(text);
    }
}

/// Copies and removes; returns whether the buffer changed.
fn cut_to_clipboard(ui: &Ui, buffer: &mut EditorBuffer) -> bool {
    let text = text_to_copy(buffer);
    if text.is_empty() {
        return false;
    }
    ui.ctx().copy_text(text);
    if !buffer.has_selection() {
        let (line, _) = buffer.line_col();
        let start = buffer.line_starts()[line];
        let end = buffer
            .line_starts()
            .get(line + 1)
            .copied()
            .unwrap_or_else(|| buffer.text().len());
        buffer.set_caret(start);
        buffer.set_drag_caret(end);
    }
    buffer.insert_text("");
    true
}

fn handle_input(ui: &mut Ui, buffer: &mut EditorBuffer) -> (bool, bool) {
    let mut changed = false;
    let mut navigated = false;
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
            Event::Copy => copy_to_clipboard(ui, buffer),
            Event::Cut => changed |= cut_to_clipboard(ui, buffer),
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let extend = modifiers.shift;
                match key {
                    Key::ArrowLeft if modifiers.ctrl => {
                        navigated = true;
                        buffer.move_caret(Motion::WordLeft, extend)
                    }
                    Key::ArrowLeft => {
                        navigated = true;
                        buffer.move_caret(Motion::Left, extend)
                    }
                    Key::ArrowRight if modifiers.ctrl => {
                        navigated = true;
                        buffer.move_caret(Motion::WordRight, extend)
                    }
                    Key::ArrowRight => {
                        navigated = true;
                        buffer.move_caret(Motion::Right, extend)
                    }
                    Key::ArrowUp => {
                        navigated = true;
                        buffer.move_caret(Motion::Up, extend)
                    }
                    Key::ArrowDown => {
                        navigated = true;
                        buffer.move_caret(Motion::Down, extend)
                    }
                    Key::Home if modifiers.ctrl => {
                        navigated = true;
                        buffer.move_caret(Motion::DocStart, extend)
                    }
                    Key::Home => {
                        navigated = true;
                        buffer.move_caret(Motion::Home, extend)
                    }
                    Key::End if modifiers.ctrl => {
                        navigated = true;
                        buffer.move_caret(Motion::DocEnd, extend)
                    }
                    Key::End => {
                        navigated = true;
                        buffer.move_caret(Motion::End, extend)
                    }
                    Key::PageUp => {
                        navigated = true;
                        buffer.move_caret(Motion::PageUp(20), extend)
                    }
                    Key::PageDown => {
                        navigated = true;
                        buffer.move_caret(Motion::PageDown(20), extend)
                    }
                    Key::Backspace => {
                        buffer.backspace();
                        changed = true;
                    }
                    // The Windows-only bindings of the keyboard come as plain
                    // key presses here, so the editor maps them itself.
                    Key::Delete if modifiers.shift => {
                        changed |= cut_to_clipboard(ui, buffer);
                    }
                    Key::Insert if modifiers.shift => {
                        // The backend owns the clipboard: ask it to read the
                        // text and send it back as a paste event.
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    }
                    Key::Insert if modifiers.ctrl || modifiers.command => {
                        copy_to_clipboard(ui, buffer);
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

    (changed, navigated)
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

#[cfg(test)]
mod line_job_tests {
    use super::{Palette, line_job};
    use crate::editor::EditorBuffer;
    use egui::FontId;
    use std::path::PathBuf;

    /// The layout job may never repeat a byte of the line: a duplicated span
    /// used to paint the same characters twice, which looked like doubled text.
    #[test]
    fn no_line_is_drawn_twice() {
        let source = include_str!("../../src/app.rs");
        let buffer = EditorBuffer::from_text(PathBuf::from("app.rs"), source.to_owned());
        let spans = buffer
            .highlighter
            .as_ref()
            .expect("rust highlighting")
            .spans_for_lines(0, buffer.line_count());
        let font = FontId::monospace(14.0);

        for line in 0..buffer.line_count() {
            let job = line_job(&buffer, &spans, &font, Palette::DARK, line);
            let expected: String = buffer.line_text(line).replace('\t', "    ");
            assert_eq!(
                job.text, expected,
                "line {line} was laid out as {:?} but holds {:?}",
                job.text, expected
            );
        }
    }
}

#[cfg(test)]
mod char_boundary_tests {
    use super::*;
    use egui::FontId;
    use std::path::PathBuf;

    const MIXED: &str = "//! The andy-ide application: window, panels and the editor.\n\nmod app;\nmod editor;\n\n//\n// При открытии каждого файла заново компилировался запрос\n// подсветки, и это было видно.\n//\nfn main() {\n    let text = \"Привет\";\n}\n";

    fn buffer(text: &str) -> EditorBuffer {
        EditorBuffer::from_text(PathBuf::from("main.rs"), text.to_owned())
    }

    fn assert_spans_fit(buffer: &EditorBuffer, stage: &str) {
        let spans = buffer
            .highlighter
            .as_ref()
            .expect("rust highlighting")
            .spans_for_lines(0, buffer.line_count());
        let text = buffer.text();
        for span in &spans {
            assert!(
                text.is_char_boundary(span.start_byte) && text.is_char_boundary(span.end_byte),
                "{stage}: span {:?} splits a character in byte {} of {text:?}",
                span,
                if !text.is_char_boundary(span.start_byte) {
                    span.start_byte
                } else {
                    span.end_byte
                }
            );
        }
        // And the layout must survive too, which is where the panic happened.
        let font = FontId::monospace(14.0);
        for line in 0..buffer.line_count() {
            let _ = line_job(buffer, &spans, &font, Palette::DARK, line);
        }
    }

    #[test]
    fn spans_stay_on_characters_after_a_block_move() {
        let mut buffer = buffer(MIXED);
        assert_spans_fit(&buffer, "fresh");

        // Cut the Cyrillic paragraph: from "//\n" down to "//\n".
        let cut_start = buffer.line_starts()[5];
        let cut_end = buffer.line_starts()[9];
        buffer.set_caret(cut_start);
        buffer.set_drag_caret(cut_end);
        buffer.insert_text("");
        assert_spans_fit(&buffer, "after the cut");

        // Paste it above `fn main`, which is where moving a block ends up.
        let paste_at = buffer.line_starts()[buffer.line_count() - 4];
        buffer.set_caret(paste_at);
        buffer.insert_text("//\n// При открытии каждого файла заново компилировался запрос\n// подсветки, и это было видно.\n//\n");
        assert_spans_fit(&buffer, "after the paste");
    }

    /// The panic that started this: a capture boundary inside a Cyrillic
    /// letter used to slice the text in half and take the window down.
    #[test]
    fn a_span_that_cuts_a_character_does_not_panic_the_layout() {
        let buffer =
            EditorBuffer::from_text(PathBuf::from("a.rs"), "// Привет, мир и покой\n".to_owned());
        let text = buffer.text().to_owned();
        let middle = text.find('П').expect("the letter is there") + 1;
        let spans = vec![HighlightSpan {
            start_byte: middle,
            end_byte: middle + 3,
            kind: ide_syntax::TokenKind::Comment,
            priority: 0,
        }];

        let font = FontId::monospace(14.0);
        let job = line_job(&buffer, &spans, &font, Palette::DARK, 0);
        assert_eq!(
            job.text,
            buffer.line_text(0),
            "every character is laid out once"
        );
    }

    #[test]
    fn slices_move_back_onto_characters() {
        let text = "abПриветcd";
        assert_eq!(text_slice(text, 0, 3), "ab", "a cut character stays out");
        assert_eq!(text_slice(text, 3, 4), "П");
        assert_eq!(text_slice(text, 4, 6), "р");
        assert_eq!(text_slice(text, 2, 2), "");
        assert_eq!(text_slice(text, 99, 99), "");
        assert_eq!(text_slice(text, 4, 99), "риветcd");
    }

    #[test]
    fn typing_cyrillic_one_letter_at_a_time_keeps_spans_clean() {
        let mut buffer = buffer("fn main() {}\n");
        buffer.set_caret(buffer.text().len());
        for letter in ["П", "р", "и", " ", "в", "е"] {
            buffer.insert_text(letter);
            assert_spans_fit(&buffer, "typing");
        }
    }
}

#[cfg(test)]
mod char_boundary_fuzz_tests {
    use super::*;
    use std::path::PathBuf;

    fn next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state >> 33
    }

    fn boundary_near(text: &str, index: usize) -> usize {
        let mut index = index.min(text.len());
        while index > 0 && !text.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    /// Whatever the user does, tree-sitter captures must land on characters of
    /// *our* text: a capture boundary from a stale tree used to cut a Cyrillic
    /// letter in half and panicked the layout.
    #[test]
    fn random_edits_never_split_a_character() {
        let source = include_str!("../../src/main.rs");
        let mut buffer = EditorBuffer::from_text(PathBuf::from("main.rs"), source.to_owned());
        let pieces = [
            "П",
            "привет ",
            "x",
            "\n",
            "// комментарий\n",
            "fn f() {}",
            "",
            "\n\n\n",
            "\"строка\"",
        ];
        let mut state = 7u64;

        for step in 0..300 {
            let text = buffer.text().to_owned();
            let caret = boundary_near(&text, (next(&mut state) as usize) % (text.len() + 1));
            let spread = (next(&mut state) as usize) % 40;
            let end = boundary_near(&text, caret + spread);
            buffer.set_caret(caret);
            buffer.set_drag_caret(end);
            let action = (next(&mut state) as usize) % 8;
            let piece = pieces[(next(&mut state) as usize) % pieces.len()];
            match action {
                0 => buffer.insert_text(piece),
                1 => {
                    buffer.backspace();
                }
                2 => {
                    buffer.delete_forward();
                }
                3 => buffer.indent(),
                4 => buffer.outdent(),
                5 => {
                    buffer.undo();
                }
                6 => {
                    buffer.redo();
                }
                _ => buffer.insert_text(piece),
            }

            let highlighter = buffer.highlighter.as_ref().expect("rust highlighting");
            assert_eq!(
                highlighter.source_text(),
                buffer.text(),
                "step {step}: the parsed text drifted from the buffer"
            );
            let spans = highlighter.spans_for_lines(0, buffer.line_count());
            let current = buffer.text();
            for span in &spans {
                assert!(
                    current.is_char_boundary(span.start_byte),
                    "step {step}: start {} splits a character",
                    span.start_byte
                );
                assert!(
                    current.is_char_boundary(span.end_byte),
                    "step {step}: end {} splits a character",
                    span.end_byte
                );
            }
        }
    }
}
