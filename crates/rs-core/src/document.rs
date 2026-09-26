//! The Document: base image + image transform + beautify + annotations.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Annotation, AnnotationId, BeautifySettings, IRect, Point, Size};

/// Current `.rsnap` / Document schema version.
pub const DOCUMENT_FORMAT_VERSION: u32 = 1;

/// An 8-bit RGBA image, **straight (non-premultiplied) alpha**, sRGB,
/// row-major, tightly packed (stride = `width * 4`), top row first.
///
/// Pixels are shared via `Arc` so cloning a Document is cheap.
///
/// **Serde:** only `width` and `height` are serialized — pixel data is
/// `#[serde(skip)]`. A deserialized image has empty `pixels`
/// (`is_loaded() == false`) until the loader (rs-smart `.rsnap`) fills it in
/// from the archive's `base.png`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub pixels: Arc<Vec<u8>>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

impl RgbaImage {
    /// Wrap raw RGBA8 pixels. Returns `None` if `pixels.len() != w*h*4`.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        (pixels.len() == width as usize * height as usize * 4)
            .then(|| Self { width, height, pixels: Arc::new(pixels) })
    }

    /// Solid-color image (handy for tests and placeholders).
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let n = width as usize * height as usize;
        let mut v = Vec::with_capacity(n * 4);
        for _ in 0..n {
            v.extend_from_slice(&rgba);
        }
        Self { width, height, pixels: Arc::new(v) }
    }

    /// True if `pixels` holds `width*height*4` bytes.
    pub fn is_loaded(&self) -> bool {
        self.pixels.len() == self.width as usize * self.height as usize * 4
    }

    pub fn size(&self) -> Size {
        Size::new(self.width as f32, self.height as f32)
    }

    /// RGBA at (x, y), or `None` out of bounds / not loaded.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height || !self.is_loaded() {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        Some([self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]])
    }
}

/// Clockwise rotation applied to the (cropped) base image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    pub fn degrees(self) -> u32 {
        match self {
            Rotation::Deg0 => 0,
            Rotation::Deg90 => 90,
            Rotation::Deg180 => 180,
            Rotation::Deg270 => 270,
        }
    }

    /// Rotate a further 90° clockwise.
    pub fn cw(self) -> Self {
        match self {
            Rotation::Deg0 => Rotation::Deg90,
            Rotation::Deg90 => Rotation::Deg180,
            Rotation::Deg180 => Rotation::Deg270,
            Rotation::Deg270 => Rotation::Deg0,
        }
    }

    /// Rotate 90° counter-clockwise.
    pub fn ccw(self) -> Self {
        self.cw().cw().cw()
    }

    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::Deg90 | Rotation::Deg270)
    }
}

/// Non-destructive crop + rotation of the base image.
///
/// Order: **crop first** (in base-image px), **then rotate** clockwise.
/// The result defines *content space*, where all annotations live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageTransform {
    /// Crop rectangle in original base-image pixels; `None` = full image.
    /// Always clamp with [`IRect::clamp_to`] before use.
    pub crop: Option<IRect>,
    pub rotation: Rotation,
}

impl ImageTransform {
    /// The effective (clamped) crop rect for an image of `w`×`h`.
    pub fn crop_rect(&self, w: u32, h: u32) -> IRect {
        self.crop.and_then(|c| c.clamp_to(w, h)).unwrap_or(IRect::new(0, 0, w, h))
    }

    /// Content size in px for a base image of `w`×`h`.
    pub fn content_size(&self, w: u32, h: u32) -> (u32, u32) {
        let c = self.crop_rect(w, h);
        if self.rotation.swaps_axes() { (c.height, c.width) } else { (c.width, c.height) }
    }

    /// Map a point in base-image px → content px.
    pub fn base_to_content(&self, p: Point, w: u32, h: u32) -> Point {
        let c = self.crop_rect(w, h);
        let (cw, ch) = (c.width as f32, c.height as f32);
        let q = Point::new(p.x - c.x as f32, p.y - c.y as f32);
        match self.rotation {
            Rotation::Deg0 => q,
            Rotation::Deg90 => Point::new(ch - q.y, q.x),
            Rotation::Deg180 => Point::new(cw - q.x, ch - q.y),
            Rotation::Deg270 => Point::new(q.y, cw - q.x),
        }
    }

    /// Map a point in content px → base-image px (inverse of `base_to_content`).
    pub fn content_to_base(&self, p: Point, w: u32, h: u32) -> Point {
        let c = self.crop_rect(w, h);
        let (cw, ch) = (c.width as f32, c.height as f32);
        let q = match self.rotation {
            Rotation::Deg0 => p,
            Rotation::Deg90 => Point::new(p.y, ch - p.x),
            Rotation::Deg180 => Point::new(cw - p.x, ch - p.y),
            Rotation::Deg270 => Point::new(cw - p.y, p.x),
        };
        Point::new(q.x + c.x as f32, q.y + c.y as f32)
    }
}

/// Re-express annotations when the image transform changes from `before` to
/// `after`, so they stay attached to the same image pixels
/// (content(before) → base → content(after)). Used by
/// [`crate::Command::SetImageTransform`].
pub fn remap_annotations(
    annotations: &mut [Annotation],
    before: &ImageTransform,
    after: &ImageTransform,
    base_w: u32,
    base_h: u32,
) {
    if before == after {
        return;
    }
    let f = |p: Point| after.base_to_content(before.content_to_base(p, base_w, base_h), base_w, base_h);
    for a in annotations {
        a.map_points(&f);
    }
}

