// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Minimal SVG path-data parser.
//!
//! Supports the subset of commands used by Lucide icons:
//! `M m L l H h V v C c S s Q q T t A a Z z` plus implicit `L`/`l`
//! after `M`/`m`.
//! Output is a sequence of [`Segment`]s that can be replayed onto an
//! [`iced::widget::canvas::path::Builder`].

use iced::widget::canvas::path::Builder;

/// A single parsed path command, in absolute coordinates.
#[derive(Clone, Copy, Debug)]
pub enum Segment {
    MoveTo { x: f32, y: f32 },
    LineTo { x: f32, y: f32 },
    Close,
    CubicTo { c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32 },
    QuadTo { cx: f32, cy: f32, x: f32, y: f32 },
    ArcTo {
        rx: f32,
        ry: f32,
        x_rotation: f32,
        large_arc: bool,
        sweep: bool,
        x: f32,
        y: f32,
    },
}

/// Parse SVG path `d` into absolute-coordinate [`Segment`]s.
pub fn parse(d: &str) -> Vec<Segment> {
    let mut p = Parser::new(d);
    let mut out = Vec::new();
    let mut cur = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    let mut last_cmd = b'\0';
    // Reflection state for `S`/`T`: the trailing control point of the
    // previous cubic (`C`/`S`) or quadratic (`Q`/`T`) command.
    let mut last_ctrl = (0.0f32, 0.0f32);
    let mut last_ctrl_kind = b'\0'; // b'C' or b'Q' when valid

    while !p.done() {
        // Track position to detect stalls — if a full iteration
        // consumes no input we break to avoid an infinite loop.
        let pos_before = p.pos;

        let cmd = if p.peek_cmd() {
            p.next_cmd()
        } else {
            last_cmd
        };
        if cmd == 0 {
            break;
        }
        last_cmd = cmd;
        let rel = cmd.is_ascii_lowercase();
        let u = cmd.to_ascii_uppercase();
        match u {
            b'M' => {
                let (x, y) = p.point(cur, rel);
                out.push(Segment::MoveTo { x, y });
                cur = (x, y);
                start = cur;
                // After M, subsequent coordinate pairs are implicit L.
                last_cmd = if rel { b'l' } else { b'L' };
                last_ctrl_kind = b'\0';
            }
            b'L' => {
                let (x, y) = p.point(cur, rel);
                out.push(Segment::LineTo { x, y });
                cur = (x, y);
                last_ctrl_kind = b'\0';
            }
            b'H' => {
                let x = p.number();
                let x = if rel { cur.0 + x } else { x };
                out.push(Segment::LineTo { x, y: cur.1 });
                cur = (x, cur.1);
                last_ctrl_kind = b'\0';
            }
            b'V' => {
                let y = p.number();
                let y = if rel { cur.1 + y } else { y };
                out.push(Segment::LineTo { x: cur.0, y });
                cur = (cur.0, y);
                last_ctrl_kind = b'\0';
            }
            b'C' => {
                let (c1x, c1y) = p.point(cur, rel);
                let (c2x, c2y) = p.point(cur, rel);
                let (x, y) = p.point(cur, rel);
                out.push(Segment::CubicTo { c1x, c1y, c2x, c2y, x, y });
                cur = (x, y);
                last_ctrl = (c2x, c2y);
                last_ctrl_kind = b'C';
            }
            b'S' => {
                // Smooth cubic: first control point is the reflection of
                // the previous cubic's second control point around the
                // current point (or the current point itself if the
                // previous command was not C/c/S/s).
                let (c1x, c1y) = if last_ctrl_kind == b'C' {
                    (2.0 * cur.0 - last_ctrl.0, 2.0 * cur.1 - last_ctrl.1)
                } else {
                    cur
                };
                let (c2x, c2y) = p.point(cur, rel);
                let (x, y) = p.point(cur, rel);
                out.push(Segment::CubicTo { c1x, c1y, c2x, c2y, x, y });
                cur = (x, y);
                last_ctrl = (c2x, c2y);
                last_ctrl_kind = b'C';
            }
            b'Q' => {
                let (cx, cy) = p.point(cur, rel);
                let (x, y) = p.point(cur, rel);
                out.push(Segment::QuadTo { cx, cy, x, y });
                cur = (x, y);
                last_ctrl = (cx, cy);
                last_ctrl_kind = b'Q';
            }
            b'T' => {
                // Smooth quadratic: control point is the reflection of
                // the previous quadratic's control point (or the current
                // point if the previous command was not Q/q/T/t).
                let (cx, cy) = if last_ctrl_kind == b'Q' {
                    (2.0 * cur.0 - last_ctrl.0, 2.0 * cur.1 - last_ctrl.1)
                } else {
                    cur
                };
                let (x, y) = p.point(cur, rel);
                out.push(Segment::QuadTo { cx, cy, x, y });
                cur = (x, y);
                last_ctrl = (cx, cy);
                last_ctrl_kind = b'Q';
            }
            b'A' => {
                let rx = p.number();
                let ry = p.number();
                let x_rotation = p.number();
                let large_arc = p.flag();
                let sweep = p.flag();
                let (x, y) = p.point(cur, rel);
                out.push(Segment::ArcTo {
                    rx,
                    ry,
                    x_rotation,
                    large_arc,
                    sweep,
                    x,
                    y,
                });
                cur = (x, y);
                last_ctrl_kind = b'\0';
            }
            b'Z' => {
                out.push(Segment::Close);
                cur = start;
                last_ctrl_kind = b'\0';
            }
            _ => break,
        }

        // Stall guard: if the iteration consumed no input, advance past
        // the offending byte (or break at EOF) to avoid an infinite loop.
        if p.pos == pos_before {
            if p.done() {
                break;
            }
            p.pos += 1;
        }
    }
    out
}

