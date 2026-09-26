//! Pure geometry helpers for the editor: distances, constraints (⇧ / ⌥),
//! rectangle resizing by handle. All values in content-space px unless noted.

use rs_core::{Point, Rect};

/// Distance from `p` to the segment `a–b`.
pub fn dist_point_segment(p: Point, a: Point, b: Point) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 <= f32::EPSILON {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Point on the quadratic Bézier `a → c (control) → b` at `t`.
pub fn quad_point(a: Point, c: Point, b: Point, t: f32) -> Point {
    let u = 1.0 - t;
    a * (u * u) + c * (2.0 * u * t) + b * (t * t)
}

/// Sample a quadratic Bézier into `n + 1` points.
pub fn quad_polyline(a: Point, c: Point, b: Point, n: usize) -> Vec<Point> {
    let n = n.max(1);
    (0..=n).map(|i| quad_point(a, c, b, i as f32 / n as f32)).collect()
}

/// Distance from `p` to a polyline (∞ for an empty one).
pub fn dist_point_polyline(p: Point, pts: &[Point]) -> f32 {
    match pts {
        [] => f32::INFINITY,
        [only] => p.distance(*only),
        _ => pts.windows(2).map(|w| dist_point_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min),
    }
}

/// Distance from `p` to the border of `r` (0 on the border, positive both
/// inside and outside).
pub fn dist_point_rect_border(p: Point, r: Rect) -> f32 {
    let [tl, tr, br, bl] = r.corners();
    [(tl, tr), (tr, br), (br, bl), (bl, tl)]
        .into_iter()
        .map(|(a, b)| dist_point_segment(p, a, b))
        .fold(f32::INFINITY, f32::min)
}

/// Normalized ellipse "radius" of `p` for the ellipse inscribed in `r`
/// (1.0 on the outline, < 1 inside). ∞ for degenerate ellipses.
pub fn ellipse_norm(p: Point, r: Rect) -> f32 {
    let (rx, ry) = (r.width / 2.0, r.height / 2.0);
    if rx <= f32::EPSILON || ry <= f32::EPSILON {
        return f32::INFINITY;
    }
    let c = r.center();
    (((p.x - c.x) / rx).powi(2) + ((p.y - c.y) / ry).powi(2)).sqrt()
}

/// Approximate distance of `p` to the ellipse outline inscribed in `r`.
pub fn dist_point_ellipse_border(p: Point, r: Rect) -> f32 {
    let n = ellipse_norm(p, r);
    if !n.is_finite() {
        return dist_point_rect_border(p, r);
    }
    let c = r.center();
    let d = p.distance(c);
    if d <= f32::EPSILON {
        return (r.width.min(r.height)) / 2.0;
    }
    // Radius of the ellipse along the direction of p.
    let radius_along = d / n;
    (d - radius_along).abs()
}

/// ⇧ for lines/arrows: snap the direction `from → to` to 45° increments.
pub fn constrain_45(from: Point, to: Point) -> Point {
    let d = to - from;
    let len = (d.x * d.x + d.y * d.y).sqrt();
    if len <= f32::EPSILON {
        return to;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (d.y.atan2(d.x) / step).round() * step;
    Point::new(from.x + len * ang.cos(), from.y + len * ang.sin())
}

/// ⇧ for rect-like shapes: make the drag `from → to` a square (keeps the
/// drag direction; side = the larger extent).
pub fn constrain_square(from: Point, to: Point) -> Point {
    let d = to - from;
    let side = d.x.abs().max(d.y.abs());
    Point::new(from.x + side * sign(d.x), from.y + side * sign(d.y))
}

fn sign(v: f32) -> f32 {
    if v < 0.0 { -1.0 } else { 1.0 }
}

/// ⌥ "from center": the drag start is the center, so the opposite corner is
/// mirrored. Returns the two corners.
pub fn from_center(center: Point, to: Point) -> (Point, Point) {
    (center - (to - center), to)
}

/// One of the 8 resize handles of a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RectHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl RectHandle {
    pub const ALL: [RectHandle; 8] = [
        RectHandle::TopLeft,
        RectHandle::Top,
        RectHandle::TopRight,
        RectHandle::Right,
        RectHandle::BottomRight,
        RectHandle::Bottom,
        RectHandle::BottomLeft,
        RectHandle::Left,
    ];
    pub const CORNERS: [RectHandle; 4] =
        [RectHandle::TopLeft, RectHandle::TopRight, RectHandle::BottomRight, RectHandle::BottomLeft];

    /// Position of the handle on `r`.
    pub fn position(self, r: Rect) -> Point {
        let (cx, cy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
        match self {
            RectHandle::TopLeft => Point::new(r.x, r.y),
            RectHandle::Top => Point::new(cx, r.y),
            RectHandle::TopRight => Point::new(r.right(), r.y),
            RectHandle::Right => Point::new(r.right(), cy),
            RectHandle::BottomRight => Point::new(r.right(), r.bottom()),
            RectHandle::Bottom => Point::new(cx, r.bottom()),
            RectHandle::BottomLeft => Point::new(r.x, r.bottom()),
            RectHandle::Left => Point::new(r.x, cy),
        }
    }

    pub fn is_corner(self) -> bool {
        matches!(self, RectHandle::TopLeft | RectHandle::TopRight | RectHandle::BottomRight | RectHandle::BottomLeft)
    }

    fn moves_left(self) -> bool {
        matches!(self, RectHandle::TopLeft | RectHandle::Left | RectHandle::BottomLeft)
    }
    fn moves_right(self) -> bool {
        matches!(self, RectHandle::TopRight | RectHandle::Right | RectHandle::BottomRight)
    }
    fn moves_top(self) -> bool {
        matches!(self, RectHandle::TopLeft | RectHandle::Top | RectHandle::TopRight)
    }
    fn moves_bottom(self) -> bool {
        matches!(self, RectHandle::BottomLeft | RectHandle::Bottom | RectHandle::BottomRight)
    }
}

/// Resize `r` by dragging `handle` to `p`.
/// * `keep_aspect` (⇧): corners keep the original aspect ratio.
/// * `from_center` (⌥): the opposite side moves symmetrically.
///
/// The result is normalized (dragging past the opposite edge flips).
pub fn resize_rect(r: Rect, handle: RectHandle, p: Point, keep_aspect: bool, from_center: bool) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.right(), r.bottom());
    let c = r.center();
    if handle.moves_left() {
        x0 = p.x;
        if from_center {
            x1 = 2.0 * c.x - p.x;
        }
    }
    if handle.moves_right() {
        x1 = p.x;
        if from_center {
            x0 = 2.0 * c.x - p.x;
        }
    }
    if handle.moves_top() {
        y0 = p.y;
        if from_center {
            y1 = 2.0 * c.y - p.y;
        }
    }
    if handle.moves_bottom() {
        y1 = p.y;
        if from_center {
            y0 = 2.0 * c.y - p.y;
        }
    }
    if keep_aspect && handle.is_corner() && r.width > 0.0 && r.height > 0.0 {
        let aspect = r.width / r.height;
        let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
        let (nw, nh) = if w / aspect > h { (w, w / aspect) } else { (h * aspect, h) };
        // Anchor = the fixed corner (or center).
        let sx = if x1 >= x0 { 1.0 } else { -1.0 };
        let sy = if y1 >= y0 { 1.0 } else { -1.0 };
        if from_center {
            x0 = c.x - nw / 2.0 * sx;
            x1 = c.x + nw / 2.0 * sx;
            y0 = c.y - nh / 2.0 * sy;
            y1 = c.y + nh / 2.0 * sy;
        } else {
            if handle.moves_left() {
                x0 = x1 - nw * sx;
            } else {
                x1 = x0 + nw * sx;
            }
            if handle.moves_top() {
                y0 = y1 - nh * sy;
            } else {
                y1 = y0 + nh * sy;
            }
        }
    }
    Rect::from_points(Point::new(x0, y0), Point::new(x1, y1))
}

