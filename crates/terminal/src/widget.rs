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
    (0x1a, 0x1b, 0x26), (0xf7, 0x76, 0x8e), (0x9e, 0xce, 0x6a), (0xe0, 0xaf, 0x68),
    (0x7a, 0xa2, 0xf7), (0xbb, 0x9a, 0xf7), (0x7d, 0xcf, 0xff), (0xc0, 0xca, 0xd5),
    (0x41, 0x48, 0x68), (0xf7, 0x76, 0x8e), (0x9e, 0xce, 0x6a), (0xe0, 0xaf, 0x68),
    (0x7a, 0xa2, 0xf7), (0xbb, 0x9a, 0xf7), (0x7d, 0xcf, 0xff), (0xac, 0xb0, 0xd0),
];


/// Terminal color scheme. Switchable in Settings → 外观.
///
/// Only the background differs between the two: the text colors and the
/// 16-color palette are Tokyo Night either way, so switching never
/// changes how a program's own output reads. A tinted surface is a
/// matter of taste (and some screenshots/pipelines assume near-black
/// behind the glyphs), a palette is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermTheme {
    #[default]
    TokyoNight,
    /// The same Tokyo Night text on a plain near-black surface.
    PlainBackground,
}

/// Colors of one [`TermTheme`], resolved once per frame.
pub struct ThemeColors {
    pub bg: Color,
    pub fg: Color,
    pub cursor: Color,
    pub selection: Color,
    pub palette: &'static [(u8, u8, u8); 16],
}

impl TermTheme {
    pub const fn colors(self) -> ThemeColors {
        ThemeColors {
            bg: match self {
                TermTheme::TokyoNight => Color::from_rgb8(0x1a, 0x1b, 0x26),
                TermTheme::PlainBackground => Color::from_rgb8(0x12, 0x12, 0x12),
            },
            fg: Color::from_rgb8(0xc0, 0xca, 0xd5),
            cursor: Color::from_rgba8(0xc0, 0xca, 0xd5, 0.55),
            selection: Color::from_rgba8(0x7a, 0xa2, 0xf7, 0.30),
            palette: &PALETTE,
        }
    }
}