/// Replay parsed [`Segment`]s onto a canvas path [`Builder`].
pub fn build(builder: &mut Builder, segments: &[Segment]) {
    // Track the current pen position so elliptic arcs know their start point.
    let mut cur = iced::Point::new(0.0, 0.0);
    let mut start = iced::Point::new(0.0, 0.0);
    for s in segments {
        match *s {
            Segment::MoveTo { x, y } => {
                builder.move_to(iced::Point::new(x, y));
                cur = iced::Point::new(x, y);
                start = cur;
            }
            Segment::LineTo { x, y } => {
                builder.line_to(iced::Point::new(x, y));
                cur = iced::Point::new(x, y);
            }
            Segment::Close => {
                builder.close();
                cur = start;
            }
            Segment::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                builder.bezier_curve_to(
                    iced::Point::new(c1x, c1y),
                    iced::Point::new(c2x, c2y),
                    iced::Point::new(x, y),
                );
                cur = iced::Point::new(x, y);
            }
            Segment::QuadTo { cx, cy, x, y } => {
                builder.quadratic_curve_to(
                    iced::Point::new(cx, cy),
                    iced::Point::new(x, y),
                );
                cur = iced::Point::new(x, y);
            }
            Segment::ArcTo {
                rx,
                ry,
                x_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                // Faithful elliptic-arc → cubic Bézier conversion (SVG spec
                // endpoint-to-center parameterization). The previous
                // placeholder ignored large_arc/sweep and produced wrong
                // geometry for any icon containing an `A` command (e.g. the
                // Bell glyph's dome and clapper).
                append_arc(
                    builder,
                    cur,
                    rx,
                    ry,
                    x_rotation,
                    large_arc,
                    sweep,
                    iced::Point::new(x, y),
                );
                cur = iced::Point::new(x, y);
            }
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self { bytes: s.as_bytes(), pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn skip_ws_sep(&mut self) {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek_cmd(&self) -> bool {
        if self.pos >= self.bytes.len() {
            return false;
        }
        let b = self.bytes[self.pos];
        matches!(
            b.to_ascii_uppercase(),
            b'M' | b'L' | b'H' | b'V' | b'C' | b'S' | b'Q' | b'T' | b'A' | b'Z'
        )
    }

    fn next_cmd(&mut self) -> u8 {
        self.skip_ws_sep();
        if self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            b
        } else {
            0
        }
    }

    fn number(&mut self) -> f32 {
        self.skip_ws_sep();
        let start = self.pos;
        // SVG path numbers can be written without separators:
        //   "1.5-2.9"  → 1.5 then -2.9
        //   "1.5.5"    → 1.5 then .5
        //   ".5e-3.2"  → 0.0005 then 2
        // A sign or a leading dot that appears mid-stream starts a new
        // number, so we stop *before* a sign (unless it follows e/E)
        // and before a second dot.
        let mut seen_dot = false;
        let mut seen_exp = false;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            match b {
                b'0'..=b'9' => {
                    self.pos += 1;
                }
                b'.' => {
                    // A second dot starts a new number ("1.5.5" → 1.5 | .5)
                    if seen_dot {
                        break;
                    }
                    seen_dot = true;
                    self.pos += 1;
                }
                b'e' | b'E' => {
                    // A second exponent is invalid → new number
                    if seen_exp {
                        break;
                    }
                    seen_exp = true;
                    self.pos += 1;
                }
                b'-' | b'+' => {
                    // Sign is part of this number only right after e/E;
                    // otherwise it begins a new number.
                    if self.pos > start {
                        let prev = self.bytes[self.pos - 1];
                        if prev != b'e' && prev != b'E' {
                            break;
                        }
                    }
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("0");
        s.parse::<f32>().unwrap_or(0.0)
    }

    fn flag(&mut self) -> bool {
        self.skip_ws_sep();
        if self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            b == b'1'
        } else {
            false
        }
    }

    fn point(&mut self, cur: (f32, f32), rel: bool) -> (f32, f32) {
        let x = self.number();
        let y = self.number();
        if rel {
            (cur.0 + x, cur.1 + y)
        } else {
            (x, y)
        }
    }
}

/// Convert an SVG elliptical arc into cubic Bézier segments and append them
/// to `builder`. Implements the W3C SVG 1.1 implementation notes
/// (Appendix F.6.5) endpoint-to-center parameterization, followed by the
/// standard Bézier approximation of circular/elliptic arcs.
#[allow(clippy::too_many_arguments)]
pub fn append_arc(
    builder: &mut Builder,
    from: iced::Point,
    rx: f32,
    ry: f32,
    x_axis_rotation: f32,
    large_arc: bool,
    sweep: bool,
    to: iced::Point,
) {
    // Degenerate arc → straight line.
    if rx <= 0.0 || ry <= 0.0 {
        builder.line_to(to);
        return;
    }
    // Coincident endpoints → nothing to draw.
    if (from.x - to.x).abs() < 1e-6 && (from.y - to.y).abs() < 1e-6 {
        return;
    }

    let phi = x_axis_rotation.to_radians();
    let (cos_phi, sin_phi) = (phi.cos(), phi.sin());

    // Step 1: compute (x1', y1') — midpoint in the rotated frame.
    let dx = (from.x - to.x) / 2.0;
    let dy = (from.y - to.y) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;

    // Step 2: ensure radii are large enough; scale up if not.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    let (rx, ry) = if lambda > 1.0 {
        let s = lambda.sqrt();
        (rx * s, ry * s)
    } else {
        (rx, ry)
    };

    // Step 3: compute centre (cx', cy') in the rotated frame.
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let num = (rx * rx * ry * ry) - (rx * rx * y1p * y1p) - (ry * ry * x1p * x1p);
    let num = num.max(0.0);
    let den = (rx * rx * y1p * y1p) + (ry * ry * x1p * x1p);
    let coef = sign * (num / den).max(0.0).sqrt();
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * (-ry * x1p / rx);

    // Step 4: compute centre (cx, cy) back in user space.
    let cx = cos_phi * cxp - sin_phi * cyp + (from.x + to.x) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (from.y + to.y) / 2.0;

    // Step 5: compute theta1 and delta_theta using atan2 (robust, no
    // quadrant ambiguity like the acos approach).
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = vec_angle(ux, uy);
    let mut dtheta = vec_angle(vx, vy) - theta1;

    // Adjust delta_theta to the correct sweep direction & arc size.
    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * std::f32::consts::PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * std::f32::consts::PI;
    }

    // Step 6: split into ≤90° segments and emit cubic Béziers.
    let n_segs = ((dtheta.abs() / (std::f32::consts::PI / 2.0)).ceil() as i32).max(1);
    let d = dtheta / n_segs as f32;
    // Distance from arc endpoints to Bézier control points.
    let t = (4.0 / 3.0) * (d / 4.0).tan();

    for i in 0..n_segs {
        let a1 = theta1 + d * i as f32;
        let a2 = theta1 + d * (i + 1) as f32;
        let (p1x, p1y) = arc_point(cx, cy, rx, ry, cos_phi, sin_phi, a1);
        let (p2x, p2y) = arc_point(cx, cy, rx, ry, cos_phi, sin_phi, a2);
        // Tangent vector at angle a (derivative of the parametric ellipse).
        let (t1x, t1y) = arc_tangent(rx, ry, cos_phi, sin_phi, a1);
        let (t2x, t2y) = arc_tangent(rx, ry, cos_phi, sin_phi, a2);
        let c1 = (p1x + t * t1x, p1y + t * t1y);
        let c2 = (p2x - t * t2x, p2y - t * t2y);
        builder.bezier_curve_to(
            iced::Point::new(c1.0, c1.1),
            iced::Point::new(c2.0, c2.1),
            iced::Point::new(p2x, p2y),
        );
    }
}

/// `atan2`-based angle of vector (x, y) — no quadrant ambiguity.
fn vec_angle(x: f32, y: f32) -> f32 {
    y.atan2(x)
}

/// Point on the rotated ellipse at parametric angle `a`.
fn arc_point(
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    cos_phi: f32,
    sin_phi: f32,
    a: f32,
) -> (f32, f32) {
    let px = cx + rx * (cos_phi * a.cos() - sin_phi * a.sin());
    let py = cy + ry * (sin_phi * a.cos() + cos_phi * a.sin());
    (px, py)
}

/// Tangent vector (derivative) of the rotated ellipse at parametric angle `a`.
fn arc_tangent(
    rx: f32,
    ry: f32,
    cos_phi: f32,
    sin_phi: f32,
    a: f32,
) -> (f32, f32) {
    let tx = -rx * (cos_phi * a.sin() + sin_phi * a.cos());
    let ty = -ry * (sin_phi * a.sin() - cos_phi * a.cos());
    (tx, ty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn bell_dome_smooth_cubic_closes_back_to_start() {
        // Lucide `bell` first subpath — the trailing `s3-2 3-9` is a
        // smooth cubic that must end back at the subpath start (6, 8).
        let segs = parse("M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9");
        // M, arc, cubic, H, s → exactly 5 segments, no garbage H-repeats.
        assert_eq!(segs.len(), 5, "segments: {segs:#?}");
        match segs[4] {
            Segment::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                // Previous command was H (no cubic control point), so
                // the first control point equals the current point (3,17).
                assert!(approx(c1x, 3.0) && approx(c1y, 17.0));
                assert!(approx(c2x, 6.0) && approx(c2y, 15.0));
                assert!(approx(x, 6.0) && approx(y, 8.0));
            }
            other => panic!("expected CubicTo, got {other:?}"),
        }
    }

    #[test]
    fn bell_clapper_arc_parses() {
        let segs = parse("M10.3 21a1.94 1.94 0 0 0 3.4 0");
        assert_eq!(segs.len(), 2);
        match segs[1] {
            Segment::ArcTo { rx, ry, large_arc, sweep, x, y, .. } => {
                assert!(approx(rx, 1.94) && approx(ry, 1.94));
                assert!(!large_arc && !sweep);
                assert!(approx(x, 13.7) && approx(y, 21.0));
            }
            other => panic!("expected ArcTo, got {other:?}"),
        }
    }

    #[test]
    fn smooth_cubic_reflects_previous_control_point() {
        // After a real cubic, `S` reflects its c2 around the current point.
        let segs = parse("M0 0C1 1 2 1 3 0S5 -1 6 0");
        assert_eq!(segs.len(), 3);
        match segs[2] {
            Segment::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                // reflection of (2,1) around (3,0) = (4,-1)
                assert!(approx(c1x, 4.0) && approx(c1y, -1.0));
                assert!(approx(c2x, 5.0) && approx(c2y, -1.0));
                assert!(approx(x, 6.0) && approx(y, 0.0));
            }
            other => panic!("expected CubicTo, got {other:?}"),
        }
    }

    #[test]
    fn smooth_quad_reflects_previous_control_point() {
        let segs = parse("M0 0Q2 2 4 0T8 0");
        assert_eq!(segs.len(), 3);
        match segs[2] {
            Segment::QuadTo { cx, cy, x, y } => {
                // reflection of (2,2) around (4,0) = (6,-2)
                assert!(approx(cx, 6.0) && approx(cy, -2.0));
                assert!(approx(x, 8.0) && approx(y, 0.0));
            }
            other => panic!("expected QuadTo, got {other:?}"),
        }
    }
}