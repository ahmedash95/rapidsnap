use super::*;
use rs_core::*;

const WHITE: [u8; 4] = [255, 255, 255, 255];

fn plain_doc(w: u32, h: u32, px: [u8; 4]) -> Document {
    let mut d = Document::new(RgbaImage::filled(w, h, px), DocumentMetadata::default());
    d.beautify = BeautifySettings::disabled();
    d
}

/// Deterministic noise image (every pixel different-ish).
fn noise(w: u32, h: u32) -> RgbaImage {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    let mut s: u32 = 0x1234_5678;
    for _ in 0..w * h {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        v.extend_from_slice(&[(s & 255) as u8, ((s >> 8) & 255) as u8, ((s >> 16) & 255) as u8, 255]);
    }
    RgbaImage::new(w, h, v).unwrap()
}

fn render(doc: &Document) -> RgbaImage {
    Renderer::new().render(doc, &RenderOptions::default()).unwrap().image
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.pixel(x, y).unwrap()
}

fn close(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (*x as i32 - *y as i32).abs() <= tol)
}

fn shape(mode: FillMode, color: Color) -> ShapeStyle {
    ShapeStyle { mode, stroke: Stroke::new(color, 4.0), fill: color }
}

fn add(doc: &mut Document, k: AnnotationKind) -> AnnotationId {
    let a = Annotation::new(k);
    let id = a.id;
    doc.annotations.push(a);
    id
}

#[test]
fn renders_solid_background_and_image() {
    let mut doc = Document::new(RgbaImage::filled(40, 20, [255, 0, 0, 255]), DocumentMetadata::default());
    doc.beautify.background = Background::Solid { color: Color::rgb(0, 0, 255) };
    doc.beautify.padding = Padding::uniform(10.0);
    doc.beautify.corner_radius = 0.0;
    doc.beautify.shadow.enabled = false;
    let out = Renderer::new().render(&doc, &RenderOptions::default()).unwrap();
    assert_eq!((out.image.width, out.image.height), (60, 40));
    assert_eq!(out.image.pixel(1, 1), Some([0, 0, 255, 255]));
    assert_eq!(out.image.pixel(30, 20), Some([255, 0, 0, 255]));
}

#[test]
fn renders_sample_document_every_kind_and_encodes() {
    let doc = test_support::sample_document();
    let mut r = Renderer::new();
    for scale in [0.5, 1.0, 2.0] {
        let out = r.render(&doc, &RenderOptions::preview(scale)).unwrap();
        assert!(out.image.is_loaded());
    }
    for f in [ExportFormat::Png, ExportFormat::Jpeg { quality: 80 }, ExportFormat::WebP] {
        let out = r.render(&doc, &RenderOptions::default()).unwrap();
        let bytes = encode(&out.image, f).unwrap();
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (out.image.width, out.image.height), "{f:?}");
    }
    let (bytes, _) = export(&mut r, &doc, &ExportSettings::default()).unwrap();
    assert_eq!(&bytes[1..4], b"PNG");
}

#[test]
fn export_scales_1x_2x() {
    let mut doc = plain_doc(200, 100, WHITE);
    doc.metadata.scale_factor = 2.0;
    let mut r = Renderer::new();
    let (_, x2) = export(&mut r, &doc, &ExportSettings { scale: ExportScale::X2, ..Default::default() }).unwrap();
    let (_, x1) = export(&mut r, &doc, &ExportSettings { scale: ExportScale::X1, ..Default::default() }).unwrap();
    assert_eq!((x2.width, x2.height), (200, 100));
    assert_eq!((x1.width, x1.height), (100, 50));
}

#[test]
fn jpeg_flattens_on_white() {
    let img = RgbaImage::filled(8, 8, [0, 0, 0, 0]);
    let bytes = encode(&img, ExportFormat::Jpeg { quality: 95 }).unwrap();
    let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
    assert!(back.get_pixel(4, 4).0.iter().all(|c| *c > 245));
}

#[test]
fn preview_equals_export_at_same_scale() {
    let mut doc = test_support::sample_document();
    doc.beautify.background = Background::Solid { color: Color::GRAY };
    // Annotations over the padding / background go through the same path.
    let cs = doc.content_size();
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(-40.0, -30.0, 60.0, 50.0), style: shape(FillMode::Outline, Color::RED), corner_radius: 6.0 }));
    add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(cs.width - 20.0, cs.height - 10.0, 60.0, 40.0), strength: 4.0, corner_radius: 0.0 }));
    add(&mut doc, AnnotationKind::Text(TextAnnotation { position: Point::new(-50.0, cs.height + 8.0), width: None, content: "Padding note".into(), style: TextStyle::default(), preset: None }));
    let mut r = Renderer::new();
    let export = RenderOptions::for_export(&doc, &ExportSettings { scale: ExportScale::X1, ..Default::default() });
    let b = r.render(&doc, &export).unwrap();
    // Same pipeline: a High-quality preview at the export scale is identical.
    let a = r.render(&doc, &RenderOptions { quality: Quality::High, ..RenderOptions::preview(0.5) }).unwrap();
    assert_eq!(a.layout, b.layout);
    assert_eq!(a.image.pixels, b.image.pixels);
    // Fast preview only differs by resampling filters (magnifier / scaling).
    let f = r.render(&doc, &RenderOptions::preview(0.5)).unwrap();
    let diff: u64 = f.image.pixels.iter().zip(b.image.pixels.iter()).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64).sum();
    assert!((diff as f64 / b.image.pixels.len() as f64) < 1.0, "mean diff {}", diff as f64 / b.image.pixels.len() as f64);
}

#[test]
fn edge_color_dominant_not_first_pixel() {
    let mut img = RgbaImage::filled(10, 10, [10, 20, 200, 255]);
    let p = std::sync::Arc::make_mut(&mut img.pixels);
    p[0..4].copy_from_slice(&[255, 0, 0, 255]);
    assert_eq!(dominant_edge_color(&img), Color::rgb(10, 20, 200));
}

#[test]
fn crop_rotate_content() {
    let mut pxs = vec![0u8; 3 * 2 * 4];
    pxs[0..4].copy_from_slice(&[9, 9, 9, 255]);
    let mut doc = Document::new(RgbaImage::new(3, 2, pxs).unwrap(), DocumentMetadata::default());
    doc.image_transform.rotation = Rotation::Deg90;
    let c = content_image(&doc).unwrap();
    assert_eq!((c.width, c.height), (2, 3));
    assert_eq!(c.pixel(1, 0), Some([9, 9, 9, 255]));
    doc.image_transform.crop = Some(IRect::new(1, 0, 2, 2));
    assert_eq!(content_image(&doc).unwrap().size(), Size::new(2.0, 2.0));
}

#[test]
fn layout_mapping_matches_rendered_pixels() {
    let mut doc = Document::new(RgbaImage::filled(100, 60, WHITE), DocumentMetadata { scale_factor: 2.0, ..Default::default() });
    doc.beautify.background = Background::Solid { color: Color::BLACK };
    doc.beautify.shadow.enabled = false;
    doc.beautify.padding = Padding::uniform(7.0);
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(40.0, 20.0, 10.0, 10.0), style: shape(FillMode::Filled, Color::BLUE), corner_radius: 0.0 }));
    for scale in [1.0f32, 0.5, 1.5] {
        let out = Renderer::new().render(&doc, &RenderOptions::preview(scale)).unwrap();
        let p = out.layout.image_to_canvas(Point::new(45.0, 25.0));
        assert_eq!(px(&out.image, p.x as u32, p.y as u32), [0x0A, 0x84, 0xFF, 255], "scale {scale}");
        let back = out.layout.canvas_to_image(p);
        assert!((back.x - 45.0).abs() < 1e-3 && (back.y - 25.0).abs() < 1e-3);
    }
}

