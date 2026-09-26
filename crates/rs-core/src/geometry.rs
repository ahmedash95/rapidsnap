//! Geometry primitives.
//!
//! All values are `f32`. Unless stated otherwise, geometry stored inside a
//! [`crate::Document`] (annotations) is in **content space**: pixels of the
//! base image *after* the document's [`crate::ImageTransform`] (crop, then
//! rotation) has been applied, origin at the top-left of that transformed
//! image, +x right, +y down. See CONTRACTS.md "Coordinate spaces".

use serde::{Deserialize, Serialize};

/// A 2D point (or vector).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn translate(self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }

    pub fn distance(self, other: Point) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    pub fn lerp(self, other: Point, t: f32) -> Point {
        Point::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Point;
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Mul<f32> for Point {
    type Output = Point;
    fn mul(self, s: f32) -> Point {
        Point::new(self.x * s, self.y * s)
    }
}

/// A 2D size.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    pub fn scale(self, s: f32) -> Self {
        Self::new(self.width * s, self.height * s)
    }

    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// Axis-aligned rectangle. `x`,`y` is the top-left corner. A rectangle is
/// *normalized* when `width >= 0 && height >= 0`; all rectangles stored in a
/// Document must be normalized (use [`Rect::from_points`] / [`Rect::normalized`]).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };

    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self::new(origin.x, origin.y, size.width, size.height)
    }

    /// Normalized rectangle spanning two arbitrary corner points.
    pub fn from_points(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    /// Smallest rectangle containing all points. `None` if `points` is empty.
    pub fn bounding(points: &[Point]) -> Option<Self> {
        let first = *points.first()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Some(Self::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn normalized(self) -> Self {
        Self::from_points(self.origin(), Point::new(self.x + self.width, self.y + self.height))
    }

    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
    pub fn left(&self) -> f32 {
        self.x
    }
    pub fn top(&self) -> f32 {
        self.y
    }
    pub fn right(&self) -> f32 {
        self.x + self.width
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Inclusive of the top/left edge, exclusive of the bottom/right edge.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    pub fn intersection(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// Grow (positive) or shrink (negative) by `d` on every side.
    pub fn outset(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.width + 2.0 * d, self.height + 2.0 * d)
    }

    /// Scale position and size around the origin (0,0).
    pub fn scale(&self, s: f32) -> Rect {
        Rect::new(self.x * s, self.y * s, self.width * s, self.height * s)
    }

    /// The four corners: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(&self) -> [Point; 4] {
        [
            Point::new(self.x, self.y),
            Point::new(self.right(), self.y),
            Point::new(self.right(), self.bottom()),
            Point::new(self.x, self.bottom()),
        ]
    }
}

/// Integer pixel rectangle (used for crop regions in *base image* pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct IRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl IRect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn to_rect(self) -> Rect {
        Rect::new(self.x as f32, self.y as f32, self.width as f32, self.height as f32)
    }

    /// Clamp to an image of `w`×`h`. Returns `None` if the result is empty.
    pub fn clamp_to(self, w: u32, h: u32) -> Option<IRect> {
        let x0 = self.x.min(w);
        let y0 = self.y.min(h);
        let x1 = self.x.saturating_add(self.width).min(w);
        let y1 = self.y.saturating_add(self.height).min(h);
        (x1 > x0 && y1 > y0).then(|| IRect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_points_normalizes() {
        let r = Rect::from_points(Point::new(10.0, 20.0), Point::new(0.0, 5.0));
        assert_eq!(r, Rect::new(0.0, 5.0, 10.0, 15.0));
    }

    #[test]
    fn irect_clamp() {
        assert_eq!(IRect::new(5, 5, 100, 100).clamp_to(50, 40), Some(IRect::new(5, 5, 45, 35)));
        assert_eq!(IRect::new(60, 5, 10, 10).clamp_to(50, 40), None);
    }
}