fn palette256(idx: u8, palette: &[(u8, u8, u8); 16]) -> (u8, u8, u8) {
    if (idx as usize) < 16 {
        return palette[idx as usize];
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

fn resolve_attr(
    attr: &Attr,
    default_fg: Color,
    default_bg: Color,
    palette: &[(u8, u8, u8); 16],
) -> (Color, Color) {
    let fg = match attr.fg_mode() {
        CM_RGB => {
            let [r, g, b] = attr.fg_rgb();
            Color::from_rgb8(r, g, b)
        }
        CM_P16 | CM_P256 => {
            let (r, g, b) = palette256(attr.fg_palette() as u8, palette);
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
            let (r, g, b) = palette256(attr.bg_palette() as u8, palette);
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
fn content_row_y(vy: f32) -> f32 {
    PAD_Y + vy * CELL_H
}

pub fn cell_height() -> f32 {
    CELL_H
}

pub fn cell_width() -> f32 {
    measured_char_width().unwrap_or(CELL_W)
}

/// Map a pointer position (canvas-local pixels) to a grid cell.
///
/// The mapping is the **exact inverse** of how `TerminalCanvas` paints:
/// row `vy` is drawn at `PAD_Y + vy * CELL_H` from the canvas top and
/// column `cx` at `PAD_X + cx * char_w`. Anything else drifts — an
/// earlier version derived the text origin from the panel height
/// (`content_h - rows * CELL_H - PAD_Y`), i.e. it assumed the text was
/// bottom-anchored while the renderer draws it top-anchored, so clicks
/// landed up to a full row off.
pub fn pixel_to_cell(term: &Terminal, mx: f32, my: f32) -> (usize, usize) {
    let char_w = cell_width();
    let rows = term.buf.rows;
    let vy = ((my - PAD_Y) / CELL_H).floor() as i32;
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

        // Resolve the scheme once per frame; every color below comes
        // from it, so switching themes is a single field read.
        let theme = self.term.theme.colors();

        // Draw background
        frame.fill_rectangle(
            Point::new(0.0, 0.0),
            bounds.size(),
            theme.bg,
        );

        // Draw only visible rows at viewport-relative positions
        for (vy, i) in (render_start..render_start + draw_rows).enumerate() {
            let line = &self.term.buf.lines[i];
            let row_top = PAD_Y + vy as f32 * CELL_H;
            let text_y = row_top + 2.0;

            let mut current_text = String::with_capacity(line.cells.len());
            let mut current_fg = theme.fg;
            let mut current_bg = Color::TRANSPARENT;
            let mut text_x = PAD_X;
            let mut span_start_x = PAD_X;

            for cell in line.cells.iter() {
                if cell.width == 0 {
                    continue;
                }
                let (fg, bg) = resolve_attr(&cell.attr, theme.fg, theme.bg, theme.palette);
                let ch = if cell.codepoint == 0 { ' ' } else {
                    char::from_u32(cell.codepoint).unwrap_or(' ')
                };
                let span_bg = if bg == theme.bg { Color::TRANSPARENT } else { bg };

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

        // Draw selection highlight. Selection rows are absolute buffer
        // lines; translate each viewport row into buffer space so the
        // highlight moves WITH the text when the viewport scrolls.
        if let Some(sel) = &self.term.selection {
            let (start, end) = normalize(*sel);
            let total = self.term.buf.lines.len();
            let offset = total.saturating_sub(self.term.buf.rows);
            let view_start = offset.saturating_sub(self.term.view_offset);
            for vy in 0..self.term.buf.rows {
                let line_idx = view_start + vy;
                if line_idx < start.1 || line_idx > end.1 || line_idx >= total {
                    continue;
                }
                let line = &self.term.buf.lines[line_idx];
                let cell_start = if line_idx == start.1 { start.0 } else { 0 };
                let cell_end = if line_idx == end.1 {
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
                    theme.selection,
                );
            }
        }

        // Draw cursor.
        //
        // `buf.cursor_y` is a **viewport row** inside the live region —
        // the live region is the last `rows` lines of the buffer, while
        // the viewport starts at `total - rows - view_offset`. So the
        // cursor's screen row is `cursor_y + view_offset`: at rest that
        // equals `cursor_y`, and once the user scrolls into history the
        // cursor slides below the window and must not be drawn (this is
        // what xterm/GNOME Terminal do).
        let state = self.term.cursor_render_state(self.focused);
        if !matches!(state, CursorRenderState::Hidden) {
            let prev_y = self.term.prev_cursor.1 + self.term.view_offset;
            let curr_y = self.term.buf.cursor_y + self.term.view_offset;
            let rows = self.term.buf.rows;
            // Only draw while the cursor line is inside the window.
            if curr_y < rows {
                let t = ease_out_cubic(self.term.cursor_anim_t);
                let from_x = self.term.prev_cursor.0 as f32 * char_w;
                let to_x = self.term.buf.cursor_x as f32 * char_w;
                let from_y = prev_y as f32;
                let to_y = curr_y as f32;
                let px = from_x + (to_x - from_x) * t;
                let py = from_y + (to_y - from_y) * t;

                let x = PAD_X + px;
                let y = content_row_y(py);
                let size = Size::new(char_w, CELL_H);

                match state {
                    CursorRenderState::Showing => {
                        frame.fill_rectangle(Point::new(x, y), size, theme.cursor);
                    }
                    CursorRenderState::NoFocus => {
                        frame.stroke_rectangle(
                            Point::new(x, y),
                            size,
                            canvas::Stroke::default()
                                .with_color(theme.fg)
                                .with_width(1.5),
                        );
                    }
                    CursorRenderState::Hidden => {}
                }
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
            background: Some(Background::Color(term.theme.colors().bg)),
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
#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal with a known grid, so click→cell mapping can be pinned
    /// against the renderer's own row/column formula.
    fn grid_term(cols: usize, rows: usize) -> Terminal {
        Terminal {
            buf: crate::buffer::Buffer::new(cols, rows, 100),
            parser: crate::parser::Parser::new(),
            pty: None,
            title: String::new(),
            prev_cursor: (0, 0),
            cursor_anim_t: 1.0,
            cursor_anim_active: false,
            cursor_blink_visible: true,
            last_blink: std::time::Instant::now(),
            selection: None,
            pending_pty_resize: None,
            view_offset: 0,
            search: None,
            shell_marks: false,
            theme: TermTheme::default(),
        }
    }

    #[test]
    fn the_switch_changes_the_background_and_nothing_else() {
        assert_eq!(TermTheme::default(), TermTheme::TokyoNight);

        let tinted = TermTheme::TokyoNight.colors();
        let plain = TermTheme::PlainBackground.colors();
        // The switch is only worth having if the background really
        // changes…
        assert_ne!(tinted.bg, plain.bg);
        // …and text colors must not follow, or "plain background" would
        // silently restyle every program's output.
        assert_eq!(tinted.fg, plain.fg);
        assert_eq!(tinted.cursor, plain.cursor);
        assert_eq!(tinted.selection, plain.selection);
        assert_eq!(tinted.palette[0], plain.palette[0], "black");
        assert_eq!(tinted.palette[1], plain.palette[1], "red");
        assert_eq!(tinted.palette[7], plain.palette[7], "bright white");
    }

    #[test]
    fn resolve_attr_follows_the_active_theme() {
        let mut attr = Attr::DEFAULT;
        attr.set_fg_palette(1); // palette red
        let tn = TermTheme::TokyoNight.colors();
        let plain = TermTheme::PlainBackground.colors();
        let (fg_tn, bg_tn) = resolve_attr(&attr, tn.fg, tn.bg, tn.palette);
        let (fg_pl, bg_pl) = resolve_attr(&attr, plain.fg, plain.bg, plain.palette);
        let rgb = |c: (u8, u8, u8)| Color::from_rgb8(c.0, c.1, c.2);
        assert_eq!(fg_tn, rgb(tn.palette[1]));
        assert_eq!(fg_pl, rgb(plain.palette[1]));
        // Palette-colored foregrounds agree across both, while the cell
        // background falls back to whichever surface is active.
        assert_eq!(fg_tn, fg_pl);
        assert_eq!(bg_tn, tn.bg);
        assert_eq!(bg_pl, plain.bg);
    }

    #[test]
    fn click_maps_back_to_the_row_that_was_painted() {
        let t = grid_term(80, 24);
        let char_w = cell_width();
        for vy in 0..24usize {
            for cx in [0usize, 1, 17, 79] {
                // Just inside the middle of the cell that the renderer
                // paints at (PAD_X + cx*char_w, PAD_Y + vy*CELL_H).
                let mx = PAD_X + (cx as f32 + 0.5) * char_w;
                let my = PAD_Y + (vy as f32 + 0.5) * CELL_H;
                assert_eq!(
                    pixel_to_cell(&t, mx, my),
                    (cx, vy),
                    "click in the middle of cell ({cx},{vy}) must select it"
                );
            }
        }
    }

    #[test]
    fn clicks_outside_the_grid_clamp_to_the_edge_cells() {
        let t = grid_term(80, 24);
        let char_w = cell_width();
        // Above the first row and left of the first column.
        assert_eq!(pixel_to_cell(&t, -100.0, -100.0), (0, 0));
        // Below the last row and right of the last column.
        let (cx, vy) = pixel_to_cell(&t, 10_000.0, 10_000.0);
        assert_eq!((cx, vy), (79, 23));
    }

    #[test]
    fn click_mapping_does_not_depend_on_the_viewport_scroll() {
        // Scrolling must never move the click mapping: a click at a
        // pixel selects the cell painted there, whatever row that is.
        let mut t = grid_term(80, 24);
        for _ in 0..40 {
            t.buf.line_feed();
        }
        t.scroll_lines(-5);
        let char_w = cell_width();
        let vy = 7usize;
        let my = PAD_Y + (vy as f32 + 0.5) * CELL_H;
        assert_eq!(pixel_to_cell(&t, PAD_X + char_w * 3.5, my), (3, vy));
    }
}
