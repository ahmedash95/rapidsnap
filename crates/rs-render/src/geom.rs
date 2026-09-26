//! Vector geometry helpers: tiny-skia conversions, rounded rects, smoothed
//! polylines, arrow construction, callout bubbles, distance queries.

use rs_core::{ArrowAnnotation, ArrowHead, Color, Point, Rect};
use tiny_skia as sk;

pub(crate) fn sk_color(c: Color) -> sk::Color {
    sk::Color::from_rgba8(c.r, c.g, c.b, c.a)
}

pub(crate) fn sk_stops(stops: &[rs_core::GradientStop]) -> Vec<sk::GradientStop> {
    stops.iter().map(|s| sk::GradientStop::new(s.offset.clamp(0.0, 1.0), sk_color(s.color))).collect()
}

pub(crate) fn to_sk_rect(r: Rect) -> sk::Rect {
    sk::Rect::from_xywh(r.x, r.y, r.width.max(0.001), r.height.max(0.001))
        .unwrap_or_else(|| sk::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("valid"))
}

/// Rounded rectangle path (radius clamped to half the smaller side).
pub(crate) fn rounded_rect_path(r: Rect, radius: f32) -> Option<sk::Path> {
    if !(r.width > 0.0 && r.height > 0.0) {
        return None;
    }
    let radius = radius.min(r.width / 2.0).min(r.height / 2.0).max(0.0);
    if radius <= 0.0 {
        return Some(sk::PathBuilder::from_rect(sk::Rect::from_xywh(r.x, r.y, r.width, r.height)?));
    }
    let k = 0.552_284_8 * radius;
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let mut pb = sk::PathBuilder::new();
    pb.move_to(x0 + radius, y0);
    pb.line_to(x1 - radius, y0);
    pb.cubic_to(x1 - radius + k, y0, x1, y0 + radius - k, x1, y0 + radius);
    pb.line_to(x1, y1 - radius);
    pb.cubic_to(x1, y1 - radius + k, x1 - radius + k, y1, x1 - radius, y1);
    pb.line_to(x0 + radius, y1);
    pb.cubic_to(x0 + radius - k, y1, x0, y1 - radius + k, x0, y1 - radius);
    pb.line_to(x0, y0 + radius);
    pb.cubic_to(x0, y0 + radius - k, x0 + radius - k, y0, x0 + radius, y0);
    pb.close();
    pb.finish()
}

pub(crate) fn circle_path(c: Point, r: f32) -> Option<sk::Path> {
    sk::PathBuilder::from_circle(c.x, c.y, r.max(0.01))
}

pub(crate) fn rounded_mask(w: u32, h: u32, r: Rect, radius: f32) -> Option<sk::Mask> {
    let mut m = sk::Mask::new(w, h)?;
    let path = rounded_rect_path(r, radius)?;
    m.fill_path(&path, sk::FillRule::Winding, true, sk::Transform::identity());
    Some(m)
}

/// tiny-skia stroke from an rs-core stroke (min dash/gap 1 px).
pub(crate) fn sk_stroke(s: &rs_core::Stroke) -> sk::Stroke {
    let mut st = sk::Stroke { width: s.width.max(0.1), line_join: sk::LineJoin::Round, ..Default::default() };
    st.line_cap = match s.cap {
        rs_core::LineCap::Butt => sk::LineCap::Butt,
        rs_core::LineCap::Round => sk::LineCap::Round,
        rs_core::LineCap::Square => sk::LineCap::Square,
    };
    if let Some([on, off]) = s.dash.dash_array(s.width) {
        if s.dash == rs_core::DashStyle::Dotted {
            st.line_cap = sk::LineCap::Round;
            st.dash = sk::StrokeDash::new(vec![0.01, off.max(1.0) + s.width.max(0.1)], 0.0);
        } else {
            // Round/square caps extend each dash by w/2 on both ends; compensate.
            let ext = if st.line_cap == sk::LineCap::Butt { 0.0 } else { s.width };
            st.dash = sk::StrokeDash::new(vec![(on - ext).max(1.0), (off + ext).max(1.0)], 0.0);
        }
    }
    st
}

