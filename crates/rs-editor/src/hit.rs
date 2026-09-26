//! Hit-testing and per-annotation edit handles (pure logic, content space).

use rs_core::{Annotation, AnnotationKind, Point, Rect};

use crate::geom::{
    self, dist_point_ellipse_border, dist_point_polyline, dist_point_rect_border, dist_point_segment, ellipse_norm,
    RectHandle,
};

/// How strongly a point hits an annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HitStrength {
    /// On the stroke / filled area / body.
    Strong,
    /// Inside an outline-only shape (selectable when nothing strong is hit).
    Weak,
}

/// Hit-test one annotation. `tol` = pick tolerance in content px.
/// `bounds` = precise bounds (from the renderer) for text-like kinds.
pub fn hit_test(a: &Annotation, p: Point, tol: f32, bounds: Rect) -> Option<HitStrength> {
    if a.hidden || a.locked {
        return None;
    }
    use HitStrength::*;
    let strong = |b: bool| b.then_some(Strong);
    match &a.kind {
        AnnotationKind::Rect(r) => {
            let half = if r.style.mode.has_stroke() { r.style.stroke.width / 2.0 } else { 0.0 };
            if r.style.mode.has_fill() && r.rect.outset(tol).contains(p) {
                return Some(Strong);
            }
            if dist_point_rect_border(p, r.rect) <= tol + half {
                return Some(Strong);
            }
            r.rect.contains(p).then_some(Weak)
        }
        AnnotationKind::Ellipse(e) => {
            let half = if e.style.mode.has_stroke() { e.style.stroke.width / 2.0 } else { 0.0 };
            let n = ellipse_norm(p, e.rect);
            if e.style.mode.has_fill() && n <= 1.0 {
                return Some(Strong);
            }
            if dist_point_ellipse_border(p, e.rect) <= tol + half {
                return Some(Strong);
            }
            (n <= 1.0).then_some(Weak)
        }
        AnnotationKind::Line(l) => strong(dist_point_segment(p, l.start, l.end) <= tol + l.stroke.width / 2.0),
        AnnotationKind::Arrow(ar) => {
            let d = match ar.control {
                Some(c) => dist_point_polyline(p, &geom::quad_polyline(ar.start, c, ar.end, 32)),
                None => dist_point_segment(p, ar.start, ar.end),
            };
            let head = if p.distance(ar.end) <= ar.head_length() + tol { ar.head_width() / 2.0 } else { 0.0 };
            strong(d <= tol + (ar.stroke.width / 2.0).max(head))
        }
        AnnotationKind::Pen(pen) => strong(dist_point_polyline(p, &pen.points) <= tol + pen.stroke.width / 2.0),
        AnnotationKind::Highlighter(h) => strong(dist_point_polyline(p, &h.points) <= tol + h.width / 2.0),
        AnnotationKind::Step(s) => strong(p.distance(s.center) <= s.radius + tol),
        AnnotationKind::Callout(c) => {
            let bubble = rs_render::callout_bubble_rect(c);
            if bubble.outset(tol).contains(p) {
                return Some(Strong);
            }
            if !bubble.contains(c.tail) {
                let base = nearest_point_on_rect(bubble, c.tail);
                if dist_point_segment(p, base, c.tail) <= tol + 6.0 {
                    return Some(Strong);
                }
            }
            None
        }
        AnnotationKind::Spotlight(s) => {
            if dist_point_rect_border(p, s.rect) <= tol {
                return Some(Strong);
            }
            s.rect.contains(p).then_some(Weak)
        }
        AnnotationKind::Magnifier(m) => {
            if p.distance(m.target_center) <= m.target_radius() + tol {
                return Some(Strong);
            }
            (p.distance(m.source_center) <= m.source_radius + tol).then_some(Weak)
        }
        AnnotationKind::Blur(b) => strong(b.rect.outset(tol).contains(p)),
        AnnotationKind::Pixelate(b) => strong(b.rect.outset(tol).contains(p)),
        AnnotationKind::Redact(b) => strong(b.rect.outset(tol).contains(p)),
        AnnotationKind::Text(_) => strong(bounds.outset(tol).contains(p)),
    }
}

