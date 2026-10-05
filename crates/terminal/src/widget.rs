// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Iced widget that renders a [`Terminal`] using Canvas `fill_text` directly.
//!
//! **Performance**: Uses Canvas `fill_text` for all rendering, avoiding
//! rich_text widget tree reconstruction. Matches xterm.js's DOM renderer
//! and JediTerm's `paintComponent` approach.

use std::sync::OnceLock;

use iced::advanced::text::{
    Alignment, Paragraph, Renderer as TextRenderer, Text,
};
use iced::font::Family;
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

/// Terminal font, measured against the real shaper rather than trusted.
///
/// The canvas draws a run of same-styled cells as one string and lets the
/// shaper place every glyph, but the *next* run starts at a grid
/// coordinate.  The two only line up while each glyph's advance equals
/// the cell slot it was given — and that is a property of the font, not
/// of the terminal.  Asking for `Family::Monospace` gives no such
/// guarantee: fontconfig resolves the generic "monospace" family to
/// whatever covers the session language best, which on a CJK desktop is
/// a CJK face (Noto Sans CJK here).  That face has *proportional* Latin
/// (space 0.22em, `M` 0.81em, `i` 0.26em — not a monospace face at all)
/// and *full-width* block elements (█░▏ = 1.00em = 1.23 cells).
///
/// An apt progress bar is fifty block characters on one line, so it
/// drifted 0.23 × 50 ≈ 11 cells past the width apt itself computed, and
/// pushed everything behind it off the right edge; box drawing was just
/// as wrong.  Programs lay their bars out with `wcwidth`, which counts
/// those characters as one column — the grid has to agree with them.
///
/// So: probe.  Pick a family whose Latin glyphs share one advance and
/// whose block glyph is exactly one cell, take the cell width from that
/// family, and scale each width class so its advance matches its slot.
/// Whatever fonts a machine has, grid and glyphs agree afterwards.
pub struct TerminalFont {
    pub font: Font,
    /// Width of one grid cell, in pixels.
    pub cell_w: f32,
    /// Font-size multiplier for 1-cell and 2-cell glyphs.
    pub scale: [f32; 3],
}

impl TerminalFont {
    /// Rendered size that lands a `width`-cell glyph on its slot.
    pub fn size_for(&self, width: u8) -> f32 {
        FONT_SIZE * self.scale[width.clamp(1, 2) as usize]
    }
}

/// Families to try, best first.  All are plain Latin monospaced faces:
/// the ones that also carry CJK (Noto Sans Mono CJK, WenQuanYi Micro Hei
/// Mono) render block elements full-width, which is the very thing being
/// fixed, so they are only ever reached as a last resort.
const MONO_CANDIDATES: &[&str] = &[
    "DejaVu Sans Mono",
    "JetBrains Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
    "Fira Mono",
    "Source Code Pro",
    "Cascadia Mono",
    "MesloLGS NF",
    "Iosevka Term",
    "Hack",
    "Inconsolata",
    "Terminus",
];

static TERMINAL_FONT: OnceLock<TerminalFont> = OnceLock::new();

/// Advance width of `text` shaped with `font` at [`FONT_SIZE`].
fn text_advance(text: &str, font: Font) -> f32 {
    let t = Text {
        content: text,
        font,
        size: Pixels(FONT_SIZE),
        line_height: iced::advanced::text::LineHeight::default(),
        bounds: Size::new(f32::INFINITY, f32::INFINITY),
        align_x: Alignment::Left,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::advanced::text::Shaping::Advanced,
        wrapping: iced::advanced::text::Wrapping::default(),
    };
    <Renderer as TextRenderer>::Paragraph::with_text(t).min_width()
}

