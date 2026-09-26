//! # rs-render — the single RapidSnap renderer (tiny-skia)
//!
//! `Document → RgbaImage`. The same code path produces the editor preview and
//! the exported file, so preview == export.
//!
//! Pipeline (canvas px): background → frame shadow → frame (chrome, inset)
//! → content image → annotations (z-order, over the **whole canvas**,
//! clipped only by the canvas edge) → watermark. Annotations are positioned
//! in content space, so they may sit over the padding / inset / chrome
//! (negative or beyond-the-image coordinates) and stay attached to the
//! screenshot when the beautify layout changes.
//!
//! Performance: everything below the annotations is cached as one *base
//! layer* per (document image, transform, beautify, scale); decoded images,
//! downsampled wallpapers, text layouts/glyph outlines and blur results are
//! cached too, so an annotation edit re-renders only the vector layer.

mod beautify;
mod draw;
mod effects;
mod geom;
mod layout;
mod text;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rs_core::{
    Annotation, AnnotationId, AnnotationKind, Color, Document, ExportFormat, ExportSettings, InsetColor, Point, Rect,
    RgbaImage, Rotation, Shadow, Size, TextStyle,
};
use thiserror::Error;
use tiny_skia as sk;

pub use layout::{auto_balance_padding_pt, compute_layout, CanvasLayout};
pub use text::{
    callout_bubble_rect, callout_text, caret_rect, font_families, layout_text, measure_text_annotation, selection_rects, text_box,
    text_hit_index, text_layout, warm_up_fonts, TextLayout, TextLine,
};

use draw::{draw_kind, map_point, map_rect};
use effects::{extract, write_back, PxRect};
use geom::{circle_path, paint, rounded_mask, rounded_rect_path, sk_color, sk_stroke};

/// Largest canvas side the renderer accepts (px).
pub const MAX_CANVAS_SIDE: u32 = 16_384;

/// Renderer errors.
#[derive(Debug, Error)]
pub enum RenderError {
    #[error("base image is not loaded (pixels missing)")]
    ImageNotLoaded,
    #[error("canvas too large: {0}x{1}")]
    TooLarge(u32, u32),
    #[error("failed to allocate pixmap {0}x{1}")]
    Alloc(u32, u32),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Render quality. `Fast` uses cheaper filters (bilinear image scaling) for
/// interactive preview; `High` (bicubic) is used for export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Quality {
    Fast,
    #[default]
    High,
}

/// Options for one render.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Output px per content px. Preview: whatever the canvas needs
    /// (e.g. `zoom * window_scale_factor / doc.scale_factor`, capped at 1.0);
    /// export: see [`RenderOptions::for_export`].
    pub scale: f32,
    /// Draw annotations (false = background + frame + image only).
    pub draw_annotations: bool,
    /// Annotations to skip (e.g. the text being edited inline, drawn by the
    /// editor itself, or the object being dragged if the editor draws it live).
    pub exclude: Vec<AnnotationId>,
    pub quality: Quality,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { scale: 1.0, draw_annotations: true, exclude: Vec::new(), quality: Quality::High }
    }
}

impl RenderOptions {
    /// Interactive preview at `scale`.
    pub fn preview(scale: f32) -> Self {
        Self { scale, quality: Quality::Fast, ..Default::default() }
    }

    /// Export options: `scale = export.scale.factor() / doc.metadata.scale_factor`.
    pub fn for_export(doc: &Document, export: &ExportSettings) -> Self {
        let sf = if doc.metadata.scale_factor > 0.0 { doc.metadata.scale_factor } else { 1.0 };
        Self { scale: export.scale.factor() / sf, quality: Quality::High, ..Default::default() }
    }
}

/// Result of a render.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    /// Straight-alpha RGBA8, `layout.canvas_width × layout.canvas_height`.
    pub image: RgbaImage,
    pub layout: CanvasLayout,
}

/// Content-image cache entry (keyed by base image identity + transform).
struct ContentEntry {
    /// Keeps the pixel buffer alive so the pointer key stays unique.
    _pixels: Arc<Vec<u8>>,
    key: (usize, usize, u32, u32, rs_core::ImageTransform),
    pixmap: Arc<sk::Pixmap>,
    edge: Color,
    /// Every pixel has alpha 255.
    opaque: bool,
    small: Vec<(u32, Arc<sk::Pixmap>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BlurKey {
    ext: PxRect,
    inner: PxRect,
    sigma: u32,
    hash: u64,
}

const BLUR_CACHE_MAX_BYTES: usize = 256 << 20;

/// Stateful renderer holding caches (decoded backgrounds, base layers, blur
/// results). Create one per editor window; not `Sync`. Text layout / fonts
/// are process-wide (see [`warm_up_fonts`]).
pub struct Renderer {
    image_cache: HashMap<PathBuf, Arc<sk::Pixmap>>,
    image_small_cache: HashMap<(PathBuf, u32), Arc<sk::Pixmap>>,
    desktop_wallpaper: Option<PathBuf>,
    bg_cache: Vec<(String, Arc<sk::Pixmap>)>,
    base_cache: Vec<(String, Arc<sk::Pixmap>)>,
    content: Option<ContentEntry>,
    blur_cache: HashMap<BlurKey, Arc<Vec<u8>>>,
    blur_bytes: usize,
    /// Key of the base layer used by the current render (blur cache signature).
    last_base_key: String,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            image_cache: HashMap::new(),
            image_small_cache: HashMap::new(),
            desktop_wallpaper: None,
            bg_cache: Vec::new(),
            base_cache: Vec::new(),
            content: None,
            blur_cache: HashMap::new(),
            blur_bytes: 0,
            last_base_key: String::new(),
        }
    }

    /// Path of the current desktop wallpaper (resolved by the app via
    /// rs-capture), used by `Background::DesktopWallpaper`.
    pub fn set_desktop_wallpaper(&mut self, path: Option<PathBuf>) {
        if self.desktop_wallpaper != path {
            self.desktop_wallpaper = path;
            self.bg_cache.clear();
            self.base_cache.clear();
        }
    }

    /// Drop all caches (e.g. on memory pressure or when a custom background
    /// file changed).
    pub fn clear_caches(&mut self) {
        self.image_cache.clear();
        self.image_small_cache.clear();
        self.bg_cache.clear();
        self.base_cache.clear();
        self.content = None;
        self.blur_cache.clear();
        self.blur_bytes = 0;
    }

    /// Decode (and cache) the background image of `doc` ahead of time, e.g.
    /// on a background thread before the editor opens. Wallpapers are 6000×4000
    /// JPEGs; the first decode is the slowest part of a first render.
    pub fn preload(&mut self, doc: &Document) {
        let path = match &doc.beautify.background {
            rs_core::Background::Wallpaper { name } => rs_core::assets::wallpaper_path(name),
            rs_core::Background::CustomImage { path } => Some(path.clone()),
            rs_core::Background::DesktopWallpaper { .. } => self.desktop_wallpaper.clone(),
            _ => None,
        };
        if let Some(p) = path {
            let _ = self.load_pixmap(&p);
        }
    }

