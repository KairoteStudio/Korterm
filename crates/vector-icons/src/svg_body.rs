// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Parse a Lucide-style SVG `<body>` (the inner XML of the `<svg>` element)
//! into a flat list of sub-paths, each with its own [`Segment`] list.
//!
//! Supported elements: `<path>`, `<circle>`, `<rect>`, `<line>`,
//! `<polyline>`, `<polygon>`, `<ellipse>`.

use crate::parser::{parse, Segment};
use iced::widget::canvas::Path;

/// A single sub-path extracted from the SVG body, ready to stroke.
#[derive(Clone)]
pub struct SubPath {
    pub segments: Vec<Segment>,
}

/// Parse the inner XML body of a Lucide SVG into sub-paths.
pub fn parse_body(body: &str) -> Vec<SubPath> {
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = body.as_bytes();
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // find tag name
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j] != b' ' && bytes[j] != b'>'
            && bytes[j] != b'/'
        {
            j += 1;
        }
        let tag = &body[start..j];
        // find end of tag '>'
        let mut end = j;
        while end < bytes.len() && bytes[end] != b'>' {
            end += 1;
        }
        let attrs = &body[j..end];
        let segs = match tag {
            "path" => parse_path(attrs),
            "circle" => parse_circle(attrs),
            "rect" => parse_rect(attrs),
            "line" => parse_line(attrs),
            "polyline" => parse_polyline(attrs, false),
            "polygon" => parse_polyline(attrs, true),
            "ellipse" => parse_ellipse(attrs),
            _ => Vec::new(),
        };
        if !segs.is_empty() {
            out.push(SubPath { segments: segs });
        }
        i = end + 1;
    }
    out
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let key = format!("{}=\"", name);
    let pos = attrs.find(&key)?;
    let start = pos + key.len();
    let end = attrs[start..].find('"')?;
    Some(attrs[start..start + end].to_string())
}

fn attr_f(attrs: &str, name: &str) -> Option<f32> {
    attr(attrs, name).and_then(|s| s.parse().ok())
}

fn attr_f_def(attrs: &str, name: &str, def: f32) -> f32 {
    attr_f(attrs, name).unwrap_or(def)
}

fn parse_path(attrs: &str) -> Vec<Segment> {
    attr(attrs, "d").map(|d| parse(&d)).unwrap_or_default()
}

fn parse_circle(attrs: &str) -> Vec<Segment> {
    let cx = attr_f_def(attrs, "cx", 0.0);
    let cy = attr_f_def(attrs, "cy", 0.0);
    let r = attr_f_def(attrs, "r", 0.0);
    if r <= 0.0 {
        return Vec::new();
    }
    // Approximate circle with 4 cubic Béziers (k = 0.5522847)
    let k = 0.5522847498307936_f32 * r;
    vec![
        Segment::MoveTo { x: cx + r, y: cy },
        Segment::CubicTo { c1x: cx + r, c1y: cy + k, c2x: cx + k, c2y: cy + r, x: cx, y: cy + r },
        Segment::CubicTo { c1x: cx - k, c1y: cy + r, c2x: cx - r, c2y: cy + k, x: cx - r, y: cy },
        Segment::CubicTo { c1x: cx - r, c1y: cy - k, c2x: cx - k, c2y: cy - r, x: cx, y: cy - r },
        Segment::CubicTo { c1x: cx + k, c1y: cy - r, c2x: cx + r, c2y: cy - k, x: cx + r, y: cy },
        Segment::Close,
    ]
}

