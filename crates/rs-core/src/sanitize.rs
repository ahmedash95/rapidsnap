//! Input sanitizing for data that comes from outside the process
//! (`.rsnap` projects, history entries, preferences, pasted annotations).
//!
//! Every `sanitize` method is **idempotent**, never fails and never panics:
//! non-finite floats are replaced by a neutral value, lengths are clamped to
//! non-negative, sane ranges ([`limits`]), rects are normalized, and duplicate
//! ids get fresh UUIDs. Call it right after deserializing untrusted data;
//! values produced by the UI are already in range and pass through unchanged.

use std::collections::HashSet;

use crate::{
    Annotation, AnnotationId, AnnotationKind, Background, BeautifySettings, CaptureRegion, Color, Document,
    ExportFormat, ExportSettings, FontWeight, FrameShadow, GradientStop, Hotkey, Hotkeys, Inset, Padding, Point,
    Preferences, Rect, Shadow, ShapeStyle, Size, Stroke, TextBackground, TextStyle, ToolStyle, Watermark,
    WatermarkContent, MAX_RECENT_COLORS,
};

/// Clamp ranges used by the sanitizers. Content lengths are px, beautify
/// lengths pt (see the owning types). Generous on purpose: they only catch
/// corrupt / malicious values, never something the UI can produce.
pub mod limits {
    /// Max |coordinate| outside the content bounds (px), as a multiple of the
    /// larger content side (plus a constant for tiny images). Annotations may
    /// legitimately live outside the image (over the beautify padding / inset
    /// / background); [`crate::Document::annotation_margin`] widens the window
    /// further when the beautify canvas is larger than this.
    pub const COORD_MARGIN_FACTOR: f32 = 4.0;
    pub const COORD_MARGIN_MIN: f32 = 4096.0;
    /// Absolute coordinate cap when no bounds are known.
    pub const MAX_COORD: f32 = 1_000_000.0;
    pub const MAX_STROKE_WIDTH: f32 = 512.0;
    pub const MAX_CORNER_RADIUS: f32 = 10_000.0;
    pub const MAX_SHADOW_BLUR: f32 = 512.0;
    pub const MAX_SHADOW_OFFSET: f32 = 4096.0;
    pub const MAX_BLUR_STRENGTH: f32 = 256.0;
    pub const MAX_PIXELATE_BLOCK: u32 = 1024;
    pub const MIN_FONT_SIZE: f32 = 1.0;
    pub const MAX_FONT_SIZE: f32 = 2048.0;
    pub const MIN_LINE_HEIGHT: f32 = 0.5;
    pub const MAX_LINE_HEIGHT: f32 = 5.0;
    pub const MAX_TEXT_PADDING: f32 = 2048.0;
    pub const MAX_TEXT_CHARS: usize = 100_000;
    pub const MAX_STEP_RADIUS: f32 = 4096.0;
    pub const MIN_MAGNIFIER_ZOOM: f32 = 1.0;
    pub const MAX_MAGNIFIER_ZOOM: f32 = 8.0;
    pub const MAX_MAGNIFIER_RADIUS: f32 = 4096.0;
    pub const MIN_HEAD_SCALE: f32 = 0.1;
    pub const MAX_HEAD_SCALE: f32 = 10.0;
    pub const MAX_HIGHLIGHTER_WIDTH: f32 = 1024.0;
    pub const MAX_POINTS: usize = 200_000;
    // beautify (pt)
    pub const MAX_PADDING: f32 = 2000.0;
    pub const MAX_INSET: f32 = 1000.0;
    pub const MAX_FRAME_RADIUS: f32 = 1000.0;
    pub const MAX_FRAME_SHADOW: f32 = 1000.0;
    pub const MAX_FRAME_SHADOW_OFFSET: f32 = 1000.0;
    pub const MAX_GRADIENT_STOPS: usize = 64;
    pub const MAX_WALLPAPER_BLUR: f32 = 500.0;
    pub const MAX_BRIGHTNESS: f32 = 4.0;
    pub const MAX_WATERMARK_MARGIN: f32 = 2000.0;
    pub const MAX_WATERMARK_WIDTH: f32 = 10_000.0;
    // document / prefs
    pub const MIN_SCALE_FACTOR: f32 = 0.25;
    pub const MAX_SCALE_FACTOR: f32 = 8.0;
    pub const MAX_HISTORY_LIMIT: u32 = 1000;
    pub const MAX_STYLE_PRESETS: usize = 500;
    pub const MAX_FILENAME_TEMPLATE_CHARS: usize = 255;
    /// Max global-coordinate magnitude of a remembered capture region (pt).
    pub const MAX_SCREEN_COORD: f32 = 100_000.0;
}

use limits::*;

/// Finite value clamped to `[lo, hi]`; NaN → `fallback` (then clamped),
/// ±inf → the respective bound.
pub fn clamp_f32(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_nan() {
        fallback.clamp(lo, hi)
    } else {
        v.clamp(lo, hi)
    }
}

/// Clamp a length to `[0, max]` (NaN → 0).
fn len(v: f32, max: f32) -> f32 {
    clamp_f32(v, 0.0, max, 0.0)
}

/// Coordinate clamp window for content `bounds`.
#[derive(Debug, Clone, Copy)]
struct CoordBox {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl CoordBox {
    fn new(bounds: Size, extra_margin: f32) -> Self {
        let w = clamp_f32(bounds.width, 0.0, MAX_COORD, 0.0);
        let h = clamp_f32(bounds.height, 0.0, MAX_COORD, 0.0);
        let m = (w.max(h) * COORD_MARGIN_FACTOR).max(COORD_MARGIN_MIN);
        let extra = if extra_margin.is_finite() { extra_margin.clamp(0.0, MAX_COORD) } else { 0.0 };
        let m = m.max(extra);
        Self { x0: -m, y0: -m, x1: w + m, y1: h + m }
    }