    /// Render the full document.
    pub fn render(&mut self, doc: &Document, opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
        if !doc.base_image.is_loaded() {
            return Err(RenderError::ImageNotLoaded);
        }
        let layout = compute_layout(doc, opts.scale);
        let (w, h) = (layout.canvas_width, layout.canvas_height);
        if w > MAX_CANVAS_SIDE || h > MAX_CANVAS_SIDE {
            return Err(RenderError::TooLarge(w, h));
        }
        let t0 = std::time::Instant::now();
        let base = self.base_layer(doc, &layout, opts.quality)?;
        let mut canvas = (*base).clone();
        let t_base = t0.elapsed();

        if opts.draw_annotations && !doc.annotations.is_empty() {
            // Annotations cover the whole canvas (padding, inset, chrome,
            // background); the pixmap edge is the only clip.
            self.draw_annotations(&mut canvas, doc, &layout, opts);
        }
        if doc.beautify.enabled
            && let Some(wm) = &doc.beautify.watermark {
                self.draw_watermark(&mut canvas, &layout, wm);
            }
        let image = pixmap_to_rgba(&canvas);
        log::debug!("rs-render: {w}x{h} in {:?} (base {:?})", t0.elapsed(), t_base);
        Ok(RenderOutput { image, layout })
    }

    /// Precise bounds of an annotation in content space, including stroke
    /// width, arrow heads and exact text extents (pill padding, outline).
    /// Shadows are not included. Use for hit-testing and selection handles.
    pub fn annotation_bounds(&mut self, a: &Annotation) -> Rect {
        annotation_bounds(a)
    }

    /// Precise hit test in content space: `true` if `p` is on the painted
    /// part of `a` (strokes within `tolerance` px, filled interiors,
    /// text boxes). See [`hit_test`].
    pub fn hit_test(&mut self, a: &Annotation, p: Point, tolerance: f32) -> bool {
        hit_test(a, p, tolerance)
    }

    /// Measure a text block (content px). `width = Some(w)` wraps at `w`.
    /// Returns the text box size (the box used for alignment; background
    /// padding extends outside it).
    pub fn measure_text(&mut self, text: &str, style: &TextStyle, width: Option<f32>) -> Size {
        layout_text(text, style, width).size
    }

    pub(crate) fn load_pixmap(&mut self, path: &Path) -> Option<Arc<sk::Pixmap>> {
        if let Some(p) = self.image_cache.get(path) {
            return Some(p.clone());
        }
        let img = match image::open(path) {
            Ok(i) => i,
            Err(e) => {
                log::warn!("rs-render: image {path:?}: {e}");
                return None;
            }
        };
        let rgba = img.to_rgba8();
        let ri = RgbaImage::new(rgba.width(), rgba.height(), rgba.into_raw())?;
        let mut pm = rgba_to_pixmap(&ri)?;
        // Wallpapers are 6000×4000: keep a ≤ 3200 px copy (integer box shrink).
        let side = pm.width().max(pm.height());
        if side > 4000 {
            let f = side.div_ceil(3200).max(2);
            if let Some(small) = effects::downsample(&pm, f) {
                pm = small;
            }
        }
        let pm = Arc::new(pm);
        if self.image_cache.len() > 16 {
            self.image_cache.clear();
        }
        self.image_cache.insert(path.to_path_buf(), pm.clone());
        Some(pm)
    }

    /// Content image as a premultiplied pixmap (cached per image + transform).
    fn content_entry(&mut self, doc: &Document) -> Result<&mut ContentEntry, RenderError> {
        let img = &doc.base_image;
        let key = (Arc::as_ptr(&img.pixels) as usize, img.pixels.len(), img.width, img.height, doc.image_transform);
        let hit = self.content.as_ref().is_some_and(|c| c.key == key);
        if !hit {
            let content = content_image(doc)?;
            let edge = dominant_edge_color(&content);
            let pm = rgba_to_pixmap(&content).ok_or(RenderError::Alloc(content.width, content.height))?;
            let opaque = content.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
            self.content = Some(ContentEntry {
                _pixels: img.pixels.clone(),
                key,
                pixmap: Arc::new(pm),
                edge,
                opaque,
                small: Vec::new(),
            });
            self.base_cache.clear();
        }
        self.content.as_mut().ok_or(RenderError::ImageNotLoaded)
    }

    /// Everything below the annotations (cached).
    fn base_layer(&mut self, doc: &Document, layout: &CanvasLayout, quality: Quality) -> Result<Arc<sk::Pixmap>, RenderError> {
        let (w, h) = (layout.canvas_width, layout.canvas_height);
        // Make sure the content cache is current (it invalidates base layers).
        self.content_entry(doc)?;
        let b = &doc.beautify;
        let beautify_key = if b.enabled {
            // Watermark is drawn on top of annotations; exclude it from the key.
            let mut bb = b.clone();
            bb.watermark = None;
            serde_json::to_string(&bb).unwrap_or_else(|_| format!("{bb:?}"))
        } else {
            "off".to_string()
        };
        // The content identity is part of the key: the blur cache signs its
        // entries with this key, and e.g. a 180° rotation keeps the canvas
        // size while changing every pixel beneath an obfuscation.
        let content_key = self.content.as_ref().map(|c| c.key);
        let key = format!(
            "{w}x{h}|{}|{quality:?}|{beautify_key}|{:?}|{content_key:?}",
            layout.scale.to_bits(),
            self.desktop_wallpaper
        );
        if let Some((_, pm)) = self.base_cache.iter().find(|(k, _)| *k == key) {
            self.last_base_key = key;
            return Ok(pm.clone());
        }

        let mut canvas = if b.enabled {
            match self.background_layer(&b.background, layout) {
                Some(bg) => (*bg).clone(),
                None => sk::Pixmap::new(w, h).ok_or(RenderError::Alloc(w, h))?,
            }
        } else {
            sk::Pixmap::new(w, h).ok_or(RenderError::Alloc(w, h))?
        };

        let frame_mask = if b.enabled && layout.corner_radius > 0.0 {
            rounded_mask(w, h, layout.frame_rect, layout.corner_radius)
        } else {
            None
        };
        let entry = self.content_entry(doc)?;
        let edge = entry.edge;
        let opaque = entry.opaque;
        if b.enabled {
            // The frame interior is fully painted when the content is opaque
            // (inset band / chrome are opaque; otherwise frame == content).
            let inset_opaque = !b.inset.enabled
                || match b.inset.color {
                    InsetColor::Auto => edge.a == 255,
                    InsetColor::Custom(c) => c.a == 255,
                };
            self.draw_frame_shadow(&mut canvas, layout, &b.shadow, opaque && inset_opaque);

            if let Some(cr) = layout.chrome_rect {
                beautify::draw_chrome(&mut canvas, &b.chrome, cr, layout.px_per_pt, frame_mask.as_ref());
            }
            if b.inset.enabled && b.inset.size > 0.0 {
                let c = match b.inset.color {
                    InsetColor::Auto => edge,
                    InsetColor::Custom(c) => c,
                };
                let mut p = sk::Paint::default();
                p.set_color(sk_color(c));
                canvas.fill_rect(geom::to_sk_rect(layout.inset_rect), &p, sk::Transform::identity(), frame_mask.as_ref());
            }
        }

        // Content image (pre-shrunk by an integer factor when downscaling).
        let s = layout.scale;
        let (src, rs) = self.content_for_scale(doc, s)?;
        let cr = layout.content_rect;
        let integral = (rs - 1.0).abs() < 1e-5 && cr.x.fract() == 0.0 && cr.y.fract() == 0.0;
        let quality = match (integral, quality) {
            (true, _) => sk::FilterQuality::Nearest,
            (false, Quality::Fast) => sk::FilterQuality::Bilinear,
            (false, Quality::High) => sk::FilterQuality::Bicubic,
        };
        let paint = sk::PixmapPaint { quality, ..Default::default() };
        let t = sk::Transform::from_row(rs, 0.0, 0.0, rs, cr.x, cr.y);
        if integral {
            let fr = if b.enabled { Some((layout.frame_rect, layout.corner_radius)) } else { None };
            blit_content(&mut canvas, &src, cr.x as i32, cr.y as i32, opaque, fr, frame_mask.as_ref());
        } else if b.enabled {
            canvas.draw_pixmap(0, 0, src.as_ref().as_ref(), &paint, t, frame_mask.as_ref());
        } else {
            canvas.draw_pixmap(0, 0, src.as_ref().as_ref(), &paint, t, None);
        }

        let pm = Arc::new(canvas);
        self.last_base_key = key.clone();
        self.base_cache.insert(0, (key, pm.clone()));
        self.base_cache.truncate(2);
        Ok(pm)
    }