#[test]
fn annotations_cover_padding_and_clip_at_canvas_edge() {
    let mut doc = Document::new(RgbaImage::filled(50, 50, WHITE), DocumentMetadata::default());
    doc.beautify.background = Background::Solid { color: Color::BLACK };
    doc.beautify.shadow.enabled = false;
    doc.beautify.corner_radius = 0.0;
    doc.beautify.padding = Padding::uniform(20.0);
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(-30.0, -30.0, 200.0, 200.0), style: shape(FillMode::Filled, Color::RED), corner_radius: 0.0 }));
    let img = render(&doc);
    assert_eq!((img.width, img.height), (90, 90), "layout ignores annotations");
    assert_eq!(px(&img, 5, 5), [0xFF, 0x3B, 0x30, 255], "annotation drawn over the padding");
    assert_eq!(px(&img, 45, 45), [0xFF, 0x3B, 0x30, 255]);
    assert_eq!(px(&img, 89, 89), [0xFF, 0x3B, 0x30, 255], "reaches the canvas edge");
}

/// Retina doc (sf 2) with a solid blue background, 20 pt padding, no shadow.
fn padded_doc(w: u32, h: u32, radius_pt: f32) -> Document {
    let mut doc = Document::new(RgbaImage::filled(w, h, WHITE), DocumentMetadata { scale_factor: 2.0, ..Default::default() });
    doc.beautify = BeautifySettings::default();
    doc.beautify.background = Background::Solid { color: Color::rgb(0, 0, 255) };
    doc.beautify.padding = Padding::uniform(20.0);
    doc.beautify.corner_radius = radius_pt;
    doc.beautify.shadow.enabled = false;
    doc.beautify.inset.enabled = false;
    doc.beautify.chrome = WindowChrome::None;
    doc.beautify.auto_balance = false;
    doc.beautify.aspect_ratio = AspectRatio::Free;
    doc.beautify.watermark = None;
    doc
}

const RED: [u8; 4] = [0xFF, 0x3B, 0x30, 255];
const BLUE_BG: [u8; 4] = [0, 0, 255, 255];

#[test]
fn annotation_in_padding_exports_at_right_position_1x_2x() {
    let mut doc = padded_doc(100, 60, 0.0);
    // Fully inside the left/top padding (content space: negative coordinates).
    let r = Rect::new(-30.0, -30.0, 20.0, 20.0);
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: r, style: shape(FillMode::Filled, Color::RED), corner_radius: 0.0 }));
    // And one right of the image, below its bottom edge.
    add(&mut doc, AnnotationKind::Redact(RedactAnnotation { rect: Rect::new(104.0, 64.0, 10.0, 10.0), color: Color::rgb(0, 255, 0), corner_radius: 0.0 }));
    let mut rr = Renderer::new();
    for (scale, pad_px) in [(ExportScale::X1, 20u32), (ExportScale::X2, 40u32)] {
        let (bytes, img) = export(&mut rr, &doc, &ExportSettings { scale, format: ExportFormat::Png, ..Default::default() }).unwrap();
        let s = scale.factor() / 2.0; // output px per content px
        assert_eq!(img.width, (100.0 * s) as u32 + 2 * pad_px, "{scale:?}");
        // The encoded file carries the same pixels.
        let dec = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(dec.as_raw(), &*img.pixels);
        // Rect center: content (-20, -20) → canvas pad + (-20)·s.
        let c = (pad_px as f32 - 20.0 * s) as u32;
        assert_eq!(px(&img, c, c), RED, "{scale:?} rect center");
        let e = (pad_px as f32 - 29.0 * s) as u32;
        assert_eq!(px(&img, e, e), RED, "{scale:?} near rect corner");
        // Just outside the rect: background.
        let o = (pad_px as f32 - 8.0 * s) as u32;
        assert_eq!(px(&img, o, o), BLUE_BG, "{scale:?} outside rect");
        assert_eq!(px(&img, 1, 1), BLUE_BG);
        // Redact right/below the image.
        let q = (pad_px as f32 + 109.0 * s, pad_px as f32 + 69.0 * s);
        assert_eq!(px(&img, q.0 as u32, q.1 as u32), [0, 255, 0, 255], "{scale:?} redact in padding");
    }
}

#[test]
fn annotations_not_clipped_by_rounded_frame() {
    let mut doc = padded_doc(100, 60, 16.0); // 32 px radius
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(-12.0, -12.0, 24.0, 24.0), style: shape(FillMode::Filled, Color::RED), corner_radius: 0.0 }));
    let out = Renderer::new().render(&doc, &RenderOptions::default()).unwrap();
    let at = |x: f32, y: f32| {
        let q = out.layout.image_to_canvas(Point::new(x, y));
        px(&out.image, q.x as u32, q.y as u32)
    };
    // The image's own rounded corner shows background at (1,1)…
    assert_eq!(at(98.5, 1.5), BLUE_BG, "frame corner still rounded");
    // …but the annotation covers the frame corner and the padding.
    assert_eq!(at(1.5, 1.5), RED);
    assert_eq!(at(-8.0, -8.0), RED);
    assert_eq!(at(8.0, 8.0), RED);
}