/// Topmost annotation under `p`: strong hits first (top → bottom), then weak.
pub fn pick<'a>(
    annotations: &'a [Annotation],
    p: Point,
    tol: f32,
    bounds_of: &mut dyn FnMut(&Annotation) -> Rect,
) -> Option<&'a Annotation> {
    let mut weak = None;
    for a in annotations.iter().rev() {
        let b = bounds_of(a);
        match hit_test(a, p, tol, b) {
            Some(HitStrength::Strong) => return Some(a),
            Some(HitStrength::Weak) if weak.is_none() => weak = Some(a),
            _ => {}
        }
    }
    weak
}

/// Nearest point to `p` on the border of `r`.
pub fn nearest_point_on_rect(r: Rect, p: Point) -> Point {
    // Documents from disk / the clipboard may carry negative sizes or
    // non-finite values; `f32::clamp` panics on min > max or NaN bounds.
    let r = r.normalized();
    if !(r.x.is_finite() && r.y.is_finite() && r.right().is_finite() && r.bottom().is_finite()) {
        return p;
    }
    let x = p.x.clamp(r.x, r.right());
    let y = p.y.clamp(r.y, r.bottom());
    if !r.contains(p) {
        return Point::new(x, y);
    }
    let d = [(p.x - r.x), (r.right() - p.x), (p.y - r.y), (r.bottom() - p.y)];
    let m = d.iter().cloned().fold(f32::INFINITY, f32::min);
    if m == d[0] {
        Point::new(r.x, p.y)
    } else if m == d[1] {
        Point::new(r.right(), p.y)
    } else if m == d[2] {
        Point::new(p.x, r.y)
    } else {
        Point::new(p.x, r.bottom())
    }
}

/// An edit handle of a single selected annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandleKind {
    /// Resize the rect-like box (or bounds, for pens/steps/magnifier/text).
    Rect(RectHandle),
    /// Line/arrow start point.
    Start,
    /// Line/arrow end (tip) point.
    End,
    /// Arrow Bézier control point (on a straight arrow: its midpoint; dragging
    /// it bends the arrow).
    Control,
    /// Callout tail tip.
    Tail,
    /// Magnifier source circle center.
    MagnifierSource,
}

/// A handle and its position (content space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    pub kind: HandleKind,
    pub pos: Point,
}

/// The rect that rect-handles of `a` operate on.
pub fn handle_rect(a: &Annotation, bounds: Rect) -> Option<Rect> {
    match &a.kind {
        AnnotationKind::Rect(r) => Some(r.rect),
        AnnotationKind::Ellipse(r) => Some(r.rect),
        AnnotationKind::Callout(r) => Some(rs_render::callout_bubble_rect(r)),
        AnnotationKind::Spotlight(r) => Some(r.rect),
        AnnotationKind::Blur(r) => Some(r.rect),
        AnnotationKind::Pixelate(r) => Some(r.rect),
        AnnotationKind::Redact(r) => Some(r.rect),
        AnnotationKind::Pen(p) => Rect::bounding(&p.points),
        AnnotationKind::Highlighter(h) => Rect::bounding(&h.points),
        AnnotationKind::Step(_) | AnnotationKind::Magnifier(_) => Some(a.geometric_bounds()),
        AnnotationKind::Text(_) => Some(bounds),
        AnnotationKind::Line(_) | AnnotationKind::Arrow(_) => None,
    }
}