    /// Content pixmap to draw at `scale`, plus the residual scale.
    fn content_for_scale(&mut self, doc: &Document, scale: f32) -> Result<(Arc<sk::Pixmap>, f32), RenderError> {
        let entry = self.content_entry(doc)?;
        let f = if scale < 0.75 { (1.0 / scale).floor().max(1.0) as u32 } else { 1 };
        if f < 2 {
            return Ok((entry.pixmap.clone(), scale));
        }
        if let Some((_, p)) = entry.small.iter().find(|(k, _)| *k == f) {
            return Ok((p.clone(), scale * f as f32));
        }
        let small = Arc::new(effects::downsample(&entry.pixmap, f).ok_or(RenderError::Alloc(1, 1))?);
        entry.small.push((f, small.clone()));
        if entry.small.len() > 3 {
            entry.small.remove(0);
        }
        // The downsample rounds the size up; residual scale maps content px exactly.
        Ok((small, scale * f as f32))
    }

    fn draw_annotations(
        &mut self,
        canvas: &mut sk::Pixmap,
        doc: &Document,
        layout: &CanvasLayout,
        opts: &RenderOptions,
    ) {
        let s = layout.scale;
        let cr = layout.content_rect;
        let t = sk::Transform::from_row(s, 0.0, 0.0, s, cr.x, cr.y);
        let sf = if doc.metadata.scale_factor > 0.0 { doc.metadata.scale_factor } else { 1.0 };
        let clip: Option<&sk::Mask> = None;
        let bounds = PxRect { x: 0, y: 0, w: canvas.width() as i32, h: canvas.height() as i32 };
        let visible: Vec<&Annotation> =
            doc.annotations.iter().filter(|a| !a.hidden && !opts.exclude.contains(&a.id)).collect();
        let first_spot = visible.iter().position(|a| matches!(a.kind, AnnotationKind::Spotlight(_)));
        let ctx = Ctx { t, scale: s, clip, bounds, sf, quality: opts.quality };
        // Blur results are cached under a signature of everything that can
        // influence the pixels under them (base layer + the annotations below
        // that overlap the blur), so a cache hit needs no pixel read-back.
        let influence: Vec<Option<Rect>> = if visible.iter().any(|a| matches!(a.kind, AnnotationKind::Blur(_))) {
            visible.iter().map(|a| influence_rect(a)).collect()
        } else {
            Vec::new()
        };
        // Spotlight dim is a presentation overlay, not content: a magnifier
        // above a spotlight must magnify the undimmed pixels (content +
        // annotations below it), otherwise the loupe shows a dark, dimmed
        // copy. When such a magnifier exists we keep an undimmed twin of the
        // canvas from the first spotlight on and draw everything into both.
        let needs_undimmed = first_spot.is_some_and(|f| {
            visible[f..].iter().any(|a| matches!(a.kind, AnnotationKind::Magnifier(_)))
        });
        let mut undimmed: Option<sk::Pixmap> = None;
        for (i, a) in visible.iter().enumerate() {
            if Some(i) == first_spot && needs_undimmed {
                undimmed = Some(canvas.clone());
            }
            if let AnnotationKind::Magnifier(m) = &a.kind {
                // Sample first (from the undimmed twin if any), then draw the
                // loupe into both canvases so later magnifiers see it.
                let src = magnifier_source(undimmed.as_ref().unwrap_or(&*canvas), m, &ctx);
                if let Some(src) = &src {
                    draw_magnifier(canvas, a, m, src, &ctx);
                    if let Some(u) = undimmed.as_mut() {
                        draw_magnifier(u, a, m, src, &ctx);
                    }
                }
                continue;
            }
            self.draw_one(canvas, &visible, i, first_spot, &influence, opts, &ctx, 0);
            if let Some(u) = undimmed.as_mut()
                && !matches!(a.kind, AnnotationKind::Spotlight(_))
            {
                self.draw_one(u, &visible, i, first_spot, &influence, opts, &ctx, 1);
            }
        }
    }

