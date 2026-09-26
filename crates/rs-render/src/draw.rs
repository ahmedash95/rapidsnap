//! Drawing of "plain" vector annotation kinds onto a pixmap. Kinds that need
//! to read back the canvas (spotlight, magnifier, blur, pixelate, redact) are
//! handled by the pipeline in `lib.rs`.

use rs_core::{AnnotationKind, Color, Point, Rect, ShapeStyle};
use tiny_skia as sk;

use crate::geom::{arrow_geometry, callout_path, circle_path, paint, rounded_rect_path, sk_stroke, smooth_path};
use crate::text::{callout_bubble_rect, callout_text, draw_text_layout, step_label_layout, text_layout};

/// Callout tail base width (content px) for a document scale factor:
/// `min(24px · scale_factor, edge/3)` (the edge/3 part is applied in geometry).
pub(crate) fn callout_base_max(scale_factor: f32) -> f32 {
    24.0 * scale_factor.max(0.1)
}

fn fade(c: Color, alpha: f32) -> Color {
    if alpha >= 0.999 { c } else { c.with_alpha(c.alpha_f32() * alpha.clamp(0.0, 1.0)) }
}

fn fill_stroke_shape(
    pm: &mut sk::Pixmap,
    path: &sk::Path,
    style: &ShapeStyle,
    t: sk::Transform,
    clip: Option<&sk::Mask>,
    alpha: f32,
) {
    if style.mode.has_fill() {
        pm.fill_path(path, &paint(fade(style.fill, alpha)), sk::FillRule::Winding, t, clip);
    }
    if style.mode.has_stroke() && style.stroke.width > 0.0 {
        pm.stroke_path(path, &paint(fade(style.stroke.color, alpha)), &sk_stroke(&style.stroke), t, clip);
    }
}