/// All handles of a (single) selected annotation.
pub fn handles_for(a: &Annotation, bounds: Rect) -> Vec<Handle> {
    let h = |kind, pos| Handle { kind, pos };
    let rect_handles = |r: Rect, corners_only: bool| -> Vec<Handle> {
        let set: &[RectHandle] = if corners_only { &RectHandle::CORNERS } else { &RectHandle::ALL };
        set.iter().map(|rh| h(HandleKind::Rect(*rh), rh.position(r))).collect()
    };
    match &a.kind {
        AnnotationKind::Line(l) => vec![h(HandleKind::Start, l.start), h(HandleKind::End, l.end)],
        AnnotationKind::Arrow(ar) => {
            let c = match ar.control {
                // Handle sits on the curve apex (t = 0.5) for easier grabbing.
                Some(c) => geom::quad_point(ar.start, c, ar.end, 0.5),
                None => ar.start.lerp(ar.end, 0.5),
            };
            vec![h(HandleKind::Start, ar.start), h(HandleKind::End, ar.end), h(HandleKind::Control, c)]
        }
        AnnotationKind::Callout(c) => {
            // The drawn bubble (grown to fit its text), not the stored rect.
            let mut v = rect_handles(rs_render::callout_bubble_rect(c), false);
            v.push(h(HandleKind::Tail, c.tail));
            v
        }
        AnnotationKind::Magnifier(m) => {
            let mut v = rect_handles(a.geometric_bounds(), true);
            v.push(h(HandleKind::MagnifierSource, m.source_center));
            v
        }
        AnnotationKind::Step(_) => rect_handles(a.geometric_bounds(), true),
        AnnotationKind::Text(_) => {
            // Left/right edges re-wrap, corners scale the font.
            let mut v = rect_handles(bounds, true);
            v.push(h(HandleKind::Rect(RectHandle::Left), RectHandle::Left.position(bounds)));
            v.push(h(HandleKind::Rect(RectHandle::Right), RectHandle::Right.position(bounds)));
            v
        }
        _ => handle_rect(a, bounds).map(|r| rect_handles(r, false)).unwrap_or_default(),
    }
}

/// Handle under `p` (within `tol`), nearest first.
pub fn handle_at(handles: &[Handle], p: Point, tol: f32) -> Option<HandleKind> {
    handles
        .iter()
        .map(|h| (h.kind, h.pos.distance(p)))
        .filter(|(_, d)| *d <= tol)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

/// Modifier state for a handle drag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragMods {
    /// ⇧: keep aspect / 45° angles.
    pub shift: bool,
    /// ⌥: symmetric around the center.
    pub alt: bool,
}