/// Where the document's image came from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CaptureSource {
    #[default]
    Unknown,
    Region,
    Window { app_name: Option<String>, title: Option<String> },
    Fullscreen { display_id: Option<u32> },
    File { path: PathBuf },
    Clipboard,
}

/// Document metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentMetadata {
    /// Creation time, Unix epoch milliseconds (UTC).
    pub created_at_ms: i64,
    pub source: CaptureSource,
    /// Physical px per logical point of the captured image (2.0 for Retina
    /// captures, 1.0 for files/clipboard unless known). Drives:
    /// * point→px conversion of beautify settings and tool styles, and
    /// * export scale (see `ExportScale`).
    pub scale_factor: f32,
    /// Optional user title / file stem.
    pub title: Option<String>,
    /// Path of the `.rsnap` this document was loaded from / saved to.
    pub project_path: Option<PathBuf>,
}

impl Default for DocumentMetadata {
    fn default() -> Self {
        Self { created_at_ms: 0, source: CaptureSource::Unknown, scale_factor: 1.0, title: None, project_path: None }
    }
}

/// Stable document identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(pub Uuid);

impl DocumentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DocumentId {
    fn default() -> Self {
        Self::new()
    }
}

/// A screenshot being edited.
///
/// Mutate only through [`crate::History::apply`] from UI code so that undo/redo
/// works (direct mutation is fine for loaders/tests).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default = "version_default")]
    pub format_version: u32,
    #[serde(default)]
    pub id: DocumentId,
    /// Original, untransformed image (see `RgbaImage` serde note).
    pub base_image: RgbaImage,
    #[serde(default)]
    pub image_transform: ImageTransform,
    #[serde(default)]
    pub beautify: BeautifySettings,
    /// Bottom → top.
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub metadata: DocumentMetadata,
}

fn version_default() -> u32 {
    DOCUMENT_FORMAT_VERSION
}

impl Document {
    /// New document with default beautify settings and no annotations.
    pub fn new(base_image: RgbaImage, metadata: DocumentMetadata) -> Self {
        Self {
            format_version: DOCUMENT_FORMAT_VERSION,
            id: DocumentId::new(),
            base_image,
            image_transform: ImageTransform::default(),
            beautify: BeautifySettings::default(),
            annotations: Vec::new(),
            metadata,
        }
    }

    /// Builder: set beautify settings (e.g. from `Preferences::last_beautify`).
    pub fn with_beautify(mut self, beautify: BeautifySettings) -> Self {
        self.beautify = beautify;
        self
    }

    /// Size of content space in px (base image after crop + rotation).
    pub fn content_size(&self) -> Size {
        let (w, h) = self.image_transform.content_size(self.base_image.width, self.base_image.height);
        Size::new(w as f32, h as f32)
    }

    /// Content size in logical points (`content_size / scale_factor`).
    pub fn content_size_pt(&self) -> Size {
        self.content_size().scale(1.0 / self.metadata.scale_factor.max(0.01))
    }

    pub fn index_of(&self, id: AnnotationId) -> Option<usize> {
        self.annotations.iter().position(|a| a.id == id)
    }

    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.annotations.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: AnnotationId) -> Option<&mut Annotation> {
        self.annotations.iter_mut().find(|a| a.id == id)
    }

    /// Next auto-increment value for a step counter with `label` style
    /// (max existing value among steps with the same label + 1).
    pub fn next_step_value(&self, label: crate::StepLabel) -> u32 {
        self.annotations
            .iter()
            .filter_map(|a| match &a.kind {
                crate::AnnotationKind::Step(s) if s.label == label => Some(s.value),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_roundtrip() {
        let (w, h) = (200, 100);
        for rot in [Rotation::Deg0, Rotation::Deg90, Rotation::Deg180, Rotation::Deg270] {
            let t = ImageTransform { crop: Some(IRect::new(10, 20, 100, 50)), rotation: rot };
            let p = Point::new(33.0, 44.0);
            let c = t.base_to_content(p, w, h);
            let back = t.content_to_base(c, w, h);
            assert!((back.x - p.x).abs() < 1e-4 && (back.y - p.y).abs() < 1e-4, "{rot:?}");
            let (cw, ch) = t.content_size(w, h);
            assert!(c.x >= 0.0 && c.y >= 0.0 && c.x <= cw as f32 && c.y <= ch as f32);
        }
        let t = ImageTransform { crop: None, rotation: Rotation::Deg90 };
        // Top-left of the base image ends up at the top-right of content.
        assert_eq!(t.base_to_content(Point::new(0.0, 0.0), w, h), Point::new(100.0, 0.0));
        assert_eq!(t.content_size(w, h), (100, 200));
    }

    #[test]
    fn document_serde_skips_pixels() {
        let doc = Document::new(RgbaImage::filled(4, 3, [1, 2, 3, 255]), DocumentMetadata::default());
        let s = serde_json::to_string(&doc).unwrap();
        assert!(s.len() < 4000);
        let back: Document = serde_json::from_str(&s).unwrap();
        assert!(!back.base_image.is_loaded());
        assert_eq!(back.base_image.width, 4);
        assert_eq!(back.beautify, doc.beautify);
    }
}