/// Write a noise PNG to a unique temp file (custom background).
fn noise_background(w: u32, h: u32) -> std::path::PathBuf {
    let n = noise(w, h);
    let dir = std::env::temp_dir().join("rs-render-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("noise-bg-{}-{:?}.png", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    std::fs::write(&path, encode(&n, ExportFormat::Png).unwrap()).unwrap();
    path
}

#[test]
fn obfuscation_over_background_is_destructive() {
    let bg = noise_background(300, 200);
    let mut doc = padded_doc(100, 60, 0.0);
    doc.beautify.background = Background::CustomImage { path: bg.clone() };
    let mut r = Renderer::new();
    let export_opts = RenderOptions::for_export(&doc, &ExportSettings { scale: ExportScale::X2, ..Default::default() });
    let plain = r.render(&doc, &export_opts).unwrap();
    let l = plain.layout;
    // Blur, pixelate and redact entirely inside the padding (left, top, right),
    // plus a blur straddling the image edge.
    let blur = Rect::new(-38.0, 5.0, 30.0, 40.0);
    let pix = Rect::new(10.0, -36.0, 32.0, 32.0);
    let red = Rect::new(103.0, 10.0, 12.0, 30.0);
    let edge = Rect::new(60.0, 50.0, 30.0, 25.0);
    add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: blur, strength: 3.0, corner_radius: 0.0 }));
    add(&mut doc, AnnotationKind::Pixelate(PixelateAnnotation { rect: pix, block_size: 8 }));
    add(&mut doc, AnnotationKind::Redact(RedactAnnotation { rect: red, color: Color::rgb(9, 9, 9), corner_radius: 0.0 }));
    add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: edge, strength: 3.0, corner_radius: 0.0 }));
    for fresh in [false, true] {
        // Second pass: a fresh renderer (cold caches) must agree.
        let out = if fresh { Renderer::new().render(&doc, &export_opts).unwrap() } else { r.render(&doc, &export_opts).unwrap() };
        assert_eq!(out.layout, l);
        let pixels = |rc: Rect| {
            let c = l.image_rect_to_canvas(rc);
            let (x0, y0) = (c.x.ceil() as u32, c.y.ceil() as u32);
            let (x1, y1) = (c.right().floor() as u32, c.bottom().floor() as u32);
            (y0..y1).flat_map(move |y| (x0..x1).map(move |x| (x, y))).collect::<Vec<_>>()
        };
        for (name, rc) in [("blur", blur), ("pixelate", pix), ("redact", red)] {
            let pts = pixels(rc);
            assert!(pts.len() > 100, "{name}");
            let same = pts.iter().filter(|&&(x, y)| px(&out.image, x, y) == px(&plain.image, x, y)).count();
            assert!(same * 20 < pts.len(), "{name}: {same}/{} original background pixels survived", pts.len());
        }
        for (x, y) in pixels(red) {
            assert_eq!(px(&out.image, x, y), [9, 9, 9, 255]);
        }
        // Pixelate blocks are uniform (X2 of a Retina doc: 8 content px = 8 output px).
        assert_eq!(l.scale, 1.0);
        let c = l.image_to_canvas(Point::new(pix.x, pix.y));
        assert_eq!(px(&out.image, c.x as u32 + 1, c.y as u32 + 1), px(&out.image, c.x as u32 + 6, c.y as u32 + 6));
        assert_ne!(px(&out.image, c.x as u32 + 1, c.y as u32 + 1), px(&out.image, c.x as u32 + 9, c.y as u32 + 1));
        // The blur straddling the image edge mixes white image and noise
        // background on both sides of the edge.
        let inside = l.image_to_canvas(Point::new(75.0, 58.0));
        assert_ne!(px(&out.image, inside.x as u32, inside.y as u32), WHITE);
        let pts = pixels(Rect::new(62.0, 61.0, 26.0, 12.0));
        let same = pts.iter().filter(|&&(x, y)| px(&out.image, x, y) == px(&plain.image, x, y)).count();
        assert!(same * 20 < pts.len(), "edge blur below the image: {same}/{}", pts.len());
        // Outside every obfuscation: untouched background.
        assert_eq!(px(&out.image, 2, 2), px(&plain.image, 2, 2));
    }
    let _ = std::fs::remove_file(bg);
}

#[test]
fn spotlight_dims_whole_canvas_and_magnifier_works_in_padding() {
    let mut doc = padded_doc(100, 60, 0.0);
    doc.beautify.padding = Padding::uniform(40.0); // 80 px
    // A small blue square in the right padding, magnified into the left padding.
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(118.0, 28.0, 4.0, 4.0), style: shape(FillMode::Filled, Color::rgb(255, 255, 0)), corner_radius: 0.0 }));
    add(&mut doc, AnnotationKind::Magnifier(MagnifierAnnotation {
        source_center: Point::new(120.0, 30.0),
        source_radius: 8.0,
        target_center: Point::new(-40.0, 30.0),
        zoom: 3.0,
        border: Stroke::new(Color::BLACK, 1.0),
    }));
    add(&mut doc, AnnotationKind::Spotlight(SpotlightAnnotation {
        rect: Rect::new(-70.0, -70.0, 30.0, 30.0),
        shape: SpotlightShape::Rect,
        corner_radius: 0.0,
        dim: Color::BLACK.with_alpha(0.5),
    }));
    let out = Renderer::new().render(&doc, &RenderOptions::default()).unwrap();
    let at = |x: f32, y: f32| {
        let q = out.layout.image_to_canvas(Point::new(x, y));
        px(&out.image, q.x as u32, q.y as u32)
    };
    // Magnified yellow square at the loupe center (in the padding), dimmed
    // by the spotlight above it (it is outside the hole).
    let m = at(-40.0, 30.0);
    assert!(close(m, [128, 128, 0, 255], 24), "loupe center {m:?}");
    // Spotlight hole in the padding keeps the background; the rest of the
    // canvas (padding and image) is dimmed.
    assert_eq!(at(-55.0, -55.0), BLUE_BG, "hole stays clear");
    assert!(close(at(-20.0, 70.0), [0, 0, 128, 255], 2), "padding dimmed {:?}", at(-20.0, 70.0));
    assert!(close(at(50.0, 30.0), [128, 128, 128, 255], 2), "image dimmed {:?}", at(50.0, 30.0));
}

#[test]
fn crop_rotate_remap_keeps_outside_annotations_attached() {
    let mut doc = padded_doc(100, 60, 0.0);
    doc.beautify.padding = Padding::uniform(30.0);
    add(&mut doc, AnnotationKind::Redact(RedactAnnotation { rect: Rect::new(-20.0, -10.0, 10.0, 10.0), color: Color::rgb(0, 255, 0), corner_radius: 0.0 }));
    let mut h = History::new();
    // Rotate 90° cw: content(-15,-5) → base (-15,-5) → content (60+5, -15).
    let before = doc.image_transform;
    let after = ImageTransform { rotation: Rotation::Deg90, ..before };
    h.apply(&mut doc, Command::SetImageTransform { before, after }).unwrap();
    let g = doc.annotations[0].geometric_bounds();
    assert_eq!(g, Rect::new(60.0, -20.0, 10.0, 10.0));
    let out = Renderer::new().render(&doc, &RenderOptions::default()).unwrap();
    let q = out.layout.image_to_canvas(Point::new(65.0, -15.0));
    assert_eq!(px(&out.image, q.x as u32, q.y as u32), [0, 255, 0, 255]);
    // Crop keeps it attached to the same base pixels (still outside).
    let before = doc.image_transform;
    let after = ImageTransform { crop: Some(IRect::new(10, 5, 50, 40)), rotation: Rotation::Deg90 };
    h.apply(&mut doc, Command::SetImageTransform { before, after }).unwrap();
    let g2 = doc.annotations[0].geometric_bounds();
    let back = after.content_to_base(g2.center(), 100, 60);
    assert!((back.x + 15.0).abs() < 1e-3 && (back.y + 5.0).abs() < 1e-3, "{back:?}");
    h.undo(&mut doc).unwrap();
    h.undo(&mut doc).unwrap();
    assert_eq!(doc.annotations[0].geometric_bounds(), Rect::new(-20.0, -10.0, 10.0, 10.0));
}

#[test]
fn beautify_off_clips_outside_annotations_at_image_edge() {
    let mut doc = plain_doc(60, 40, WHITE);
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(-20.0, -20.0, 30.0, 30.0), style: shape(FillMode::Filled, Color::RED), corner_radius: 0.0 }));
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(100.0, 100.0, 30.0, 30.0), style: shape(FillMode::Filled, Color::RED), corner_radius: 0.0 }));
    let img = render(&doc);
    assert_eq!((img.width, img.height), (60, 40));
    assert_eq!(px(&img, 0, 0), RED);
    assert_eq!(px(&img, 9, 9), RED);
    assert_eq!(px(&img, 11, 11), WHITE);
    assert_eq!(px(&img, 59, 39), WHITE);
}