fn probe_terminal_font() -> TerminalFont {
    let mut monospaced: Option<(Font, f32)> = None;
    for name in MONO_CANDIDATES {
        let font = Font {
            family: Family::Name(*name),
            ..Font::DEFAULT
        };
        // A single Latin letter defines the cell, but only if the face is
        // really monospaced — otherwise every row would be laid out on
        // proportional advances and the grid would mean nothing.
        let ascii = text_advance("M", font);
        if ascii <= 0.0 {
            continue;
        }
        let is_mono = (text_advance("i", font) - ascii).abs() < ascii * 0.02
            && (text_advance("W", font) - ascii).abs() < ascii * 0.02;
        if !is_mono {
            continue;
        }
        if monospaced.is_none() {
            monospaced = Some((font, ascii));
        }
        // One-cell glyphs are what progress bars, box drawing and
        // separators are made of; a face that gets those wrong cannot be
        // corrected by scaling without tearing the glyphs apart.
        if (text_advance("█", font) - ascii).abs() <= ascii * 0.02 {
            return TerminalFont::measured(font, ascii);
        }
    }
    let (font, cell_w) = monospaced.unwrap_or((Font::MONOSPACE, CELL_W));
    TerminalFont::measured(font, cell_w)
}

impl TerminalFont {
    fn measured(font: Font, cell_w: f32) -> Self {
        // Both probes fall back through the shaper when the family lacks
        // the glyph, so these are the advances the cells will actually
        // be drawn with, not what the family advertises.
        TerminalFont {
            font,
            cell_w,
            scale: scales_for(cell_w, text_advance("█", font), text_advance("汉", font)),
        }
    }
}

/// Font-size multipliers that make each width class land on its slot.
///
/// `narrow_adv` / `wide_adv` are what the shaper actually advances for a
/// one-cell and a two-cell glyph — for a CJK face that is *more* than the
/// slot, for a Latin monospace face less, and only the first case can be
/// seen at all as text landing on top of its neighbour.
fn scales_for(cell_w: f32, narrow_adv: f32, wide_adv: f32) -> [f32; 3] {
    // Never wider than its slot: a glyph that overflows lands on top of
    // its neighbour, a glyph that is merely small just leaves a gap.
    let narrow = if narrow_adv > 0.0 {
        (cell_w / narrow_adv).clamp(0.55, 1.0)
    } else {
        1.0
    };
    let wide = if wide_adv > 0.0 {
        (2.0 * cell_w / wide_adv).clamp(0.7, 1.4)
    } else {
        1.0
    };
    [1.0, narrow, wide]
}

/// The measured terminal font. Probed once, on first use.
pub fn terminal_font() -> &'static TerminalFont {
    TERMINAL_FONT.get_or_init(probe_terminal_font)
}

pub fn measured_char_width() -> Option<f32> {
    TERMINAL_FONT.get().map(|f| f.cell_w)
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
    terminal_font().cell_w
}