    /// Draw `visible[i]` (anything but a magnifier). `salt` separates blur
    /// cache entries of different target canvases.
    #[allow(clippy::too_many_arguments)]
    fn draw_one(
        &mut self,
        canvas: &mut sk::Pixmap,
        visible: &[&Annotation],
        i: usize,
        first_spot: Option<usize>,
        influence: &[Option<Rect>],
        opts: &RenderOptions,
        ctx: &Ctx,
        salt: u8,
    ) {
        let a = visible[i];
        match &a.kind {
            AnnotationKind::Spotlight(_) => {
                if Some(i) == first_spot {
                    draw_spotlights(canvas, visible, ctx);
                }
            }
            AnnotationKind::Blur(b) => {
                use std::hash::{Hash, Hasher};
                let s = ctx.scale;
                let reach = b.rect.outset(b.strength.max(1.0 / s) * 3.0 + 2.0 / s);
                let mut sig = std::collections::hash_map::DefaultHasher::new();
                self.last_base_key.hash(&mut sig);
                opts.quality.hash(&mut sig);
                salt.hash(&mut sig);
                for (j, below) in visible[..i].iter().enumerate() {
                    let hit = match influence.get(j).copied().flatten() {
                        None => true,
                        Some(r) => r.intersects(&reach),
                    };
                    if hit {
                        format!("{:?}|{}|{:?}", below.kind, below.opacity, below.shadow).hash(&mut sig);
                    }
                }
                self.apply_blur(canvas, b.rect, b.strength, b.corner_radius, sig.finish(), ctx);
            }
            AnnotationKind::Pixelate(p) => apply_pixelate(canvas, p.rect, p.block_size, ctx),
            AnnotationKind::Redact(r) => apply_redact(canvas, r.rect, r.color, r.corner_radius, ctx),
            AnnotationKind::Magnifier(_) => {}
            AnnotationKind::Highlighter(_) => {
                draw_kind(canvas, &a.kind, ctx.t, ctx.clip, ctx.sf, a.opacity.clamp(0.0, 1.0));
            }
            _ => draw_group(canvas, a, ctx),
        }
    }

    /// Destructive gaussian blur of the canvas region under `rect`.
    fn apply_blur(&mut self, canvas: &mut sk::Pixmap, rect: Rect, strength: f32, radius: f32, below: u64, ctx: &Ctx) {
        let r = map_rect(ctx.t, rect);
        let inner = PxRect::cover(r.x, r.y, r.width, r.height).intersect(&ctx.bounds);
        if inner.is_empty() {
            return;
        }
        // Minimum 1 px sigma so the region never keeps original pixels.
        let sigma = effects::safe_sigma(strength * ctx.scale).max(1.0);
        let margin = (sigma * 3.0).ceil() as i32;
        let ext = inner.outset(margin).intersect(&ctx.bounds);
        if ext.w < inner.w || ext.h < inner.h {
            return;
        }
        let key = BlurKey { ext, inner, sigma: sigma.to_bits(), hash: below };
        let out = match self.blur_cache.get(&key) {
            Some(o) => o.clone(),
            None => {
                let mut buf = extract(canvas, ext);
                effects::gaussian_blur::<4>(&mut buf, ext.w as usize, ext.h as usize, sigma);
                let mut o = Vec::with_capacity(inner.w as usize * inner.h as usize * 4);
                for y in inner.y..inner.y + inner.h {
                    let s = ((y - ext.y) as usize * ext.w as usize + (inner.x - ext.x) as usize) * 4;
                    o.extend_from_slice(&buf[s..s + inner.w as usize * 4]);
                }
                let o = Arc::new(o);
                if self.blur_bytes + o.len() > BLUR_CACHE_MAX_BYTES {
                    self.blur_cache.clear();
                    self.blur_bytes = 0;
                }
                self.blur_bytes += o.len();
                self.blur_cache.insert(key, o.clone());
                o
            }
        };
        let shape = shape_mask(inner, r, radius * ctx.scale);
        write_back(canvas, inner, &out, ctx.clip, shape.as_ref());
    }
}

/// 1:1 blit of the content pixmap at an integer offset. Rows away from the
/// rounded frame corners are copied / blended directly; corner rows use the
/// frame mask.
fn blit_content(
    canvas: &mut sk::Pixmap,
    src: &sk::Pixmap,
    ox: i32,
    oy: i32,
    opaque: bool,
    frame: Option<(Rect, f32)>,
    mask: Option<&sk::Mask>,
) {
    let (cw, ch) = (canvas.width() as i32, canvas.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let x0 = ox.max(0);
    let x1 = (ox + sw).min(cw);
    if x1 <= x0 {
        return;
    }
    let stride = cw as usize * 4;
    let sstride = sw as usize * 4;
    let sdata = src.data();
    let mw = canvas.width() as usize;
    let data = canvas.data_mut();
    for y in oy.max(0)..(oy + sh).min(ch) {
        let sy = (y - oy) as usize;
        let srow = &sdata[sy * sstride + (x0 - ox) as usize * 4..sy * sstride + (x1 - ox) as usize * 4];
        let drow = &mut data[y as usize * stride + x0 as usize * 4..y as usize * stride + x1 as usize * 4];
        let in_corner_band = match frame {
            Some((f, r)) if r > 0.0 => (y as f32) < f.y + r + 1.0 || (y as f32 + 1.0) > f.bottom() - r - 1.0,
            _ => false,
        };
        let mrow = if in_corner_band { mask.map(|m| &m.data()[y as usize * mw + x0 as usize..y as usize * mw + x1 as usize]) } else { None };
        match mrow {
            None if opaque => drow.copy_from_slice(srow),
            _ => {
                for (i, (d, s)) in drow.as_chunks_mut::<4>().0.iter_mut().zip(srow.as_chunks::<4>().0.iter()).enumerate() {
                    let cov = mrow.map_or(255u32, |m| m[i] as u32);
                    if cov == 0 {
                        continue;
                    }
                    let sa = s[3] as u32 * cov / 255;
                    if sa == 255 {
                        d.copy_from_slice(s);
                        continue;
                    }
                    let inv = 255 - sa;
                    for c in 0..4 {
                        let sv = s[c] as u32 * cov / 255;
                        d[c] = (sv + (d[c] as u32 * inv + 127) / 255).min(255) as u8;
                    }
                }
            }
        }
    }
}

/// Shared per-render drawing context.
struct Ctx<'a> {
    /// Content space → canvas px.
    t: sk::Transform,
    scale: f32,
    /// Always `None` today (the canvas edge is the only clip); kept so an
    /// optional clip can be reintroduced without touching every draw call.
    clip: Option<&'a sk::Mask>,
    /// Drawable area in canvas px = the whole canvas (annotations may sit
    /// over the background / padding / inset / chrome).
    bounds: PxRect,
    sf: f32,
    quality: Quality,
}

/// Region-sized AA mask of a rounded rect (None = plain rect, full coverage).
fn shape_mask(px: PxRect, r: Rect, radius: f32) -> Option<sk::Mask> {
    if radius <= 0.0 {
        return None;
    }
    let mut m = sk::Mask::new(px.w as u32, px.h as u32)?;
    let local = Rect::new(r.x - px.x as f32, r.y - px.y as f32, r.width, r.height);
    let path = rounded_rect_path(local, radius)?;
    m.fill_path(&path, sk::FillRule::Winding, true, sk::Transform::identity());
    Some(m)
}