#[test]
fn blur_cache_tracks_content_changes_with_same_canvas_size() {
    // A 180° rotation keeps the canvas size but changes every pixel beneath a
    // blur: the cached blur must not be reused.
    let mut doc = plain_doc(80, 40, WHITE);
    doc.base_image = noise(80, 40);
    add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(5.0, 5.0, 30.0, 20.0), strength: 2.0, corner_radius: 0.0 }));
    let mut r = Renderer::new();
    r.render(&doc, &RenderOptions::default()).unwrap();
    doc.image_transform.rotation = Rotation::Deg180;
    let warm = r.render(&doc, &RenderOptions::default()).unwrap().image;
    let cold = Renderer::new().render(&doc, &RenderOptions::default()).unwrap().image;
    assert_eq!(warm.pixels, cold.pixels);
}

#[test]
fn redact_is_destructive_and_opaque() {
    let base = noise(120, 80);
    let mut doc = Document::new(base.clone(), DocumentMetadata::default());
    doc.beautify.background = Background::Solid { color: Color::WHITE };
    doc.beautify.shadow.enabled = false;
    add(&mut doc, AnnotationKind::Redact(RedactAnnotation { rect: Rect::new(10.3, 10.7, 50.2, 30.1), color: Color::rgba(10, 20, 30, 40), corner_radius: 0.0 }));
    let out = Renderer::new().render(&doc, &RenderOptions::default()).unwrap();
    let l = out.layout;
    for y in 10..41 {
        for x in 10..61 {
            let p = l.image_to_canvas(Point::new(x as f32, y as f32));
            assert_eq!(px(&out.image, p.x as u32, p.y as u32), [10, 20, 30, 255], "({x},{y})");
        }
    }
}

#[test]
fn blur_is_destructive_and_local() {
    let base = noise(120, 80);
    let mut doc = plain_doc(120, 80, WHITE);
    doc.base_image = base.clone();
    add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(20.0, 20.0, 60.0, 40.0), strength: 4.0, corner_radius: 0.0 }));
    let img = render(&doc);
    let mut same = 0;
    for y in 20..60 {
        for x in 20..80 {
            if px(&img, x, y) == px(&base, x, y) {
                same += 1;
            }
        }
    }
    assert!(same < 24, "{same} original pixels survived");
    // Outside untouched.
    assert_eq!(px(&img, 5, 5), px(&base, 5, 5));
    assert_eq!(px(&img, 100, 70), px(&base, 100, 70));
    // Blurred noise is smooth: neighbours close.
    let a = px(&img, 50, 40);
    let b = px(&img, 51, 40);
    assert!(close(a, b, 24), "{a:?} {b:?}");
}

#[test]
fn tiny_blur_strength_still_destructive() {
    let base = noise(40, 40);
    let mut doc = plain_doc(40, 40, WHITE);
    doc.base_image = base.clone();
    add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(10.0, 10.0, 20.0, 20.0), strength: 0.0, corner_radius: 0.0 }));
    let img = render(&doc);
    let same = (10..30).flat_map(|y| (10..30).map(move |x| (x, y))).filter(|&(x, y)| px(&img, x, y) == px(&base, x, y)).count();
    assert!(same < 8, "{same}");
}

#[test]
fn blur_cache_hits_and_invalidates() {
    let mut doc = plain_doc(100, 60, WHITE);
    doc.base_image = noise(100, 60);
    add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(10.0, 10.0, 50.0, 30.0), strength: 5.0, corner_radius: 4.0 }));
    let mut r = Renderer::new();
    let a = r.render(&doc, &RenderOptions::default()).unwrap().image;
    assert_eq!(r.blur_cache.len(), 1);
    let b = r.render(&doc, &RenderOptions::default()).unwrap().image;
    assert_eq!(a.pixels, b.pixels);
    assert_eq!(r.blur_cache.len(), 1);
    // A far-away annotation below doesn't invalidate.
    doc.annotations.insert(0, Annotation::new(AnnotationKind::Rect(RectAnnotation { rect: Rect::new(85.0, 45.0, 5.0, 5.0), style: shape(FillMode::Filled, Color::RED), corner_radius: 0.0 })));
    r.render(&doc, &RenderOptions::default()).unwrap();
    assert_eq!(r.blur_cache.len(), 1);
    // An overlapping one does, and the result reflects it.
    doc.annotations.insert(0, Annotation::new(AnnotationKind::Rect(RectAnnotation { rect: Rect::new(20.0, 15.0, 20.0, 20.0), style: shape(FillMode::Filled, Color::RED), corner_radius: 0.0 })));
    let c = r.render(&doc, &RenderOptions::default()).unwrap().image;
    assert_eq!(r.blur_cache.len(), 2);
    assert_ne!(px(&c, 30, 25), px(&a, 30, 25));
}

#[test]
fn pixelate_is_blocky_and_destructive() {
    let base = noise(64, 64);
    let mut doc = plain_doc(64, 64, WHITE);
    doc.base_image = base.clone();
    add(&mut doc, AnnotationKind::Pixelate(PixelateAnnotation { rect: Rect::new(8.0, 8.0, 32.0, 32.0), block_size: 8 }));
    let img = render(&doc);
    assert_eq!(px(&img, 8, 8), px(&img, 15, 15));
    assert_ne!(px(&img, 8, 8), px(&img, 16, 8));
    let same = (8..40).flat_map(|y| (8..40).map(move |x| (x, y))).filter(|&(x, y)| px(&img, x, y) == px(&base, x, y)).count();
    assert!(same < 8, "{same}");
}

#[test]
fn obfuscation_reads_lower_annotations() {
    let mut doc = plain_doc(60, 60, WHITE);
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(0.0, 0.0, 30.0, 60.0), style: shape(FillMode::Filled, Color::BLACK), corner_radius: 0.0 }));
    add(&mut doc, AnnotationKind::Pixelate(PixelateAnnotation { rect: Rect::new(20.0, 0.0, 20.0, 20.0), block_size: 20 }));
    let img = render(&doc);
    let p = px(&img, 30, 10);
    assert!(p[0] > 100 && p[0] < 160, "{p:?} (avg of half black / half white)");
}

#[test]
fn arrow_head_points_to_end() {
    let mut doc = plain_doc(200, 100, WHITE);
    add(&mut doc, AnnotationKind::Arrow(ArrowAnnotation {
        start: Point::new(20.0, 50.0),
        end: Point::new(180.0, 50.0),
        control: None,
        stroke: Stroke::new(Color::BLACK, 5.0),
        head: ArrowHead::FilledTriangle,
        double_headed: false,
        tapered: false,
        head_scale: 1.0,
    }));
    let img = render(&doc);
    // Head is 20 long × 15 wide: wide near the base, empty beyond the tip.
    assert_eq!(px(&img, 163, 45), [0, 0, 0, 255]);
    assert_eq!(px(&img, 163, 54), [0, 0, 0, 255]);
    assert_eq!(px(&img, 25, 44), WHITE, "no head at the tail");
    assert_eq!(px(&img, 186, 50), WHITE, "nothing past the tip");
}