/// Apply dragging `handle` of `original` to content point `p`.
/// `bounds` = the precise bounds of `original` (for text).
pub fn apply_handle(original: &Annotation, bounds: Rect, handle: HandleKind, p: Point, mods: DragMods) -> Annotation {
    let mut a = original.clone();
    let resize = |r: Rect, rh: RectHandle| geom::resize_rect(r, rh, p, mods.shift, mods.alt);
    let bubble = match &original.kind {
        AnnotationKind::Callout(c) => rs_render::callout_bubble_rect(c),
        _ => Rect::default(),
    };
    match (&mut a.kind, handle) {
        (AnnotationKind::Line(l), HandleKind::Start) => {
            l.start = if mods.shift { geom::constrain_45(l.end, p) } else { p }
        }
        (AnnotationKind::Line(l), HandleKind::End) => l.end = if mods.shift { geom::constrain_45(l.start, p) } else { p },
        (AnnotationKind::Arrow(ar), HandleKind::Start) => {
            ar.start = if mods.shift { geom::constrain_45(ar.end, p) } else { p }
        }
        (AnnotationKind::Arrow(ar), HandleKind::End) => {
            ar.end = if mods.shift { geom::constrain_45(ar.start, p) } else { p }
        }
        (AnnotationKind::Arrow(ar), HandleKind::Control) => {
            // The handle is the curve apex B(0.5) = (a + 2c + b)/4 → c = 2p − (a+b)/2.
            let mid = ar.start.lerp(ar.end, 0.5);
            if p.distance(mid) < 2.0 {
                ar.control = None;
            } else {
                ar.control = Some(p * 2.0 - mid);
            }
        }
        (AnnotationKind::Callout(c), HandleKind::Tail) => c.tail = p,
        (AnnotationKind::Magnifier(m), HandleKind::MagnifierSource) => m.source_center = p,
        (AnnotationKind::Magnifier(m), HandleKind::Rect(_)) => {
            let r = (p.x - m.target_center.x).abs().max((p.y - m.target_center.y).abs()).max(4.0);
            m.source_radius = r / m.zoom.max(1.0);
        }
        (AnnotationKind::Step(s), HandleKind::Rect(_)) => {
            s.radius = (p.x - s.center.x).abs().max((p.y - s.center.y).abs()).max(4.0);
        }
        (AnnotationKind::Rect(r), HandleKind::Rect(rh)) => r.rect = resize(r.rect, rh),
        (AnnotationKind::Ellipse(r), HandleKind::Rect(rh)) => r.rect = resize(r.rect, rh),
        (AnnotationKind::Callout(r), HandleKind::Rect(rh)) => r.rect = resize(bubble, rh),
        (AnnotationKind::Spotlight(r), HandleKind::Rect(rh)) => r.rect = resize(r.rect, rh),
        (AnnotationKind::Blur(r), HandleKind::Rect(rh)) => r.rect = resize(r.rect, rh),
        (AnnotationKind::Pixelate(r), HandleKind::Rect(rh)) => r.rect = resize(r.rect, rh),
        (AnnotationKind::Redact(r), HandleKind::Rect(rh)) => r.rect = resize(r.rect, rh),
        (AnnotationKind::Pen(pen), HandleKind::Rect(rh)) => {
            if let Some(from) = Rect::bounding(&pen.points) {
                let to = resize(from, rh);
                pen.points.iter_mut().for_each(|q| *q = geom::map_between_rects(*q, from, to));
            }
        }
        (AnnotationKind::Highlighter(hl), HandleKind::Rect(rh)) => {
            if let Some(from) = Rect::bounding(&hl.points) {
                let to = resize(from, rh);
                hl.points.iter_mut().for_each(|q| *q = geom::map_between_rects(*q, from, to));
            }
        }
        (AnnotationKind::Text(t), HandleKind::Rect(rh)) => match rh {
            RectHandle::Left | RectHandle::Right => {
                let r = geom::resize_rect(bounds, rh, p, false, mods.alt);
                let w = r.width.max(t.style.size);
                if rh == RectHandle::Left {
                    t.position.x = bounds.right() - w;
                } else if mods.alt {
                    t.position.x = r.x;
                }
                t.width = Some(w);
            }
            _ if rh.is_corner() => {
                // Scale font (and fixed width) with the box height.
                let r = geom::resize_rect(bounds, rh, p, true, mods.alt);
                if bounds.height > 1.0 {
                    let k = (r.height / bounds.height).clamp(0.05, 50.0);
                    t.style = t.style.scaled(k);
                    t.width = t.width.map(|w| w * k);
                    t.position = r.origin();
                }
            }
            _ => {}
        },
        _ => {}
    }
    a
}