fn apply_pixelate(canvas: &mut sk::Pixmap, rect: Rect, block: u32, ctx: &Ctx) {
    let r = map_rect(ctx.t, rect);
    let px = PxRect::cover(r.x, r.y, r.width, r.height).intersect(&ctx.bounds);
    if px.is_empty() {
        return;
    }
    let mut buf = extract(canvas, px);
    let b = (block.max(1) as f32 * ctx.scale).max(1.0);
    // Blocks of at least 2 px guarantee no original pixel survives.
    let b = if px.w * px.h > 1 { b.max(2.0) } else { b };
    effects::pixelate(&mut buf, px.w as usize, px.h as usize, b, r.x - px.x as f32, r.y - px.y as f32);
    write_back(canvas, px, &buf, ctx.clip, None);
}

fn apply_redact(canvas: &mut sk::Pixmap, rect: Rect, color: Color, radius: f32, ctx: &Ctx) {
    let r = map_rect(ctx.t, rect);
    let px = PxRect::cover(r.x, r.y, r.width, r.height).intersect(&ctx.bounds);
    if px.is_empty() {
        return;
    }
    let c = [color.r, color.g, color.b, 255];
    let buf: Vec<u8> = std::iter::repeat_n(c, px.w as usize * px.h as usize).flatten().collect();
    let shape = shape_mask(px, r, radius * ctx.scale);
    write_back(canvas, px, &buf, ctx.clip, shape.as_ref());
}

/// One dim layer with holes for every visible spotlight; color/opacity of
/// the topmost spotlight.
fn draw_spotlights(canvas: &mut sk::Pixmap, visible: &[&Annotation], ctx: &Ctx) {
    let spots: Vec<(&Annotation, &rs_core::SpotlightAnnotation)> = visible
        .iter()
        .filter_map(|a| match &a.kind {
            AnnotationKind::Spotlight(s) => Some((*a, s)),
            _ => None,
        })
        .collect();
    let Some((top_a, top)) = spots.last() else { return };
    let b = ctx.bounds;
    if b.is_empty() {
        return;
    }
    // Hole mask sized to the content bounds only.
    let Some(mut holes) = sk::Mask::new(b.w as u32, b.h as u32) else { return };
    let local = ctx.t.post_translate(-b.x as f32, -b.y as f32);
    for (_, s) in &spots {
        let path = match s.shape {
            rs_core::SpotlightShape::Rect => rounded_rect_path(s.rect, s.corner_radius),
            rs_core::SpotlightShape::Ellipse => sk::Rect::from_xywh(s.rect.x, s.rect.y, s.rect.width, s.rect.height)
                .and_then(sk::PathBuilder::from_oval),
        };
        if let Some(p) = path {
            holes.fill_path(&p, sk::FillRule::Winding, true, local);
        }
    }
    let dim = top.dim.with_alpha(top.dim.alpha_f32() * top_a.opacity.clamp(0.0, 1.0));
    effects::blend_solid(canvas, b, dim, ctx.clip, &holes);
}

/// Pixels under a magnifier's source circle (canvas px box + pixmap).
struct MagnifierSource {
    bx: PxRect,
    pixmap: sk::Pixmap,
}

fn magnifier_source(canvas: &sk::Pixmap, m: &rs_core::MagnifierAnnotation, ctx: &Ctx) -> Option<MagnifierSource> {
    let s = map_point(ctx.t, m.source_center);
    let sr = (m.source_radius * ctx.scale).max(0.5);
    let canvas_px = PxRect { x: 0, y: 0, w: canvas.width() as i32, h: canvas.height() as i32 };
    let bx = PxRect::cover(s.x - sr - 2.0, s.y - sr - 2.0, 2.0 * sr + 4.0, 2.0 * sr + 4.0).intersect(&canvas_px);
    if bx.is_empty() {
        return None;
    }
    let data = extract(canvas, bx);
    let pixmap = sk::IntSize::from_wh(bx.w as u32, bx.h as u32).and_then(|sz| sk::Pixmap::from_vec(data, sz))?;
    Some(MagnifierSource { bx, pixmap })
}

fn draw_magnifier(
    canvas: &mut sk::Pixmap,
    a: &Annotation,
    m: &rs_core::MagnifierAnnotation,
    src: &MagnifierSource,
    ctx: &Ctx,
) {
    let opacity = a.opacity.clamp(0.0, 1.0);
    let zoom = m.zoom.clamp(1.0, 8.0);
    let s = map_point(ctx.t, m.source_center);
    let sr = (m.source_radius * ctx.scale).max(0.5);
    let tc = map_point(ctx.t, m.target_center);
    let tr = sr * zoom;
    let src_box = src.bx;
    let snap = &src.pixmap;
    if opacity <= 0.0 {
        return;
    }
    let Some(circle) = circle_path(Point::new(tc.x, tc.y), tr) else { return };

    if let Some(sh) = a.shadow {
        let pad = 2i32;
        let sigma = effects::safe_sigma(sh.blur * ctx.scale / 2.0);
        // Only the part that can cast a visible shadow onto the canvas (a huge
        // loupe would otherwise allocate a mask far bigger than the canvas).
        let offset = (sh.offset_x.abs().max(sh.offset_y.abs()) * ctx.scale).min(1.0e6);
        let reach = (sigma * 3.0 + offset).ceil() as i32 + pad + 1;
        let px = PxRect::cover(tc.x - tr, tc.y - tr, 2.0 * tr, 2.0 * tr).outset(pad).intersect(&ctx.bounds.outset(reach));
        if let Some(mut mk) =
            (!px.is_empty()).then(|| sk::Mask::new(px.w.max(1) as u32, px.h.max(1) as u32)).flatten()
        {
            mk.fill_path(&circle, sk::FillRule::Winding, true, sk::Transform::from_translate(-px.x as f32, -px.y as f32));
            if let Some(img) =
                effects::shadow_from_alpha(mk.data(), px.w as usize, px.h as usize, px.x as f32, px.y as f32, sigma, sh.color)
            {
                img.draw(canvas, sh.offset_x * ctx.scale, sh.offset_y * ctx.scale, opacity, ctx.clip);
            }
        }
    }

    let pat_t = sk::Transform::from_row(
        zoom,
        0.0,
        0.0,
        zoom,
        tc.x + (src_box.x as f32 - s.x) * zoom,
        tc.y + (src_box.y as f32 - s.y) * zoom,
    );
    let q = if ctx.quality == Quality::High { sk::FilterQuality::Bicubic } else { sk::FilterQuality::Bilinear };
    let p = sk::Paint {
        anti_alias: true,
        shader: sk::Pattern::new(snap.as_ref(), sk::SpreadMode::Pad, q, opacity, pat_t),
        ..Default::default()
    };
    canvas.fill_path(&circle, &p, sk::FillRule::Winding, sk::Transform::identity(), ctx.clip);

    if m.border.width > 0.0 {
        let mut st = m.border;
        st.width *= ctx.scale;
        let col = m.border.color.with_alpha(m.border.color.alpha_f32() * opacity);
        let mut sks = sk_stroke(&st);
        sks.line_join = sk::LineJoin::Round;
        canvas.stroke_path(&circle, &paint(col), &sks, sk::Transform::identity(), ctx.clip);
        // Thin ring around the source region when the loupe is moved away.
        if s.distance(tc) > tr + sr
            && let Some(sc) = circle_path(s, sr) {
                let ring = sk::Stroke { width: (st.width * 0.5).max(1.0), ..Default::default() };
                canvas.stroke_path(&sc, &paint(col), &ring, sk::Transform::identity(), ctx.clip);
            }
    }
}

