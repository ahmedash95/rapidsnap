//! Snapping to other objects' edges/centers, to the image edges/center and
//! (with beautify) to the output canvas edges/center, with visible guides.
//! Pure logic, content space (canvas targets may be negative / beyond the
//! image: annotations can live over the padding).

use rs_core::{Point, Rect, Size};

/// Candidate snap lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapTargets {
    /// Vertical lines (x positions).
    pub xs: Vec<f32>,
    /// Horizontal lines (y positions).
    pub ys: Vec<f32>,
}

impl SnapTargets {
    /// Targets from the content rect (edges + center) and other objects'
    /// bounds (edges + centers).
    pub fn new(content: Size, others: impl IntoIterator<Item = Rect>) -> Self {
        let mut t = SnapTargets {
            xs: vec![0.0, content.width / 2.0, content.width],
            ys: vec![0.0, content.height / 2.0, content.height],
        };
        for r in others {
            t.xs.extend([r.left(), r.center().x, r.right()]);
            t.ys.extend([r.top(), r.center().y, r.bottom()]);
        }
        t
    }

    /// Also snap to the edges and center of the whole output canvas
    /// (`canvas` in content space, e.g. the beautify padding's outer edge).
    /// `None` (beautify off → canvas == image) adds nothing.
    pub fn with_canvas(mut self, canvas: Option<Rect>) -> Self {
        if let Some(c) = canvas.filter(|c| c.x.is_finite() && c.y.is_finite() && c.width.is_finite() && c.height.is_finite()) {
            self.xs.extend([c.left(), c.center().x, c.right()]);
            self.ys.extend([c.top(), c.center().y, c.bottom()]);
        }
        self
    }
}

/// A guide line to draw (content space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    /// `true` = vertical line at x = `pos`; `false` = horizontal at y = `pos`.
    pub vertical: bool,
    pub pos: f32,
}

/// Best offset (smallest |d|) to bring any of `values` onto any `targets`,
/// within `threshold`. Returns (offset, snapped target).
fn best(values: &[f32], targets: &[f32], threshold: f32) -> Option<(f32, f32)> {
    let mut out: Option<(f32, f32)> = None;
    for &v in values {
        for &t in targets {
            let d = t - v;
            if d.abs() <= threshold && out.is_none_or(|(od, _)| d.abs() < od.abs()) {
                out = Some((d, t));
            }
        }
    }
    out
}

/// Snap a moving rect: aligns its left/center/right and top/middle/bottom to
/// targets. Returns the (dx, dy) correction to add and the guides.
pub fn snap_rect(moving: Rect, targets: &SnapTargets, threshold: f32) -> (f32, f32, Vec<Guide>) {
    let mut guides = Vec::new();
    let xs = [moving.left(), moving.center().x, moving.right()];
    let ys = [moving.top(), moving.center().y, moving.bottom()];
    let dx = best(&xs, &targets.xs, threshold).map(|(d, t)| {
        guides.push(Guide { vertical: true, pos: t });
        d
    });
    let dy = best(&ys, &targets.ys, threshold).map(|(d, t)| {
        guides.push(Guide { vertical: false, pos: t });
        d
    });
    (dx.unwrap_or(0.0), dy.unwrap_or(0.0), guides)
}

/// Snap a single point (drawing / resizing).
pub fn snap_point(p: Point, targets: &SnapTargets, threshold: f32) -> (Point, Vec<Guide>) {
    let mut guides = Vec::new();
    let mut q = p;
    if let Some((d, t)) = best(&[p.x], &targets.xs, threshold) {
        q.x += d;
        guides.push(Guide { vertical: true, pos: t });
    }
    if let Some((d, t)) = best(&[p.y], &targets.ys, threshold) {
        q.y += d;
        guides.push(Guide { vertical: false, pos: t });
    }
    (q, guides)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_snaps_to_image_center_and_other_edges() {
        let t = SnapTargets::new(Size::new(200.0, 100.0), [Rect::new(150.0, 10.0, 20.0, 20.0)]);
        // Center 98 → 100 (image center).
        let (dx, dy, g) = snap_rect(Rect::new(88.0, 40.0, 20.0, 20.0), &t, 3.0);
        assert_eq!(dx, 2.0);
        // Middle 50 → 50 exactly (image middle).
        assert_eq!(dy, 0.0);
        assert!(g.iter().any(|g| g.vertical && g.pos == 100.0));
        // Right edge 148 → 150 (other's left edge).
        let (dx, _, _) = snap_rect(Rect::new(128.0, 70.0, 20.0, 10.0), &t, 3.0);
        assert_eq!(dx, 2.0);
        // Out of range: nothing.
        let (dx, dy, g) = snap_rect(Rect::new(60.0, 65.0, 7.0, 7.0), &t, 1.0);
        assert_eq!((dx, dy), (0.0, 0.0));
        assert!(g.is_empty());
    }

    #[test]
    fn snaps_to_canvas_edges_and_center_outside_the_image() {
        // Image 200×100 with 40 px padding: canvas = (-40,-40, 280×180).
        let canvas = Rect::new(-40.0, -40.0, 280.0, 180.0);
        let t = SnapTargets::new(Size::new(200.0, 100.0), []).with_canvas(Some(canvas));
        let (q, g) = snap_point(Point::new(-38.5, 141.0), &t, 2.0);
        assert_eq!(q, Point::new(-40.0, 140.0));
        assert!(g.iter().any(|g| g.vertical && g.pos == -40.0));
        assert!(g.iter().any(|g| !g.vertical && g.pos == 140.0));
        // A rect in the padding aligns its right edge to the canvas edge.
        let (dx, dy, _) = snap_rect(Rect::new(210.0, -35.0, 28.5, 10.0), &t, 2.0);
        assert_eq!((dx, dy), (1.5, 0.0));
        // Canvas center (100, 50) coincides with the image center here.
        let (q, _) = snap_point(Point::new(101.0, 49.0), &t, 2.0);
        assert_eq!(q, Point::new(100.0, 50.0));
        // Beautify off: nothing extra.
        assert_eq!(SnapTargets::new(Size::new(200.0, 100.0), []).with_canvas(None).xs.len(), 3);
    }

    #[test]
    fn point_snaps() {
        let t = SnapTargets::new(Size::new(200.0, 100.0), []);
        let (q, g) = snap_point(Point::new(198.5, 51.0), &t, 2.0);
        assert_eq!(q, Point::new(200.0, 50.0));
        assert_eq!(g.len(), 2);
    }
}