    fn point(&self, p: Point) -> Point {
        Point::new(clamp_f32(p.x, self.x0, self.x1, 0.0), clamp_f32(p.y, self.y0, self.y1, 0.0))
    }

    /// Normalized rect with finite, clamped corners (non-finite size → 0).
    fn rect(&self, r: Rect) -> Rect {
        let w = if r.width.is_finite() { r.width } else { 0.0 };
        let h = if r.height.is_finite() { r.height } else { 0.0 };
        let a = self.point(Point::new(r.x, r.y));
        let b = self.point(Point::new(a.x + w, a.y + h));
        Rect::from_points(a, b)
    }
}

fn color_alpha_opacity(v: f32) -> f32 {
    clamp_f32(v, 0.0, 1.0, 1.0)
}

impl Stroke {
    /// Clamp width to `[0, MAX_STROKE_WIDTH]` (NaN → default 4).
    pub fn sanitize(&mut self) {
        self.width = clamp_f32(self.width, 0.0, MAX_STROKE_WIDTH, 4.0);
    }
}

impl ShapeStyle {
    pub fn sanitize(&mut self) {
        self.stroke.sanitize();
    }
}

impl Shadow {
    /// Non-negative blur, bounded offsets.
    pub fn sanitize(&mut self) {
        self.blur = len(self.blur, MAX_SHADOW_BLUR);
        self.offset_x = clamp_f32(self.offset_x, -MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET, 0.0);
        self.offset_y = clamp_f32(self.offset_y, -MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET, 0.0);
    }
}

impl FontWeight {
    /// Clamped to 100..=900.
    pub fn sanitized(self) -> FontWeight {
        FontWeight(self.0.clamp(100, 900))
    }
}

impl TextBackground {
    pub fn sanitize(&mut self) {
        match self {
            TextBackground::None => {}
            TextBackground::Pill { corner_radius, padding_x, padding_y, .. }
            | TextBackground::SemiTransparent { corner_radius, padding_x, padding_y, .. } => {
                *corner_radius = len(*corner_radius, MAX_CORNER_RADIUS);
                *padding_x = len(*padding_x, MAX_TEXT_PADDING);
                *padding_y = len(*padding_y, MAX_TEXT_PADDING);
            }
            TextBackground::Outline { width, .. } => *width = len(*width, MAX_STROKE_WIDTH),
        }
    }
}

impl TextStyle {
    /// Font size `[1, 2048]` (NaN → 24), line height `[0.5, 5]` (NaN → 1.2),
    /// weight 100..=900, background & shadow clamped.
    pub fn sanitize(&mut self) {
        self.size = clamp_f32(self.size, MIN_FONT_SIZE, MAX_FONT_SIZE, 24.0);
        self.line_height = clamp_f32(self.line_height, MIN_LINE_HEIGHT, MAX_LINE_HEIGHT, 1.2);
        self.weight = self.weight.sanitized();
        self.background.sanitize();
        if let Some(s) = &mut self.shadow {
            s.sanitize();
        }
    }
}

fn truncate_chars(s: &mut String, max: usize) {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
    }
}

fn sanitize_points(points: &mut Vec<Point>, b: &CoordBox) {
    points.retain(|p| p.x.is_finite() && p.y.is_finite());
    points.truncate(MAX_POINTS);
    for p in points.iter_mut() {
        *p = b.point(*p);
    }
}

impl Annotation {
    /// Make an annotation safe to render/edit: every float finite and within
    /// [`limits`], rects normalized, coordinates within `bounds` (content size
    /// in px) outset by a generous margin, opacity `[0, 1]`. Does **not**
    /// change the id (see [`Document::sanitize`] for duplicate ids).
    ///
    /// Use on anything deserialized from outside (projects, history, pasted
    /// `AnnotationClipboard` payloads).
    pub fn sanitize(&mut self, bounds: Size) {
        self.sanitize_with_margin(bounds, 0.0);
    }

