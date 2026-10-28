//! Drawing and input for the text editor.
//!
//! Only the visible lines are laid out: the scroll area hands back the viewport,
//! the widget allocates the full content size for the scrollbars, and then
//! paints the slice of lines that intersects the viewport.

use egui::text::{ByteIndex, LayoutJob, LayoutSection};
use egui::{
    Color32, CornerRadius, Event, FontId, Key, Pos2, Rect, Sense, TextFormat, TextStyle, Ui, Vec2,
};
use ide_syntax::TokenKind;

use super::EditorBuffer;
use super::edit::Motion;

/// How many spaces a tab takes on screen.
const TAB_WIDTH: usize = 4;
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
            paint_selection(
                &painter,
                buffer,
                text_origin,
                char_w,
                row_h,
                first_line,
                last_line,
            );
            paint_lines(
                ui,
                &painter,
                buffer,
                &spans,
                &font,
                text_origin,
                row_h,
                first_line,
                last_line,
            );
            let caret_pos = paint_caret(&painter, buffer, text_origin, char_w, row_h);

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
            if scroll {
                ui.scroll_to_rect(row.expand(3.0), None);
            }

            // Pointer first: clicking must place the caret even though the very
            // same click gives the widget focus.
            if (response.clicked() || response.dragged())
                && let Some(pos) = response.interact_pointer_pos()
            {
                let (line, col) = position_to_line_col(buffer, text_origin, pos, char_w, row_h);
                let index = buffer.char_index(line, col);
                if response.dragged() {
                    buffer.set_drag_caret(index);
                } else {
                    buffer.set_caret(index);
                }
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

fn paint_selection(
    painter: &egui::Painter,
    buffer: &EditorBuffer,
    origin: Pos2,
    char_w: f32,
    row_h: f32,
    first_line: usize,
    last_line: usize,
) {
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

        let x0 = col_to_x(text, from, char_w);
        let mut x1 = col_to_x(text, to, char_w);
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

#[allow(clippy::too_many_arguments)]
fn paint_lines(
    ui: &Ui,
    painter: &egui::Painter,
    buffer: &EditorBuffer,
    spans: &[ide_syntax::HighlightSpan],
    font: &FontId,
    origin: Pos2,
    row_h: f32,
    first_line: usize,
    last_line: usize,
) {
    for line in first_line..last_line {
        let text = buffer.line_text(line);
        let line_start = buffer.line_starts()[line];
        let line_end = line_start + text.len();
        let y = origin.y + (line as f32) * row_h;

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
            continue;
        }
        let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
        painter.galley(Pos2::new(origin.x, y), galley, DEFAULT_FG);
    }
}

fn paint_caret(
    painter: &egui::Painter,
    buffer: &EditorBuffer,
    origin: Pos2,
    char_w: f32,
    row_h: f32,
) -> Pos2 {
    let (line, col) = buffer.line_col();
    let text = buffer.line_text(line);
    let x = col_to_x(text, col, char_w);
    let y = origin.y + (line as f32) * row_h;
    let position = Pos2::new(origin.x + x, y);
    painter.rect_filled(
        Rect::from_min_size(position, Vec2::new(CARET_WIDTH, row_h)),
        CornerRadius::ZERO,
        CARET,
    );
    position
}

/// Absolute caret position from a pointer position.
fn position_to_line_col(
    buffer: &EditorBuffer,
    origin: Pos2,
    pos: Pos2,
    char_w: f32,
    row_h: f32,
) -> (usize, usize) {
    let line = (((pos.y - origin.y) / row_h).floor().max(0.0) as usize)
        .min(buffer.line_count().saturating_sub(1));
    let cl = (pos.x - origin.x) / char_w;
    let text = buffer.line_text(line);
    let mut col = 0;
    let mut width = 0.0;
    for ch in text.chars() {
        let advance = if ch == '\t' {
            char_w * TAB_WIDTH as f32
        } else {
            char_w
        };
        if cl * char_w < width + advance / 2.0 {
            break;
        }
        width += advance;
        col += 1;
    }
    (line, col)
}

fn col_to_x(text: &str, col: usize, char_w: f32) -> f32 {
    let mut width = 0.0;
    for (index, ch) in text.chars().enumerate() {
        if index >= col {
            break;
        }
        width += if ch == '\t' {
            char_w * TAB_WIDTH as f32
        } else {
            char_w
        };
    }
    width
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
                    Key::Tab => {
                        buffer.insert_text(&" ".repeat(TAB_WIDTH));
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
    use super::caret_follow;
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

    #[test]
    fn the_first_frame_does_not_scroll_when_the_caret_is_visible() {
        assert_eq!(caret_follow(None, 0, viewport(), row(200.0)), (true, false));
    }
}
