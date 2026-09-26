//! Annotation model (§5 of PRODUCT.md).
//!
//! * All geometry is in **content space** px (see [`crate::geometry`]).
//! * Z-order = index in `Document::annotations` (0 = bottom, last = top).
//! * Annotations are clipped to the content rect when rendered.
//! * Obfuscation kinds (blur / pixelate / redact) operate on everything
//!   rendered *below* them (base image + lower annotations) and are
//!   destructive in the exported bitmap.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Color, FontFamily, FontWeight, Point, Rect, Shadow, ShapeStyle, Stroke, TextStyle, ToolKind};

/// Stable identity of an annotation (UUID v4). Survives move/restyle/undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnnotationId(pub Uuid);

impl AnnotationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for AnnotationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AnnotationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// One vector annotation. Common properties live here; kind-specific data in
/// [`AnnotationKind`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: AnnotationId,
    /// Whole-object opacity 0..=1, applied to the annotation as a group
    /// (stroke + fill + text composited together, then faded).
    #[serde(default = "one")]
    pub opacity: f32,
    /// Optional drop shadow for the whole object.
    #[serde(default)]
    pub shadow: Option<Shadow>,
    /// Hidden annotations are skipped by the renderer and hit-testing.
    #[serde(default)]
    pub hidden: bool,
    /// Locked annotations render normally but cannot be selected on canvas.
    #[serde(default)]
    pub locked: bool,
    pub kind: AnnotationKind,
}

fn one() -> f32 {
    1.0
}

impl Annotation {
    /// New annotation with a fresh id, opacity 1, no shadow.
    pub fn new(kind: AnnotationKind) -> Self {
        Self { id: AnnotationId::new(), opacity: 1.0, shadow: None, hidden: false, locked: false, kind }
    }

    /// Builder: set shadow.
    pub fn with_shadow(mut self, shadow: Option<Shadow>) -> Self {
        self.shadow = shadow;
        self
    }

    /// Builder: set opacity.
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }

    /// Clone with a *new* id (for duplicate ⌘D / paste).
    pub fn duplicate(&self) -> Self {
        Self { id: AnnotationId::new(), ..self.clone() }
    }

    /// The tool that creates this kind of annotation.
    pub fn tool(&self) -> ToolKind {
        self.kind.tool()
    }

    /// Whether this is a destructive obfuscation (blur / pixelate / redact).
    pub fn is_obfuscation(&self) -> bool {
        matches!(self.kind, AnnotationKind::Blur(_) | AnnotationKind::Pixelate(_) | AnnotationKind::Redact(_))
    }

    /// Approximate axis-aligned bounds in content space, **excluding** stroke
    /// width, shadow and (for text) exact glyph extents. Text/callout text
    /// uses the stored box if `width` is set, else an estimate
    /// (`0.6 * size` per char of the longest line). For precise bounds of
    /// text, use `rs_render::Renderer::annotation_bounds`.
    pub fn geometric_bounds(&self) -> Rect {
        self.kind.geometric_bounds()
    }

    /// Move by (dx, dy) in content space.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.map_points(&|p| p.translate(dx, dy));
    }

    /// Apply a point mapping to every geometric point of the annotation.
    ///
    /// Rect-like fields are mapped by their two opposite corners and then
    /// re-normalized, so this is exact for translations, uniform scales and
    /// 90° rotations (the only transforms the document ever applies). Lengths
    /// (stroke width, radius, font size, zoom...) are **not** changed.
    pub fn map_points(&mut self, f: &dyn Fn(Point) -> Point) {
        let map_rect = |r: Rect| -> Rect {
            Rect::from_points(f(r.origin()), f(Point::new(r.right(), r.bottom())))
        };
        match &mut self.kind {
            AnnotationKind::Rect(a) => a.rect = map_rect(a.rect),
            AnnotationKind::Ellipse(a) => a.rect = map_rect(a.rect),
            AnnotationKind::Line(a) => {
                a.start = f(a.start);
                a.end = f(a.end);
            }
            AnnotationKind::Arrow(a) => {
                a.start = f(a.start);
                a.end = f(a.end);
                a.control = a.control.map(f);
            }
            AnnotationKind::Pen(a) => a.points.iter_mut().for_each(|p| *p = f(*p)),
            AnnotationKind::Highlighter(a) => a.points.iter_mut().for_each(|p| *p = f(*p)),
            AnnotationKind::Step(a) => a.center = f(a.center),
            AnnotationKind::Callout(a) => {
                a.rect = map_rect(a.rect);
                a.tail = f(a.tail);
            }
            AnnotationKind::Spotlight(a) => a.rect = map_rect(a.rect),
            AnnotationKind::Magnifier(a) => {
                a.source_center = f(a.source_center);
                a.target_center = f(a.target_center);
            }
            AnnotationKind::Blur(a) => a.rect = map_rect(a.rect),
            AnnotationKind::Pixelate(a) => a.rect = map_rect(a.rect),
            AnnotationKind::Redact(a) => a.rect = map_rect(a.rect),
            AnnotationKind::Text(a) => {
                // Text boxes don't rotate; keep the box anchored at the mapped
                // top-left of its (estimated) bounds.
                let b = AnnotationKind::Text(a.clone()).geometric_bounds();
                let r = map_rect(b);
                a.position = r.origin();
            }
        }
    }
}