    /// [`Annotation::sanitize`] with the coordinate window widened to at
    /// least `margin` px around `bounds` (e.g.
    /// [`Document::annotation_margin`], so annotations drawn over a large
    /// beautify padding survive). Non-finite / negative margins count as 0;
    /// the window never exceeds [`limits::MAX_COORD`].
    pub fn sanitize_with_margin(&mut self, bounds: Size, margin: f32) {
        let b = CoordBox::new(bounds, margin);
        self.opacity = color_alpha_opacity(self.opacity);
        if let Some(s) = &mut self.shadow {
            s.sanitize();
        }
        match &mut self.kind {
            AnnotationKind::Rect(a) => {
                a.rect = b.rect(a.rect);
                a.style.sanitize();
                a.corner_radius = len(a.corner_radius, MAX_CORNER_RADIUS);
            }
            AnnotationKind::Ellipse(a) => {
                a.rect = b.rect(a.rect);
                a.style.sanitize();
            }
            AnnotationKind::Line(a) => {
                a.start = b.point(a.start);
                a.end = b.point(a.end);
                a.stroke.sanitize();
            }
            AnnotationKind::Arrow(a) => {
                a.start = b.point(a.start);
                a.end = b.point(a.end);
                a.control = a.control.filter(|c| c.x.is_finite() && c.y.is_finite()).map(|c| b.point(c));
                a.stroke.sanitize();
                a.head_scale = clamp_f32(a.head_scale, MIN_HEAD_SCALE, MAX_HEAD_SCALE, 1.0);
            }
            AnnotationKind::Pen(a) => {
                sanitize_points(&mut a.points, &b);
                a.stroke.sanitize();
            }
            AnnotationKind::Highlighter(a) => {
                sanitize_points(&mut a.points, &b);
                a.width = clamp_f32(a.width, 1.0, MAX_HIGHLIGHTER_WIDTH, 20.0);
            }
            AnnotationKind::Step(a) => {
                a.center = b.point(a.center);
                a.radius = clamp_f32(a.radius, 1.0, MAX_STEP_RADIUS, 14.0);
                a.value = a.value.max(1);
                if let Some(s) = &mut a.border {
                    s.sanitize();
                }
                a.font_weight = a.font_weight.sanitized();
            }
            AnnotationKind::Callout(a) => {
                a.rect = b.rect(a.rect);
                a.tail = b.point(a.tail);
                a.corner_radius = len(a.corner_radius, MAX_CORNER_RADIUS);
                a.style.sanitize();
                truncate_chars(&mut a.text, MAX_TEXT_CHARS);
                a.text_style.sanitize();
                a.padding = len(a.padding, MAX_TEXT_PADDING);
            }
            AnnotationKind::Spotlight(a) => {
                a.rect = b.rect(a.rect);
                a.corner_radius = len(a.corner_radius, MAX_CORNER_RADIUS);
            }
            AnnotationKind::Magnifier(a) => {
                a.source_center = b.point(a.source_center);
                a.target_center = b.point(a.target_center);
                a.source_radius = clamp_f32(a.source_radius, 1.0, MAX_MAGNIFIER_RADIUS, 40.0);
                a.zoom = clamp_f32(a.zoom, MIN_MAGNIFIER_ZOOM, MAX_MAGNIFIER_ZOOM, 2.0);
                a.border.sanitize();
            }
            AnnotationKind::Blur(a) => {
                a.rect = b.rect(a.rect);
                a.strength = clamp_f32(a.strength, 0.0, MAX_BLUR_STRENGTH, 12.0);
                a.corner_radius = len(a.corner_radius, MAX_CORNER_RADIUS);
            }
            AnnotationKind::Pixelate(a) => {
                a.rect = b.rect(a.rect);
                a.block_size = a.block_size.clamp(1, MAX_PIXELATE_BLOCK);
            }
            AnnotationKind::Redact(a) => {
                a.rect = b.rect(a.rect);
                a.corner_radius = len(a.corner_radius, MAX_CORNER_RADIUS);
            }
            AnnotationKind::Text(a) => {
                a.position = b.point(a.position);
                a.width = a.width.filter(|w| w.is_finite() && *w > 0.0).map(|w| w.clamp(1.0, b.x1 - b.x0));
                truncate_chars(&mut a.content, MAX_TEXT_CHARS);
                a.style.sanitize();
            }
        }
    }

    /// Sanitize a batch of annotations (e.g. a paste) for a document with
    /// content size `bounds`, giving a fresh id to any annotation whose id is
    /// already in `taken` or repeated within the batch. `taken` is extended.
    pub fn sanitize_all(annotations: &mut [Annotation], bounds: Size, taken: &mut HashSet<AnnotationId>) {
        Self::sanitize_all_with_margin(annotations, bounds, 0.0, taken);
    }

    /// [`Annotation::sanitize_all`] with a widened coordinate window (see
    /// [`Annotation::sanitize_with_margin`]).
    pub fn sanitize_all_with_margin(
        annotations: &mut [Annotation],
        bounds: Size,
        margin: f32,
        taken: &mut HashSet<AnnotationId>,
    ) {
        for a in annotations {
            a.sanitize_with_margin(bounds, margin);
            while !taken.insert(a.id) {
                a.id = AnnotationId::new();
            }
        }
    }
}

impl GradientStop {
    pub fn sanitize(&mut self) {
        self.offset = clamp_f32(self.offset, 0.0, 1.0, 0.0);
    }
}

fn sanitize_stops(stops: &mut Vec<GradientStop>) -> bool {
    stops.truncate(MAX_GRADIENT_STOPS);
    stops.iter_mut().for_each(GradientStop::sanitize);
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    !stops.is_empty()
}

impl Background {
    /// Finite gradient geometry (angle wrapped to 0..360, center −1..2,
    /// radius 0.01..10), wallpaper blur/brightness bounded. A gradient with
    /// no stops becomes solid white.
    pub fn sanitize(&mut self) {
        match self {
            Background::None | Background::Solid { .. } | Background::Wallpaper { .. } | Background::CustomImage { .. } => {}
            Background::LinearGradient { stops, angle_deg } => {
                *angle_deg = if angle_deg.is_finite() { angle_deg.rem_euclid(360.0) } else { 0.0 };
                if !sanitize_stops(stops) {
                    *self = Background::Solid { color: Color::WHITE };
                }
            }
            Background::RadialGradient { stops, center, radius } => {
                center[0] = clamp_f32(center[0], -1.0, 2.0, 0.5);
                center[1] = clamp_f32(center[1], -1.0, 2.0, 0.5);
                *radius = clamp_f32(*radius, 0.01, 10.0, 1.0);
                if !sanitize_stops(stops) {
                    *self = Background::Solid { color: Color::WHITE };
                }
            }
            Background::DesktopWallpaper { blur, brightness } => {
                *blur = len(*blur, MAX_WALLPAPER_BLUR);
                *brightness = clamp_f32(*brightness, 0.0, MAX_BRIGHTNESS, 1.0);
            }
        }
    }
}

impl Padding {
    pub fn sanitize(&mut self) {
        self.top = len(self.top, MAX_PADDING);
        self.right = len(self.right, MAX_PADDING);
        self.bottom = len(self.bottom, MAX_PADDING);
        self.left = len(self.left, MAX_PADDING);
    }
}

impl Inset {
    pub fn sanitize(&mut self) {
        self.size = len(self.size, MAX_INSET);
    }
}