/// Paint one run of same-styled, same-width-class cells.
///
/// `bg_w` comes from the grid (columns × cell width), never from the
/// character count: a CJK cell is one *character* but two *columns*, and
/// counting characters left every coloured background behind Chinese text
/// at half its real width.
#[allow(clippy::too_many_arguments)]
fn flush_run(
    frame: &mut canvas::Frame,
    text: &str,
    x: f32,
    text_y: f32,
    row_top: f32,
    fg: Color,
    bg: Color,
    bg_w: f32,
    size: f32,
    font: Font,
) {
    if text.is_empty() {
        return;
    }
    if bg != Color::TRANSPARENT && bg_w > 0.0 {
        frame.fill_rectangle(Point::new(x, row_top), Size::new(bg_w, CELL_H), bg);
    }
    frame.fill_text(canvas::Text {
        content: text.to_string(),
        position: Point::new(x, text_y),
        color: fg,
        size: Pixels(size),
        font,
        ..canvas::Text::default()
    });
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
        let tf = terminal_font();
        let char_w = tf.cell_w;

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
            // Glyphs are placed by the shaper, so a run may only mix cells
            // that share a width class; 1-cell and 2-cell glyphs are also
            // rendered at different sizes, which the shaper has to see as
            // separate runs.
            let mut current_class = 1u8;

            for cell in line.cells.iter() {
                if cell.width == 0 {
                    continue;
                }
                let (fg, bg) = resolve_attr(&cell.attr, theme.fg, theme.bg, theme.palette);
                let ch = if cell.codepoint == 0 { ' ' } else {
                    char::from_u32(cell.codepoint).unwrap_or(' ')
                };
                let span_bg = if bg == theme.bg { Color::TRANSPARENT } else { bg };
                let class = cell.width.clamp(1, 2);

                // Flush if style or width class changed
                if !current_text.is_empty()
                    && (fg != current_fg || span_bg != current_bg || class != current_class)
                {
                    flush_run(
                        &mut frame,
                        &current_text,
                        span_start_x,
                        text_y,
                        row_top,
                        current_fg,
                        current_bg,
                        text_x - span_start_x,
                        tf.size_for(current_class),
                        tf.font,
                    );
                    current_text.clear();
                }

                if current_text.is_empty() {
                    current_fg = fg;
                    current_bg = span_bg;
                    span_start_x = text_x;
                    current_class = class;
                }
                current_text.push(ch);
                text_x += char_w * cell.width as f32;
            }

            // Flush remaining
            flush_run(
                &mut frame,
                &current_text,
                span_start_x,
                text_y,
                row_top,
                current_fg,
                current_bg,
                text_x - span_start_x,
                tf.size_for(current_class),
                tf.font,
            );
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
    // Measure the font (and with it the grid) once, on first use.
    let _ = terminal_font();

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

#[cfg(test)]
mod font_tests {
    use super::{scales_for, TerminalFont, FONT_SIZE};
    use iced::Font;

    /// Advance, at [`FONT_SIZE`], a glyph ends up with once rendered at
    /// [`TerminalFont::size_for`].
    fn drawn_advance(tf: &TerminalFont, advance: f32, width: u8) -> f32 {
        advance * tf.size_for(width) / FONT_SIZE
    }

    fn font_with(cell_w: f32, narrow: f32, wide: f32) -> TerminalFont {
        TerminalFont {
            font: Font::MONOSPACE,
            cell_w,
            scale: scales_for(cell_w, narrow, wide),
        }
    }

    /// The apt case: `Family::Monospace` resolved to Noto Sans CJK, whose
    /// block and CJK glyphs are 1.23 cells wide. Fifty of them on one
    /// progress-bar line overran the row by eleven columns.
    #[test]
    fn a_wide_one_cell_glyph_is_scaled_back_onto_its_cell() {
        let cell_w = 0.812 * FONT_SIZE;
        let tf = font_with(cell_w, 1.23 * cell_w, 1.23 * cell_w);
        assert!(drawn_advance(&tf, 1.23 * cell_w, 1) <= cell_w + 0.01);
        // Two cells cannot be reached without 1.63× — a glyph that size
        // would tower over its row — so the clamp holds it there instead.
        // This is exactly why the probe prefers a family whose one-cell
        // glyphs already fit: a CJK-primary face cannot be rescued for
        // both classes at once.
        assert!(drawn_advance(&tf, 1.23 * cell_w, 2) <= 2.0 * cell_w * 1.4);
    }

    /// DejaVu Sans Mono: block glyphs already fit, CJK comes from a
    /// fallback at 1.66 cells and has to grow to fill two.
    #[test]
    fn a_narrow_two_cell_glyph_is_scaled_up_to_its_two_cells() {
        let cell_w = 0.602 * FONT_SIZE;
        let tf = font_with(cell_w, cell_w, 1.66 * cell_w);
        assert!((drawn_advance(&tf, cell_w, 1) - cell_w).abs() < 0.01);
        assert!((drawn_advance(&tf, 1.66 * cell_w, 2) - 2.0 * cell_w).abs() < 0.05);
    }

    #[test]
    fn a_matching_font_is_left_alone() {
        let cell_w = 0.6 * FONT_SIZE;
        let tf = font_with(cell_w, cell_w, 2.0 * cell_w);
        assert!((tf.size_for(1) - FONT_SIZE).abs() < 0.001);
        assert!((tf.size_for(2) - FONT_SIZE).abs() < 0.001);
    }

    #[test]
    fn missing_glyphs_do_not_produce_a_zero_or_huge_size() {
        let tf = font_with(7.8, 0.0, 0.0);
        assert!((FONT_SIZE..=FONT_SIZE * 1.4).contains(&tf.size_for(1)));
        assert!((FONT_SIZE..=FONT_SIZE * 1.4).contains(&tf.size_for(2)));
    }
}