/// Every annotation kind. Serialized with an internal `"type"` tag
/// (snake_case variant names).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnnotationKind {
    Rect(RectAnnotation),
    Ellipse(EllipseAnnotation),
    Line(LineAnnotation),
    Arrow(ArrowAnnotation),
    Pen(PenAnnotation),
    Highlighter(HighlighterAnnotation),
    Step(StepAnnotation),
    Callout(CalloutAnnotation),
    Spotlight(SpotlightAnnotation),
    Magnifier(MagnifierAnnotation),
    Blur(BlurAnnotation),
    Pixelate(PixelateAnnotation),
    Redact(RedactAnnotation),
    Text(TextAnnotation),
}

impl AnnotationKind {
    pub fn tool(&self) -> ToolKind {
        match self {
            AnnotationKind::Rect(_) => ToolKind::Rect,
            AnnotationKind::Ellipse(_) => ToolKind::Ellipse,
            AnnotationKind::Line(_) => ToolKind::Line,
            AnnotationKind::Arrow(_) => ToolKind::Arrow,
            AnnotationKind::Pen(_) => ToolKind::Pen,
            AnnotationKind::Highlighter(_) => ToolKind::Highlighter,
            AnnotationKind::Step(_) => ToolKind::Step,
            AnnotationKind::Callout(_) => ToolKind::Callout,
            AnnotationKind::Spotlight(_) => ToolKind::Spotlight,
            AnnotationKind::Magnifier(_) => ToolKind::Magnifier,
            AnnotationKind::Blur(_) => ToolKind::Blur,
            AnnotationKind::Pixelate(_) => ToolKind::Pixelate,
            AnnotationKind::Redact(_) => ToolKind::Redact,
            AnnotationKind::Text(_) => ToolKind::Text,
        }
    }

