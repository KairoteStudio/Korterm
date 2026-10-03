// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Radial-glow background — matches Kortina's Fleet workspace page.

use iced::widget::canvas;
use iced::{Color, Point, Rectangle};

use crate::theme;

/// Tunable glow parameters (persisted via the app config).
pub struct Glow {
    cache: canvas::Cache,
    /// Top-left glow color.
    pub blue: Color,
    /// Bottom-right glow color.
    pub amber: Color,
    /// Multiplier applied to the top glow's alpha (1.0 = default).
    pub intensity_top: f32,
    /// Multiplier applied to the bottom glow's alpha.
    pub intensity_bottom: f32,
}

impl Glow {
    pub fn new() -> Self {
        let cfg = crate::config::load();
        Self {
            cache: canvas::Cache::default(),
            blue: glow_color(cfg.glow_blue, theme::GLOW_BLUE),
            amber: glow_color(cfg.glow_amber, theme::GLOW_AMBER),
            intensity_top: cfg.glow_intensity_top.clamp(0.1, 2.5),
            intensity_bottom: cfg.glow_intensity_bottom.clamp(0.1, 2.5),
        }
    }

    /// Drop the cached geometry so the next frame re-renders.
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    /// Restore theme defaults.
    pub fn reset_to_defaults(&mut self) {
        self.blue = theme::GLOW_BLUE;
        self.amber = theme::GLOW_AMBER;
        self.intensity_top = 1.0;
        self.intensity_bottom = 1.0;
    }
}

fn glow_color(rgb: u32, default: Color) -> Color {
    if rgb == 0 {
        return default;
    }
    Color::from_rgb8(
        ((rgb >> 16) & 0xFF) as u8,
        ((rgb >> 8) & 0xFF) as u8,
        (rgb & 0xFF) as u8,
    )
}

/// Pack a color's RGB into 0xRRGGBB (for config persistence).
pub fn rgb_of(c: Color) -> u32 {
    ((c.r * 255.0).round() as u32) << 16
        | ((c.g * 255.0).round() as u32) << 8
        | (c.b * 255.0).round() as u32
}

impl canvas::Program<crate::terminal_panel::Message> for &Glow {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::BG_PRIMARY);
            let blue = Color {
                a: theme::GLOW_BLUE.a * self.intensity_top,
                ..self.blue
            };
            let amber = Color {
                a: theme::GLOW_AMBER.a * self.intensity_bottom,
                ..self.amber
            };
            glow(frame, bounds, 0.22, -0.12, 0.30, blue);
            glow(frame, bounds, 1.00, 0.12, 0.28, amber);
        });
        vec![geometry]
    }
}

fn glow(frame: &mut canvas::Frame, bounds: Rectangle, fx: f32, fy: f32, fr: f32, color: Color) {
    let center = Point::new(bounds.width * fx, bounds.height * fy);
    let radius = fr * farthest_corner_distance(bounds, center);
    if radius <= 0.0 {
        return;
    }

    const STEPS: usize = 96;
    for i in 0..STEPS {
        let t0 = i as f32 / STEPS as f32;
        let t1 = (i + 1) as f32 / STEPS as f32;
        let t_mid = (t0 + t1) * 0.5;
        let a = (1.0 - t_mid) * color.a;
        if a <= 0.0015 {
            continue;
        }
        ring(
            frame,
            center,
            radius * t0,
            radius * t1,
            Color { a, ..color },
        );
    }
}

fn farthest_corner_distance(bounds: Rectangle, center: Point) -> f32 {
    let corners = [
        (0.0, 0.0),
        (bounds.width, 0.0),
        (0.0, bounds.height),
        (bounds.width, bounds.height),
    ];
    corners
        .iter()
        .map(|(x, y)| {
            let dx = x - center.x;
            let dy = y - center.y;
            (dx * dx + dy * dy).sqrt()
        })
        .fold(0.0_f32, f32::max)
}

fn ring(frame: &mut canvas::Frame, center: Point, r_in: f32, r_out: f32, color: Color) {
    if r_out <= r_in {
        return;
    }
    if r_in <= 0.5 {
        frame.fill(&canvas::Path::circle(center, r_out), color);
        return;
    }

    const SEGMENTS: usize = 64;
    let tau = std::f32::consts::TAU;

    let path = canvas::Path::new(|p| {
        for i in 0..=SEGMENTS {
            let angle = tau * i as f32 / SEGMENTS as f32;
            let pt = Point::new(
                center.x + r_out * angle.cos(),
                center.y + r_out * angle.sin(),
            );
            if i == 0 {
                p.move_to(pt);
            } else {
                p.line_to(pt);
            }
        }
        for i in (0..=SEGMENTS).rev() {
            let angle = tau * i as f32 / SEGMENTS as f32;
            p.line_to(Point::new(
                center.x + r_in * angle.cos(),
                center.y + r_in * angle.sin(),
            ));
        }
        p.close();
    });

    frame.fill(&path, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glow_color_zero_falls_back_to_default() {
        let d = Color::from_rgb8(0x12, 0x34, 0x56);
        let c = glow_color(0, d);
        assert!((c.r - d.r).abs() < 1e-6);
        assert!((c.g - d.g).abs() < 1e-6);
        assert!((c.b - d.b).abs() < 1e-6);
    }

    #[test]
    fn glow_color_unpacks_rgb() {
        let c = glow_color(0x4a_8c_ff, Color::BLACK);
        assert!((c.r - 0x4a as f32 / 255.0).abs() < 1e-6);
        assert!((c.g - 0x8c as f32 / 255.0).abs() < 1e-6);
        assert!((c.b - 0xff as f32 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn rgb_of_roundtrip() {
        for rgb in [0x000000u32, 0xffffff, 0x4a8cff, 0x010203] {
            let c = Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
            assert_eq!(rgb_of(c), rgb);
        }
    }
}