/// Content-space area an annotation can modify (`None` = everywhere, e.g.
/// spotlight dim).
fn influence_rect(a: &Annotation) -> Option<Rect> {
    if matches!(a.kind, AnnotationKind::Spotlight(_)) {
        return None;
    }
    let mut r = annotation_bounds(a).outset(2.0);
    if let Some(sh) = effective_shadow(a) {
        let ext = sh.blur.max(0.0) * 1.5 + 2.0;
        let moved = r.translate(sh.offset_x, sh.offset_y).outset(ext);
        r = r.union(&moved);
    }
    Some(r)
}

/// Shadow of an annotation: text uses its style shadow first.
fn effective_shadow(a: &Annotation) -> Option<Shadow> {
    match &a.kind {
        AnnotationKind::Text(t) => t.style.shadow.or(a.shadow),
        AnnotationKind::Callout(c) => a.shadow.or(c.text_style.shadow),
        _ => a.shadow,
    }
    .filter(|s| s.color.a > 0)
}

/// Draw a vector annotation with group opacity and drop shadow (through an
/// offscreen layer when needed).
fn draw_group(canvas: &mut sk::Pixmap, a: &Annotation, ctx: &Ctx) {
    let opacity = a.opacity.clamp(0.0, 1.0);
    if opacity <= 0.001 {
        return;
    }
    let shadow = effective_shadow(a);
    if opacity >= 0.999 && shadow.is_none() {
        draw_kind(canvas, &a.kind, ctx.t, ctx.clip, ctx.sf, 1.0);
        return;
    }
    let b = map_rect(ctx.t, annotation_bounds(a)).outset(2.0);
    let px = PxRect::cover(b.x, b.y, b.width, b.height).intersect(&ctx.bounds);
    if px.is_empty() {
        return;
    }
    let Some(mut layer) = sk::Pixmap::new(px.w as u32, px.h as u32) else { return };
    draw_kind(&mut layer, &a.kind, ctx.t.post_translate(-px.x as f32, -px.y as f32), None, ctx.sf, 1.0);
    if let Some(sh) = shadow {
        let alpha: Vec<u8> = layer.pixels().iter().map(|p| p.alpha()).collect();
        let sigma = sh.blur.max(0.0) * ctx.scale / 2.0;
        if let Some(img) =
            effects::shadow_from_alpha(&alpha, px.w as usize, px.h as usize, px.x as f32, px.y as f32, sigma, sh.color)
        {
            img.draw(canvas, sh.offset_x * ctx.scale, sh.offset_y * ctx.scale, opacity, ctx.clip);
        }
    }
    let paint = sk::PixmapPaint { opacity, ..Default::default() };
    canvas.draw_pixmap(px.x, px.y, layer.as_ref(), &paint, sk::Transform::identity(), ctx.clip);
}

// ---------------------------------------------------------------------------
// Bounds & hit-testing (content space)
// ---------------------------------------------------------------------------

/// Precise bounds of an annotation in content space (strokes, heads, text
/// extents; no shadow). Same as [`Renderer::annotation_bounds`].
pub fn annotation_bounds(a: &Annotation) -> Rect {
    match &a.kind {
        AnnotationKind::Rect(x) => {
            if x.style.mode.has_stroke() { x.rect.outset(x.style.stroke.width / 2.0) } else { x.rect }
        }
        AnnotationKind::Ellipse(x) => {
            if x.style.mode.has_stroke() { x.rect.outset(x.style.stroke.width / 2.0) } else { x.rect }
        }
        AnnotationKind::Line(x) => {
            let cap = if x.stroke.cap == rs_core::LineCap::Square { std::f32::consts::SQRT_2 } else { 1.0 };
            Rect::from_points(x.start, x.end).outset(x.stroke.width / 2.0 * cap)
        }
        AnnotationKind::Arrow(x) => {
            let g = geom::arrow_geometry(x);
            let mut r = Rect::bounding(&g.centerline).unwrap_or_default().outset(x.stroke.width / 2.0);
            for p in g.fills.iter() {
                let b = p.bounds();
                r = r.union(&Rect::new(b.x(), b.y(), b.width(), b.height()));
            }
            for p in g.strokes.iter() {
                let b = p.bounds();
                r = r.union(&Rect::new(b.x(), b.y(), b.width(), b.height()).outset(x.stroke.width / 2.0));
            }
            r
        }
        AnnotationKind::Pen(x) => Rect::bounding(&geom::smooth_polyline(&x.points))
            .unwrap_or_default()
            .outset(x.stroke.width / 2.0),
        AnnotationKind::Highlighter(x) => {
            Rect::bounding(&geom::smooth_polyline(&x.points)).unwrap_or_default().outset(x.width / 2.0)
        }
        AnnotationKind::Step(x) => {
            let bw = x.border.map(|b| b.width / 2.0).unwrap_or(0.0);
            let r = x.radius + bw;
            Rect::new(x.center.x - r, x.center.y - r, 2.0 * r, 2.0 * r)
        }
        AnnotationKind::Callout(x) => {
            let sw = if x.style.mode.has_stroke() { x.style.stroke.width / 2.0 } else { 0.0 };
            let mut r = callout_bubble_rect(x).union(&Rect::new(x.tail.x, x.tail.y, 0.0, 0.0)).outset(sw);
            if !x.text.is_empty() {
                r = r.union(&measure_text_annotation(&callout_text(x)));
            }
            r
        }
        AnnotationKind::Spotlight(x) => x.rect,
        AnnotationKind::Magnifier(x) => {
            let tr = x.target_radius() + x.border.width / 2.0;
            let mut r = Rect::new(x.target_center.x - tr, x.target_center.y - tr, 2.0 * tr, 2.0 * tr);
            if x.source_center.distance(x.target_center) > x.target_radius() + x.source_radius {
                let sr = x.source_radius + x.border.width / 2.0;
                r = r.union(&Rect::new(x.source_center.x - sr, x.source_center.y - sr, 2.0 * sr, 2.0 * sr));
            }
            r
        }
        AnnotationKind::Blur(x) => x.rect,
        AnnotationKind::Pixelate(x) => x.rect,
        AnnotationKind::Redact(x) => x.rect,
        AnnotationKind::Text(x) => measure_text_annotation(x),
    }
}