/// Map a point from rect `from` to rect `to` (proportional; used to scale
/// freehand strokes when their bounds are resized).
pub fn map_between_rects(p: Point, from: Rect, to: Rect) -> Point {
    let fx = if from.width > f32::EPSILON { (p.x - from.x) / from.width } else { 0.5 };
    let fy = if from.height > f32::EPSILON { (p.y - from.y) / from.height } else { 0.5 };
    Point::new(to.x + fx * to.width, to.y + fy * to.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn segment_distance() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, 0.0);
        assert!(close(dist_point_segment(Point::new(5.0, 3.0), a, b), 3.0));
        assert!(close(dist_point_segment(Point::new(-4.0, 3.0), a, b), 5.0));
        assert!(close(dist_point_segment(Point::new(1.0, 1.0), a, a), 2f32.sqrt()));
    }

    #[test]
    fn polyline_and_bezier() {
        let pts = quad_polyline(Point::new(0.0, 0.0), Point::new(5.0, 10.0), Point::new(10.0, 0.0), 32);
        assert_eq!(pts.len(), 33);
        // Apex of the curve is at y = 5.
        assert!(close(quad_point(Point::new(0.0, 0.0), Point::new(5.0, 10.0), Point::new(10.0, 0.0), 0.5).y, 5.0));
        assert!(dist_point_polyline(Point::new(5.0, 5.0), &pts) < 0.1);
        assert_eq!(dist_point_polyline(Point::ZERO, &[]), f32::INFINITY);
    }

    #[test]
    fn ellipse_metrics() {
        let r = Rect::new(0.0, 0.0, 20.0, 10.0);
        assert!(close(ellipse_norm(Point::new(10.0, 5.0), r), 0.0));
        assert!(close(ellipse_norm(Point::new(20.0, 5.0), r), 1.0));
        assert!(dist_point_ellipse_border(Point::new(20.0, 5.0), r) < 1e-3);
        assert!(close(dist_point_ellipse_border(Point::new(25.0, 5.0), r), 5.0));
    }

    #[test]
    fn constraints() {
        let p = constrain_45(Point::ZERO, Point::new(10.0, 1.0));
        assert!(close(p.y, 0.0) && p.x > 9.9);
        let p = constrain_45(Point::ZERO, Point::new(10.0, 9.0));
        assert!(close(p.x, p.y));
        assert_eq!(constrain_square(Point::ZERO, Point::new(-5.0, 3.0)), Point::new(-5.0, 5.0));
        assert_eq!(from_center(Point::new(10.0, 10.0), Point::new(15.0, 12.0)).0, Point::new(5.0, 8.0));
    }

    #[test]
    fn resize_handles() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(resize_rect(r, RectHandle::BottomRight, Point::new(120.0, 60.0), false, false), Rect::new(0.0, 0.0, 120.0, 60.0));
        assert_eq!(resize_rect(r, RectHandle::Left, Point::new(10.0, 999.0), false, false), Rect::new(10.0, 0.0, 90.0, 50.0));
        // Flip past the opposite edge.
        assert_eq!(resize_rect(r, RectHandle::Right, Point::new(-20.0, 0.0), false, false), Rect::new(-20.0, 0.0, 20.0, 50.0));
        // Aspect kept (2:1).
        let k = resize_rect(r, RectHandle::BottomRight, Point::new(200.0, 60.0), true, false);
        assert!(close(k.width / k.height, 2.0), "{k:?}");
        assert_eq!(k.origin(), Point::ZERO);
        // From center: symmetric.
        let c = resize_rect(r, RectHandle::Right, Point::new(110.0, 0.0), false, true);
        assert_eq!(c, Rect::new(-10.0, 0.0, 120.0, 50.0));
        assert_eq!(RectHandle::Top.position(r), Point::new(50.0, 0.0));
    }

    #[test]
    fn map_rects() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(10.0, 10.0, 20.0, 40.0);
        assert_eq!(map_between_rects(Point::new(5.0, 5.0), a, b), Point::new(20.0, 30.0));
    }
}