/// Draw one annotation kind. `t` maps content space → target pixmap px.
/// `alpha` multiplies every color (used for the highlighter, which is drawn
/// directly with multiply blending; other kinds get group opacity via layers).
pub(crate) fn draw_kind(
    pm: &mut sk::Pixmap,
    kind: &AnnotationKind,
    t: sk::Transform,
    clip: Option<&sk::Mask>,
    scale_factor: f32,
    alpha: f32,
) {
    match kind {
        AnnotationKind::Rect(r) => {
            if let Some(path) = rounded_rect_path(r.rect, r.corner_radius) {
                fill_stroke_shape(pm, &path, &r.style, t, clip, alpha);
            }
        }
        AnnotationKind::Ellipse(e) => {
            if let Some(path) =
                sk::Rect::from_xywh(e.rect.x, e.rect.y, e.rect.width, e.rect.height).and_then(sk::PathBuilder::from_oval)
            {
                fill_stroke_shape(pm, &path, &e.style, t, clip, alpha);
            }
        }
        AnnotationKind::Line(l) => {
            let mut pb = sk::PathBuilder::new();
            pb.move_to(l.start.x, l.start.y);
            if l.start.distance(l.end) < 0.01 {
                pb.line_to(l.end.x + 0.01, l.end.y);
            } else {
                pb.line_to(l.end.x, l.end.y);
            }
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &paint(fade(l.stroke.color, alpha)), &sk_stroke(&l.stroke), t, clip);
            }
        }
        AnnotationKind::Arrow(a) => {
            let g = arrow_geometry(a);
            let p = paint(fade(a.stroke.color, alpha));
            if let Some(s) = &g.shaft {
                pm.stroke_path(s, &p, &sk_stroke(&a.stroke), t, clip);
            }
            if let Some(tp) = &g.taper {
                pm.fill_path(tp, &p, sk::FillRule::Winding, t, clip);
            }
            for f in &g.fills {
                pm.fill_path(f, &p, sk::FillRule::Winding, t, clip);
            }
            if !g.strokes.is_empty() {
                // Heads are always solid (no dash).
                let solid = rs_core::Stroke { dash: rs_core::DashStyle::Solid, ..a.stroke };
                let mut st = sk_stroke(&solid);
                st.line_join = sk::LineJoin::Round;
                for s in &g.strokes {
                    pm.stroke_path(s, &p, &st, t, clip);
                }
            }
        }
        AnnotationKind::Pen(pen) => {
            if let Some(path) = smooth_path(&pen.points) {
                let mut st = sk_stroke(&pen.stroke);
                st.line_join = sk::LineJoin::Round;
                if pen.points.len() <= 1 || crate::geom::dedup_points(&pen.points, 0.75).len() <= 1 {
                    st.line_cap = sk::LineCap::Round;
                }
                pm.stroke_path(&path, &paint(fade(pen.stroke.color, alpha)), &st, t, clip);
            }
        }
        AnnotationKind::Highlighter(h) => {
            if let Some(path) = smooth_path(&h.points) {
                let mut p = paint(fade(h.color, alpha));
                p.blend_mode = sk::BlendMode::Multiply;
                let st = sk::Stroke {
                    width: h.width.max(0.5),
                    line_cap: sk::LineCap::Butt,
                    line_join: sk::LineJoin::Round,
                    ..Default::default()
                };
                pm.stroke_path(&path, &p, &st, t, clip);
            }
        }
        AnnotationKind::Step(s) => {
            let r = s.radius.max(0.5);
            if let Some(c) = circle_path(s.center, r) {
                pm.fill_path(&c, &paint(fade(s.fill, alpha)), sk::FillRule::Winding, t, clip);
                if let Some(b) = &s.border
                    && b.width > 0.0 {
                        pm.stroke_path(&c, &paint(fade(b.color, alpha)), &sk_stroke(b), t, clip);
                    }
            }
            let label = s.label_text();
            if !label.is_empty() {
                let (layout, mut style) = step_label_layout(&label, r, &s.font_family, s.font_weight);
                style.color = fade(s.text_color, alpha);
                if let Some(ink) = layout.ink_bounds {
                    let dx = s.center.x - (ink.x + ink.width / 2.0);
                    let dy = s.center.y - (ink.y + ink.height / 2.0);
                    draw_text_layout(pm, &layout, &style, t.pre_translate(dx, dy), clip);
                }
            }
        }
        AnnotationKind::Callout(c) => {
            if let Some(path) = callout_path(callout_bubble_rect(c), c.corner_radius, c.tail, callout_base_max(scale_factor)) {
                fill_stroke_shape(pm, &path, &c.style, t, clip, alpha);
            }
            if !c.text.is_empty() {
                let ta = callout_text(c);
                let layout = text_layout(&ta);
                let mut style = ta.style.clone();
                style.color = fade(style.color, alpha);
                draw_text_layout(pm, &layout, &style, t.pre_translate(ta.position.x, ta.position.y), clip);
            }
        }
        AnnotationKind::Text(ta) => {
            let layout = text_layout(ta);
            let mut style = ta.style.clone();
            if alpha < 0.999 {
                style.color = fade(style.color, alpha);
            }
            draw_text_layout(pm, &layout, &style, t.pre_translate(ta.position.x, ta.position.y), clip);
        }
        // Handled by the pipeline (need canvas read-back).
        AnnotationKind::Spotlight(_)
        | AnnotationKind::Magnifier(_)
        | AnnotationKind::Blur(_)
        | AnnotationKind::Pixelate(_)
        | AnnotationKind::Redact(_) => {}
    }
}

/// Map a content-space rect through a scale+translate transform.
pub(crate) fn map_rect(t: sk::Transform, r: Rect) -> Rect {
    let mut a = sk::Point::from_xy(r.x, r.y);
    let mut b = sk::Point::from_xy(r.right(), r.bottom());
    t.map_point(&mut a);
    t.map_point(&mut b);
    Rect::from_points(Point::new(a.x, a.y), Point::new(b.x, b.y))
}

pub(crate) fn map_point(t: sk::Transform, p: Point) -> Point {
    let mut a = sk::Point::from_xy(p.x, p.y);
    t.map_point(&mut a);
    Point::new(a.x, a.y)
}