    /// See [`Annotation::geometric_bounds`].
    pub fn geometric_bounds(&self) -> Rect {
        match self {
            AnnotationKind::Rect(a) => a.rect,
            AnnotationKind::Ellipse(a) => a.rect,
            AnnotationKind::Line(a) => Rect::from_points(a.start, a.end),
            AnnotationKind::Arrow(a) => {
                let mut pts = vec![a.start, a.end];
                pts.extend(a.control);
                Rect::bounding(&pts).unwrap_or_default()
            }
            AnnotationKind::Pen(a) => Rect::bounding(&a.points).unwrap_or_default(),
            AnnotationKind::Highlighter(a) => {
                Rect::bounding(&a.points).unwrap_or_default().outset(a.width / 2.0)
            }
            AnnotationKind::Step(a) => {
                Rect::new(a.center.x - a.radius, a.center.y - a.radius, a.radius * 2.0, a.radius * 2.0)
            }
            AnnotationKind::Callout(a) => a.rect.union(&Rect::new(a.tail.x, a.tail.y, 0.0, 0.0)),
            AnnotationKind::Spotlight(a) => a.rect,
            AnnotationKind::Magnifier(a) => {
                let r = a.target_radius();
                Rect::new(a.target_center.x - r, a.target_center.y - r, 2.0 * r, 2.0 * r)
            }
            AnnotationKind::Blur(a) => a.rect,
            AnnotationKind::Pixelate(a) => a.rect,
            AnnotationKind::Redact(a) => a.rect,
            AnnotationKind::Text(a) => {
                let lines: Vec<&str> = a.content.split('\n').collect();
                let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(1);
                let w = a.width.unwrap_or(longest as f32 * a.style.size * 0.6);
                let h = lines.len().max(1) as f32 * a.style.size * a.style.line_height;
                Rect::new(a.position.x, a.position.y, w, h)
            }
        }
    }
}

/// Clipboard payload for copy/paste of annotations between screenshots:
/// JSON text `{"rapidsnap_annotations":[...]}` put on the clipboard as a
/// plain string (content-space coordinates of the source document).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationClipboard {
    pub rapidsnap_annotations: Vec<Annotation>,
}

impl AnnotationClipboard {
    /// Serialize annotations for the clipboard.
    pub fn to_json(annotations: &[Annotation]) -> String {
        serde_json::to_string(&AnnotationClipboard { rapidsnap_annotations: annotations.to_vec() })
            .unwrap_or_default()
    }

    /// Parse clipboard text; returns annotations with **fresh ids** (ready for
    /// `Command::Add`), or `None` if the text isn't a RapidSnap payload.
    pub fn from_json(text: &str) -> Option<Vec<Annotation>> {
        let c: AnnotationClipboard = serde_json::from_str(text).ok()?;
        Some(c.rapidsnap_annotations.iter().map(Annotation::duplicate).collect())
    }
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// Rectangle `R`: outline / filled / both, corner radius, dashed stroke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RectAnnotation {
    pub rect: Rect,
    pub style: ShapeStyle,
    /// Corner radius px; clamped by the renderer to min(w,h)/2.
    #[serde(default)]
    pub corner_radius: f32,
}

/// Ellipse `O` inscribed in `rect`: outline / filled, dashed stroke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EllipseAnnotation {
    pub rect: Rect,
    pub style: ShapeStyle,
}

/// Straight line `L`. Caps and dash come from `stroke`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineAnnotation {
    pub start: Point,
    pub end: Point,
    pub stroke: Stroke,
}

/// Arrow head style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrowHead {
    /// Filled triangle (v1 look).
    #[default]
    FilledTriangle,
    /// Two stroked lines forming a "V" (open chevron).
    Open,
    /// Filled circle (dot) at the tip.
    Round,
    /// No head (useful for the start of a single-headed arrow internally).
    None,
}

/// Arrow `A`: straight or curved, optional double head, head style, tapered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrowAnnotation {
    /// Tail point.
    pub start: Point,
    /// Tip point (the head is drawn here).
    pub end: Point,
    /// `Some(p)` = curved arrow: quadratic Bézier `start → p (control) → end`.
    /// The editor exposes `p` as a draggable handle. `None` = straight.
    #[serde(default)]
    pub control: Option<Point>,
    pub stroke: Stroke,
    #[serde(default)]
    pub head: ArrowHead,
    /// Draw a head at `start` too (same style as `head`).
    #[serde(default)]
    pub double_headed: bool,
    /// Tapered stroke: width grows linearly from ~0 at `start` to
    /// `stroke.width` at the head base (drawn as a filled polygon).
    #[serde(default)]
    pub tapered: bool,
    /// Multiplier on the default head size (1.0 = default). See [`Self::head_length`].
    #[serde(default = "one")]
    pub head_scale: f32,
}

