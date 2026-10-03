// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Vector icon widget rendered through iced's GPU canvas pipeline.
//!
//! Unlike the `svg` widget (which rasterises via `resvg`/`tiny-skia` and
//! suffers from coverage-AA jaggies on thin strokes), this module replays
//! the SVG path data directly onto an iced `canvas::Frame` using
//! `canvas::Path`. The lyon-based GPU renderer produces smooth,
//! analytically anti-aliased edges identical to the editor's own vector
//! overlays (islands, pills, etc.).
//!
//! # Why no `Cache`
//!
//! iced rebuilds the whole widget tree on every `update` (~60 Hz when
//! the cursor blinks or the clock ticks). A `canvas::Cache` owns GPU
//! geometry buffers; if a fresh one is created per frame the driver
//! leaks buffers until the process OOMs. Holding a persistent `Cache`
//! outside the widget tree (e.g. in `thread_local` + `Rc`) still leaks
//! because the `Geometry` returned by `Cache::draw` is reference-counted
//! GPU memory that outlives the frame.
//!
//! Instead we stroke the paths directly on every `draw` call. The lyon
//! tessellation is cheap for a handful of small Bézier segments and the
//! GPU memory is reclaimed the moment the frame is submitted.

pub mod parser;
pub mod svg_body;

use std::sync::OnceLock;

use iced::widget::canvas::{self, Canvas};
use iced::Color;
use iced::Length;

/// A vector icon rendered with the GPU canvas.
///
/// Construct with [`vector_icon`] / [`vector_icon_cached`] and use like any
/// other iced widget.
#[derive(Clone)]
pub struct VectorIcon {
    body: &'static str,
    color: Color,
    size: f32,
    stroke_width: f32,
    view_box: f32,
    rotation: f32,
}

impl VectorIcon {
    pub fn new(body: &'static str, view_box: f32) -> Self {
        Self {
            body,
            color: Color::WHITE,
            size: 16.0,
            stroke_width: 2.0,
            view_box,
            rotation: 0.0,
        }
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn stroke_width(mut self, w: f32) -> Self {
        self.stroke_width = w;
        self
    }

    /// Clockwise rotation in degrees.
    pub fn rotation(mut self, deg: f32) -> Self {
        self.rotation = deg;
        self
    }
}

/// Program backing a [`VectorIcon`]. Holds the parsed sub-paths and draws
/// them onto the canvas.
pub struct IconProgram<M> {
    sub_paths: Vec<svg_body::SubPath>,
    color: Color,
    size: f32,
    stroke_width: f32,
    view_box: f32,
    rotation: f32,
    _msg: std::marker::PhantomData<M>,
}

impl<M> IconProgram<M> {
    fn new(icon: &VectorIcon) -> Self {
        let sub_paths = svg_body::parse_body(icon.body);
        Self {
            sub_paths,
            color: icon.color,
            size: icon.size,
            stroke_width: icon.stroke_width,
            view_box: icon.view_box,
            rotation: icon.rotation,
            _msg: std::marker::PhantomData,
        }
    }

    fn from_parts(icon: &VectorIcon, sub_paths: Vec<svg_body::SubPath>) -> Self {
        Self {
            sub_paths,
            color: icon.color,
            size: icon.size,
            stroke_width: icon.stroke_width,
            view_box: icon.view_box,
            rotation: icon.rotation,
            _msg: std::marker::PhantomData,
        }
    }
}

impl<M> canvas::Program<M> for IconProgram<M> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry<iced::Renderer>> {
        // Build a fresh Frame per draw — no `Cache`, no retained GPU memory.
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // The icon is drawn in view_box units (0..view_box) and scaled down
        // to `size` pixels. We want to rotate around the *pixel* centre of
        // the glyph, so:
        //   1. scale first (view_box units → pixels)
        //   2. translate to the pixel centre, rotate, translate back
        let scale = self.size / self.view_box;
        frame.scale(scale);

        if self.rotation != 0.0 {
            let half_px = self.size / 2.0 / scale; // back to view_box units
            frame.translate(iced::Vector::new(half_px, half_px));
            frame.rotate(self.rotation.to_radians());
            frame.translate(iced::Vector::new(-half_px, -half_px));
        }

        let stroke = canvas::Stroke {
            width: self.stroke_width,
            style: canvas::Style::Solid(self.color),
            line_cap: canvas::LineCap::Round,
            line_join: canvas::LineJoin::Round,
            ..Default::default()
        };
        svg_body::stroke_all(&mut frame, &self.sub_paths, stroke);

        vec![frame.into_geometry()]
    }
}

/// Build a [`Canvas`] widget that renders `icon` with GPU vector paths.
///
/// Each call re-parses the body; for hot paths prefer
/// [`vector_icon_cached`].
pub fn vector_icon<M>(icon: &VectorIcon) -> Canvas<IconProgram<M>, M> {
    let prog = IconProgram::new(icon);
    Canvas::new(prog)
        .width(Length::Fixed(icon.size))
        .height(Length::Fixed(icon.size))
}

/// A process-global cache of parsed icon bodies, keyed by the static
/// string pointer. Avoids re-parsing on every frame for icons that are
/// constructed fresh each view call.
static PARSED: OnceLock<std::sync::Mutex<std::collections::HashMap<usize, Vec<svg_body::SubPath>>>> =
    OnceLock::new();

fn parsed_cache()
-> &'static std::sync::Mutex<std::collections::HashMap<usize, Vec<svg_body::SubPath>>>
{
    PARSED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Like [`vector_icon`] but caches the parsed sub-paths globally so the
/// SVG body is only parsed once per unique `body` string.
pub fn vector_icon_cached<M>(icon: &VectorIcon) -> Canvas<IconProgram<M>, M> {
    let key = icon.body.as_ptr() as usize;
    let sub_paths = {
        let mut m = parsed_cache().lock().unwrap();
        m.entry(key)
            .or_insert_with(|| svg_body::parse_body(icon.body))
            .clone()
    };
    let prog = IconProgram::from_parts(icon, sub_paths);
    Canvas::new(prog)
        .width(Length::Fixed(icon.size))
        .height(Length::Fixed(icon.size))
}

/// Helper to build a widget from a Lucide body string (view_box = 24).
pub fn lucide<M>(
    body: &'static str,
    color: Color,
    size: f32,
    stroke: f32,
) -> Canvas<IconProgram<M>, M> {
    vector_icon_cached(&VectorIcon::new(body, 24.0).color(color).size(size).stroke_width(stroke))
}

/// Like [`lucide`] but rotates the glyph clockwise by `rotation_deg`.
pub fn lucide_rotated<M>(
    body: &'static str,
    color: Color,
    size: f32,
    stroke: f32,
    rotation_deg: f32,
) -> Canvas<IconProgram<M>, M> {
    vector_icon_cached(
        &VectorIcon::new(body, 24.0)
            .color(color)
            .size(size)
            .stroke_width(stroke)
            .rotation(rotation_deg),
    )
}

/// Helper to build a widget from an arbitrary view-box size.
pub fn custom<M>(
    body: &'static str,
    view_box: f32,
    color: Color,
    size: f32,
    stroke: f32,
) -> Canvas<IconProgram<M>, M> {
    vector_icon_cached(
        &VectorIcon::new(body, view_box).color(color).size(size).stroke_width(stroke),
    )
}

/// Re-export for callers that want to build `Pixels`-sized widgets.
pub use iced::Pixels;

/// Convert an iced `Color` to a hex string (kept for parity with the old
/// svg-based icons module).
pub fn hex(c: Color) -> String {
    let f = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", f(c.r), f(c.g), f(c.b))
}