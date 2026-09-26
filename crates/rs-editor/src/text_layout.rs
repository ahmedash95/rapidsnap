//! Inline text editor geometry. The text being edited is *rendered* by
//! rs-render (same engine, layout and glyphs as the committed result); this
//! module maps rs-render's caret / selection / hit-test geometry
//! (`caret_rect`, `selection_rects`, `text_hit_index`, `text_box`) into
//! window px for the GPUI overlays (caret, selection, IME marked text) and
//! for the platform input handler (`bounds_for_range`).

use gpui_kit::*;
use rs_core::{FontFamily, Point as CPoint, Rect as CRect, TextAnnotation, TextStyle};
use rs_render::CanvasLayout;

/// Geometry of the text being edited, as painted last frame.
#[derive(Clone)]
pub struct EditGeom {
    /// The exact text box rs-render draws (for callouts: `callout_text`).
    pub ann: TextAnnotation,
    pub layout1: CanvasLayout,
    /// Window position of canvas px (0, 0) at scale 1.
    pub origin: Point<Pixels>,
    /// Window px per canvas px (scale 1).
    pub k: f32,
}

impl EditGeom {
    pub fn to_window(&self, c: CPoint) -> Point<Pixels> {
        let q = self.layout1.image_to_canvas(c);
        point(self.origin.x + px(q.x * self.k), self.origin.y + px(q.y * self.k))
    }

    pub fn to_content(&self, p: Point<Pixels>) -> CPoint {
        let k = self.k.max(1e-6);
        let c = CPoint::new(f32::from(p.x - self.origin.x) / k, f32::from(p.y - self.origin.y) / k);
        self.layout1.canvas_to_image(c)
    }

    pub fn rect_to_window(&self, r: CRect) -> Bounds<Pixels> {
        let a = self.to_window(r.origin());
        let b = self.to_window(CPoint::new(r.right(), r.bottom()));
        Bounds::from_corners(point(a.x.min(b.x), a.y.min(b.y)), point(a.x.max(b.x), a.y.max(b.y)))
    }

    /// Text box (window px).
    pub fn box_bounds(&self) -> Bounds<Pixels> {
        self.rect_to_window(rs_render::text_box(&self.ann))
    }

    /// Caret rect in content space.
    pub fn caret_content(&self, idx: usize) -> CRect {
        rs_render::caret_rect(&self.ann, self.clamp(idx))
    }

    /// Caret rect (window px, full line height).
    pub fn caret(&self, idx: usize) -> Bounds<Pixels> {
        self.rect_to_window(self.caret_content(idx))
    }

    /// Closest caret index (char boundary) for a window point.
    pub fn index_at(&self, p: Point<Pixels>) -> usize {
        self.clamp(rs_render::text_hit_index(&self.ann, self.to_content(p)))
    }

    /// Index one visual line up/down from `idx`, keeping x; `None` at the
    /// first / last line.
    pub fn vertical(&self, idx: usize, down: bool) -> Option<usize> {
        let c = self.caret_content(idx);
        let b = rs_render::text_box(&self.ann);
        let y = if down { c.bottom() + c.height * 0.5 } else { c.y - c.height * 0.5 };
        if y < b.y || y > b.bottom() {
            return None;
        }
        let p = CPoint::new(c.center().x, y);
        Some(self.clamp(rs_render::text_hit_index(&self.ann, p)))
    }

    /// Selection rectangles (window px) for a byte range.
    pub fn selection_rects(&self, range: std::ops::Range<usize>) -> Vec<Bounds<Pixels>> {
        let r = self.clamp(range.start)..self.clamp(range.end);
        rs_render::selection_rects(&self.ann, r).into_iter().map(|r| self.rect_to_window(r)).collect()
    }

    /// Screen bounds of a range for the IME candidate window: union of the
    /// selection rects, or the caret for an empty range.
    pub fn bounds_for_range(&self, range: std::ops::Range<usize>) -> Bounds<Pixels> {
        let rects = self.selection_rects(range.clone());
        let mut it = rects.into_iter();
        match it.next() {
            Some(first) => it.fold(first, |a, b| a.union(&b)),
            None => self.caret(range.start),
        }
    }

    fn clamp(&self, i: usize) -> usize {
        let t = &self.ann.content;
        t.floor_char_boundary(i.min(t.len()))
    }
}

pub fn gpui_font(style: &TextStyle) -> Font {
    let family: SharedString = match &style.family {
        FontFamily::System => ".SystemUIFont".into(),
        FontFamily::Monospace => "Menlo".into(),
        FontFamily::Serif => "Times New Roman".into(),
        FontFamily::Named(n) => n.clone().into(),
    };
    Font {
        family,
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight(style.weight.0 as f32),
        style: if style.italic { FontStyle::Italic } else { FontStyle::Normal },
    }
}

pub fn to_hsla(c: rs_core::Color) -> Hsla {
    Hsla::from(Rgba { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: c.a as f32 / 255.0 })
}

pub fn from_hsla(h: Hsla) -> rs_core::Color {
    let c = Rgba::from(h);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    rs_core::Color::rgba(q(c.r), q(c.g), q(c.b), q(c.a))
}
