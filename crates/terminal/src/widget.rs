// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Iced widget that renders a [`Terminal`] using Canvas `fill_text` directly.
//!
//! **Performance**: Uses Canvas `fill_text` for all rendering, avoiding
//! rich_text widget tree reconstruction. Matches xterm.js's DOM renderer
//! and JediTerm's `paintComponent` approach.

use std::sync::atomic::{AtomicU32, Ordering};

use iced::advanced::text::{
    Alignment, Paragraph, Renderer as TextRenderer, Text,
};
use iced::mouse;
use iced::widget::canvas::{self, Canvas};
use iced::widget::container;
use iced::{
    Color, Element, Font, Length, Pixels, Point, Rectangle, Renderer, Size,
    Theme, Background,
};

use crate::Terminal;
use crate::CursorRenderState;
use crate::ease_out_cubic;
use crate::normalize;
use crate::attr::{Attr, CM_P16, CM_P256, CM_RGB};

static MEASURED_CHAR_W: AtomicU32 = AtomicU32::new(0);

pub fn measured_char_width() -> Option<f32> {
    let bits = MEASURED_CHAR_W.load(Ordering::Relaxed);
    if bits == 0 {
        None
    } else {
        Some(f32::from_bits(bits))
    }
}

fn store_char_width(w: f32) {
    if w > 0.0 && w.is_finite() {
        MEASURED_CHAR_W.store(w.to_bits(), Ordering::Relaxed);
    }
}

fn measure_char_width() -> f32 {
    const REPEAT: usize = 32;
    let sample = "W".repeat(REPEAT);
    let text = Text {
        content: sample.as_str(),
        font: Font::MONOSPACE,
        size: Pixels(FONT_SIZE),
        line_height: iced::advanced::text::LineHeight::default(),
        bounds: Size::new(f32::INFINITY, f32::INFINITY),
        align_x: Alignment::Left,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::advanced::text::Shaping::Advanced,
        wrapping: iced::advanced::text::Wrapping::default(),
    };
    let p = <Renderer as TextRenderer>::Paragraph::with_text(text);
    p.min_width() / REPEAT as f32
}

const PALETTE: [(u8, u8, u8); 16] = [
    (0, 0, 0), (205, 0, 0), (0, 205, 0), (205, 205, 0),
    (0, 0, 238), (205, 0, 205), (0, 205, 205), (229, 229, 229),
    (127, 127, 127), (255, 0, 0), (0, 255, 0), (255, 255, 0),
    (92, 92, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255),
];

fn palette256(idx: u8) -> (u8, u8, u8) {
    if (idx as usize) < 16 {
        return PALETTE[idx as usize];
    }
    if idx >= 232 {
        let v = 8 + (idx - 232) * 10;
        return (v, v, v);
    }
    let i = idx - 16;
    let r = i / 36;
    let g = (i / 6) % 6;
    let b = i % 6;
    let component = |c: u8| -> u8 {
        if c == 0 { 0 } else { 55 + c * 40 }
    };
    (component(r), component(g), component(b))
}

fn resolve_attr(attr: &Attr, default_fg: Color, default_bg: Color) -> (Color, Color) {
    let fg = match attr.fg_mode() {
        CM_RGB => {
            let [r, g, b] = attr.fg_rgb();
            Color::from_rgb8(r, g, b)
        }
        CM_P16 | CM_P256 => {
            let (r, g, b) = palette256(attr.fg_palette() as u8);
            Color::from_rgb8(r, g, b)
        }
        _ => default_fg,
    };
    let bg = match attr.bg_mode() {
        CM_RGB => {
            let [r, g, b] = attr.bg_rgb();
            Color::from_rgb8(r, g, b)
        }
        CM_P16 | CM_P256 => {
            let (r, g, b) = palette256(attr.bg_palette() as u8);
            Color::from_rgb8(r, g, b)
        }
        _ => default_bg,
    };
    if attr.is_inverse() { (bg, fg) } else { (fg, bg) }
}

const FONT_SIZE: f32 = 13.0;
const CELL_H: f32 = FONT_SIZE * 1.35;
const CELL_W: f32 = FONT_SIZE * 0.62;
pub const PAD_X: f32 = 6.0;
pub const PAD_Y: f32 = 4.0;
const BG: Color = Color::from_rgb8(0x12, 0x12, 0x12);
const FG: Color = Color::from_rgb8(0xD4, 0xD4, 0xD4);
const CURSOR: Color = Color::from_rgba8(0xD4, 0xD4, 0xD4, 0.55);
const SELECTION: Color = Color::from_rgba8(0x6B, 0x9C, 0xFF, 0.30);