#[test]
fn highlighter_multiplies() {
    let mut base = RgbaImage::filled(100, 40, WHITE);
    let p = std::sync::Arc::make_mut(&mut base.pixels);
    for y in 0..40 {
        for x in 50..100 {
            p[(y * 100 + x) * 4..(y * 100 + x) * 4 + 3].copy_from_slice(&[0, 0, 0]);
        }
    }
    let mut doc = plain_doc(100, 40, WHITE);
    doc.base_image = base;
    add(&mut doc, AnnotationKind::Highlighter(HighlighterAnnotation {
        points: vec![Point::new(0.0, 20.0), Point::new(100.0, 20.0)],
        color: Color::rgb(255, 255, 0),
        width: 20.0,
    }));
    let img = render(&doc);
    assert_eq!(px(&img, 20, 20), [255, 255, 0, 255]);
    assert_eq!(px(&img, 80, 20), [0, 0, 0, 255], "multiply keeps black black");
    assert_eq!(px(&img, 20, 2), WHITE);
}

#[test]
fn spotlight_dims_outside_only() {
    let mut doc = plain_doc(100, 100, WHITE);
    for r in [Rect::new(10.0, 10.0, 30.0, 30.0), Rect::new(30.0, 30.0, 30.0, 30.0)] {
        add(&mut doc, AnnotationKind::Spotlight(SpotlightAnnotation { rect: r, shape: SpotlightShape::Rect, corner_radius: 0.0, dim: Color::BLACK.with_alpha(0.5) }));
    }
    let img = render(&doc);
    assert_eq!(px(&img, 20, 20), WHITE);
    assert_eq!(px(&img, 35, 35), WHITE, "overlapping holes stay clear");
    let d = px(&img, 90, 90);
    assert!(close(d, [128, 128, 128, 255], 2), "{d:?}");
}

#[test]
fn magnifier_shows_zoomed_source() {
    let mut doc = plain_doc(200, 100, WHITE);
    add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(18.0, 18.0, 4.0, 4.0), style: shape(FillMode::Filled, Color::BLUE), corner_radius: 0.0 }));
    add(&mut doc, AnnotationKind::Magnifier(MagnifierAnnotation {
        source_center: Point::new(20.0, 20.0),
        source_radius: 10.0,
        target_center: Point::new(150.0, 50.0),
        zoom: 3.0,
        border: Stroke::new(Color::BLACK, 2.0),
    }));
    let img = render(&doc);
    // Blue 4×4 square magnified ×3 → 12×12 around the loupe center.
    // Bicubic resampling may ring slightly.
    assert!(close(px(&img, 150, 50), [0x0A, 0x84, 0xFF, 255], 16), "{:?}", px(&img, 150, 50));
    assert!(close(px(&img, 152, 52), [0x0A, 0x84, 0xFF, 255], 16), "{:?}", px(&img, 152, 52));
    assert_eq!(px(&img, 150, 30), WHITE);
    assert_eq!(px(&img, 150, 20), [0, 0, 0, 255], "border ring at r=30");
}

#[test]
fn text_outline_is_true_stroke_behind_fill() {
    let mut doc = plain_doc(400, 120, [0, 128, 0, 255]);
    let mut st = TextPreset::Title.style();
    st.size = 60.0;
    st.background = TextBackground::Outline { width: 4.0, color: Color::BLACK };
    add(&mut doc, AnnotationKind::Text(TextAnnotation { position: Point::new(10.0, 10.0), width: None, content: "HIM".into(), style: st, preset: Some(TextPreset::Title) }));
    let img = render(&doc);
    let (mut white, mut black) = (0, 0);
    for p in img.pixels.as_chunks::<4>().0 {
        if p[..3] == [255, 255, 255] {
            white += 1;
        }
        if p[..3] == [0, 0, 0] {
            black += 1;
        }
    }
    assert!(white > 500 && black > 500, "white {white} black {black}");
}

#[test]
fn text_pill_background_and_bounds() {
    let st = TextPreset::Label.style().scaled(2.0);
    let a = Annotation::new(AnnotationKind::Text(TextAnnotation { position: Point::new(40.0, 40.0), width: None, content: "Label".into(), style: st.clone(), preset: None }));
    let b = annotation_bounds(&a);
    let AnnotationKind::Text(t) = &a.kind else { unreachable!() };
    let bx = text_box(t);
    assert!(b.x < bx.x - 19.0 && b.y < bx.y - 7.0, "pill padding outside box: {b:?} vs {bx:?}");
    let mut doc = plain_doc(300, 150, WHITE);
    doc.annotations.push(a.clone());
    let img = render(&doc);
    // Pill color just inside its left padding.
    let p = px(&img, (b.x + 12.0) as u32, (b.y + b.height / 2.0) as u32);
    assert_eq!(p, [0xFF, 0x3B, 0x30, 255]);
    assert!(hit_test(&a, Point::new(bx.x + 2.0, bx.y + 2.0), 0.0));
    assert!(!hit_test(&a, Point::new(290.0, 140.0), 0.0));
}

#[test]
fn text_wraps_with_fixed_width() {
    let mut r = Renderer::new();
    let st = TextStyle { size: 20.0, ..TextStyle::default() };
    let one = r.measure_text("hello world foo bar", &st, None);
    let wrapped = r.measure_text("hello world foo bar", &st, Some(60.0));
    assert_eq!(wrapped.width, 60.0);
    assert!(wrapped.height > one.height * 2.5);
}

#[test]
fn step_counter_label_centered() {
    let mut doc = plain_doc(100, 100, WHITE);
    add(&mut doc, AnnotationKind::Step(StepAnnotation {
        center: Point::new(50.0, 50.0),
        radius: 30.0,
        value: 1,
        label: StepLabel::Number,
        fill: Color::RED,
        text_color: Color::WHITE,
        border: None,
        font_family: FontFamily::System,
        font_weight: FontWeight::BOLD,
    }));
    let img = render(&doc);
    assert_eq!(px(&img, 50, 25), [0xFF, 0x3B, 0x30, 255], "fill above label");
    // Label ink roughly symmetric around the center.
    let mut xs = Vec::new();
    for y in 30..70 {
        for x in 20..80 {
            let p = px(&img, x, y);
            if p[1] > 200 && p[2] > 200 {
                xs.push(x as f32);
            }
        }
    }
    assert!(!xs.is_empty());
    let mean = xs.iter().sum::<f32>() / xs.len() as f32;
    assert!((mean - 50.0).abs() < 3.0, "label centroid {mean}");
}

#[test]
fn callout_tail_is_filled() {
    let mut doc = plain_doc(200, 200, [200, 200, 200, 255]);
    add(&mut doc, AnnotationKind::Callout(CalloutAnnotation {
        rect: Rect::new(20.0, 20.0, 160.0, 80.0),
        tail: Point::new(60.0, 180.0),
        corner_radius: 10.0,
        style: ShapeStyle { mode: FillMode::OutlineAndFill, stroke: Stroke::new(Color::BLACK, 2.0), fill: Color::WHITE },
        text: "Hi there".into(),
        text_style: TextStyle { size: 20.0, color: Color::BLACK, ..TextStyle::default() },
        padding: 10.0,
    }));
    let img = render(&doc);
    assert_eq!(px(&img, 60, 120), WHITE, "inside tail");
    assert_eq!(px(&img, 150, 150), [200, 200, 200, 255], "outside");
    // No seam between bubble and tail (tail base is spliced into the outline).
    assert_eq!(px(&img, 60, 100), WHITE);
}