pub(crate) fn paint(c: Color) -> sk::Paint<'static> {
    let mut p = sk::Paint::default();
    p.set_color(sk_color(c));
    p.anti_alias = true;
    p
}

fn norm(v: Point) -> Point {
    let l = (v.x * v.x + v.y * v.y).sqrt();
    if l < 1e-6 { Point::new(1.0, 0.0) } else { Point::new(v.x / l, v.y / l) }
}

fn perp(v: Point) -> Point {
    Point::new(-v.y, v.x)
}

/// Remove consecutive near-duplicate points (< `eps` apart).
pub(crate) fn dedup_points(pts: &[Point], eps: f32) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(pts.len());
    for &p in pts {
        if !p.x.is_finite() || !p.y.is_finite() {
            continue;
        }
        match out.last() {
            Some(l) if l.distance(p) < eps => {}
            _ => out.push(p),
        }
    }
    if out.len() == 1 && pts.len() > 1
        && let Some(&last) = pts.last()
            && last.distance(out[0]) > 0.0 {
                out.push(last);
            }
    out
}

/// Smooth open path through `pts` (Catmull-Rom → cubic Béziers).
pub(crate) fn smooth_path(pts: &[Point]) -> Option<sk::Path> {
    let pts = dedup_points(pts, 0.75);
    let mut pb = sk::PathBuilder::new();
    match pts.len() {
        0 => return None,
        1 => {
            // A dot: zero-length segment; round cap renders it.
            pb.move_to(pts[0].x, pts[0].y);
            pb.line_to(pts[0].x + 0.01, pts[0].y);
        }
        2 => {
            pb.move_to(pts[0].x, pts[0].y);
            pb.line_to(pts[1].x, pts[1].y);
        }
        n => {
            pb.move_to(pts[0].x, pts[0].y);
            for i in 0..n - 1 {
                let p0 = pts[i.saturating_sub(1)];
                let p1 = pts[i];
                let p2 = pts[i + 1];
                let p3 = pts[(i + 2).min(n - 1)];
                let c1 = p1 + (p2 - p0) * (1.0 / 6.0);
                let c2 = p2 - (p3 - p1) * (1.0 / 6.0);
                pb.cubic_to(c1.x, c1.y, c2.x, c2.y, p2.x, p2.y);
            }
        }
    }
    pb.finish()
}