fn ellipse_hit(r: Rect, p: Point, filled: bool, half_w: f32) -> bool {
    let (a, b) = (r.width / 2.0, r.height / 2.0);
    if a <= 0.0 || b <= 0.0 {
        return false;
    }
    let c = r.center();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    let d = ((dx / a).powi(2) + (dy / b).powi(2)).sqrt();
    if filled && d <= 1.0 {
        return true;
    }
    ((d - 1.0) * a.min(b)).abs() <= half_w
}

fn rect_edge_hit(r: Rect, p: Point, half_w: f32) -> bool {
    r.outset(half_w).contains(p) && !r.outset(-half_w).contains(p)
}

/// Precise hit test in content space (see [`Renderer::hit_test`]).
/// Obfuscation, spotlight and filled shapes hit on their whole area;
/// outlines and strokes within `stroke/2 + tolerance`.
pub fn hit_test(a: &Annotation, p: Point, tolerance: f32) -> bool {
    let tol = tolerance.max(0.0);
    match &a.kind {
        AnnotationKind::Rect(x) => {
            (x.style.mode.has_fill() && x.rect.contains(p))
                || (x.style.mode.has_stroke() && rect_edge_hit(x.rect, p, x.style.stroke.width / 2.0 + tol))
        }
        AnnotationKind::Ellipse(x) => ellipse_hit(
            x.rect,
            p,
            x.style.mode.has_fill(),
            if x.style.mode.has_stroke() { x.style.stroke.width / 2.0 + tol } else { tol },
        ),
        AnnotationKind::Line(x) => geom::dist_to_segment(p, x.start, x.end) <= x.stroke.width / 2.0 + tol,
        AnnotationKind::Arrow(x) => {
            let g = geom::arrow_geometry(x);
            if geom::dist_to_polyline(p, &g.centerline) <= x.stroke.width / 2.0 + tol {
                return true;
            }
            let hr = x.head_width() / 2.0 + tol;
            p.distance(x.end) <= hr || (x.double_headed && p.distance(x.start) <= hr)
        }
        AnnotationKind::Pen(x) => {
            geom::dist_to_polyline(p, &geom::smooth_polyline(&x.points)) <= x.stroke.width / 2.0 + tol
        }
        AnnotationKind::Highlighter(x) => {
            geom::dist_to_polyline(p, &geom::smooth_polyline(&x.points)) <= x.width / 2.0 + tol
        }
        AnnotationKind::Step(x) => p.distance(x.center) <= x.radius + tol,
        AnnotationKind::Callout(x) => {
            let bubble = callout_bubble_rect(x);
            if bubble.outset(tol).contains(p) {
                return true;
            }
            match geom::callout_tail(bubble, x.corner_radius, x.tail, 24.0) {
                Some((ta, tip, tb, _)) => geom::point_in_triangle(p, ta, tip, tb) || p.distance(tip) <= tol,
                None => false,
            }
        }
        AnnotationKind::Magnifier(x) => {
            p.distance(x.target_center) <= x.target_radius() + x.border.width / 2.0 + tol
                || p.distance(x.source_center) <= x.source_radius + tol
        }
        AnnotationKind::Text(x) => measure_text_annotation(x).outset(tol).contains(p),
        AnnotationKind::Spotlight(_)
        | AnnotationKind::Blur(_)
        | AnnotationKind::Pixelate(_)
        | AnnotationKind::Redact(_) => annotation_bounds(a).outset(tol).contains(p),
    }
}

// ---------------------------------------------------------------------------
// Export / encode
// ---------------------------------------------------------------------------

/// Render and encode for export in one call. Returns encoded bytes and the
/// rendered image (for clipboard use).
pub fn export(
    renderer: &mut Renderer,
    doc: &Document,
    settings: &ExportSettings,
) -> Result<(Vec<u8>, RgbaImage), RenderError> {
    let out = renderer.render(doc, &RenderOptions::for_export(doc, settings))?;
    // Output px per pt == the export scale factor (X2 → 144 dpi), so a
    // Retina export opens at its point size in Preview & co.
    let bytes = encode_with_density(&out.image, settings.format, settings.scale.factor())?;
    Ok((bytes, out.image))
}

/// Encode an image. JPEG flattens alpha onto white. WebP is lossless.
/// PNG uses fast deflate (4K encodes in well under 100 ms). No density
/// metadata (see [`encode_with_density`]).
pub fn encode(img: &RgbaImage, format: ExportFormat) -> Result<Vec<u8>, RenderError> {
    encode_impl(img, format, None)
}

/// Like [`encode`], but records the pixel density: `px_per_pt` output
/// pixels per point → `72 · px_per_pt` dpi (2.0 → 144 dpi). PNG gets a
/// `pHYs` chunk (pixels per metre), JPEG a JFIF density in dots per inch.
/// WebP has no density field (the `image` encoder writes no EXIF), so it is
/// encoded without one. Non-finite / non-positive densities are ignored.
pub fn encode_with_density(img: &RgbaImage, format: ExportFormat, px_per_pt: f32) -> Result<Vec<u8>, RenderError> {
    let d = (px_per_pt.is_finite() && px_per_pt > 0.0).then_some(px_per_pt);
    encode_impl(img, format, d)
}

/// Dots per inch for `px_per_pt` (72 pt per inch).
pub fn dpi_for_px_per_pt(px_per_pt: f32) -> f32 {
    72.0 * px_per_pt
}

fn encode_impl(img: &RgbaImage, format: ExportFormat, px_per_pt: Option<f32>) -> Result<Vec<u8>, RenderError> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    match format {
        ExportFormat::Png => {
            image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Adaptive,
            )
            .write_image(&img.pixels, img.width, img.height, image::ExtendedColorType::Rgba8)?;
            if let Some(d) = px_per_pt {
                out = png_with_phys(out, dpi_for_px_per_pt(d));
            }
        }
        ExportFormat::Jpeg { quality } => {
            let mut rgb = Vec::with_capacity(img.width as usize * img.height as usize * 3);
            for px in img.pixels.as_chunks::<4>().0.iter() {
                let a = px[3] as u32;
                for c in &px[..3] {
                    rgb.push(((*c as u32 * a + 255 * (255 - a) + 127) / 255) as u8);
                }
            }
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
            if let Some(d) = px_per_pt {
                let dpi = dpi_for_px_per_pt(d).round().clamp(1.0, u16::MAX as f32) as u16;
                enc.set_pixel_density(image::codecs::jpeg::PixelDensity {
                    density: (dpi, dpi),
                    unit: image::codecs::jpeg::PixelDensityUnit::Inches,
                });
            }
            enc.write_image(
                &rgb,
                img.width,
                img.height,
                image::ExtendedColorType::Rgb8,
            )?;
        }
        ExportFormat::WebP => {
            image::codecs::webp::WebPEncoder::new_lossless(&mut out).write_image(
                &img.pixels,
                img.width,
                img.height,
                image::ExtendedColorType::Rgba8,
            )?;
        }
    }
    Ok(out)
}

