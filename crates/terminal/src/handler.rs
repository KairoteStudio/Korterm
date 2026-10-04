// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Input handler — applies parsed [`Action`]s to the [`Buffer`].
//!
//! Implements the VT100/xterm CSI/C0/ESC/OSC subset that real shells
//! (bash, zsh, fish) emit: cursor movement, SGR colours, line/char
//! erase & insert, scroll, and OSC title / colour queries.

use crate::attr::{decode_sgr, Sgr, Attr, FG_BOLD, FG_UNDERLINE, FG_BLINK, FG_INVERSE, FG_INVISIBLE, FG_STRIKETHROUGH, BG_ITALIC, BG_DIM, BG_OVERLINE};
use crate::buffer::Buffer;
use crate::parser::Action;

pub struct InputHandler;

impl InputHandler {
    /// Apply a batch of actions to the buffer.
    pub fn apply(buf: &mut Buffer, actions: &[Action]) {
        for action in actions {
            Self::apply_one(buf, action);
        }
    }

    fn apply_one(buf: &mut Buffer, action: &Action) {
        match action {
            Action::Print { ch, width, combined } => {
                let cp = *ch as u32;
                buf.write_char(cp, *width, combined);
            }
            Action::C0(b) => Self::c0(buf, *b),
            Action::Csi { params, intermediates, final_byte } => {
                Self::csi(buf, params, intermediates, *final_byte)
            }
            Action::Esc { intermediates, final_byte } => {
                Self::esc(buf, intermediates, *final_byte)
            }
            Action::Osc { params, data } => {
                Self::osc(buf, params, data);
            }
        }
    }

    fn c0(buf: &mut Buffer, b: u8) {
        match b {
            0x08 => buf.backspace(),       // BS
            0x09 => {                       // HT (tab)
                let next = (buf.cursor_x / 8 + 1) * 8;
                buf.cursor_x = next.min(buf.cols - 1);
            }
            0x0A | 0x0B | 0x0C => buf.line_feed(), // LF / VT / FF
            0x0D => buf.carriage_return(),  // CR
            0x07 => { /* BEL — TODO: visual bell */ }
            _ => {}
        }
    }

    fn param(params: &[u16], idx: usize, default: u16) -> u16 {
        params.get(idx).copied().unwrap_or(default)
    }

    fn csi(buf: &mut Buffer, params: &[u16], intermediates: &[u8], final_byte: u8) {
        // Private marker '?' (e.g. `ESC [ ? 25 h` for cursor visibility).
        let private = intermediates.iter().any(|&b| b == b'?');

        match final_byte {
            // ---- cursor movement ----
            b'A' => { // CUU — cursor up
                let n = Self::param(params, 0, 1) as usize;
                buf.cursor_y = buf.cursor_y.saturating_sub(n);
            }
            b'B' => { // CUD — cursor down
                let n = Self::param(params, 0, 1) as usize;
                buf.cursor_y = (buf.cursor_y + n).min(buf.rows - 1);
            }
            b'C' => { // CUF — cursor forward
                let n = Self::param(params, 0, 1) as usize;
                buf.cursor_x = (buf.cursor_x + n).min(buf.cols - 1);
            }
            b'D' => { // CUB — cursor back
                let n = Self::param(params, 0, 1) as usize;
                buf.cursor_x = buf.cursor_x.saturating_sub(n);
            }
            b'E' => { // CNL — cursor next line
                let n = Self::param(params, 0, 1) as usize;
                buf.cursor_y = (buf.cursor_y + n).min(buf.rows - 1);
                buf.cursor_x = 0;
            }
            b'F' => { // CPL — cursor previous line
                let n = Self::param(params, 0, 1) as usize;
                buf.cursor_y = buf.cursor_y.saturating_sub(n);
                buf.cursor_x = 0;
            }
            b'G' => { // CHA — cursor horizontal absolute
                let col = Self::param(params, 0, 1) as usize;
                buf.cursor_x = col.saturating_sub(1).min(buf.cols - 1);
            }
            b'd' => { // VPA — vertical position absolute
                let row = Self::param(params, 0, 1) as usize;
                buf.cursor_y = row.saturating_sub(1).min(buf.rows - 1);
            }
            b'H' | b'f' => { // CUP / HVP — cursor position
                let row = Self::param(params, 0, 1) as usize;
                let col = Self::param(params, 1, 1) as usize;
                buf.set_cursor(row, col);
            }

            // ---- erase ----
            b'J' => { // ED — erase in display
                let mode = Self::param(params, 0, 0);
                match mode {
                    0 => buf.erase_to_end_of_screen(),
                    1 => buf.erase_to_cursor_in_screen(),
                    2 | 3 => buf.erase_screen(),
                    _ => {}
                }
            }
            b'K' => { // EL — erase in line
                let mode = Self::param(params, 0, 0);
                match mode {
                    0 => buf.erase_to_end_of_line(),
                    1 => buf.erase_to_cursor_in_line(),
                    2 => buf.erase_line(),
                    _ => {}
                }
            }

            // ---- insert / delete ----
            b'L' => { // IL — insert lines
                let n = Self::param(params, 0, 1) as usize;
                buf.insert_lines(n);
            }
            b'M' => { // DL — delete lines
                let n = Self::param(params, 0, 1) as usize;
                buf.delete_lines(n);
            }
            b'@' => { // ICH — insert chars
                let n = Self::param(params, 0, 1) as usize;
                buf.insert_chars(n);
            }
            b'P' => { // DCH — delete chars
                let n = Self::param(params, 0, 1) as usize;
                buf.delete_chars(n);
            }
            b'X' => { // ECH — erase chars
                let n = Self::param(params, 0, 1) as usize;
                buf.erase_chars(n);
            }

            // ---- scroll ----
            b'S' => { // SU — scroll up
                let n = Self::param(params, 0, 1) as usize;
                buf.scroll_up(n);
            }
            b'T' => { // SD — scroll down
                let n = Self::param(params, 0, 1) as usize;
                buf.scroll_down(n);
            }

            // ---- SGR (colours / styles) ----
            b'm' => {
                if params.is_empty() {
                    buf.cur_attr = Attr::DEFAULT;
                } else {
                    for sgr in decode_sgr(params) {
                        Self::apply_sgr(&mut buf.cur_attr, sgr);
                    }
                }
            }

            // ---- cursor save / restore ----
            b's' if !private => buf.save_cursor(),
            b'u' if !private => buf.restore_cursor(),

            // ---- DEC private modes (ESC [ ? … h / l) ----
            b'h' if private => {
                for &p in params {
                    match p {
                        25 => buf.cursor_visible = true,  // DECTCEM — text cursor enable
                        1 => { /* DECCKM — application cursor keys; ignored */ }
                        2004 => { /* Bracketed paste mode; ignored */ }
                        1049 | 47 | 1047 => buf.alt_screen = true,
                        _ => {}
                    }
                }
            }
            b'l' if private => {
                for &p in params {
                    match p {
                        25 => buf.cursor_visible = false,
                        1 => { /* DECCKM off */ }
                        2004 => { /* Bracketed paste off */ }
                        1049 | 47 | 1047 => buf.alt_screen = false,
                        _ => {}
                    }
                }
            }

            _ => {}
        }
    }