#[test]
fn opacity_is_group_opacity() {
    let mut doc = plain_doc(60, 60, WHITE);
    doc.annotations.push(
        Annotation::new(AnnotationKind::Rect(RectAnnotation { rect: Rect::new(10.0, 10.0, 40.0, 40.0), style: shape(FillMode::OutlineAndFill, Color::BLACK), corner_radius: 0.0 }))
            .with_opacity(0.5),
    );
    let img = render(&doc);
    // Fill and stroke overlap at the edge: group opacity → same value.
    let edge = px(&img, 10, 30);
    let inner = px(&img, 30, 30);
    assert!(close(inner, [128, 128, 128, 255], 2), "{inner:?}");
    assert!(close(edge, inner, 2), "{edge:?} vs {inner:?}");
}

#[test]
fn annotation_shadow_is_drawn() {
    let mut doc = plain_doc(100, 100, WHITE);
    doc.annotations.push(
        Annotation::new(AnnotationKind::Rect(RectAnnotation { rect: Rect::new(20.0, 20.0, 40.0, 40.0), style: shape(FillMode::Filled, Color::BLUE), corner_radius: 0.0 }))
            .with_shadow(Some(Shadow { color: Color::BLACK.with_alpha(0.8), blur: 4.0, offset_x: 6.0, offset_y: 6.0 })),
    );
    let img = render(&doc);
    let s = px(&img, 63, 63);
    assert!(s[0] < 200, "shadow below-right {s:?}");
    assert_eq!(px(&img, 10, 10), WHITE);
}

#[test]
fn hidden_and_excluded_are_skipped() {
    let mut doc = plain_doc(40, 40, WHITE);
    let id = add(&mut doc, AnnotationKind::Rect(RectAnnotation { rect: Rect::new(0.0, 0.0, 40.0, 40.0), style: shape(FillMode::Filled, Color::BLACK), corner_radius: 0.0 }));
    let mut r = Renderer::new();
    let opts = RenderOptions { exclude: vec![id], ..Default::default() };
    assert_eq!(px(&r.render(&doc, &opts).unwrap().image, 20, 20), WHITE);
    doc.annotations[0].hidden = true;
    assert_eq!(px(&r.render(&doc, &RenderOptions::default()).unwrap().image, 20, 20), WHITE);
}

#[test]
fn dashed_line_has_gaps() {
    let mut doc = plain_doc(200, 20, WHITE);
    add(&mut doc, AnnotationKind::Line(LineAnnotation {
        start: Point::new(10.0, 10.0),
        end: Point::new(190.0, 10.0),
        stroke: Stroke { dash: DashStyle::Dashed, cap: LineCap::Butt, ..Stroke::new(Color::BLACK, 4.0) },
    }));
    let img = render(&doc);
    let row: Vec<bool> = (10..190).map(|x| px(&img, x, 10)[0] < 128).collect();
    let on = row.iter().filter(|b| **b).count();
    assert!(on > 60 && on < 150, "{on}");
}

#[test]
fn frame_shadow_chrome_and_watermark() {
    let mut doc = Document::new(RgbaImage::filled(200, 120, WHITE), DocumentMetadata::default());
    doc.beautify.background = Background::Solid { color: Color::rgb(100, 150, 200) };
    doc.beautify.padding = Padding::uniform(40.0);
    doc.beautify.corner_radius = 10.0;
    doc.beautify.shadow = FrameShadow { enabled: true, size: 20.0, opacity: 0.8, offset_x: 0.0, offset_y: 10.0, color: Color::BLACK };
    doc.beautify.chrome = WindowChrome::MacOs { dark: false, title: Some("T".into()) };
    doc.beautify.watermark = Some(Watermark {
        content: WatermarkContent::Text { text: "WM".into(), style: TextStyle { size: 20.0, color: Color::rgb(255, 0, 255), ..TextStyle::default() } },
        position: Corner::BottomRight,
        opacity: 1.0,
        margin: 4.0,
    });
    let out = Renderer::new().render(&doc, &RenderOptions::default()).unwrap();
    let l = out.layout;
    let img = out.image;
    let fr = l.frame_rect;
    // Shadow darkens just below the frame.
    let below = px(&img, (fr.x + fr.width / 2.0) as u32, (fr.bottom() + 6.0) as u32);
    assert!(below[2] < 170, "{below:?}");
    // Background far away unchanged.
    assert_eq!(px(&img, 2, 2), [100, 150, 200, 255]);
    // Red traffic light at 12pt + 6pt from the left, centered vertically.
    let cr = l.chrome_rect.unwrap();
    assert_eq!(cr.height, 28.0);
    let tl = px(&img, (cr.x + 18.0) as u32, (cr.y + 14.0) as u32);
    assert!(close(tl, [0xFF, 0x5F, 0x57, 255], 3), "{tl:?}");
    // Watermark magenta somewhere in the bottom-right corner.
    let mut found = false;
    for y in img.height - 40..img.height {
        for x in img.width - 60..img.width {
            let p = px(&img, x, y);
            if p[0] > 200 && p[1] < 80 && p[2] > 200 {
                found = true;
            }
        }
    }
    assert!(found, "watermark");
}

#[test]
fn gradients_and_aspect_ratio() {
    let mut doc = Document::new(RgbaImage::filled(100, 100, WHITE), DocumentMetadata::default());
    doc.beautify.shadow.enabled = false;
    doc.beautify.padding = Padding::uniform(20.0);
    doc.beautify.aspect_ratio = AspectRatio::SixteenNine;
    doc.beautify.background = Background::LinearGradient {
        stops: vec![GradientStop::new(0.0, Color::rgb(255, 0, 0)), GradientStop::new(1.0, Color::rgb(0, 0, 255))],
        angle_deg: 0.0,
    };
    let img = render(&doc);
    assert!(((img.width as f32 / img.height as f32) - 16.0 / 9.0).abs() < 0.02);
    let l = px(&img, 0, 5);
    let r = px(&img, img.width - 1, 5);
    assert!(l[0] > 240 && l[2] < 15, "{l:?}");
    assert!(r[2] > 240 && r[0] < 15, "{r:?}");
    doc.beautify.background = Background::RadialGradient {
        stops: vec![GradientStop::new(0.0, Color::WHITE), GradientStop::new(1.0, Color::BLACK)],
        center: [0.5, 0.5],
        radius: 1.0,
    };
    let img = render(&doc);
    assert!(px(&img, 0, 0)[0] < 20);
    assert!(px(&img, img.width / 2, 5)[0] > px(&img, 1, 5)[0]);
}

#[test]
fn inset_uses_edge_color() {
    let mut doc = Document::new(RgbaImage::filled(50, 50, [30, 60, 90, 255]), DocumentMetadata::default());
    doc.beautify.background = Background::Solid { color: Color::WHITE };
    doc.beautify.shadow.enabled = false;
    doc.beautify.corner_radius = 0.0;
    doc.beautify.inset = Inset { enabled: true, size: 10.0, color: InsetColor::Auto };
    let out = Renderer::new().render(&doc, &RenderOptions::default()).unwrap();
    let ir = out.layout.inset_rect;
    assert_eq!(px(&out.image, ir.x as u32 + 2, ir.y as u32 + 2), [30, 60, 90, 255]);
}

