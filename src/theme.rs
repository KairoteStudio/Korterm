// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Korterm Theme — Based on Fleet Dark Theme

use iced::font::Family;
use iced::{Color, Font};

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color { a, ..rgb(r, g, b) }
}

/// Linear interpolation between two colors (including alpha), `t = 0` → a.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

pub const BG_PRIMARY: Color = rgb(0x06, 0x06, 0x07);
pub const BG_PANEL: Color = rgb(0x13, 0x13, 0x16);
pub const BG_ELEVATED: Color = rgb(0x26, 0x27, 0x2b);
pub const BORDER: Color = rgb(0x3c, 0x3e, 0x43);
pub const HOVER: Color = rgba(0xff, 0xff, 0xff, 0.05);
pub const ISLAND_RING: Color = rgba(0xff, 0xff, 0xff, 0.04);
pub const TEXT: Color = rgb(0xcf, 0xd1, 0xd4);
pub const DIM: Color = rgb(0x8b, 0x8e, 0x94);
pub const DIMMER: Color = rgb(0x5b, 0x5d, 0x63);
pub const BLUE: Color = rgb(0x4a, 0x8c, 0xff);

// Glow colors
pub const GLOW_BLUE: Color = rgba(0x4a, 0x8c, 0xff, 0.12);
pub const GLOW_AMBER: Color = rgba(0xc9, 0x9a, 0x5b, 0.10);

// Traffic light colors
pub const LIGHT_CLOSE: Color = rgb(0xff, 0x5f, 0x57);
pub const LIGHT_MIN: Color = rgb(0xfe, 0xbc, 0x2e);
pub const LIGHT_MAX: Color = rgb(0x28, 0xc8, 0x40);
pub const LIGHT_BORDER: Color = rgba(0x00, 0x00, 0x00, 0.12);

/// Font family for every Chinese label in the UI.
///
/// Deliberately a single-face `.ttf`, not the usual "Noto Sans CJK SC":
/// that family lives in a `.ttc` collection, and with it cosmic-text 0.15
/// (through iced 0.14) hands back a **zero advance** for some CJK glyphs —
/// "背景" laid out 13px wide, one character wide, so both glyphs printed
/// on top of each other inside a one-character box. Same code, same font
/// size, but five CJK characters in a row came out fine, which is why it
/// read as a rendering fluke rather than a layout bug.
const SANS_FAMILY: &str = "WenQuanYi Micro Hei";

pub fn sans() -> Font {
    Font {
        family: Family::Name(SANS_FAMILY),
        weight: iced::font::Weight::Normal,
        ..Font::DEFAULT
    }
}