/// Proportionally map `a` from rect `from` to rect `to` (multi-selection
/// resize). Positions and sizes scale; stroke widths and other style lengths
/// stay unchanged. Radii (steps, magnifier source) scale by the geometric
/// mean of the x/y factors. With `scale_text` (corner handles) text font
/// size (and a fixed wrap width) scales by that mean as well — like a
/// single text's corner handle; edge handles only re-position text (and
/// scale a fixed wrap width horizontally).
pub fn scale_annotation(a: &Annotation, from: Rect, to: Rect, scale_text: bool) -> Annotation {
    let from = from.normalized();
    let to = to.normalized();
    let sx = if from.width > 1e-3 { to.width / from.width } else { 1.0 };
    let sy = if from.height > 1e-3 { to.height / from.height } else { 1.0 };
    let sx = if sx.is_finite() { sx.max(1e-3) } else { 1.0 };
    let sy = if sy.is_finite() { sy.max(1e-3) } else { 1.0 };
    let mean = (sx * sy).sqrt();
    let m = |p: Point| geom::map_between_rects(p, from, to);
    let mr = |r: Rect| Rect::from_points(m(r.origin()), m(Point::new(r.right(), r.bottom())));
    let mut out = a.clone();
    match &mut out.kind {
        AnnotationKind::Rect(x) => x.rect = mr(x.rect),
        AnnotationKind::Ellipse(x) => x.rect = mr(x.rect),
        AnnotationKind::Spotlight(x) => x.rect = mr(x.rect),
        AnnotationKind::Blur(x) => x.rect = mr(x.rect),
        AnnotationKind::Pixelate(x) => x.rect = mr(x.rect),
        AnnotationKind::Redact(x) => x.rect = mr(x.rect),
        AnnotationKind::Line(x) => {
            x.start = m(x.start);
            x.end = m(x.end);
        }
        AnnotationKind::Arrow(x) => {
            x.start = m(x.start);
            x.end = m(x.end);
            x.control = x.control.map(m);
        }
        AnnotationKind::Pen(x) => x.points.iter_mut().for_each(|p| *p = m(*p)),
        AnnotationKind::Highlighter(x) => x.points.iter_mut().for_each(|p| *p = m(*p)),
        AnnotationKind::Step(x) => {
            x.center = m(x.center);
            x.radius = (x.radius * mean).max(2.0);
        }
        AnnotationKind::Callout(x) => {
            x.rect = mr(rs_render::callout_bubble_rect(x));
            x.tail = m(x.tail);
            if scale_text {
                x.text_style = x.text_style.scaled(mean);
            }
        }
        AnnotationKind::Magnifier(x) => {
            x.source_center = m(x.source_center);
            x.target_center = m(x.target_center);
            x.source_radius = (x.source_radius * mean).max(1.0);
        }
        AnnotationKind::Text(x) => {
            x.position = m(x.position);
            if scale_text {
                x.style = x.style.scaled(mean);
                x.width = x.width.map(|w| w * mean);
            } else {
                x.width = x.width.map(|w| (w * sx).max(1.0));
            }
        }
    }
    out
}