impl FrameShadow {
    pub fn sanitize(&mut self) {
        self.size = len(self.size, MAX_FRAME_SHADOW);
        self.opacity = clamp_f32(self.opacity, 0.0, 1.0, 0.45);
        self.offset_x = clamp_f32(self.offset_x, -MAX_FRAME_SHADOW_OFFSET, MAX_FRAME_SHADOW_OFFSET, 0.0);
        self.offset_y = clamp_f32(self.offset_y, -MAX_FRAME_SHADOW_OFFSET, MAX_FRAME_SHADOW_OFFSET, 0.0);
    }
}

impl Watermark {
    pub fn sanitize(&mut self) {
        self.opacity = clamp_f32(self.opacity, 0.0, 1.0, 1.0);
        self.margin = len(self.margin, MAX_WATERMARK_MARGIN);
        match &mut self.content {
            WatermarkContent::Text { text, style } => {
                truncate_chars(text, 1000);
                style.sanitize();
            }
            WatermarkContent::Image { width, .. } => *width = clamp_f32(*width, 1.0, MAX_WATERMARK_WIDTH, 120.0),
        }
    }
}

impl BeautifySettings {
    /// Clamp padding / inset / radius / shadow / background / watermark.
    pub fn sanitize(&mut self) {
        self.background.sanitize();
        self.padding.sanitize();
        self.inset.sanitize();
        self.corner_radius = len(self.corner_radius, MAX_FRAME_RADIUS);
        self.shadow.sanitize();
        if let Some(w) = &mut self.watermark {
            w.sanitize();
        }
    }
}

impl Document {
    /// Make a loaded document safe: beautify clamped, crop clamped to the
    /// base image (empty crop → none), scale factor in `[0.25, 8]` (bad → 1),
    /// every annotation sanitized against the content size, and duplicate
    /// annotation ids replaced by fresh UUIDs (first occurrence keeps its id).
    pub fn sanitize(&mut self) {
        let (w, h) = (self.base_image.width, self.base_image.height);
        self.image_transform.crop = self.image_transform.crop.and_then(|c| c.clamp_to(w, h));
        let sf = self.metadata.scale_factor;
        self.metadata.scale_factor =
            if sf.is_finite() && sf > 0.0 { sf.clamp(MIN_SCALE_FACTOR, MAX_SCALE_FACTOR) } else { 1.0 };
        self.beautify.sanitize();
        let bounds = self.content_size();
        let margin = self.annotation_margin();
        let mut taken = HashSet::with_capacity(self.annotations.len());
        Annotation::sanitize_all_with_margin(&mut self.annotations, bounds, margin, &mut taken);
    }

    /// Conservative distance (content px) from the image to the farthest
    /// edge of the beautify canvas: the whole canvas size (content + inset +
    /// chrome + padding, grown to the aspect ratio) at scale 1. Every canvas
    /// point lies within this margin of the content rect, so annotations
    /// placed anywhere on the canvas (over the padding / background) pass
    /// [`Annotation::sanitize_with_margin`] unchanged. 0 when beautify is off.
    /// Pure arithmetic (mirrors `rs_render::compute_layout`, but with the
    /// auto-balance padding at its 160 pt maximum), never depends on the
    /// annotations.
    pub fn annotation_margin(&self) -> f32 {
        let b = &self.beautify;
        if !b.enabled {
            return 0.0;
        }
        let sf = self.metadata.scale_factor;
        let sf = if sf.is_finite() && sf > 0.0 { sf.min(MAX_SCALE_FACTOR) } else { 1.0 };
        let c = self.content_size();
        let pt = |v: f32, max: f32| len(v, max) * sf;
        let inset = if b.inset.enabled { pt(b.inset.size, MAX_INSET) } else { 0.0 };
        let chrome = b.chrome.height_pt().max(0.0) * sf;
        let (pl, pr, ptop, pb) = if b.auto_balance {
            let p = 160.0 * sf;
            (p, p, p, p)
        } else {
            (
                pt(b.padding.left, MAX_PADDING),
                pt(b.padding.right, MAX_PADDING),
                pt(b.padding.top, MAX_PADDING),
                pt(b.padding.bottom, MAX_PADDING),
            )
        };
        let mut w = c.width + 2.0 * inset + pl + pr;
        let mut h = c.height + 2.0 * inset + chrome + ptop + pb;
        if let Some(r) = b.aspect_ratio.ratio().filter(|r| r.is_finite() && *r > 0.0) {
            if w / h.max(1e-6) < r {
                w = h * r;
            } else {
                h = w / r;
            }
        }
        let m = w.max(h);
        if m.is_finite() { m.clamp(0.0, MAX_COORD) } else { 0.0 }
    }
}

impl ToolStyle {
    /// Clamp every knob (logical points) to the same ranges as annotations.
    pub fn sanitize(&mut self) {
        self.stroke.sanitize();
        self.corner_radius = len(self.corner_radius, MAX_CORNER_RADIUS);
        self.opacity = color_alpha_opacity(self.opacity);
        if let Some(s) = &mut self.shadow {
            s.sanitize();
        }
        self.text.sanitize();
        self.highlighter_width = clamp_f32(self.highlighter_width, 1.0, MAX_HIGHLIGHTER_WIDTH, 20.0);
        self.step_radius = clamp_f32(self.step_radius, 1.0, MAX_STEP_RADIUS, 14.0);
        self.magnifier_zoom = clamp_f32(self.magnifier_zoom, MIN_MAGNIFIER_ZOOM, MAX_MAGNIFIER_ZOOM, 2.0);
        self.blur_strength = clamp_f32(self.blur_strength, 0.0, MAX_BLUR_STRENGTH, 12.0);
        self.pixelate_block = self.pixelate_block.clamp(1, MAX_PIXELATE_BLOCK);
        self.callout_padding = len(self.callout_padding, MAX_TEXT_PADDING);
    }
}

