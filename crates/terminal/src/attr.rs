// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Cell attribute data — colours and text style flags.
//!
//! Mirrors xterm.js `AttributeData` + `Constants.ts`: foreground and
//! background are packed into `u32` bitfields encoding both the colour
//! value and the colour mode (default / 16-colour palette / 256-colour
//! palette / 24-bit RGB), plus style flags (bold, italic, underline…).

// ---- colour mode bits (bits 25..26 of fg/bg) -------------------------------
pub const CM_DEFAULT: u32 = 0;
pub const CM_P16: u32 = 0x0100_0000; // 16-colour palette
pub const CM_P256: u32 = 0x0200_0000; // 256-colour palette
pub const CM_RGB: u32 = 0x0300_0000; // 24-bit RGB
pub const CM_MASK: u32 = 0x0300_0000;
pub const PCOLOR_MASK: u32 = 0xFF; // palette index
pub const RGB_MASK: u32 = 0x00FF_FFFF;

// ---- foreground style flags (bits 27..32) -----------------------------------
pub const FG_INVERSE: u32 = 0x0400_0000;
pub const FG_BOLD: u32 = 0x0800_0000;
pub const FG_UNDERLINE: u32 = 0x1000_0000;
pub const FG_BLINK: u32 = 0x2000_0000;
pub const FG_INVISIBLE: u32 = 0x4000_0000;
pub const FG_STRIKETHROUGH: u32 = 0x8000_0000;

// ---- background style flags (bits 27..32) ----------------------------------
pub const BG_ITALIC: u32 = 0x0400_0000;
pub const BG_DIM: u32 = 0x0800_0000;
pub const BG_OVERLINE: u32 = 0x4000_0000;

/// Packed cell attributes (fg + bg), exactly like xterm.js `AttributeData`.
#[derive(Clone, Copy, Default, Debug)]
pub struct Attr {
    pub fg: u32,
    pub bg: u32,
}

impl Attr {
    pub const DEFAULT: Attr = Attr { fg: 0, bg: 0 };

    pub fn is_bold(&self) -> bool {
        self.fg & FG_BOLD != 0
    }
    pub fn is_italic(&self) -> bool {
        self.bg & BG_ITALIC != 0
    }
    pub fn is_underline(&self) -> bool {
        self.fg & FG_UNDERLINE != 0
    }
    pub fn is_inverse(&self) -> bool {
        self.fg & FG_INVERSE != 0
    }
    pub fn is_dim(&self) -> bool {
        self.bg & BG_DIM != 0
    }
    pub fn is_blink(&self) -> bool {
        self.fg & FG_BLINK != 0
    }
    pub fn is_invisible(&self) -> bool {
        self.fg & FG_INVISIBLE != 0
    }
    pub fn is_strikethrough(&self) -> bool {
        self.fg & FG_STRIKETHROUGH != 0
    }
    pub fn is_overline(&self) -> bool {
        self.bg & BG_OVERLINE != 0
    }