    fn apply_sgr(attr: &mut Attr, sgr: Sgr) {
        match sgr {
            Sgr::Reset => *attr = Attr::DEFAULT,
            Sgr::ResetFg => attr.reset_fg(),
            Sgr::ResetBg => attr.reset_bg(),
            Sgr::Bold(on) => attr.fg_set_flag(FG_BOLD, on),
            Sgr::Italic(on) => attr.bg_set_flag(BG_ITALIC, on),
            Sgr::Underline(on) => attr.fg_set_flag(FG_UNDERLINE, on),
            Sgr::Blink(on) => attr.fg_set_flag(FG_BLINK, on),
            Sgr::Inverse(on) => attr.fg_set_flag(FG_INVERSE, on),
            Sgr::Invisible(on) => attr.fg_set_flag(FG_INVISIBLE, on),
            Sgr::Strikethrough(on) => attr.fg_set_flag(FG_STRIKETHROUGH, on),
            Sgr::Dim(on) => attr.bg_set_flag(BG_DIM, on),
            Sgr::Overline(on) => attr.bg_set_flag(BG_OVERLINE, on),
            Sgr::FgPalette(idx) => attr.set_fg_palette(idx as u32),
            Sgr::FgPalette256(idx) => attr.set_fg_palette(idx as u32),
            Sgr::FgRgb(r, g, b) => attr.set_fg_rgb(r, g, b),
            Sgr::FgDefault => attr.reset_fg(),
            Sgr::BgPalette(idx) => attr.set_bg_palette(idx as u32),
            Sgr::BgPalette256(idx) => attr.set_bg_palette(idx as u32),
            Sgr::BgRgb(r, g, b) => attr.set_bg_rgb(r, g, b),
            Sgr::BgDefault => attr.reset_bg(),
        }
    }

    fn esc(buf: &mut Buffer, _intermediates: &[u8], final_byte: u8) {
        match final_byte {
            b'7' => buf.save_cursor(),   // DECSC
            b'8' => buf.restore_cursor(), // DECRC
            b'D' => { // IND — index (move down, scroll if needed)
                if buf.cursor_y == buf.scroll_bottom {
                    buf.scroll_up(1);
                } else if buf.cursor_y < buf.rows - 1 {
                    buf.cursor_y += 1;
                }
            }
            b'M' => { // RI — reverse index (move up, scroll if needed)
                if buf.cursor_y == buf.scroll_top {
                    buf.scroll_down(1);
                } else if buf.cursor_y > 0 {
                    buf.cursor_y -= 1;
                }
            }
            b'E' => { // NEL — next line
                buf.line_feed();
            }
            _ => {}
        }
    }

    fn osc(buf: &mut Buffer, params: &[String], data: &str) {
        // OSC 0 / 2 = set window title — handled at the widget level.
        // OSC 4 / 104 = set / reset palette colour — not implemented yet.
        // OSC 133 = shell integration marks (final byte):
        //   A = prompt about to be drawn, D = command is starting.
        // These tell the app whether the shell is idle at a prompt or a
        // foreground program owns the terminal.
        match params.first().map(String::as_str) {
            Some("133") => match data {
                "A" => buf.at_prompt = true,
                "D" | "B" => buf.at_prompt = false,
                _ => {}
            },
            Some("104") => {
                // OSC 104 n — reset palette entry; n is the colour spec.
                let _ = data;
            }
            _ => {}
        }
    }
}