fn content_row_y(vy: f32) -> f32 {
    PAD_Y + vy * CELL_H
}

pub fn cell_height() -> f32 {
    CELL_H
}

pub fn cell_width() -> f32 {
    measured_char_width().unwrap_or(CELL_W)
}

pub fn pixel_to_cell(term: &Terminal, mx: f32, my: f32, content_h: f32) -> (usize, usize) {
    let char_w = cell_width();
    let rows = term.buf.rows;
    let top_of_text = content_h - rows as f32 * CELL_H - PAD_Y;
    let vy = ((my - top_of_text) / CELL_H).floor() as i32;
    let vy = vy.clamp(0, rows as i32 - 1) as usize;
    let cx = ((mx - PAD_X) / char_w).floor() as i32;
    let cx = cx.clamp(0, term.buf.cols as i32 - 1) as usize;
    (cx, vy)
}

/// Canvas program that draws the entire terminal using fill_text.
struct TerminalCanvas<'a> {
    term: &'a Terminal,
    focused: bool,
}

impl<'a, M: 'static> canvas::Program<M> for TerminalCanvas<'a> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let char_w = measured_char_width().unwrap_or_else(|| {
            let w = measure_char_width();
            store_char_width(w);
            w
        });

        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let visible_rows = self.term.buf.rows;
        let total_lines = self.term.buf.lines.len();
        // Viewport: live bottom (offset 0) or scrolled up into scrollback.
        let render_start = total_lines
            .saturating_sub(visible_rows)
            .saturating_sub(self.term.view_offset);
        // Never draw more rows than fit (when scrolled up, the range
        // render_start..total is longer than the viewport).
        let draw_rows = visible_rows.min(total_lines.saturating_sub(render_start));

        // Draw background
        frame.fill_rectangle(
            Point::new(0.0, 0.0),
            bounds.size(),
            BG,
        );

        // Draw only visible rows at viewport-relative positions
        for (vy, i) in (render_start..render_start + draw_rows).enumerate() {
            let line = &self.term.buf.lines[i];
            let row_top = PAD_Y + vy as f32 * CELL_H;
            let text_y = row_top + 2.0;

            let mut current_text = String::with_capacity(line.cells.len());
            let mut current_fg = FG;
            let mut current_bg = Color::TRANSPARENT;
            let mut text_x = PAD_X;
            let mut span_start_x = PAD_X;

            for cell in line.cells.iter() {
                if cell.width == 0 {
                    continue;
                }
                let (fg, bg) = resolve_attr(&cell.attr, FG, BG);
                let ch = if cell.codepoint == 0 { ' ' } else {
                    char::from_u32(cell.codepoint).unwrap_or(' ')
                };
                let span_bg = if bg == BG { Color::TRANSPARENT } else { bg };

                // Flush if style changed
                if !current_text.is_empty() && (fg != current_fg || span_bg != current_bg) {
                    if current_bg != Color::TRANSPARENT {
                        let text_width = current_text.chars().count() as f32 * char_w;
                        frame.fill_rectangle(
                            Point::new(span_start_x, row_top),
                            Size::new(text_width, CELL_H),
                            current_bg,
                        );
                    }
                    frame.fill_text(canvas::Text {
                        content: current_text.clone(),
                        position: Point::new(span_start_x, text_y),
                        color: current_fg,
                        size: Pixels(FONT_SIZE),
                        font: Font::MONOSPACE,
                        ..canvas::Text::default()
                    });
                    current_text.clear();
                    span_start_x = text_x;
                }

                if current_text.is_empty() {
                    current_fg = fg;
                    current_bg = span_bg;
                    span_start_x = text_x;
                }
                current_text.push(ch);
                text_x += char_w * cell.width as f32;
            }

            // Flush remaining
            if !current_text.is_empty() {
                if current_bg != Color::TRANSPARENT {
                    let text_width = current_text.chars().count() as f32 * char_w;
                    frame.fill_rectangle(
                        Point::new(span_start_x, row_top),
                        Size::new(text_width, CELL_H),
                        current_bg,
                    );
                }
                frame.fill_text(canvas::Text {
                    content: current_text.clone(),
                    position: Point::new(span_start_x, text_y),
                    color: current_fg,
                    size: Pixels(FONT_SIZE),
                    font: Font::MONOSPACE,
                    ..canvas::Text::default()
                });
            }
        }

        // Draw search highlights (all matches + a brighter current one)
        if let Some(search) = &self.term.search {
            const MATCH: Color = Color::from_rgba8(0xFF, 0xB3, 0x00, 0.28);
            const CURRENT: Color = Color::from_rgba8(0xFF, 0xD5, 0x4F, 0.55);
            for (mi, m) in search.matches.iter().enumerate() {
                if m.line < render_start || m.line >= render_start + draw_rows {
                    continue;
                }
                let vy = m.line - render_start;
                let y = PAD_Y + vy as f32 * CELL_H;
                let x = PAD_X + m.col_start as f32 * char_w;
                let w = (m.col_end - m.col_start).max(1) as f32 * char_w;
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(w, CELL_H),
                    if mi == search.current { CURRENT } else { MATCH },
                );
            }
        }

        // Draw selection highlight
        if let Some(sel) = &self.term.selection {
            let (start, end) = normalize(*sel);
            let total = self.term.buf.lines.len();
            let offset = total.saturating_sub(self.term.buf.rows);
            for vy in start.1..=end.1 {
                let line_idx = offset + vy;
                if line_idx >= total {
                    break;
                }
                let line = &self.term.buf.lines[line_idx];
                let cell_start = if vy == start.1 { start.0 } else { 0 };
                let cell_end = if vy == end.1 {
                    end.0
                } else {
                    line.cells.len().saturating_sub(1)
                };
                if cell_start > cell_end {
                    continue;
                }
                let from_x = PAD_X + cell_start as f32 * char_w;
                let width = ((cell_end + 1 - cell_start) as f32 * char_w).max(char_w);
                let y = content_row_y(vy as f32);
                frame.fill_rectangle(
                    Point::new(from_x, y),
                    Size::new(width, CELL_H),
                    SELECTION,
                );
            }
        }

        // Draw cursor
        let state = self.term.cursor_render_state(self.focused);
        if !matches!(state, CursorRenderState::Hidden) {
            let prev = self.term.prev_cursor;
            let curr = (self.term.buf.cursor_x, self.term.buf.cursor_y);
            let t = ease_out_cubic(self.term.cursor_anim_t);
            let from_x = prev.0 as f32 * char_w;
            let to_x = curr.0 as f32 * char_w;
            let from_y = prev.1 as f32;
            let to_y = curr.1 as f32;
            let px = from_x + (to_x - from_x) * t;
            let py = from_y + (to_y - from_y) * t;

            let x = PAD_X + px;
            let y = content_row_y(py);
            let size = Size::new(char_w, CELL_H);

            match state {
                CursorRenderState::Showing => {
                    frame.fill_rectangle(Point::new(x, y), size, CURSOR);
                }
                CursorRenderState::NoFocus => {
                    frame.stroke_rectangle(
                        Point::new(x, y),
                        size,
                        canvas::Stroke::default()
                            .with_color(FG)
                            .with_width(1.5),
                    );
                }
                CursorRenderState::Hidden => {}
            }
        }

        vec![frame.into_geometry()]
    }
}

