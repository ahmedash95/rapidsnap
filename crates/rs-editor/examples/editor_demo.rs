//! Editor demo: opens the editor on a bundled wallpaper (downscaled, treated
//! as a Retina capture) pre-populated with one annotation of every kind.
//!
//! ```sh
//! cargo run -p rs-editor --example editor_demo
//! RAPIDSNAP_EDITOR_SELFTEST=1 RAPIDSNAP_EDITOR_SELFTEST_DIR=/tmp/x cargo run -p rs-editor --example editor_demo
//! ```
//! Demo services (fake OCR / detection) are wired so the Copy-text and
//! Auto-redact flows can be exercised without rs-smart.

use std::sync::Arc;

use gpui_kit::App;
use rs_core::*;
use rs_editor::{EditorOptions, EditorServices, OcrTextLine, PreferencesGlobal, SensitiveMatch};

fn base_image() -> RgbaImage {
    let (w, h) = (1440u32, 900u32);
    if let Some(path) = assets::wallpaper_path("wallpaper-03")
        && let Ok(img) = image::open(&path)
    {
        let img = img.resize_to_fill(w, h, image::imageops::FilterType::Triangle).to_rgba8();
        if let Some(r) = RgbaImage::new(img.width(), img.height(), img.into_raw()) {
            return r;
        }
    }
    // Fallback: generated gradient with "window" blocks.
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let inside = (200..1240).contains(&x) && (150..750).contains(&y);
            let (r, g, b) = if inside { (245, 245, 247) } else { ((x * 255 / w) as u8, (y * 255 / h) as u8, 180) };
            px.extend_from_slice(&[r, g, b, 255]);
        }
    }
    RgbaImage::new(w, h, px).unwrap_or_else(|| RgbaImage::filled(w, h, [200, 200, 200, 255]))
}

fn demo_document() -> Document {
    let mut doc = Document::new(
        base_image(),
        DocumentMetadata { scale_factor: 2.0, title: Some("Editor demo".into()), ..Default::default() },
    );
    let sf = doc.metadata.scale_factor;
    let st = |t: ToolKind| ToolStyle::default_for(t).to_content_px(sf);
    let p = Point::new;
    let mut add = |a: Option<Annotation>| {
        if let Some(a) = a {
            doc.annotations.push(a);
        }
    };
    add(st(ToolKind::Rect).create(ToolKind::Rect, p(60.0, 60.0), p(300.0, 200.0)));
    add(st(ToolKind::Ellipse).create(ToolKind::Ellipse, p(340.0, 60.0), p(540.0, 200.0)));
    add(st(ToolKind::Line).create(ToolKind::Line, p(580.0, 80.0), p(780.0, 180.0)));
    add(st(ToolKind::Arrow).create(ToolKind::Arrow, p(820.0, 200.0), p(1020.0, 70.0)));
    let mut curved = st(ToolKind::Arrow);
    curved.arrow_curved = true;
    curved.stroke.color = Color::BLUE;
    add(curved.create(ToolKind::Arrow, p(1060.0, 200.0), p(1360.0, 90.0)));
    if let Some(mut pen) = st(ToolKind::Pen).create(ToolKind::Pen, p(60.0, 300.0), p(60.0, 300.0))
        && let AnnotationKind::Pen(x) = &mut pen.kind
    {
        x.points = (0..40).map(|i| p(60.0 + i as f32 * 6.0, 300.0 + (i as f32 * 0.4).sin() * 30.0)).collect();
        add(Some(pen));
    }
    add(st(ToolKind::Highlighter).create(ToolKind::Highlighter, p(340.0, 300.0), p(600.0, 300.0)));
    for (i, x) in [680.0f32, 740.0, 800.0].into_iter().enumerate() {
        if let Some(mut s) = st(ToolKind::Step).create(ToolKind::Step, p(x, 300.0), p(x, 300.0)) {
            if let AnnotationKind::Step(k) = &mut s.kind {
                k.value = i as u32 + 1;
            }
            add(Some(s));
        }
    }
    if let Some(mut c) = st(ToolKind::Callout).create(ToolKind::Callout, p(880.0, 260.0), p(1180.0, 340.0))
        && let AnnotationKind::Callout(k) = &mut c.kind
    {
        k.text = "Callouts wrap text inside the bubble".into();
        add(Some(c));
    }
    add(st(ToolKind::Spotlight).create(ToolKind::Spotlight, p(60.0, 420.0), p(360.0, 600.0)));
    // Loupe magnifying the step counters (source ring shown), placed over
    // the area dimmed by the spotlight: it must show the undimmed content.
    if let Some(mut m) = st(ToolKind::Magnifier).create(ToolKind::Magnifier, p(740.0, 300.0), p(560.0, 520.0))
        && let AnnotationKind::Magnifier(x) = &mut m.kind
    {
        x.source_radius = 44.0;
        x.zoom = 2.0;
        add(Some(m));
    }
    add(st(ToolKind::Blur).create(ToolKind::Blur, p(760.0, 420.0), p(960.0, 520.0)));
    add(st(ToolKind::Pixelate).create(ToolKind::Pixelate, p(1000.0, 420.0), p(1200.0, 520.0)));
    add(st(ToolKind::Redact).create(ToolKind::Redact, p(1240.0, 420.0), p(1400.0, 470.0)));
    for (i, preset) in TextPreset::ALL.into_iter().enumerate() {
        add(Some(Annotation::new(AnnotationKind::Text(TextAnnotation {
            position: p(80.0 + i as f32 * 330.0, 700.0),
            width: None,
            content: format!("{} text", preset.name()),
            style: preset.style().scaled(sf),
            preset: Some(preset),
        }))));
    }
    add(Some(Annotation::new(AnnotationKind::Text(TextAnnotation {
        position: p(760.0, 580.0),
        width: Some(560.0),
        content: "A fixed-width text box wraps its content automatically when resized.".into(),
        style: TextStyle { size: 28.0, color: Color::WHITE, ..Default::default() },
        preset: None,
    }))));
    doc
}

fn demo_services() -> EditorServices {
    EditorServices {
        recognize_text: Some(Arc::new(|_img: &RgbaImage| {
            Ok(vec![OcrTextLine { text: "(demo OCR) RapidSnap editor".into(), bounds: Rect::new(0.0, 0.0, 10.0, 10.0) }])
        })),
        detect_sensitive: Some(Arc::new(|_img: &RgbaImage| {
            Ok(vec![
                SensitiveMatch { kind: "Email (demo)".into(), text: "someone@example.com".into(), bounds: Rect::new(1100.0, 780.0, 260.0, 40.0) },
                SensitiveMatch { kind: "IP (demo)".into(), text: "10.0.0.1".into(), bounds: Rect::new(1100.0, 830.0, 160.0, 40.0) },
            ])
        })),
        ..Default::default()
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        rs_editor::init(cx);
        cx.set_global(PreferencesGlobal(Preferences { auto_close_on_copy: false, ..Default::default() }));
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let doc = demo_document();
        log::info!("demo document: {} annotations", doc.annotations.len());
        let opts = EditorOptions { services: demo_services(), ..Default::default() };
        if let Err(e) = rs_editor::open_editor(cx, doc, opts) {
            log::error!("open_editor: {e}");
            cx.quit();
        }
    });
}