/// Named (non-character) W3C key codes accepted for global hotkeys.
const NAMED_KEYS: &[&str] = &[
    "Space", "Enter", "Tab", "Backspace", "Delete", "Escape", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown",
    "Home", "End", "PageUp", "PageDown", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash",
    "Semicolon", "Quote", "Comma", "Period", "Slash", "Backquote",
];

impl Hotkey {
    /// Whether `key` is an F-key `F1`..`F20`.
    pub fn is_function_key(&self) -> bool {
        self.key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| (1..=20).contains(&n))
    }

    /// Structurally valid global hotkey: a supported W3C code (`DigitN`,
    /// `KeyA`..`KeyZ`, `F1`..`F20`, or a named key such as `Space`) and at
    /// least one of ⌘ ⌥ ⌃ unless it's an F-key (same rule as the recorder).
    pub fn is_valid(&self) -> bool {
        let k = self.key.as_str();
        let single = |p: &str, f: fn(char) -> bool| {
            k.strip_prefix(p).is_some_and(|r| r.chars().count() == 1 && r.chars().all(f))
        };
        let known = single("Digit", |c| c.is_ascii_digit())
            || single("Key", |c| c.is_ascii_uppercase())
            || self.is_function_key()
            || NAMED_KEYS.contains(&k);
        let m = self.modifiers;
        known && (self.is_function_key() || m.cmd || m.alt || m.ctrl)
    }
}

impl Hotkeys {
    /// Invalid hotkeys fall back to that slot's default (`repeat_last` → none);
    /// a hotkey already used by an earlier slot (region, window, fullscreen,
    /// repeat_last order) is unassigned.
    pub fn sanitize(&mut self) {
        let defaults = Hotkeys::default();
        let mut seen: Vec<Hotkey> = Vec::new();
        let slots = [
            (&mut self.region, defaults.region),
            (&mut self.window, defaults.window),
            (&mut self.fullscreen, defaults.fullscreen),
            (&mut self.repeat_last, defaults.repeat_last),
        ];
        for (slot, default) in slots {
            if slot.as_ref().is_some_and(|h| !h.is_valid()) {
                *slot = default;
            }
            if let Some(h) = slot.as_ref() {
                if seen.contains(h) {
                    *slot = None;
                } else {
                    seen.push(h.clone());
                }
            }
        }
    }
}

impl ExportSettings {
    /// JPEG quality 1..=100, non-empty filename template (≤255 chars).
    pub fn sanitize(&mut self) {
        if let ExportFormat::Jpeg { quality } = &mut self.format {
            *quality = (*quality).clamp(1, 100);
        }
        truncate_chars(&mut self.filename_template, MAX_FILENAME_TEMPLATE_CHARS);
        if self.filename_template.trim().is_empty() {
            self.filename_template = ExportSettings::default().filename_template;
        }
    }
}

impl CaptureRegion {
    /// `None` if the rect isn't finite, is empty or absurdly far away.
    pub fn sanitized(self) -> Option<CaptureRegion> {
        let r = self.rect;
        let ok = [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite())
            && r.width >= 1.0
            && r.height >= 1.0
            && r.x.abs() <= MAX_SCREEN_COORD
            && r.y.abs() <= MAX_SCREEN_COORD
            && r.width <= MAX_SCREEN_COORD
            && r.height <= MAX_SCREEN_COORD;
        ok.then_some(self)
    }
}