/// Polyline approximation of the smoothed path (for hit-testing).
pub(crate) fn smooth_polyline(pts: &[Point]) -> Vec<Point> {
    let pts = dedup_points(pts, 0.75);
    let n = pts.len();
    if n < 3 {
        return pts;
    }
    let mut out = vec![pts[0]];
    for i in 0..n - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = pts[(i + 2).min(n - 1)];
        let c1 = p1 + (p2 - p0) * (1.0 / 6.0);
        let c2 = p2 - (p3 - p1) * (1.0 / 6.0);
        for k in 1..=4 {
            let t = k as f32 / 4.0;
            let u = 1.0 - t;
            out.push(p1 * (u * u * u) + c1 * (3.0 * u * u * t) + c2 * (3.0 * u * t * t) + p2 * (t * t * t));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Arrow
// ---------------------------------------------------------------------------

/// Point on the arrow's centerline at parameter t.
fn arrow_at(a: &ArrowAnnotation, t: f32) -> Point {
    match a.control {
        Some(c) => {
            let u = 1.0 - t;
            a.start * (u * u) + c * (2.0 * u * t) + a.end * (t * t)
        }
        None => a.start.lerp(a.end, t),
    }
}

fn arrow_deriv(a: &ArrowAnnotation, t: f32) -> Point {
    match a.control {
        Some(c) => ((c - a.start) * (1.0 - t) + (a.end - c) * t) * 2.0,
        None => a.end - a.start,
    }
}

fn arrow_len(a: &ArrowAnnotation) -> f32 {
    let mut len = 0.0;
    let mut prev = a.start;
    let n = if a.control.is_some() { 32 } else { 1 };
    for i in 1..=n {
        let p = arrow_at(a, i as f32 / n as f32);
        len += prev.distance(p);
        prev = p;
    }
    len
}

/// Parameter t such that |B(t) − anchor| ≈ d, searching from the anchor end.
fn t_at_distance(a: &ArrowAnnotation, from_end: bool, d: f32) -> f32 {
    let anchor = if from_end { a.end } else { a.start };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        let dist = arrow_at(a, mid).distance(anchor);
        // from_end: distance decreases as t → 1.
        let too_close = dist < d;
        if from_end == too_close {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (lo + hi) / 2.0
}

/// Constructed arrow geometry (content space).
pub(crate) struct ArrowGeom {
    /// Stroked shaft (None if tapered or degenerate).
    pub shaft: Option<sk::Path>,
    /// Filled tapered shaft polygon.
    pub taper: Option<sk::Path>,
    /// Filled heads (triangles / dots).
    pub fills: Vec<sk::Path>,
    /// Stroked heads (open chevrons).
    pub strokes: Vec<sk::Path>,
    /// Sample points of the whole centerline (hit-testing).
    pub centerline: Vec<Point>,
}

pub(crate) fn arrow_geometry(a: &ArrowAnnotation) -> ArrowGeom {
    let w = a.stroke.width.max(0.1);
    let total = arrow_len(a);
    let heads = if a.double_headed { 2.0 } else { 1.0 };
    let mut hl = a.head_length();
    if a.head == ArrowHead::None {
        hl = 0.0;
    }
    // Shrink heads on very short arrows.
    let max_hl = total * if a.double_headed { 0.45 } else { 0.7 };
    let hl = hl.min(max_hl).max(0.0);
    let hw = if a.head_length() > 0.0 { a.head_width() * hl / a.head_length() } else { 0.0 };
    let _ = heads;

    let mut fills = Vec::new();
    let mut strokes = Vec::new();
    // Where the shaft ends (distance from the tip) for each head style.
    let trim = |style: ArrowHead| -> f32 {
        match style {
            ArrowHead::FilledTriangle => hl * 0.8,
            ArrowHead::Open => (w * 0.5).min(hl * 0.5),
            ArrowHead::Round | ArrowHead::None => 0.0,
        }
    };

    let mut build_head = |from_end: bool| {
        if hl <= 0.0 {
            return;
        }
        let tip = if from_end { a.end } else { a.start };
        let tb = t_at_distance(a, from_end, hl);
        let base = arrow_at(a, tb);
        let dir = norm(tip - base);
        let n = perp(dir);
        let b = tip - dir * hl;
        let l = b + n * (hw / 2.0);
        let r = b - n * (hw / 2.0);
        match a.head {
            ArrowHead::FilledTriangle => {
                let mut pb = sk::PathBuilder::new();
                pb.move_to(tip.x, tip.y);
                pb.line_to(l.x, l.y);
                pb.line_to(r.x, r.y);
                pb.close();
                if let Some(p) = pb.finish() {
                    fills.push(p);
                }
            }
            ArrowHead::Open => {
                let mut pb = sk::PathBuilder::new();
                pb.move_to(l.x, l.y);
                pb.line_to(tip.x, tip.y);
                pb.line_to(r.x, r.y);
                if let Some(p) = pb.finish() {
                    strokes.push(p);
                }
            }
            ArrowHead::Round => {
                let rad = (hw * 0.45).max(w * 1.1);
                if let Some(p) = circle_path(tip, rad) {
                    fills.push(p);
                }
            }
            ArrowHead::None => {}
        }
    };
    build_head(true);
    if a.double_headed {
        build_head(false);
    }

    let t1 = if trim(a.head) > 0.0 && hl > 0.0 { t_at_distance(a, true, trim(a.head)) } else { 1.0 };
    let t0 = if a.double_headed && trim(a.head) > 0.0 && hl > 0.0 { t_at_distance(a, false, trim(a.head)) } else { 0.0 };
    let (t0, t1) = if t1 > t0 { (t0, t1) } else { (0.5, 0.5) };

    let mut centerline = Vec::new();
    let n = if a.control.is_some() { 32 } else { 1 };
    for i in 0..=n {
        centerline.push(arrow_at(a, i as f32 / n as f32));
    }

    let (mut shaft, mut taper) = (None, None);
    if t1 - t0 > 1e-4 {
        if a.tapered {
            let samples = if a.control.is_some() { 40 } else { 12 };
            let mut left = Vec::with_capacity(samples + 1);
            let mut right = Vec::with_capacity(samples + 1);
            for i in 0..=samples {
                let s = i as f32 / samples as f32;
                let t = t0 + (t1 - t0) * s;
                let p = arrow_at(a, t);
                let nrm = perp(norm(arrow_deriv(a, t)));
                let f = if a.double_headed { 0.35 + 0.65 * (2.0 * s - 1.0).abs() } else { 0.12 + 0.88 * s };
                let half = w * f / 2.0;
                left.push(p + nrm * half);
                right.push(p - nrm * half);
            }
            let mut pb = sk::PathBuilder::new();
            pb.move_to(left[0].x, left[0].y);
            for p in &left[1..] {
                pb.line_to(p.x, p.y);
            }
            for p in right.iter().rev() {
                pb.line_to(p.x, p.y);
            }
            pb.close();
            taper = pb.finish();
        } else {
            let p0 = arrow_at(a, t0);
            let p1 = arrow_at(a, t1);
            let mut pb = sk::PathBuilder::new();
            pb.move_to(p0.x, p0.y);
            match a.control {
                Some(_) => {
                    // Exact sub-curve [t0, t1] of the quadratic.
                    let c = p0 + arrow_deriv(a, t0) * ((t1 - t0) / 2.0);
                    pb.quad_to(c.x, c.y, p1.x, p1.y);
                }
                None => pb.line_to(p1.x, p1.y),
            }
            shaft = pb.finish();
        }
    }
    ArrowGeom { shaft, taper, fills, strokes, centerline }
}

// ---------------------------------------------------------------------------
// Callout bubble
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// Tail triangle (base a, tip, base b) for a callout, if the tail is outside.
pub(crate) fn callout_tail(rect: Rect, radius: f32, tail: Point, base_max: f32) -> Option<(Point, Point, Point, u8)> {
    let over = [
        (rect.y - tail.y, Edge::Top),
        (tail.x - rect.right(), Edge::Right),
        (tail.y - rect.bottom(), Edge::Bottom),
        (rect.x - tail.x, Edge::Left),
    ];
    let (amt, edge) = over.iter().copied().max_by(|a, b| a.0.total_cmp(&b.0))?;
    if amt <= 0.0 {
        return None;
    }
    let r = radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    let (lo, hi, along) = match edge {
        Edge::Top | Edge::Bottom => (rect.x + r, rect.right() - r, tail.x),
        Edge::Left | Edge::Right => (rect.y + r, rect.bottom() - r, tail.y),
    };
    let span = (hi - lo).max(0.0);
    let edge_len = match edge {
        Edge::Top | Edge::Bottom => rect.width,
        _ => rect.height,
    };
    let half = (base_max.min(edge_len / 3.0) / 2.0).min(span / 2.0).max(0.5);
    let c = if span >= 2.0 * half { along.clamp(lo + half, hi - half) } else { (lo + hi) / 2.0 };
    let (a, b) = match edge {
        Edge::Top => (Point::new(c - half, rect.y), Point::new(c + half, rect.y)),
        Edge::Bottom => (Point::new(c + half, rect.bottom()), Point::new(c - half, rect.bottom())),
        Edge::Right => (Point::new(rect.right(), c - half), Point::new(rect.right(), c + half)),
        Edge::Left => (Point::new(rect.x, c + half), Point::new(rect.x, c - half)),
    };
    let e = match edge {
        Edge::Top => 0,
        Edge::Right => 1,
        Edge::Bottom => 2,
        Edge::Left => 3,
    };
    Some((a, tail, b, e))
}

/// Single closed outline: rounded rect with the tail spliced into one edge
/// (so fill and stroke have no seams).
pub(crate) fn callout_path(rect: Rect, radius: f32, tail: Point, base_max: f32) -> Option<sk::Path> {
    if !(rect.width > 0.0 && rect.height > 0.0) {
        return None;
    }
    let t = callout_tail(rect, radius, tail, base_max);
    let r = radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    let k = 0.552_284_8 * r;
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
    let mut pb = sk::PathBuilder::new();
    let splice = |pb: &mut sk::PathBuilder, e: u8| {
        if let Some((a, tip, b, te)) = t
            && te == e {
                pb.line_to(a.x, a.y);
                pb.line_to(tip.x, tip.y);
                pb.line_to(b.x, b.y);
            }
    };
    pb.move_to(x0 + r, y0);
    splice(&mut pb, 0);
    pb.line_to(x1 - r, y0);
    if r > 0.0 {
        pb.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    }
    splice(&mut pb, 1);
    pb.line_to(x1, y1 - r);
    if r > 0.0 {
        pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    }
    splice(&mut pb, 2);
    pb.line_to(x0 + r, y1);
    if r > 0.0 {
        pb.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    }
    splice(&mut pb, 3);
    pb.line_to(x0, y0 + r);
    if r > 0.0 {
        pb.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    }
    pb.close();
    pb.finish()
}

// ---------------------------------------------------------------------------
// Distance queries (hit-testing)
// ---------------------------------------------------------------------------

pub(crate) fn dist_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let ab = b - a;
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-9 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

pub(crate) fn dist_to_polyline(p: Point, pts: &[Point]) -> f32 {
    match pts.len() {
        0 => f32::INFINITY,
        1 => p.distance(pts[0]),
        _ => pts.windows(2).map(|w| dist_to_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min),
    }
}

pub(crate) fn point_in_triangle(p: Point, a: Point, b: Point, c: Point) -> bool {
    let s = |p1: Point, p2: Point, p3: Point| (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y);
    let d1 = s(p, a, b);
    let d2 = s(p, b, c);
    let d3 = s(p, c, a);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::Stroke;

    fn arrow(control: Option<Point>) -> ArrowAnnotation {
        ArrowAnnotation {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
            control,
            stroke: Stroke::default(),
            head: ArrowHead::FilledTriangle,
            double_headed: false,
            tapered: false,
            head_scale: 1.0,
        }
    }

    #[test]
    fn arrow_head_points_at_tip() {
        let a = arrow(None);
        let g = arrow_geometry(&a);
        assert_eq!(g.fills.len(), 1);
        let b = g.fills[0].bounds();
        assert!((b.right() - 100.0).abs() < 0.01, "{b:?}");
        assert!((b.left() - (100.0 - a.head_length())).abs() < 0.01);
        assert!((b.height() - a.head_width()).abs() < 0.01);
        let shaft = g.shaft.unwrap().bounds();
        assert!(shaft.right() < 100.0 && shaft.right() > 100.0 - a.head_length());
    }

    #[test]
    fn curved_arrow_trims_and_heads_follow_curve() {
        let a = arrow(Some(Point::new(50.0, -60.0)));
        let g = arrow_geometry(&a);
        let head = g.fills[0].bounds();
        // Head comes from above-left (curve arrives going down-right).
        assert!(head.top() < 0.0 && head.bottom() > -1.0);
        assert!(g.centerline.len() > 10);
    }

    #[test]
    fn callout_tail_on_bottom_edge() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let (a, tip, b, e) = callout_tail(r, 10.0, Point::new(30.0, 90.0), 24.0).unwrap();
        assert_eq!(e, 2);
        assert_eq!(tip, Point::new(30.0, 90.0));
        assert_eq!(a.y, 50.0);
        assert!((a.x - b.x).abs() <= 24.0 + 1e-3);
        assert!(callout_tail(r, 10.0, Point::new(30.0, 20.0), 24.0).is_none());
    }
}