#[test]
fn desktop_wallpaper_blur_and_custom_image() {
    let dir = std::env::temp_dir().join(format!("rs-render-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("checker.png");
    let mut img = image::RgbaImage::new(64, 64);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = if (x / 2 + y / 2) % 2 == 0 { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([0, 0, 0, 255]) };
    }
    img.save(&path).unwrap();
    let mut doc = Document::new(RgbaImage::filled(20, 20, WHITE), DocumentMetadata::default());
    doc.beautify.shadow.enabled = false;
    doc.beautify.padding = Padding::uniform(30.0);
    doc.beautify.background = Background::CustomImage { path: path.clone() };
    let mut r = Renderer::new();
    let sharp = r.render(&doc, &RenderOptions::default()).unwrap().image;
    r.set_desktop_wallpaper(Some(path.clone()));
    doc.beautify.background = Background::DesktopWallpaper { blur: 6.0, brightness: 0.5 };
    let soft = r.render(&doc, &RenderOptions::default()).unwrap().image;
    let var = |im: &RgbaImage| {
        let v: Vec<f32> = (0..20).map(|x| px(im, x, 5)[0] as f32).collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32
    };
    assert!(var(&sharp) > 1000.0, "{}", var(&sharp));
    assert!(var(&soft) < 100.0, "{}", var(&soft));
    assert!(px(&soft, 5, 5)[0] < 90, "brightness 0.5 of ~50% gray");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bounds_and_hit_testing() {
    let arrow = Annotation::new(AnnotationKind::Arrow(ArrowAnnotation {
        start: Point::new(0.0, 0.0),
        end: Point::new(100.0, 0.0),
        control: Some(Point::new(50.0, 80.0)),
        stroke: Stroke::new(Color::RED, 4.0),
        head: ArrowHead::FilledTriangle,
        double_headed: false,
        tapered: false,
        head_scale: 1.0,
    }));
    let b = annotation_bounds(&arrow);
    assert!(b.bottom() > 38.0 && b.bottom() < 50.0, "{b:?}");
    assert!(hit_test(&arrow, Point::new(50.0, 40.0), 1.0), "curve apex");
    assert!(!hit_test(&arrow, Point::new(50.0, 5.0), 1.0), "chord is not the curve");
    let ell = Annotation::new(AnnotationKind::Ellipse(EllipseAnnotation { rect: Rect::new(0.0, 0.0, 100.0, 50.0), style: shape(FillMode::Outline, Color::RED) }));
    assert!(hit_test(&ell, Point::new(50.0, 1.0), 1.0));
    assert!(!hit_test(&ell, Point::new(50.0, 25.0), 1.0));
    let callout = Annotation::new(AnnotationKind::Callout(CalloutAnnotation {
        rect: Rect::new(0.0, 0.0, 100.0, 50.0),
        tail: Point::new(30.0, 100.0),
        corner_radius: 8.0,
        style: ShapeStyle::default(),
        text: String::new(),
        text_style: TextStyle::default(),
        padding: 8.0,
    }));
    assert!(hit_test(&callout, Point::new(30.0, 90.0), 1.0));
    assert!(annotation_bounds(&callout).bottom() >= 100.0);
}

#[test]
fn callout_text_helper_matches_render_box() {
    let c = CalloutAnnotation {
        rect: Rect::new(10.0, 10.0, 200.0, 100.0),
        tail: Point::new(0.0, 0.0),
        corner_radius: 8.0,
        style: ShapeStyle::default(),
        text: "hello".into(),
        text_style: TextStyle { size: 20.0, ..TextStyle::default() },
        padding: 10.0,
    };
    let t = callout_text(&c);
    assert_eq!(t.width, Some(180.0));
    assert_eq!(t.position.x, 20.0);
    let bx = text_box(&t);
    assert!((bx.center().y - c.rect.center().y).abs() < 1.0, "vertically centered");
    let caret = caret_rect(&t, 5);
    assert!(caret.x > t.position.x);
}

#[test]
fn too_large_is_error() {
    let doc = plain_doc(10, 10, WHITE);
    assert!(matches!(Renderer::new().render(&doc, &RenderOptions::preview(2000.0)), Err(RenderError::TooLarge(..))));
    let mut d = doc.clone();
    d.base_image = RgbaImage { width: 10, height: 10, pixels: std::sync::Arc::new(Vec::new()) };
    assert!(matches!(Renderer::new().render(&d, &RenderOptions::default()), Err(RenderError::ImageNotLoaded)));
}

/// Hand-edited documents / prefs with absurd effect sizes must render (fast)
/// instead of panicking, hanging or aborting on a giant allocation.
#[test]
fn absurd_effect_sizes_are_clamped() {
    let mut doc = plain_doc(120, 80, WHITE);
    doc.beautify = BeautifySettings::default();
    doc.beautify.background = Background::Solid { color: Color::BLUE };
    doc.beautify.shadow.enabled = true;
    doc.beautify.shadow.size = 1e9;
    for strength in [1e12_f32, f32::INFINITY, f32::NAN] {
        add(&mut doc, AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(10., 10., 50., 30.), strength, corner_radius: 0. }));
    }
    let shadow = Shadow { color: Color::BLACK, blur: 1e9, offset_x: 0., offset_y: 0. };
    let mut mag = Annotation::new(AnnotationKind::Magnifier(MagnifierAnnotation {
        source_center: Point::new(60., 40.),
        source_radius: 1e6,
        target_center: Point::new(60., 40.),
        zoom: 2.0,
        border: Stroke::new(Color::WHITE, 2.0),
    }));
    mag.shadow = Some(shadow);
    doc.annotations.push(mag);
    let mut rect = Annotation::new(AnnotationKind::Rect(RectAnnotation {
        rect: Rect::new(20., 20., 30., 20.),
        style: shape(FillMode::Filled, Color::RED),
        corner_radius: 0.,
    }));
    rect.shadow = Some(shadow);
    doc.annotations.push(rect);
    let t = std::time::Instant::now();
    let out = Renderer::new().render(&doc, &RenderOptions::default());
    assert!(out.is_ok());
    assert!(t.elapsed() < std::time::Duration::from_secs(20), "took {:?}", t.elapsed());
}

/// White image with a black 10×10 square centered at (50, 50) content px,
/// Retina scale factor, beautify padding on.
fn magnifier_doc(with_spotlight: bool) -> (Document, Point) {
    let (w, h) = (200u32, 100u32);
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let inside = (45..55).contains(&x) && (45..55).contains(&y);
            v.extend_from_slice(if inside { &[0, 0, 0, 255] } else { &WHITE });
        }
    }
    let mut doc = Document::new(RgbaImage::new(w, h, v).unwrap(), DocumentMetadata { scale_factor: 2.0, ..Default::default() });
    doc.beautify = BeautifySettings::default();
    doc.beautify.background = Background::Solid { color: Color::rgb(0, 200, 0) };
    doc.beautify.padding = Padding::uniform(20.0);
    doc.beautify.corner_radius = 0.0;
    doc.beautify.shadow.enabled = false;
    doc.beautify.inset.size = 0.0;
    if with_spotlight {
        add(&mut doc, AnnotationKind::Spotlight(SpotlightAnnotation {
            rect: Rect::new(150.0, 70.0, 20.0, 20.0),
            shape: SpotlightShape::Rect,
            corner_radius: 0.0,
            dim: Color::rgba(0, 0, 0, 140),
        }));
    }
    let target = Point::new(150.0, 40.0);
    add(&mut doc, AnnotationKind::Magnifier(MagnifierAnnotation {
        source_center: Point::new(50.0, 50.0),
        source_radius: 10.0,
        target_center: target,
        zoom: 2.0,
        border: Stroke::new(Color::rgb(255, 0, 255), 1.0),
    }));
    (doc, target)
}