impl ArrowAnnotation {
    /// Head length along the shaft, in px:
    /// `max(stroke.width * 4, 10) * head_scale`. (v1: fixed 20 px at width ~5.)
    pub fn head_length(&self) -> f32 {
        (self.stroke.width * 4.0).max(10.0) * self.head_scale
    }

    /// Head width across the shaft, in px: `head_length * 0.75`.
    pub fn head_width(&self) -> f32 {
        self.head_length() * 0.75
    }

    pub fn is_curved(&self) -> bool {
        self.control.is_some()
    }
}

/// Freehand pen `P`. `points` are raw input samples in content space; the
/// renderer smooths them (Catmull-Rom → cubic Bézier). Constant width.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PenAnnotation {
    pub points: Vec<Point>,
    pub stroke: Stroke,
}

/// Highlighter `H`: wide translucent marker drawn with **multiply** blend
/// against what's below. Flat (butt/square) caps. Straight line = 2 points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HighlighterAnnotation {
    pub points: Vec<Point>,
    /// Marker color (alpha is respected; default yellow @ ~50%).
    pub color: Color,
    /// Marker width px (default 20 pt × scale).
    pub width: f32,
}

/// How a step counter renders its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepLabel {
    /// 1, 2, 3, …
    #[default]
    Number,
    /// A, B, C, … Z, AA, AB, … (spreadsheet-style).
    UpperLetter,
    /// a, b, c, …
    LowerLetter,
}

impl StepLabel {
    /// Text for `value` (1-based). `value == 0` renders as "0" / "".
    pub fn format(self, value: u32) -> String {
        match self {
            StepLabel::Number => value.to_string(),
            StepLabel::UpperLetter | StepLabel::LowerLetter => {
                let base = if self == StepLabel::UpperLetter { b'A' } else { b'a' };
                let mut n = value;
                let mut out = Vec::new();
                while n > 0 {
                    n -= 1;
                    out.push((base + (n % 26) as u8) as char);
                    n /= 26;
                }
                out.iter().rev().collect()
            }
        }
    }
}

/// Step counter `N`: numbered/lettered filled circle. Auto-increment is an
/// editor concern (next value = max existing value + 1 for the same label kind).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepAnnotation {
    pub center: Point,
    /// Circle radius px.
    pub radius: f32,
    /// 1-based value.
    pub value: u32,
    #[serde(default)]
    pub label: StepLabel,
    /// Circle fill.
    pub fill: Color,
    /// Label color.
    pub text_color: Color,
    /// Optional ring around the circle.
    #[serde(default)]
    pub border: Option<Stroke>,
    #[serde(default)]
    pub font_family: FontFamily,
    #[serde(default = "bold")]
    pub font_weight: FontWeight,
}

fn bold() -> FontWeight {
    FontWeight::BOLD
}

impl StepAnnotation {
    pub fn label_text(&self) -> String {
        self.label.format(self.value)
    }
}

/// Callout / speech bubble `C`: rounded rect bubble + triangular tail whose tip
/// is `tail`; the tail base is on the bubble edge closest to `tail`
/// (base width = min(24px·scale, 1/3 of that edge)). Text is laid out inside
/// `rect` inset by `padding`, wrapping at `rect.width - 2*padding`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalloutAnnotation {
    pub rect: Rect,
    /// Tip of the tail in content space. If inside `rect`, no tail is drawn.
    pub tail: Point,
    pub corner_radius: f32,
    pub style: ShapeStyle,
    pub text: String,
    pub text_style: TextStyle,
    /// Inner padding px between bubble edge and text.
    pub padding: f32,
}

/// Spotlight shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpotlightShape {
    #[default]
    Rect,
    Ellipse,
}

/// Spotlight `S`: dims the whole content *except* `rect`.
///
/// Semantics with multiple spotlights: the renderer draws **one** dim layer
/// over the content (at the position of the lowest spotlight in z-order),
/// with the dim color = the `dim` of the topmost spotlight, and cuts holes
/// for every spotlight's shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotlightAnnotation {
    pub rect: Rect,
    #[serde(default)]
    pub shape: SpotlightShape,
    /// Corner radius for `SpotlightShape::Rect`.
    #[serde(default)]
    pub corner_radius: f32,
    /// Dim overlay color, alpha = dim amount (default black @ 55%).
    pub dim: Color,
}