impl Preferences {
    /// Clamp everything loaded from disk: hotkeys valid & unique, export
    /// settings, tool styles/presets (duplicate preset ids → fresh), recent
    /// colors deduped/truncated, `last_beautify` ranges (padding etc.), last
    /// capture region sane, `history_limit` ≤ 1000.
    pub fn sanitize(&mut self) {
        self.hotkeys.sanitize();
        self.export.sanitize();
        self.tool_styles.values_mut().for_each(ToolStyle::sanitize);
        self.style_presets.truncate(MAX_STYLE_PRESETS);
        let mut ids = HashSet::new();
        for p in &mut self.style_presets {
            p.style.sanitize();
            truncate_chars(&mut p.name, 200);
            while !ids.insert(p.id) {
                p.id = uuid::Uuid::new_v4();
            }
        }
        let mut seen = HashSet::new();
        self.recent_colors.retain(|c| seen.insert(*c));
        self.recent_colors.truncate(MAX_RECENT_COLORS);
        self.last_beautify.sanitize();
        self.last_capture_region = self.last_capture_region.and_then(CaptureRegion::sanitized);
        self.history_limit = self.history_limit.min(MAX_HISTORY_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{all_kinds, sample_document};
    use crate::*;

    const BAD: [f32; 5] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30];

    fn poison(a: &mut Annotation, v: f32) {
        a.opacity = v;
        a.shadow = Some(Shadow { blur: v, offset_x: v, offset_y: v, ..Shadow::default() });
        let p = Point::new(v, -v);
        let r = Rect::new(v, v, -v, v);
        let st = Stroke { width: v, ..Stroke::default() };
        match &mut a.kind {
            AnnotationKind::Rect(k) => {
                k.rect = r;
                k.corner_radius = v;
                k.style.stroke = st;
            }
            AnnotationKind::Ellipse(k) => k.rect = r,
            AnnotationKind::Line(k) => {
                k.start = p;
                k.end = p;
                k.stroke = st;
            }
            AnnotationKind::Arrow(k) => {
                k.start = p;
                k.control = Some(p);
                k.head_scale = v;
                k.stroke = st;
            }
            AnnotationKind::Pen(k) => k.points = vec![p, Point::new(1.0, 2.0)],
            AnnotationKind::Highlighter(k) => {
                k.points = vec![p];
                k.width = v;
            }
            AnnotationKind::Step(k) => {
                k.center = p;
                k.radius = v;
                k.value = 0;
                k.font_weight = FontWeight(u16::MAX);
            }
            AnnotationKind::Callout(k) => {
                k.rect = r;
                k.tail = p;
                k.padding = v;
                k.text_style.size = v;
                k.text_style.line_height = v;
            }
            AnnotationKind::Spotlight(k) => {
                k.rect = r;
                k.corner_radius = v;
            }
            AnnotationKind::Magnifier(k) => {
                k.source_center = p;
                k.source_radius = v;
                k.zoom = v;
                k.border = st;
            }
            AnnotationKind::Blur(k) => {
                k.rect = r;
                k.strength = v;
            }
            AnnotationKind::Pixelate(k) => {
                k.rect = r;
                k.block_size = if v.is_sign_negative() { 0 } else { u32::MAX };
            }
            AnnotationKind::Redact(k) => k.rect = r,
            AnnotationKind::Text(k) => {
                k.position = p;
                k.width = Some(v);
                k.style.size = v;
                k.style.background = TextBackground::Pill {
                    color: Color::RED,
                    corner_radius: v,
                    padding_x: v,
                    padding_y: v,
                };
            }
        }
    }

    fn check_sane(a: &Annotation, bounds: Size) {
        assert!((0.0..=1.0).contains(&a.opacity));
        let g = a.geometric_bounds();
        for v in [g.x, g.y, g.width, g.height] {
            assert!(v.is_finite(), "{a:?}");
        }
        assert!(g.width >= 0.0 && g.height >= 0.0);
        let lim = bounds.width.max(bounds.height) * 10.0 + 1e5;
        assert!(g.x.abs() < lim && g.y.abs() < lim && g.width < lim * 3.0, "{a:?}");
        if let Some(s) = a.shadow {
            assert!(s.blur >= 0.0 && s.blur <= limits::MAX_SHADOW_BLUR);
            assert!(s.offset_x.is_finite() && s.offset_y.is_finite());
        }
        match &a.kind {
            AnnotationKind::Rect(k) => {
                assert!(k.style.stroke.width >= 0.0 && k.style.stroke.width <= limits::MAX_STROKE_WIDTH);
                assert!(k.corner_radius >= 0.0);
            }
            AnnotationKind::Arrow(k) => {
                assert!(k.head_scale >= limits::MIN_HEAD_SCALE && k.head_length().is_finite());
            }
            AnnotationKind::Step(k) => {
                assert!(k.radius >= 1.0 && k.value >= 1 && k.font_weight.0 <= 900);
            }
            AnnotationKind::Magnifier(k) => {
                assert!((1.0..=8.0).contains(&k.zoom) && k.target_radius().is_finite());
            }
            AnnotationKind::Pixelate(k) => assert!(k.block_size >= 1 && k.block_size <= 1024),
            AnnotationKind::Blur(k) => assert!(k.strength >= 0.0 && k.strength <= 256.0),
            AnnotationKind::Text(k) => {
                assert!(k.style.size >= 1.0 && k.style.size <= 2048.0);
                if let Some(w) = k.width {
                    assert!(w.is_finite() && w >= 1.0);
                }
                if let TextBackground::Pill { corner_radius, padding_x, .. } = k.style.background {
                    assert!(corner_radius >= 0.0 && padding_x >= 0.0 && padding_x.is_finite());
                }
            }
            AnnotationKind::Callout(k) => {
                assert!(k.padding >= 0.0 && k.text_style.size >= 1.0 && k.text_style.line_height >= 0.5);
            }
            _ => {}
        }
    }

    #[test]
    fn poisoned_annotations_are_sanitized() {
        let bounds = Size::new(200.0, 100.0);
        for v in BAD {
            for k in all_kinds() {
                let mut a = Annotation::new(k);
                poison(&mut a, v);
                a.sanitize(bounds);
                check_sane(&a, bounds);
                let once = a.clone();
                a.sanitize(bounds);
                assert_eq!(once, a, "idempotent");
                // serde_json writes non-finite floats as `null`, which would
                // not deserialize back into f32: a lossless round-trip proves
                // every float is finite.
                let json = serde_json::to_string(&a).unwrap();
                let back: Annotation = serde_json::from_str(&json).unwrap();
                assert_eq!(back, a);
            }
        }
    }

    #[test]
    fn valid_annotations_unchanged() {
        let doc = sample_document();
        let mut d2 = doc.clone();
        d2.sanitize();
        assert_eq!(doc.annotations, d2.annotations);
        assert_eq!(doc.beautify, d2.beautify);
        let mut p = Preferences::default();
        p.sanitize();
        assert_eq!(p, Preferences::default());
    }

    #[test]
    fn duplicate_ids_get_fresh_uuid() {
        let mut doc = sample_document();
        let id = doc.annotations[0].id;
        for a in doc.annotations.iter_mut() {
            a.id = id;
        }
        doc.sanitize();
        let ids: HashSet<_> = doc.annotations.iter().map(|a| a.id).collect();
        assert_eq!(ids.len(), doc.annotations.len());
        assert_eq!(doc.annotations[0].id, id, "first occurrence keeps its id");
    }

    #[test]
    fn rect_normalized_and_nonfinite_points_dropped() {
        let mut a = Annotation::new(AnnotationKind::Blur(BlurAnnotation {
            rect: Rect::new(50.0, 50.0, -20.0, -30.0),
            strength: 5.0,
            corner_radius: -3.0,
        }));
        a.sanitize(Size::new(100.0, 100.0));
        let AnnotationKind::Blur(b) = &a.kind else { unreachable!() };
        assert_eq!(b.rect, Rect::new(30.0, 20.0, 20.0, 30.0));
        assert_eq!(b.corner_radius, 0.0);

        let mut p = Annotation::new(AnnotationKind::Pen(PenAnnotation {
            points: vec![Point::new(f32::NAN, 1.0), Point::new(1.0, 1.0), Point::new(2.0, f32::INFINITY)],
            stroke: Stroke::default(),
        }));
        p.sanitize(Size::new(100.0, 100.0));
        let AnnotationKind::Pen(k) = &p.kind else { unreachable!() };
        assert_eq!(k.points, vec![Point::new(1.0, 1.0)]);
    }

    #[test]
    fn document_fields_sanitized() {
        for v in BAD {
            let mut doc = sample_document();
            doc.metadata.scale_factor = v;
            doc.image_transform.crop = Some(IRect::new(500, 500, 10, 10));
            doc.beautify.padding = Padding { top: v, right: v, bottom: -5.0, left: 1e9, linked: false };
            doc.beautify.corner_radius = v;
            doc.beautify.shadow.size = v;
            doc.beautify.shadow.opacity = v;
            doc.beautify.inset.size = v;
            doc.beautify.background = Background::LinearGradient { stops: vec![], angle_deg: v };
            doc.beautify.watermark = Some(Watermark {
                content: WatermarkContent::Image { path: "x.png".into(), width: v },
                position: Corner::TopLeft,
                opacity: v,
                margin: v,
            });
            doc.sanitize();
            let sf = doc.metadata.scale_factor;
            assert!((limits::MIN_SCALE_FACTOR..=limits::MAX_SCALE_FACTOR).contains(&sf));
            assert_eq!(doc.image_transform.crop, None);
            let pd = doc.beautify.padding;
            for x in [pd.top, pd.right, pd.bottom, pd.left] {
                assert!((0.0..=limits::MAX_PADDING).contains(&x));
            }
            assert!((0.0..=limits::MAX_FRAME_RADIUS).contains(&doc.beautify.corner_radius));
            assert!((0.0..=1.0).contains(&doc.beautify.shadow.opacity));
            assert!(doc.beautify.shadow.size.is_finite() && doc.beautify.inset.size >= 0.0);
            assert_eq!(doc.beautify.background, Background::Solid { color: Color::WHITE });
            let w = doc.beautify.watermark.as_ref().unwrap();
            assert!((0.0..=1.0).contains(&w.opacity) && w.margin.is_finite());
        }
    }

    #[test]
    fn gradients_sanitized() {
        let mut b = Background::RadialGradient {
            stops: vec![GradientStop::new(f32::NAN, Color::RED), GradientStop::new(-3.0, Color::BLUE)],
            center: [f32::INFINITY, f32::NAN],
            radius: -1.0,
        };
        b.sanitize();
        let Background::RadialGradient { stops, center, radius } = b else { unreachable!() };
        assert!(stops.iter().all(|s| (0.0..=1.0).contains(&s.offset)));
        assert_eq!(center, [2.0, 0.5]);
        assert_eq!(radius, 0.01);
        let mut l = Background::LinearGradient { stops: vec![GradientStop::new(0.5, Color::RED)], angle_deg: -90.0 };
        l.sanitize();
        assert!(matches!(l, Background::LinearGradient { angle_deg, .. } if angle_deg == 270.0));
    }

    #[test]
    fn preferences_sanitized() {
        let mut p = Preferences::default();
        p.hotkeys.region = Some(Hotkey::new(Modifiers::default(), "KeyA")); // no modifier → default
        p.hotkeys.window = Some(Hotkey::new(Modifiers { cmd: true, ..Default::default() }, "Bogus")); // bad code → default
        p.hotkeys.fullscreen = p.hotkeys.window.clone(); // also invalid → its own default
        p.hotkeys.repeat_last = Hotkeys::default().region; // duplicate of region → unassigned
        p.export.format = ExportFormat::Jpeg { quality: 0 };
        p.export.filename_template = "   ".into();
        p.history_limit = u32::MAX;
        p.recent_colors = vec![Color::RED; 40];
        p.last_beautify.padding = Padding::uniform(f32::NAN);
        p.last_capture_region = Some(CaptureRegion { display_id: None, rect: Rect::new(0.0, 0.0, f32::NAN, 5.0) });
        let mut ts = ToolStyle::default();
        ts.stroke.width = f32::INFINITY;
        ts.text.size = -4.0;
        ts.magnifier_zoom = f32::NAN;
        ts.pixelate_block = 0;
        p.tool_styles.insert(ToolKind::Rect, ts.clone());
        let preset = StylePreset { id: uuid::Uuid::new_v4(), name: "a".into(), tool: None, style: ts };
        p.style_presets = vec![preset.clone(), preset];
        p.sanitize();

        let d = Hotkeys::default();
        assert_eq!(p.hotkeys.region, d.region);
        assert_eq!(p.hotkeys.window, d.window);
        assert_eq!(p.hotkeys.fullscreen, d.fullscreen);
        assert_eq!(p.hotkeys.repeat_last, None);
        assert_eq!(p.export.format, ExportFormat::Jpeg { quality: 1 });
        assert_eq!(p.export.filename_template, ExportSettings::default().filename_template);
        assert_eq!(p.history_limit, limits::MAX_HISTORY_LIMIT);
        assert_eq!(p.recent_colors, vec![Color::RED]);
        assert_eq!(p.last_beautify.padding.top, 0.0);
        assert_eq!(p.last_capture_region, None);
        let ts = &p.tool_styles[&ToolKind::Rect];
        assert_eq!(ts.stroke.width, limits::MAX_STROKE_WIDTH);
        assert_eq!(ts.text.size, limits::MIN_FONT_SIZE);
        assert_eq!(ts.magnifier_zoom, 2.0);
        assert_eq!(ts.pixelate_block, 1);
        assert_ne!(p.style_presets[0].id, p.style_presets[1].id);
    }

    #[test]
    fn hotkey_validity() {
        let cmd = Modifiers { cmd: true, ..Default::default() };
        assert!(Hotkey::new(cmd, "Digit2").is_valid());
        assert!(Hotkey::new(cmd, "KeyZ").is_valid());
        assert!(Hotkey::new(cmd, "Space").is_valid());
        assert!(Hotkey::new(Modifiers::default(), "F5").is_valid());
        assert!(!Hotkey::new(Modifiers::default(), "Digit2").is_valid());
        assert!(!Hotkey::new(Modifiers { shift: true, ..Default::default() }, "KeyA").is_valid());
        assert!(!Hotkey::new(cmd, "").is_valid());
        assert!(!Hotkey::new(cmd, "Keya").is_valid());
        assert!(!Hotkey::new(cmd, "F99").is_valid());
        assert!(!Hotkey::new(cmd, "Digit22").is_valid());
    }

    #[test]
    fn pasted_batch_ids() {
        let doc = sample_document();
        let mut taken: HashSet<_> = doc.annotations.iter().map(|a| a.id).collect();
        let mut paste = vec![doc.annotations[0].clone(), doc.annotations[0].clone()];
        Annotation::sanitize_all(&mut paste, doc.content_size(), &mut taken);
        assert_ne!(paste[0].id, doc.annotations[0].id);
        assert_ne!(paste[0].id, paste[1].id);
    }

    fn redact_at(r: Rect) -> Annotation {
        Annotation::new(AnnotationKind::Redact(RedactAnnotation { rect: r, color: Color::BLACK, corner_radius: 0.0 }))
    }

    fn redact_rect(a: &Annotation) -> Rect {
        match &a.kind {
            AnnotationKind::Redact(r) => r.rect,
            _ => unreachable!(),
        }
    }

    #[test]
    fn outside_image_coordinates_are_kept() {
        // Annotations over the beautify padding have negative / beyond-image
        // coordinates: they must survive sanitize (projects, history, paste).
        let mut doc = Document::new(RgbaImage::filled(200, 100, [255; 4]), DocumentMetadata { scale_factor: 2.0, ..Default::default() });
        doc.beautify.padding = Padding::uniform(120.0);
        let inside_padding = [
            Rect::new(-230.0, -200.0, 60.0, 40.0),
            Rect::new(210.0, 120.0, 50.0, 50.0),
            Rect::new(-10.0, 100.5, 220.0, 30.0),
        ];
        for r in inside_padding {
            doc.annotations.push(redact_at(r));
        }
        let mut pen = Annotation::new(AnnotationKind::Pen(PenAnnotation {
            points: vec![Point::new(-150.0, -150.0), Point::new(350.0, 250.0)],
            stroke: Stroke::default(),
        }));
        pen.sanitize(doc.content_size());
        doc.annotations.push(pen.clone());
        let before = doc.annotations.clone();
        doc.sanitize();
        assert_eq!(doc.annotations, before, "legit outside-image geometry unchanged");

        // A huge padding on a tiny image: the canvas reaches far beyond the
        // default ±4096 px window; Document::sanitize widens it.
        let mut doc = Document::new(RgbaImage::filled(40, 30, [255; 4]), DocumentMetadata { scale_factor: 4.0, ..Default::default() });
        doc.beautify.padding = Padding::uniform(limits::MAX_PADDING);
        doc.beautify.aspect_ratio = AspectRatio::SixteenNine;
        let far = Rect::new(-7000.0, -7500.0, 100.0, 100.0);
        doc.annotations.push(redact_at(far));
        assert!(doc.annotation_margin() >= 8000.0, "{}", doc.annotation_margin());
        doc.sanitize();
        assert_eq!(redact_rect(&doc.annotations[0]), far);
        // The plain (content-only) sanitize still clamps it to its window.
        let mut a = redact_at(far);
        a.sanitize(Size::new(40.0, 30.0));
        assert_eq!(redact_rect(&a).x, -limits::COORD_MARGIN_MIN);

        // Beautify off: no extra margin (default window only).
        doc.beautify.enabled = false;
        assert_eq!(doc.annotation_margin(), 0.0);
    }

    #[test]
    fn outside_image_still_rejects_garbage() {
        let bounds = Size::new(200.0, 100.0);
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e12, -1e12] {
            for margin in [0.0, 1e5, f32::NAN, f32::INFINITY, -5.0] {
                let mut a = redact_at(Rect::new(v, -v, 10.0, v));
                a.sanitize_with_margin(bounds, margin);
                let r = redact_rect(&a);
                for x in [r.x, r.y, r.width, r.height] {
                    assert!(x.is_finite(), "v={v} margin={margin}: {r:?}");
                }
                assert!(r.x.abs() <= 200.0 + limits::MAX_COORD && r.y.abs() <= 100.0 + limits::MAX_COORD);
                if !margin.is_finite() || margin < 1e5 {
                    // Non-finite / negative margins count as 0 → default window.
                    let lim = limits::COORD_MARGIN_MIN.max(800.0) + 200.0;
                    assert!(r.x.abs() <= lim && r.y.abs() <= lim && r.right().abs() <= lim, "v={v} margin={margin}: {r:?}");
                }
            }
        }
        // A poisoned beautify can't make the margin absurd.
        let mut doc = sample_document();
        doc.metadata.scale_factor = f32::INFINITY;
        doc.beautify.padding = Padding::uniform(f32::INFINITY);
        doc.beautify.inset.size = f32::NAN;
        let m = doc.annotation_margin();
        assert!(m.is_finite() && m <= limits::MAX_COORD, "{m}");
    }
}
