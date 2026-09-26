//! Canvas layout: maps content space (annotation coordinates) to output
//! canvas pixels, accounting for beautify padding / inset / chrome / aspect
//! ratio. This is the single source of truth used by both the renderer and
//! the editor's hit-testing.

use rs_core::{AspectRatio, Document, Point, Rect, Size};

/// Result of [`compute_layout`]. All rects are in **output canvas px**
/// (origin top-left of the rendered image).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasLayout {
    /// Output image size in px (integers; `ceil` of the float layout).
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// Output px per content px (the `RenderOptions::scale` used).
    pub scale: f32,
    /// Output px per logical point = `scale_factor * scale`. Multiply any
    /// beautify/pt value by this to get output px.
    pub px_per_pt: f32,
    /// The whole framed card (chrome + inset + content). Gets the frame
    /// shadow and `corner_radius` clipping. Equals `content_rect` when
    /// beautify is disabled.
    pub frame_rect: Rect,
    /// Title bar area at the top of `frame_rect` (None if no chrome).
    pub chrome_rect: Option<Rect>,
    /// Area below the chrome, filled with the inset edge color (equals
    /// `content_rect` if inset disabled).
    pub inset_rect: Rect,
    /// Where content space (0,0)..(content_w, content_h) is drawn.
    pub content_rect: Rect,
    /// Frame corner radius in output px (already clamped to half the smaller
    /// frame side). 0 when beautify disabled.
    pub corner_radius: f32,
}

impl CanvasLayout {
    pub fn canvas_size(&self) -> Size {
        Size::new(self.canvas_width as f32, self.canvas_height as f32)
    }

    /// Content-space point → canvas px.
    pub fn image_to_canvas(&self, p: Point) -> Point {
        Point::new(self.content_rect.x + p.x * self.scale, self.content_rect.y + p.y * self.scale)
    }

    /// Canvas px → content-space point (may be outside the content bounds).
    pub fn canvas_to_image(&self, p: Point) -> Point {
        Point::new((p.x - self.content_rect.x) / self.scale, (p.y - self.content_rect.y) / self.scale)
    }

    /// Content-space rect → canvas px rect.
    pub fn image_rect_to_canvas(&self, r: Rect) -> Rect {
        let o = self.image_to_canvas(r.origin());
        Rect::new(o.x, o.y, r.width * self.scale, r.height * self.scale)
    }

    /// Canvas px rect → content-space rect.
    pub fn canvas_rect_to_image(&self, r: Rect) -> Rect {
        let o = self.canvas_to_image(r.origin());
        Rect::new(o.x, o.y, r.width / self.scale, r.height / self.scale)
    }

    /// Content-space length → canvas px.
    pub fn len_to_canvas(&self, l: f32) -> f32 {
        l * self.scale
    }
}

/// Auto-balance padding in pt for a content size in pt:
/// `clamp(0.08 * max(w, h), 16, 160)`.
pub fn auto_balance_padding_pt(content_pt: Size) -> f32 {
    (0.08 * content_pt.width.max(content_pt.height)).clamp(16.0, 160.0)
}