/// Magnifier `M`: circular loupe showing the content under a source circle,
/// magnified `zoom`× and drawn centered at `target_center`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MagnifierAnnotation {
    /// Center of the magnified region (content space).
    pub source_center: Point,
    /// Radius of the magnified region (content space px).
    pub source_radius: f32,
    /// Center of the drawn loupe. May equal `source_center` (in-place zoom).
    pub target_center: Point,
    /// 2.0..=4.0.
    pub zoom: f32,
    /// Ring around the loupe.
    pub border: Stroke,
}

impl MagnifierAnnotation {
    /// Drawn loupe radius = `source_radius * zoom`.
    pub fn target_radius(&self) -> f32 {
        self.source_radius * self.zoom
    }
}

// ---------------------------------------------------------------------------
// Obfuscation (destructive on export)
// ---------------------------------------------------------------------------

/// Blur `B`: Gaussian blur of everything below, clipped to `rect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlurAnnotation {
    pub rect: Rect,
    /// Gaussian sigma in content px (UI range 2..=40, default 12).
    pub strength: f32,
    #[serde(default)]
    pub corner_radius: f32,
}

/// Pixelate `X`: mosaic of everything below, clipped to `rect`. Blocks are
/// aligned to `rect`'s top-left corner; each block = average color.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PixelateAnnotation {
    pub rect: Rect,
    /// Block edge length in content px (UI range 4..=64, default 12).
    pub block_size: u32,
}

/// Solid redact `D`: opaque bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedactAnnotation {
    pub rect: Rect,
    /// Default black. Alpha is forced to 255 by the renderer (must be destructive).
    pub color: Color,
    #[serde(default)]
    pub corner_radius: f32,
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Text `T`.
///
/// * `position` = top-left of the text box (before background padding —
///   pill/semi-transparent padding extends *outside* this box).
/// * `width = None` → auto width (no wrapping; lines break only at '\n').
///   `Some(w)` → fixed-width box, text wraps at `w` px.
/// * `content` may contain '\n' for hard line breaks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextAnnotation {
    pub position: Point,
    #[serde(default)]
    pub width: Option<f32>,
    pub content: String,
    pub style: TextStyle,
    /// The preset this style was derived from (UI hint only).
    #[serde(default)]
    pub preset: Option<crate::TextPreset>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_labels() {
        assert_eq!(StepLabel::Number.format(12), "12");
        assert_eq!(StepLabel::UpperLetter.format(1), "A");
        assert_eq!(StepLabel::UpperLetter.format(26), "Z");
        assert_eq!(StepLabel::UpperLetter.format(27), "AA");
        assert_eq!(StepLabel::LowerLetter.format(3), "c");
    }

    #[test]
    fn serde_roundtrip_all_kinds() {
        let kinds = crate::test_support::all_kinds();
        for k in kinds {
            let a = Annotation::new(k);
            let s = serde_json::to_string(&a).unwrap();
            let back: Annotation = serde_json::from_str(&s).unwrap();
            assert_eq!(a, back, "{s}");
        }
    }

    #[test]
    fn clipboard_roundtrip_new_ids() {
        let a = Annotation::new(crate::test_support::all_kinds().remove(0));
        let json = AnnotationClipboard::to_json(std::slice::from_ref(&a));
        let back = AnnotationClipboard::from_json(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_ne!(back[0].id, a.id);
        assert_eq!(back[0].kind, a.kind);
        assert!(AnnotationClipboard::from_json("hello").is_none());
    }

    #[test]
    fn translate_moves_geometry() {
        let mut a = Annotation::new(AnnotationKind::Line(LineAnnotation {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 10.0),
            stroke: Stroke::default(),
        }));
        a.translate(5.0, -5.0);
        assert_eq!(a.geometric_bounds(), Rect::new(5.0, -5.0, 10.0, 10.0));
    }
}