pub fn terminal_view<'a, M: 'static>(
    term: &'a Terminal,
    focused: bool,
    _scroll_id: iced::widget::Id,
    on_resize: impl Fn(usize, usize, f32, f32) -> M + 'a,
) -> Element<'a, M> {
    // Measure char width if not yet measured
    if measured_char_width().is_none() {
        let w = measure_char_width();
        store_char_width(w);
    }

    let canvas = Canvas::new(TerminalCanvas { term, focused })
        .width(Length::Fill)
        .height(Length::Fill);

    let content = container(canvas)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_t| container::Style {
            background: Some(Background::Color(BG)),
            ..Default::default()
        });

    iced::widget::sensor(content)
        .on_resize(move |size| {
            let (cols, rows) = grid_size_for_pixels(size.width, size.height);
            on_resize(cols, rows, size.width, size.height)
        })
        .into()
}

pub fn grid_size_for_pixels(width: f32, height: f32) -> (usize, usize) {
    let char_w = measured_char_width().unwrap_or(CELL_W);
    let avail_w = (width - 12.0).max(char_w);
    let avail_h = (height - 8.0).max(CELL_H);
    let cols = (avail_w / char_w).floor() as usize;
    let rows = (avail_h / CELL_H).floor() as usize;
    (cols.max(1), rows.max(1))
}