fn parse_ellipse(attrs: &str) -> Vec<Segment> {
    let cx = attr_f_def(attrs, "cx", 0.0);
    let cy = attr_f_def(attrs, "cy", 0.0);
    let rx = attr_f_def(attrs, "rx", 0.0);
    let ry = attr_f_def(attrs, "ry", 0.0);
    if rx <= 0.0 || ry <= 0.0 {
        return Vec::new();
    }
    let kx = 0.5522847498307936_f32 * rx;
    let ky = 0.5522847498307936_f32 * ry;
    vec![
        Segment::MoveTo { x: cx + rx, y: cy },
        Segment::CubicTo { c1x: cx + rx, c1y: cy + ky, c2x: cx + kx, c2y: cy + ry, x: cx, y: cy + ry },
        Segment::CubicTo { c1x: cx - kx, c1y: cy + ry, c2x: cx - rx, c2y: cy + ky, x: cx - rx, y: cy },
        Segment::CubicTo { c1x: cx - rx, c1y: cy - ky, c2x: cx - kx, c2y: cy - ry, x: cx, y: cy - ry },
        Segment::CubicTo { c1x: cx + kx, c1y: cy - ry, c2x: cx + rx, c2y: cy - ky, x: cx + rx, y: cy },
        Segment::Close,
    ]
}

fn parse_rect(attrs: &str) -> Vec<Segment> {
    let x = attr_f_def(attrs, "x", 0.0);
    let y = attr_f_def(attrs, "y", 0.0);
    let w = attr_f_def(attrs, "width", 0.0);
    let h = attr_f_def(attrs, "height", 0.0);
    let rx = attr_f_def(attrs, "rx", 0.0);
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    if rx <= 0.0 {
        return vec![
            Segment::MoveTo { x, y },
            Segment::LineTo { x: x + w, y },
            Segment::LineTo { x: x + w, y: y + h },
            Segment::LineTo { x, y: y + h },
            Segment::Close,
        ];
    }
    let r = rx.min(w / 2.0).min(h / 2.0);
    vec![
        Segment::MoveTo { x: x + r, y },
        Segment::LineTo { x: x + w - r, y },
        Segment::QuadTo { cx: x + w, cy: y, x: x + w, y: y + r },
        Segment::LineTo { x: x + w, y: y + h - r },
        Segment::QuadTo { cx: x + w, cy: y + h, x: x + w - r, y: y + h },
        Segment::LineTo { x: x + r, y: y + h },
        Segment::QuadTo { cx: x, cy: y + h, x, y: y + h - r },
        Segment::LineTo { x, y: y + r },
        Segment::QuadTo { cx: x, cy: y, x: x + r, y },
        Segment::Close,
    ]
}

fn parse_line(attrs: &str) -> Vec<Segment> {
    let x1 = attr_f_def(attrs, "x1", 0.0);
    let y1 = attr_f_def(attrs, "y1", 0.0);
    let x2 = attr_f_def(attrs, "x2", 0.0);
    let y2 = attr_f_def(attrs, "y2", 0.0);
    vec![
        Segment::MoveTo { x: x1, y: y1 },
        Segment::LineTo { x: x2, y: y2 },
    ]
}

fn parse_polyline(attrs: &str, closed: bool) -> Vec<Segment> {
    let pts = match attr(attrs, "points") {
        Some(p) => p,
        None => return Vec::new(),
    };
    let nums: Vec<f32> = pts
        .split(|c: char| c == ' ' || c == ',' || c == '\n')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let mut segs = Vec::new();
    let mut iter = nums.iter();
    if let (Some(&x), Some(&y)) = (iter.next(), iter.next()) {
        segs.push(Segment::MoveTo { x, y });
    }
    for chunk in iter.collect::<Vec<_>>().chunks(2) {
        if chunk.len() == 2 {
            segs.push(Segment::LineTo { x: *chunk[0], y: *chunk[1] });
        }
    }
    if closed {
        segs.push(Segment::Close);
    }
    segs
}

/// Build an iced [`Path`] from a [`SubPath`].
pub fn to_path(sub: &SubPath) -> Path {
    Path::new(|b| {
        crate::parser::build(b, &sub.segments);
    })
}

/// Build all sub-paths and stroke them onto the frame.
pub fn stroke_all(
    frame: &mut iced::widget::canvas::Frame,
    sub_paths: &[SubPath],
    stroke: iced::widget::canvas::Stroke,
) {
    for sub in sub_paths {
        let path = to_path(sub);
        frame.stroke(&path, stroke);
    }
}