/// Compute the canvas layout of `doc` rendered at `scale` output px per
/// content px.
///
/// Algorithm (all in output px, `k = scale_factor * scale` px/pt):
/// 1. content = `doc.content_size() * scale`.
/// 2. If `!beautify.enabled`: canvas = content, everything = content rect.
/// 3. inset = `inset.size * k` if enabled; chrome_h = `chrome.height_pt() * k`.
/// 4. frame = content + 2·inset (both axes) + chrome_h (height).
/// 5. padding = per-side `padding * k`, or auto-balance uniform padding.
/// 6. canvas = frame + padding; then grown (never shrunk) along one axis to
///    match the aspect ratio; extra space split equally on both sides.
/// 7. Frame origin rounded to whole px; canvas size = ceil.
pub fn compute_layout(doc: &Document, scale: f32) -> CanvasLayout {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let sf = if doc.metadata.scale_factor > 0.0 { doc.metadata.scale_factor } else { 1.0 };
    let k = sf * scale;
    let content = doc.content_size().scale(scale);
    let b = &doc.beautify;

    if !b.enabled {
        let r = Rect::new(0.0, 0.0, content.width, content.height);
        return CanvasLayout {
            canvas_width: content.width.ceil().max(1.0) as u32,
            canvas_height: content.height.ceil().max(1.0) as u32,
            scale,
            px_per_pt: k,
            frame_rect: r,
            chrome_rect: None,
            inset_rect: r,
            content_rect: r,
            corner_radius: 0.0,
        };
    }

    let inset = if b.inset.enabled { b.inset.size.max(0.0) * k } else { 0.0 };
    let chrome_h = b.chrome.height_pt() * k;
    let frame_w = content.width + 2.0 * inset;
    let frame_h = content.height + 2.0 * inset + chrome_h;

    let (pt, pr, pb, pl) = if b.auto_balance {
        let p = auto_balance_padding_pt(doc.content_size_pt()) * k;
        (p, p, p, p)
    } else {
        (
            b.padding.top.max(0.0) * k,
            b.padding.right.max(0.0) * k,
            b.padding.bottom.max(0.0) * k,
            b.padding.left.max(0.0) * k,
        )
    };

    let mut canvas_w = frame_w + pl + pr;
    let mut canvas_h = frame_h + pt + pb;
    if let Some(ratio) = b.aspect_ratio.ratio().filter(|_| b.aspect_ratio != AspectRatio::Free) {
        if canvas_w / canvas_h < ratio {
            canvas_w = canvas_h * ratio;
        } else {
            canvas_h = canvas_w / ratio;
        }
    }
    let extra_x = canvas_w - (frame_w + pl + pr);
    let extra_y = canvas_h - (frame_h + pt + pb);
    let fx = (pl + extra_x / 2.0).round();
    let fy = (pt + extra_y / 2.0).round();

    let frame_rect = Rect::new(fx, fy, frame_w, frame_h);
    let chrome_rect = (chrome_h > 0.0).then(|| Rect::new(fx, fy, frame_w, chrome_h));
    let inset_rect = Rect::new(fx, fy + chrome_h, frame_w, frame_h - chrome_h);
    let content_rect = Rect::new(fx + inset, fy + chrome_h + inset, content.width, content.height);
    let corner_radius = (b.corner_radius.max(0.0) * k).min(frame_w.min(frame_h) / 2.0);

    CanvasLayout {
        canvas_width: canvas_w.ceil().max(1.0) as u32,
        canvas_height: canvas_h.ceil().max(1.0) as u32,
        scale,
        px_per_pt: k,
        frame_rect,
        chrome_rect,
        inset_rect,
        content_rect,
        corner_radius,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::*;

    fn doc(w: u32, h: u32, sf: f32) -> Document {
        Document::new(RgbaImage::filled(w, h, [0, 0, 0, 255]), DocumentMetadata { scale_factor: sf, ..Default::default() })
    }

    #[test]
    fn disabled_is_identity() {
        let mut d = doc(100, 50, 2.0);
        d.beautify = BeautifySettings::disabled();
        let l = compute_layout(&d, 1.0);
        assert_eq!((l.canvas_width, l.canvas_height), (100, 50));
        assert_eq!(l.image_to_canvas(Point::new(3.0, 4.0)), Point::new(3.0, 4.0));
    }

    #[test]
    fn padding_scales_with_points() {
        let mut d = doc(200, 100, 2.0);
        d.beautify.padding = Padding::uniform(10.0);
        d.beautify.inset = Inset { enabled: true, size: 5.0, color: InsetColor::Auto };
        d.beautify.chrome = WindowChrome::None;
        // scale 1 → 2 px/pt → padding 20, inset 10.
        let l = compute_layout(&d, 1.0);
        assert_eq!((l.canvas_width, l.canvas_height), (200 + 20 * 2 + 10 * 2, 100 + 20 * 2 + 10 * 2));
        assert_eq!(l.content_rect, Rect::new(30.0, 30.0, 200.0, 100.0));
        let p = Point::new(12.5, 7.0);
        let back = l.canvas_to_image(l.image_to_canvas(p));
        assert!((back.x - p.x).abs() < 1e-4 && (back.y - p.y).abs() < 1e-4);
        // Export 1x of a Retina doc: scale 0.5 → canvas halves.
        let l2 = compute_layout(&d, 0.5);
        assert_eq!((l2.canvas_width, l2.canvas_height), (130, 80));
    }

    #[test]
    fn aspect_ratio_grows_canvas() {
        let mut d = doc(100, 100, 1.0);
        d.beautify.padding = Padding::uniform(0.0);
        d.beautify.aspect_ratio = AspectRatio::SixteenNine;
        let l = compute_layout(&d, 1.0);
        assert_eq!(l.canvas_height, 100);
        assert_eq!(l.canvas_width, 178);
        assert_eq!(l.content_rect.x, 39.0);
    }
}