/// Annotations whose bounds intersect the marquee rect.
pub fn marquee_select(
    annotations: &[Annotation],
    marquee: Rect,
    bounds_of: &mut dyn FnMut(&Annotation) -> Rect,
) -> Vec<rs_core::AnnotationId> {
    annotations
        .iter()
        .filter(|a| !a.hidden && !a.locked)
        .filter(|a| {
            let b = bounds_of(a);
            // Zero-area bounds (horizontal lines) still count.
            let b = if b.width <= 0.0 || b.height <= 0.0 { b.outset(0.5) } else { b };
            b.intersects(&marquee)
        })
        .map(|a| a.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::{test_support::all_kinds, FillMode, Point, RectAnnotation, ShapeStyle, ToolKind, ToolStyle};

    fn rect_anno(mode: FillMode) -> Annotation {
        Annotation::new(AnnotationKind::Rect(RectAnnotation {
            rect: Rect::new(10.0, 10.0, 100.0, 50.0),
            style: ShapeStyle { mode, ..Default::default() },
            corner_radius: 0.0,
        }))
    }

    #[test]
    fn rect_hits() {
        let a = rect_anno(FillMode::Outline);
        let b = a.geometric_bounds();
        assert_eq!(hit_test(&a, Point::new(10.0, 30.0), 2.0, b), Some(HitStrength::Strong));
        assert_eq!(hit_test(&a, Point::new(50.0, 30.0), 2.0, b), Some(HitStrength::Weak));
        assert_eq!(hit_test(&a, Point::new(200.0, 30.0), 2.0, b), None);
        let f = rect_anno(FillMode::Filled);
        assert_eq!(hit_test(&f, Point::new(50.0, 30.0), 2.0, b), Some(HitStrength::Strong));
    }

    #[test]
    fn pick_prefers_strong_and_top() {
        let big = Annotation::new(AnnotationKind::Rect(RectAnnotation {
            rect: Rect::new(0.0, 0.0, 200.0, 200.0),
            style: ShapeStyle::default(),
            corner_radius: 0.0,
        }));
        let small = rect_anno(FillMode::Filled);
        // Big outline rect on top of the small filled one: clicking inside
        // the small one picks the small one (strong beats weak).
        let list = vec![small.clone(), big.clone()];
        let got = pick(&list, Point::new(50.0, 30.0), 2.0, &mut |a| a.geometric_bounds()).unwrap();
        assert_eq!(got.id, small.id);
        let got = pick(&list, Point::new(150.0, 150.0), 2.0, &mut |a| a.geometric_bounds()).unwrap();
        assert_eq!(got.id, big.id);
        let hidden = Annotation { hidden: true, ..small.clone() };
        assert!(pick(&[hidden], Point::new(50.0, 30.0), 2.0, &mut |a| a.geometric_bounds()).is_none());
    }

    #[test]
    fn every_kind_is_hittable_at_its_geometry() {
        let st = ToolStyle::default();
        for t in ToolKind::ALL {
            let Some(a) = st.create(t, Point::new(10.0, 10.0), Point::new(80.0, 60.0)) else { continue };
            let b = a.geometric_bounds();
            let probe = match &a.kind {
                AnnotationKind::Line(l) => l.start.lerp(l.end, 0.5),
                AnnotationKind::Arrow(l) => l.start.lerp(l.end, 0.5),
                AnnotationKind::Pen(p) => p.points[0],
                AnnotationKind::Highlighter(p) => p.points[0],
                AnnotationKind::Magnifier(m) => m.target_center,
                AnnotationKind::Step(s) => s.center,
                AnnotationKind::Ellipse(_) => Point::new(b.x, b.center().y),
                _ => Point::new(b.x + 1.0, b.y + 1.0),
            };
            assert!(hit_test(&a, probe, 3.0, b.outset(1.0)).is_some(), "{t:?}");
        }
    }

    #[test]
    fn handles_exist_for_all_kinds() {
        for k in all_kinds() {
            let a = Annotation::new(k);
            let hs = handles_for(&a, a.geometric_bounds());
            assert!(!hs.is_empty(), "{:?}", a.tool());
        }
    }

    #[test]
    fn arrow_control_handle_bends() {
        let a = ToolStyle::default().create(ToolKind::Arrow, Point::new(0.0, 0.0), Point::new(100.0, 0.0)).unwrap();
        let bent = apply_handle(&a, a.geometric_bounds(), HandleKind::Control, Point::new(50.0, 20.0), DragMods::default());
        let AnnotationKind::Arrow(ar) = &bent.kind else { panic!() };
        let c = ar.control.unwrap();
        // Apex of the resulting curve is where we dragged.
        let apex = geom::quad_point(ar.start, c, ar.end, 0.5);
        assert!((apex.y - 20.0).abs() < 1e-3 && (apex.x - 50.0).abs() < 1e-3);
        // Dragging back onto the chord straightens.
        let straight = apply_handle(&bent, bent.geometric_bounds(), HandleKind::Control, Point::new(50.0, 0.5), DragMods::default());
        let AnnotationKind::Arrow(ar) = &straight.kind else { panic!() };
        assert!(ar.control.is_none());
    }

    #[test]
    fn endpoint_and_rect_handles() {
        let a = ToolStyle::default().create(ToolKind::Line, Point::new(0.0, 0.0), Point::new(100.0, 0.0)).unwrap();
        let m = apply_handle(&a, a.geometric_bounds(), HandleKind::End, Point::new(50.0, 48.0), DragMods { shift: true, alt: false });
        let AnnotationKind::Line(l) = &m.kind else { panic!() };
        assert!((l.end.x - l.end.y).abs() < 1e-3, "45° constrained {l:?}");
        let r = rect_anno(FillMode::Outline);
        let m = apply_handle(&r, r.geometric_bounds(), HandleKind::Rect(RectHandle::BottomRight), Point::new(200.0, 100.0), DragMods::default());
        assert_eq!(m.geometric_bounds(), Rect::new(10.0, 10.0, 190.0, 90.0));
        assert_eq!(m.id, r.id);
    }

    #[test]
    fn text_edge_handle_sets_width() {
        let mut t = ToolStyle::default().create(ToolKind::Text, Point::new(10.0, 10.0), Point::new(10.0, 10.0)).unwrap();
        if let AnnotationKind::Text(x) = &mut t.kind {
            x.content = "hello world".into();
        }
        let b = t.geometric_bounds();
        let m = apply_handle(&t, b, HandleKind::Rect(RectHandle::Right), Point::new(b.x + 60.0, b.y), DragMods::default());
        let AnnotationKind::Text(x) = &m.kind else { panic!() };
        assert_eq!(x.width, Some(60.0));
    }

    #[test]
    fn scale_annotation_is_proportional() {
        use rs_core::{TextAnnotation, TextStyle};
        let from = Rect::new(0.0, 0.0, 100.0, 100.0);
        let to = Rect::new(0.0, 0.0, 200.0, 200.0);
        let r = rect_anno(FillMode::Outline);
        let s = scale_annotation(&r, from, to, true);
        let g = s.geometric_bounds();
        for (a, b) in [(g.x, 20.0), (g.y, 20.0), (g.width, 200.0), (g.height, 100.0)] {
            assert!((a - b).abs() < 1e-3, "{g:?}");
        }
        let AnnotationKind::Rect(x) = &s.kind else { panic!() };
        assert_eq!(x.style.stroke.width, 4.0, "stroke width unchanged");
        let t = Annotation::new(AnnotationKind::Text(TextAnnotation {
            position: Point::new(50.0, 50.0),
            width: Some(40.0),
            content: "hi".into(),
            style: TextStyle { size: 20.0, ..TextStyle::default() },
            preset: None,
        }));
        let st = scale_annotation(&t, from, to, true);
        let AnnotationKind::Text(x) = &st.kind else { panic!() };
        assert_eq!((x.position, x.style.size, x.width), (Point::new(100.0, 100.0), 40.0, Some(80.0)));
        // Edge handle (x only): font kept, width scaled horizontally.
        let wide = Rect::new(0.0, 0.0, 300.0, 100.0);
        let se = scale_annotation(&t, from, wide, false);
        let AnnotationKind::Text(x) = &se.kind else { panic!() };
        assert_eq!((x.position, x.style.size, x.width), (Point::new(150.0, 50.0), 20.0, Some(120.0)));
        let mut step = ToolStyle::default().create(ToolKind::Step, Point::new(10.0, 10.0), Point::new(10.0, 10.0)).unwrap();
        if let AnnotationKind::Step(x) = &mut step.kind {
            x.radius = 10.0;
        }
        let ss = scale_annotation(&step, from, to, true);
        let AnnotationKind::Step(x) = &ss.kind else { panic!() };
        assert_eq!((x.center, x.radius), (Point::new(20.0, 20.0), 20.0));
        assert_eq!(ss.id, step.id);
    }

    #[test]
    fn marquee() {
        let a = rect_anno(FillMode::Outline);
        let hits = marquee_select(std::slice::from_ref(&a), Rect::new(0.0, 0.0, 20.0, 20.0), &mut |a| a.geometric_bounds());
        assert_eq!(hits, vec![a.id]);
        let none = marquee_select(std::slice::from_ref(&a), Rect::new(300.0, 0.0, 20.0, 20.0), &mut |a| a.geometric_bounds());
        assert!(none.is_empty());
    }
}