#[test]
fn magnifier_samples_content_with_padding_and_scale_2() {
    for spot in [false, true] {
        let (doc, target) = magnifier_doc(spot);
        let out = Renderer::new().render(&doc, &RenderOptions::preview(2.0)).unwrap();
        assert!(out.layout.content_rect.x > 30.0, "padding applied {:?}", out.layout.content_rect);
        let at = |dx: f32, dy: f32| {
            let q = out.layout.image_to_canvas(Point::new(target.x + dx, target.y + dy));
            px(&out.image, q.x as u32, q.y as u32)
        };
        // Center of the loupe = magnified black square (±5 src → ±10 in the loupe).
        for (dx, dy) in [(0.0, 0.0), (6.0, 0.0), (-6.0, 6.0)] {
            assert!(close(at(dx, dy), [0, 0, 0, 255], 40), "spot={spot} black at {dx},{dy}: {:?}", at(dx, dy));
        }
        // Outside the square but inside the loupe: white content, never the
        // green background or dimmed pixels (spotlight is below the loupe).
        for (dx, dy) in [(14.0, 0.0), (0.0, -14.0), (-12.0, -8.0)] {
            assert!(close(at(dx, dy), WHITE, 12), "spot={spot} white at {dx},{dy}: {:?}", at(dx, dy));
        }
        if spot {
            // The area outside the spotlight (and outside the loupe) is dimmed.
            let q = out.layout.image_to_canvas(Point::new(100.0, 90.0));
            assert!(px(&out.image, q.x as u32, q.y as u32)[0] < 200);
        }
    }
}

#[test]
fn callout_bubble_grows_to_fit_text() {
    let c = CalloutAnnotation {
        rect: Rect::new(20.0, 20.0, 150.0, 30.0),
        tail: Point::new(40.0, 10.0),
        corner_radius: 6.0,
        style: ShapeStyle { mode: FillMode::Filled, stroke: Stroke::new(Color::BLACK, 2.0), fill: Color::rgb(0, 0, 255) },
        text: "Callouts wrap text inside the bubble and it grows".into(),
        text_style: TextStyle { size: 18.0, color: Color::WHITE, ..TextStyle::default() },
        padding: 8.0,
    };
    let b = callout_bubble_rect(&c);
    assert_eq!(b.x, 20.0);
    assert_eq!(b.width, 150.0);
    assert_eq!(b.y, 20.0, "tail above → grows downwards");
    assert!(b.height > 60.0, "{b:?}");
    let t = callout_text(&c);
    let tb = text_box(&t);
    assert!(tb.y >= b.y + 7.9 && tb.bottom() <= b.bottom() - 7.9, "text {tb:?} inside bubble {b:?}");
    let ink = measure_text_annotation(&t);
    assert!(ink.x >= b.x && ink.right() <= b.right() + 1.0, "{ink:?} {b:?}");
    // Tail below → grows upwards.
    let up = CalloutAnnotation { tail: Point::new(40.0, 200.0), ..c.clone() };
    let bu = callout_bubble_rect(&up);
    assert!((bu.bottom() - 50.0).abs() < 1e-3 && bu.y < 20.0, "{bu:?}");
    // Rendered: bubble fill below the original rect bottom (grown area).
    let mut doc = plain_doc(200, 120, WHITE);
    add(&mut doc, AnnotationKind::Callout(c.clone()));
    let img = render(&doc);
    assert!(close(px(&img, 22, (b.bottom() - 3.0) as u32), [0, 0, 255, 255], 30));
    let a = Annotation::new(AnnotationKind::Callout(c));
    assert!(annotation_bounds(&a).bottom() >= b.bottom());
    assert!(hit_test(&a, Point::new(100.0, b.bottom() - 2.0), 0.0));
}

fn find_chunk<'a>(png: &'a [u8], ty: &[u8; 4]) -> Option<&'a [u8]> {
    let mut i = 8;
    while i + 12 <= png.len() {
        let len = u32::from_be_bytes(png[i..i + 4].try_into().ok()?) as usize;
        if &png[i + 4..i + 8] == ty {
            return Some(&png[i + 8..i + 8 + len]);
        }
        i += 12 + len;
    }
    None
}

#[test]
fn png_and_jpeg_carry_density() {
    let img = noise(16, 8);
    let png = encode_with_density(&img, ExportFormat::Png, 2.0).unwrap();
    let phys = find_chunk(&png, b"pHYs").expect("pHYs chunk");
    let ppm = u32::from_be_bytes(phys[0..4].try_into().unwrap());
    assert_eq!(ppm, 5669, "144 dpi");
    assert_eq!(u32::from_be_bytes(phys[4..8].try_into().unwrap()), 5669);
    assert_eq!(phys[8], 1);
    // pHYs precedes IDAT and the file still decodes (CRC valid).
    let p_phys = png.windows(4).position(|w| w == b"pHYs").unwrap();
    let p_idat = png.windows(4).position(|w| w == b"IDAT").unwrap();
    assert!(p_phys < p_idat);
    let dec = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(dec.as_raw(), img.pixels.as_ref());
    // Plain encode: no pHYs; 1× density = 72 dpi (2835 ppm).
    assert!(find_chunk(&encode(&img, ExportFormat::Png).unwrap(), b"pHYs").is_none());
    let p1 = encode_with_density(&img, ExportFormat::Png, 1.0).unwrap();
    assert_eq!(u32::from_be_bytes(find_chunk(&p1, b"pHYs").unwrap()[0..4].try_into().unwrap()), 2835);
    // Re-applying replaces instead of duplicating.
    let twice = png_with_phys(png.clone(), 216.0);
    assert_eq!(twice.windows(4).filter(|w| *w == b"pHYs").count(), 1);
    assert!(image::load_from_memory(&twice).is_ok());

    let jpg = encode_with_density(&img, ExportFormat::Jpeg { quality: 90 }, 2.0).unwrap();
    let j = jpg.windows(5).position(|w| w == b"JFIF\0").expect("JFIF APP0");
    // JFIF\0, version(2), units(1)=1 dpi, Xdensity(2), Ydensity(2).
    assert_eq!(jpg[j + 7], 1);
    assert_eq!(u16::from_be_bytes([jpg[j + 8], jpg[j + 9]]), 144);
    assert_eq!(u16::from_be_bytes([jpg[j + 10], jpg[j + 11]]), 144);

    // export() uses the export scale: X2 → 144 dpi.
    let mut doc = plain_doc(20, 10, WHITE);
    doc.metadata.scale_factor = 2.0;
    let (bytes, _) = export(&mut Renderer::new(), &doc, &ExportSettings { format: ExportFormat::Png, scale: ExportScale::X2, ..Default::default() }).unwrap();
    assert_eq!(u32::from_be_bytes(find_chunk(&bytes, b"pHYs").unwrap()[0..4].try_into().unwrap()), 5669);
}