/// Insert (or replace) a `pHYs` chunk right after IHDR. Returns the input
/// unchanged if it is not a well-formed PNG.
fn png_with_phys(png: Vec<u8>, dpi: f32) -> Vec<u8> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 33 || &png[..8] != SIG || &png[12..16] != b"IHDR" {
        return png;
    }
    let ppm = (dpi / 0.0254).round().clamp(1.0, u32::MAX as f32) as u32;
    let mut data = Vec::with_capacity(9);
    data.extend_from_slice(&ppm.to_be_bytes());
    data.extend_from_slice(&ppm.to_be_bytes());
    data.push(1); // unit: metre
    let mut chunk = Vec::with_capacity(21);
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(b"pHYs");
    chunk.extend_from_slice(&data);
    chunk.extend_from_slice(&crc32(&chunk[4..]).to_be_bytes());
    // Walk chunks: copy everything, dropping any existing pHYs.
    let mut out = Vec::with_capacity(png.len() + chunk.len());
    out.extend_from_slice(&png[..33]);
    out.extend_from_slice(&chunk);
    let mut i = 33;
    while i + 12 <= png.len() {
        let len = u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as usize;
        let end = i.saturating_add(12).saturating_add(len);
        if end > png.len() {
            return png;
        }
        if &png[i + 4..i + 8] != b"pHYs" {
            out.extend_from_slice(&png[i..end]);
        }
        i = end;
    }
    out
}

/// CRC-32 (IEEE, as used by PNG).
fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// The base image with the document's crop + rotation applied
/// (= content space bitmap, `doc.content_size()` px).
pub fn content_image(doc: &Document) -> Result<RgbaImage, RenderError> {
    let src = &doc.base_image;
    if !src.is_loaded() {
        return Err(RenderError::ImageNotLoaded);
    }
    let t = &doc.image_transform;
    let c = t.crop_rect(src.width, src.height);
    if c == rs_core::IRect::new(0, 0, src.width, src.height) && t.rotation == Rotation::Deg0 {
        return Ok(src.clone());
    }
    let (ow, oh) = t.content_size(src.width, src.height);
    let mut out = vec![0u8; ow as usize * oh as usize * 4];
    for y in 0..c.height {
        for x in 0..c.width {
            let si = (((c.y + y) * src.width + (c.x + x)) * 4) as usize;
            let (dx, dy) = match t.rotation {
                Rotation::Deg0 => (x, y),
                Rotation::Deg90 => (c.height - 1 - y, x),
                Rotation::Deg180 => (c.width - 1 - x, c.height - 1 - y),
                Rotation::Deg270 => (y, c.width - 1 - x),
            };
            let di = ((dy * ow + dx) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src.pixels[si..si + 4]);
        }
    }
    RgbaImage::new(ow, oh, out).ok_or(RenderError::Alloc(ow, oh))
}

/// Dominant color of the 1-px border ring of `img` (all four edges):
/// quantize to 5 bits/channel, take the most frequent bucket, return the
/// average of its members. Transparent image → `Color::WHITE`.
pub fn dominant_edge_color(img: &RgbaImage) -> Color {
    if !img.is_loaded() || img.width == 0 || img.height == 0 {
        return Color::WHITE;
    }
    let mut buckets: HashMap<u16, (u32, [u64; 4])> = HashMap::new();
    let mut add = |x: u32, y: u32| {
        if let Some(p) = img.pixel(x, y) {
            let key = ((p[0] as u16 >> 3) << 10) | ((p[1] as u16 >> 3) << 5) | (p[2] as u16 >> 3);
            let e = buckets.entry(key).or_insert((0, [0; 4]));
            e.0 += 1;
            for (acc, v) in e.1.iter_mut().zip(p) {
                *acc += v as u64;
            }
        }
    };
    for x in 0..img.width {
        add(x, 0);
        add(x, img.height - 1);
    }
    for y in 1..img.height.saturating_sub(1) {
        add(0, y);
        add(img.width - 1, y);
    }
    let Some((n, sums)) = buckets.into_values().max_by_key(|(n, _)| *n) else { return Color::WHITE };
    let n = n as u64;
    let c = Color::rgba((sums[0] / n) as u8, (sums[1] / n) as u8, (sums[2] / n) as u8, (sums[3] / n) as u8);
    if c.a == 0 { Color::WHITE } else { c }
}

/// Straight RGBA → premultiplied tiny-skia pixmap.
pub fn rgba_to_pixmap(img: &RgbaImage) -> Option<sk::Pixmap> {
    let mut pm = sk::Pixmap::new(img.width, img.height)?;
    for (dst, src) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(img.pixels.as_chunks::<4>().0.iter()) {
        let a = src[3];
        if a == 255 {
            dst.copy_from_slice(src);
        } else if a != 0 {
            let a = a as u16;
            dst[0] = ((src[0] as u16 * a + 127) / 255) as u8;
            dst[1] = ((src[1] as u16 * a + 127) / 255) as u8;
            dst[2] = ((src[2] as u16 * a + 127) / 255) as u8;
            dst[3] = src[3];
        }
    }
    Some(pm)
}

/// Premultiplied tiny-skia pixmap → straight RGBA.
pub fn pixmap_to_rgba(pm: &sk::Pixmap) -> RgbaImage {
    fn convert(src: &[u8], dst: &mut [u8]) {
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0.iter()) {
            let a = s[3];
            if a == 255 {
                d.copy_from_slice(s);
            } else if a != 0 {
                let a32 = a as u32;
                d[0] = ((s[0] as u32 * 255 + a32 / 2) / a32).min(255) as u8;
                d[1] = ((s[1] as u32 * 255 + a32 / 2) / a32).min(255) as u8;
                d[2] = ((s[2] as u32 * 255 + a32 / 2) / a32).min(255) as u8;
                d[3] = a;
            }
        }
    }
    let src = pm.data();
    let mut out = vec![0u8; src.len()];
    let px = src.len() / 4;
    if px < 512 * 1024 {
        convert(src, &mut out);
    } else {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
        let chunk = (px.div_ceil(n)) * 4;
        std::thread::scope(|sc| {
            for (s, d) in src.chunks(chunk).zip(out.chunks_mut(chunk)) {
                sc.spawn(move || convert(s, d));
            }
        });
    }
    RgbaImage::new(pm.width(), pm.height(), out).unwrap_or_else(|| RgbaImage::filled(1, 1, [0, 0, 0, 0]))
}

#[cfg(test)]
mod tests;