    /// Foreground colour mode.
    pub fn fg_mode(&self) -> u32 {
        self.fg & CM_MASK
    }
    pub fn bg_mode(&self) -> u32 {
        self.bg & CM_MASK
    }
    /// Palette index (0..15 or 0..255) if in palette mode.
    pub fn fg_palette(&self) -> u32 {
        self.fg & PCOLOR_MASK
    }
    pub fn bg_palette(&self) -> u32 {
        self.bg & PCOLOR_MASK
    }
    /// RGB triple if in RGB mode.
    pub fn fg_rgb(&self) -> [u8; 3] {
        let v = self.fg & RGB_MASK;
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }
    pub fn bg_rgb(&self) -> [u8; 3] {
        let v = self.bg & RGB_MASK;
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    /// Set foreground to a 16/256-colour palette index.
    pub fn set_fg_palette(&mut self, idx: u32) {
        self.fg = (self.fg & !CM_MASK & !PCOLOR_MASK) | CM_P256 | (idx & PCOLOR_MASK);
    }
    /// Set foreground to 24-bit RGB.
    pub fn set_fg_rgb(&mut self, r: u8, g: u8, b: u8) {
        let rgb = (r as u32) << 16 | (g as u32) << 8 | b as u32;
        self.fg = (self.fg & !CM_MASK & !RGB_MASK) | CM_RGB | rgb;
    }
    /// Reset foreground to the default colour.
    pub fn reset_fg(&mut self) {
        self.fg = 0;
    }
    /// Set background to a palette index.
    pub fn set_bg_palette(&mut self, idx: u32) {
        self.bg = (self.bg & !CM_MASK & !PCOLOR_MASK) | CM_P256 | (idx & PCOLOR_MASK);
    }
    /// Set background to 24-bit RGB.
    pub fn set_bg_rgb(&mut self, r: u8, g: u8, b: u8) {
        let rgb = (r as u32) << 16 | (g as u32) << 8 | b as u32;
        self.bg = (self.bg & !CM_MASK & !RGB_MASK) | CM_RGB | rgb;
    }
    pub fn reset_bg(&mut self) {
        self.bg = 0;
    }

    /// Toggle a style flag on foreground.
    pub fn fg_set_flag(&mut self, mask: u32, on: bool) {
        if on {
            self.fg |= mask;
        } else {
            self.fg &= !mask;
        }
    }
    pub fn bg_set_flag(&mut self, mask: u32, on: bool) {
        if on {
            self.bg |= mask;
        } else {
            self.bg &= !mask;
        }
    }
}

/// SGR (Select Graphic Rendition) parameter, decoded from `CSI … m`.
#[derive(Clone, Copy, Debug)]
pub enum Sgr {
    Reset,
    ResetFg,
    ResetBg,
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Blink(bool),
    Inverse(bool),
    Invisible(bool),
    Strikethrough(bool),
    Dim(bool),
    Overline(bool),
    FgPalette(u8),
    FgRgb(u8, u8, u8),
    FgDefault,
    BgPalette(u8),
    BgRgb(u8, u8, u8),
    BgDefault,
    /// 8-bit palette (38;5;n or 48;5;n)
    FgPalette256(u8),
    BgPalette256(u8),
}

/// Decode SGR parameter list into a sequence of [`Sgr`] operations.
/// Handles the `38;2;r;g;b` / `38;5;n` sub-parameter forms.
pub fn decode_sgr(params: &[u16]) -> Vec<Sgr> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => out.push(Sgr::Reset),
            1 => out.push(Sgr::Bold(true)),
            2 => out.push(Sgr::Dim(true)),
            3 => out.push(Sgr::Italic(true)),
            4 => out.push(Sgr::Underline(true)),
            5 => out.push(Sgr::Blink(true)),
            7 => out.push(Sgr::Inverse(true)),
            8 => out.push(Sgr::Invisible(true)),
            9 => out.push(Sgr::Strikethrough(true)),
            21 => out.push(Sgr::Bold(false)), // double underline, often mapped to bold-off
            22 => {
                out.push(Sgr::Bold(false));
                out.push(Sgr::Dim(false));
            }
            23 => out.push(Sgr::Italic(false)),
            24 => out.push(Sgr::Underline(false)),
            25 => out.push(Sgr::Blink(false)),
            27 => out.push(Sgr::Inverse(false)),
            28 => out.push(Sgr::Invisible(false)),
            29 => out.push(Sgr::Strikethrough(false)),
            53 => out.push(Sgr::Overline(true)),
            55 => out.push(Sgr::Overline(false)),
            30..=37 => out.push(Sgr::FgPalette((params[i] - 30) as u8)),
            38 => {
                if i + 1 < params.len() {
                    match params[i + 1] {
                        5 => {
                            if i + 2 < params.len() {
                                out.push(Sgr::FgPalette256(params[i + 2] as u8));
                                i += 2;
                            }
                        }
                        2 => {
                            if i + 4 < params.len() {
                                out.push(Sgr::FgRgb(
                                    params[i + 2] as u8,
                                    params[i + 3] as u8,
                                    params[i + 4] as u8,
                                ));
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                }
            }
            39 => out.push(Sgr::FgDefault),
            40..=47 => out.push(Sgr::BgPalette((params[i] - 40) as u8)),
            48 => {
                if i + 1 < params.len() {
                    match params[i + 1] {
                        5 => {
                            if i + 2 < params.len() {
                                out.push(Sgr::BgPalette256(params[i + 2] as u8));
                                i += 2;
                            }
                        }
                        2 => {
                            if i + 4 < params.len() {
                                out.push(Sgr::BgRgb(
                                    params[i + 2] as u8,
                                    params[i + 3] as u8,
                                    params[i + 4] as u8,
                                ));
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                }
            }
            49 => out.push(Sgr::BgDefault),
            90..=97 => out.push(Sgr::FgPalette((params[i] - 90 + 8) as u8)),
            100..=107 => out.push(Sgr::BgPalette((params[i] - 100 + 8) as u8)),
            _ => {}
        }
        i += 1;
    }
    out
}