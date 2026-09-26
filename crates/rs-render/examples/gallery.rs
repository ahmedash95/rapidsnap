//! Renders sample documents exercising every annotation kind and beautify
//! option to PNGs, and benchmarks a 4K export.
//!
//! `cargo run -p rs-render --example gallery --release [-- OUT_DIR]`

use std::path::PathBuf;
use std::time::Instant;

use rs_core::*;
use rs_render::*;

fn out_dir() -> PathBuf {
    std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("rs-render-gallery"))
}

fn save(img: &RgbaImage, name: &str) {
    let dir = out_dir();
    let _ = std::fs::create_dir_all(&dir);
    let bytes = encode(img, ExportFormat::Png).expect("encode");
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("write");
    println!("wrote {}", p.display());
}

fn text(pos: Point, s: &str, style: TextStyle, width: Option<f32>) -> Annotation {
    Annotation::new(AnnotationKind::Text(TextAnnotation {
        position: pos,
        width,
        content: s.into(),
        style,
        preset: None,
    }))
}

/// A fake "screenshot": light UI with a sidebar, text lines and a card.
fn fake_screenshot(w: u32, h: u32) -> RgbaImage {
    let base = RgbaImage::filled(w, h, [250, 250, 252, 255]);
    let mut doc = Document::new(base, DocumentMetadata { scale_factor: 1.0, ..Default::default() });
    doc.beautify = BeautifySettings::disabled();
    let fw = w as f32;
    let fh = h as f32;
    let s = fw / 1200.0;
    let mut anns = vec![
        Annotation::new(AnnotationKind::Rect(RectAnnotation {
            rect: Rect::new(0.0, 0.0, 220.0 * s, fh),
            style: ShapeStyle { mode: FillMode::Filled, fill: Color::rgb(0xEE, 0xEF, 0xF3), ..Default::default() },
            corner_radius: 0.0,
        })),
        Annotation::new(AnnotationKind::Rect(RectAnnotation {
            rect: Rect::new(260.0 * s, 330.0 * s, 420.0 * s, 200.0 * s),
            style: ShapeStyle { mode: FillMode::Filled, fill: Color::rgb(0x0A, 0x84, 0xFF), ..Default::default() },
            corner_radius: 16.0 * s,
        })),
        Annotation::new(AnnotationKind::Ellipse(EllipseAnnotation {
            rect: Rect::new(760.0 * s, 330.0 * s, 180.0 * s, 180.0 * s),
            style: ShapeStyle { mode: FillMode::Filled, fill: Color::rgb(0xFF, 0x9F, 0x0A), ..Default::default() },
        })),
    ];
    let body = TextStyle { size: 18.0 * s, color: Color::rgb(0x33, 0x33, 0x38), ..TextStyle::default() };
    let head = TextStyle { size: 30.0 * s, weight: FontWeight::BOLD, color: Color::rgb(0x11, 0x11, 0x16), ..TextStyle::default() };
    anns.push(text(Point::new(260.0 * s, 40.0 * s), "Account settings", head, None));
    for (i, line) in [
        "Email: jane.doe@example.com",
        "API key: sk-live-4f9a8b7c6d5e4f3a2b1c",
        "Card: 4242 4242 4242 4242   exp 12/29",
        "Server: 192.168.10.42  (eu-west)",
        "The quick brown fox jumps over the lazy dog 0123456789",
    ]
    .iter()
    .enumerate()
    {
        anns.push(text(Point::new(260.0 * s, (100.0 + i as f32 * 40.0) * s), line, body.clone(), None));
    }
    for (i, item) in ["Home", "Profile", "Billing", "Security", "Help"].iter().enumerate() {
        anns.push(text(Point::new(28.0 * s, (40.0 + i as f32 * 44.0) * s), item, body.clone(), None));
    }
    for i in 0..6 {
        let y = (580.0 + i as f32 * 30.0) * s;
        if y > fh - 20.0 {
            break;
        }
        anns.push(Annotation::new(AnnotationKind::Rect(RectAnnotation {
            rect: Rect::new(260.0 * s, y, (600.0 - (i % 3) as f32 * 120.0) * s, 12.0 * s),
            style: ShapeStyle { mode: FillMode::Filled, fill: Color::rgb(0xD5, 0xD7, 0xDD), ..Default::default() },
            corner_radius: 6.0 * s,
        })));
    }
    doc.annotations = anns;
    let mut r = Renderer::new();
    r.render(&doc, &RenderOptions::default()).expect("render").image
}

fn st(color: Color, width: f32) -> Stroke {
    Stroke::new(color, width)
}

fn shapes_doc() -> Document {
    let mut doc = Document::new(RgbaImage::filled(1200, 900, [255, 255, 255, 255]), DocumentMetadata::default());
    doc.beautify = BeautifySettings::disabled();
    let mut a = Vec::new();
    let sh = |mode, fill| ShapeStyle { mode, stroke: st(Color::RED, 6.0), fill };
    // Row 1: rects
    a.push(AnnotationKind::Rect(RectAnnotation { rect: Rect::new(30.0, 30.0, 160.0, 100.0), style: sh(FillMode::Outline, Color::TRANSPARENT), corner_radius: 0.0 }));
    a.push(AnnotationKind::Rect(RectAnnotation { rect: Rect::new(220.0, 30.0, 160.0, 100.0), style: sh(FillMode::Filled, Color::BLUE), corner_radius: 12.0 }));
    a.push(AnnotationKind::Rect(RectAnnotation { rect: Rect::new(410.0, 30.0, 160.0, 100.0), style: sh(FillMode::OutlineAndFill, Color::YELLOW.with_alpha(0.5)), corner_radius: 24.0 }));
    a.push(AnnotationKind::Rect(RectAnnotation {
        rect: Rect::new(600.0, 30.0, 160.0, 100.0),
        style: ShapeStyle { mode: FillMode::Outline, stroke: Stroke { dash: DashStyle::Dashed, ..st(Color::GREEN, 5.0) }, fill: Color::TRANSPARENT },
        corner_radius: 8.0,
    }));
    a.push(AnnotationKind::Ellipse(EllipseAnnotation { rect: Rect::new(790.0, 30.0, 160.0, 100.0), style: sh(FillMode::Outline, Color::TRANSPARENT) }));
    a.push(AnnotationKind::Ellipse(EllipseAnnotation {
        rect: Rect::new(980.0, 30.0, 190.0, 100.0),
        style: ShapeStyle { mode: FillMode::OutlineAndFill, stroke: Stroke { dash: DashStyle::Dotted, ..st(Color::PURPLE, 6.0) }, fill: Color::PURPLE.with_alpha(0.2) },
    }));
    // Row 2: lines
    for (i, (cap, dash)) in [
        (LineCap::Round, DashStyle::Solid),
        (LineCap::Square, DashStyle::Solid),
        (LineCap::Butt, DashStyle::Dashed),
        (LineCap::Round, DashStyle::Dotted),
    ]
    .iter()
    .enumerate()
    {
        let x = 40.0 + i as f32 * 290.0;
        a.push(AnnotationKind::Line(LineAnnotation {
            start: Point::new(x, 190.0),
            end: Point::new(x + 240.0, 230.0),
            stroke: Stroke { cap: *cap, dash: *dash, ..st(Color::BLACK, 10.0) },
        }));
    }
    // Row 3: arrows (straight)
    let heads = [ArrowHead::FilledTriangle, ArrowHead::Open, ArrowHead::Round, ArrowHead::None];
    for (i, h) in heads.iter().enumerate() {
        let x = 40.0 + i as f32 * 290.0;
        a.push(AnnotationKind::Arrow(ArrowAnnotation {
            start: Point::new(x, 330.0),
            end: Point::new(x + 230.0, 280.0),
            control: None,
            stroke: st(Color::RED, 6.0),
            head: *h,
            double_headed: false,
            tapered: false,
            head_scale: 1.0,
        }));
    }
    // Row 4: double, tapered, curved, curved double tapered, dashed, big head
    let arrow = |s: Point, e: Point, c: Option<Point>, dbl: bool, tap: bool, head: ArrowHead, dash: DashStyle, hs: f32, w: f32| {
        AnnotationKind::Arrow(ArrowAnnotation {
            start: s,
            end: e,
            control: c,
            stroke: Stroke { dash, ..st(Color::BLUE, w) },
            head,
            double_headed: dbl,
            tapered: tap,
            head_scale: hs,
        })
    };
    a.push(arrow(Point::new(40.0, 420.0), Point::new(260.0, 420.0), None, true, false, ArrowHead::FilledTriangle, DashStyle::Solid, 1.0, 5.0));
    a.push(arrow(Point::new(320.0, 460.0), Point::new(540.0, 380.0), None, false, true, ArrowHead::FilledTriangle, DashStyle::Solid, 1.0, 10.0));
    a.push(arrow(Point::new(600.0, 470.0), Point::new(820.0, 470.0), Some(Point::new(710.0, 340.0)), false, false, ArrowHead::FilledTriangle, DashStyle::Solid, 1.0, 6.0));
    a.push(arrow(Point::new(880.0, 470.0), Point::new(1150.0, 380.0), Some(Point::new(900.0, 350.0)), true, true, ArrowHead::FilledTriangle, DashStyle::Solid, 1.0, 8.0));
    a.push(arrow(Point::new(40.0, 560.0), Point::new(260.0, 520.0), Some(Point::new(150.0, 620.0)), false, false, ArrowHead::Open, DashStyle::Dashed, 1.0, 5.0));
    a.push(arrow(Point::new(320.0, 560.0), Point::new(540.0, 540.0), None, false, false, ArrowHead::FilledTriangle, DashStyle::Solid, 2.0, 4.0));
    // Pen + highlighter
    let pen: Vec<Point> = (0..60).map(|i| {
        let t = i as f32 / 59.0;
        Point::new(620.0 + t * 500.0, 560.0 + (t * 12.0).sin() * 40.0 + (i % 3) as f32 * 2.0)
    }).collect();
    a.push(AnnotationKind::Pen(PenAnnotation { points: pen, stroke: st(Color::GREEN, 6.0) }));
    a.push(AnnotationKind::Text(TextAnnotation {
        position: Point::new(40.0, 640.0),
        width: None,
        content: "Highlighted text sits under a multiply marker".into(),
        style: TextStyle { size: 30.0, color: Color::BLACK, ..TextStyle::default() },
        preset: None,
    }));
    a.push(AnnotationKind::Highlighter(HighlighterAnnotation {
        points: vec![Point::new(40.0, 660.0), Point::new(420.0, 660.0)],
        color: Color::YELLOW.with_alpha(0.6),
        width: 34.0,
    }));
    a.push(AnnotationKind::Highlighter(HighlighterAnnotation {
        points: vec![Point::new(300.0, 640.0), Point::new(500.0, 700.0), Point::new(650.0, 650.0)],
        color: Color::GREEN.with_alpha(0.5),
        width: 24.0,
    }));
    // Steps
    let labels = [(1, StepLabel::Number), (2, StepLabel::Number), (12, StepLabel::Number), (1, StepLabel::UpperLetter), (28, StepLabel::UpperLetter), (3, StepLabel::LowerLetter)];
    for (i, (v, l)) in labels.iter().enumerate() {
        a.push(AnnotationKind::Step(StepAnnotation {
            center: Point::new(70.0 + i as f32 * 90.0, 800.0),
            radius: 28.0,
            value: *v,
            label: *l,
            fill: if i % 2 == 0 { Color::RED } else { Color::BLUE },
            text_color: Color::WHITE,
            border: (i == 4).then(|| st(Color::WHITE, 4.0)),
            font_family: FontFamily::System,
            font_weight: FontWeight::BOLD,
        }));
    }
    doc.annotations = a.into_iter().map(Annotation::new).collect();
    doc
}

fn text_doc() -> Document {
    let mut doc = Document::new(RgbaImage::filled(1200, 900, [236, 240, 245, 255]), DocumentMetadata::default());
    doc.beautify = BeautifySettings::disabled();
    let mut a: Vec<Annotation> = Vec::new();
    // Photo-ish busy background patch to show outline legibility.
    a.push(Annotation::new(AnnotationKind::Rect(RectAnnotation {
        rect: Rect::new(0.0, 0.0, 1200.0, 170.0),
        style: ShapeStyle { mode: FillMode::Filled, fill: Color::rgb(0x55, 0x8B, 0xCF), ..Default::default() },
        corner_radius: 0.0,
    })));
    for p in TextPreset::ALL.iter().enumerate() {
        let (i, preset) = p;
        let mut s = preset.style().scaled(1.5);
        if *preset == TextPreset::Title {
            s.size = 60.0;
        }
        a.push(text(Point::new(40.0 + (i % 2) as f32 * 560.0, 30.0 + (i / 2) as f32 * 80.0 + if i >= 2 { 30.0 } else { 0.0 }), &format!("{} preset — Ag", preset.name()), s, None));
    }
    // Alignment with fixed width.
    for (i, al) in [TextAlign::Left, TextAlign::Center, TextAlign::Right].iter().enumerate() {
        let x = 40.0 + i as f32 * 380.0;
        a.push(Annotation::new(AnnotationKind::Rect(RectAnnotation {
            rect: Rect::new(x, 300.0, 340.0, 150.0),
            style: ShapeStyle { mode: FillMode::Outline, stroke: Stroke { dash: DashStyle::Dashed, ..st(Color::GRAY, 1.0) }, fill: Color::TRANSPARENT },
            corner_radius: 0.0,
        })));
        a.push(text(
            Point::new(x, 300.0),
            &format!("{al:?} aligned text that wraps inside a fixed width box.\nSecond paragraph."),
            TextStyle { size: 22.0, align: *al, color: Color::BLACK, ..TextStyle::default() },
            Some(340.0),
        ));
    }
    // Weights / italic / serif / mono / shadow.
    let mut y = 480.0;
    for (label, s) in [
        ("Light 300", TextStyle { weight: FontWeight::LIGHT, ..TextStyle::default() }),
        ("Bold 700 italic", TextStyle { weight: FontWeight::BOLD, italic: true, ..TextStyle::default() }),
        ("Serif family", TextStyle { family: FontFamily::Serif, ..TextStyle::default() }),
        ("Monospace fn main() {}", TextStyle { family: FontFamily::Monospace, ..TextStyle::default() }),
        ("Named: Helvetica Neue", TextStyle { family: FontFamily::Named("Helvetica Neue".into()), ..TextStyle::default() }),
    ] {
        a.push(text(Point::new(40.0, y), label, TextStyle { size: 30.0, color: Color::rgb(0x22, 0x22, 0x22), ..s }, None));
        y += 44.0;
    }
    a.push(text(
        Point::new(40.0, y + 10.0),
        "Drop shadow text",
        TextStyle {
            size: 40.0,
            weight: FontWeight::BOLD,
            color: Color::WHITE,
            shadow: Some(Shadow { color: Color::BLACK.with_alpha(0.7), blur: 8.0, offset_x: 3.0, offset_y: 4.0 }),
            ..TextStyle::default()
        },
        None,
    ));
    a.push(text(Point::new(40.0, y + 70.0), "Emoji & fallback: 😀 日本語 — ✓", TextStyle { size: 30.0, color: Color::BLACK, ..TextStyle::default() }, None));
    // Callouts with tails on each side.
    let callout = |rect: Rect, tail: Point, txt: &str| {
        Annotation::new(AnnotationKind::Callout(CalloutAnnotation {
            rect,
            tail,
            corner_radius: 14.0,
            style: ShapeStyle { mode: FillMode::OutlineAndFill, stroke: st(Color::BLACK, 3.0), fill: Color::WHITE },
            text: txt.into(),
            text_style: TextStyle { size: 20.0, color: Color::BLACK, align: TextAlign::Center, ..TextStyle::default() },
            padding: 12.0,
        }))
        .with_shadow(Some(Shadow::default()))
    };
    a.push(callout(Rect::new(640.0, 480.0, 240.0, 90.0), Point::new(700.0, 640.0), "Tail below: click here to continue"));
    a.push(callout(Rect::new(930.0, 480.0, 220.0, 90.0), Point::new(1180.0, 620.0), "Tail bottom-right corner"));
    a.push(callout(Rect::new(760.0, 700.0, 220.0, 80.0), Point::new(650.0, 760.0), "Tail on the left"));
    a.push(callout(Rect::new(1000.0, 780.0, 180.0, 80.0), Point::new(1060.0, 660.0), "Tail on top"));
    doc.annotations = a;
    doc
}

fn effects_doc(base: RgbaImage) -> Document {
    let mut doc = Document::new(base, DocumentMetadata::default());
    doc.beautify = BeautifySettings::disabled();
    let mut a: Vec<Annotation> = Vec::with_capacity(12);
    a.push(Annotation::new(AnnotationKind::Spotlight(SpotlightAnnotation {
        rect: Rect::new(250.0, 30.0, 330.0, 60.0),
        shape: SpotlightShape::Rect,
        corner_radius: 10.0,
        dim: Color::BLACK.with_alpha(0.5),
    })));
    a.push(Annotation::new(AnnotationKind::Spotlight(SpotlightAnnotation {
        rect: Rect::new(740.0, 310.0, 220.0, 220.0),
        shape: SpotlightShape::Ellipse,
        corner_radius: 0.0,
        dim: Color::BLACK.with_alpha(0.55),
    })));
    a.push(Annotation::new(AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(310.0, 95.0, 300.0, 35.0), strength: 8.0, corner_radius: 6.0 })));
    a.push(Annotation::new(AnnotationKind::Pixelate(PixelateAnnotation { rect: Rect::new(330.0, 135.0, 340.0, 35.0), block_size: 10 })));
    a.push(Annotation::new(AnnotationKind::Redact(RedactAnnotation { rect: Rect::new(305.0, 175.0, 300.0, 34.0), color: Color::BLACK, corner_radius: 0.0 })));
    a.push(Annotation::new(AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(320.0, 215.0, 200.0, 35.0), strength: 20.0, corner_radius: 0.0 })));
    a.push(
        Annotation::new(AnnotationKind::Magnifier(MagnifierAnnotation {
            source_center: Point::new(300.0, 272.0),
            source_radius: 30.0,
            target_center: Point::new(1040.0, 170.0),
            zoom: 3.0,
            border: st(Color::WHITE, 4.0),
        }))
        .with_shadow(Some(Shadow { color: Color::BLACK.with_alpha(0.5), blur: 16.0, offset_x: 0.0, offset_y: 6.0 })),
    );
    // Opacity + shadow on shapes.
    a.push(
        Annotation::new(AnnotationKind::Rect(RectAnnotation {
            rect: Rect::new(280.0, 600.0, 240.0, 120.0),
            style: ShapeStyle { mode: FillMode::OutlineAndFill, stroke: st(Color::RED, 10.0), fill: Color::RED },
            corner_radius: 12.0,
        }))
        .with_opacity(0.5),
    );
    a.push(
        Annotation::new(AnnotationKind::Arrow(ArrowAnnotation {
            start: Point::new(600.0, 760.0),
            end: Point::new(760.0, 560.0),
            control: Some(Point::new(760.0, 740.0)),
            stroke: st(Color::RED, 8.0),
            head: ArrowHead::FilledTriangle,
            double_headed: false,
            tapered: false,
            head_scale: 1.0,
        }))
        .with_shadow(Some(Shadow { color: Color::BLACK.with_alpha(0.5), blur: 10.0, offset_x: 3.0, offset_y: 5.0 })),
    );
    a.push(
        Annotation::new(AnnotationKind::Step(StepAnnotation {
            center: Point::new(860.0, 640.0),
            radius: 26.0,
            value: 1,
            label: StepLabel::Number,
            fill: Color::RED,
            text_color: Color::WHITE,
            border: Some(st(Color::WHITE, 3.0)),
            font_family: FontFamily::System,
            font_weight: FontWeight::BOLD,
        }))
        .with_shadow(Some(Shadow::default())),
    );
    doc.annotations = a;
    doc
}

fn gradient(a: Color, b: Color, angle: f32) -> Background {
    Background::LinearGradient { stops: vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)], angle_deg: angle }
}

fn main() {
    let t0 = Instant::now();
    warm_up_fonts();
    let base = fake_screenshot(1200, 800);
    println!("fake screenshot {:?}", t0.elapsed());
    save(&base, "00-base.png");

    let mut r = Renderer::new();
    save(&r.render(&shapes_doc(), &RenderOptions::default()).expect("shapes").image, "01-shapes.png");
    save(&r.render(&text_doc(), &RenderOptions::default()).expect("text").image, "02-text.png");
    let eff = effects_doc(base.clone());
    save(&r.render(&eff, &RenderOptions::default()).expect("effects").image, "03-effects.png");

    // Beautify variants (retina doc, 1x export so pt ≈ px).
    let meta = DocumentMetadata { scale_factor: 2.0, ..Default::default() };
    let mut doc = Document::new(base.clone(), meta.clone());
    doc.annotations = eff.annotations.clone();
    doc.beautify = BeautifySettings {
        background: gradient(Color::rgb(0xFF, 0x7E, 0x5F), Color::rgb(0x7F, 0x00, 0xFF), 45.0),
        padding: Padding::uniform(48.0),
        inset: Inset { enabled: true, size: 16.0, color: InsetColor::Auto },
        corner_radius: 14.0,
        shadow: FrameShadow { size: 30.0, opacity: 0.5, offset_y: 12.0, ..FrameShadow::default() },
        chrome: WindowChrome::MacOs { dark: false, title: Some("Account settings — RapidSnap".into()) },
        watermark: Some(Watermark {
            content: WatermarkContent::Text {
                text: "@rapidsnap".into(),
                style: TextStyle { size: 14.0, weight: FontWeight::SEMIBOLD, color: Color::WHITE, ..TextStyle::default() },
            },
            position: Corner::BottomRight,
            opacity: 0.8,
            margin: 14.0,
        }),
        ..BeautifySettings::default()
    };
    let x1 = ExportSettings { scale: ExportScale::X1, ..Default::default() };
    let x2 = ExportSettings { scale: ExportScale::X2, ..Default::default() };
    save(&export(&mut r, &doc, &x2).expect("b1").1, "10-gradient-macos-inset-watermark.png");

    doc.beautify.background = Background::RadialGradient {
        stops: vec![GradientStop::new(0.0, Color::rgb(0x8E, 0xC5, 0xFC)), GradientStop::new(1.0, Color::rgb(0x2E, 0x3C, 0xD8))],
        center: [0.5, 0.3],
        radius: 1.0,
    };
    doc.beautify.chrome = WindowChrome::Browser { dark: true, url: "https://example.com/settings".into() };
    doc.beautify.inset.enabled = false;
    doc.beautify.aspect_ratio = AspectRatio::SixteenNine;
    doc.beautify.watermark = Some(Watermark {
        content: WatermarkContent::Image { path: rs_core::assets::asset_path("icons/app-icon-256.png"), width: 40.0 },
        position: Corner::TopLeft,
        opacity: 0.9,
        margin: 16.0,
    });
    save(&export(&mut r, &doc, &x1).expect("b2").1, "11-radial-browser-dark-16x9-imgwatermark.png");

    doc.beautify.background = Background::Wallpaper { name: "wallpaper-03".into() };
    doc.beautify.chrome = WindowChrome::MacOs { dark: true, title: None };
    doc.beautify.aspect_ratio = AspectRatio::Square;
    doc.beautify.auto_balance = true;
    doc.beautify.watermark = None;
    save(&export(&mut r, &doc, &x1).expect("b3").1, "12-wallpaper-square-autobalance.png");

    r.set_desktop_wallpaper(rs_core::assets::wallpaper_path("wallpaper-07"));
    doc.beautify.background = Background::DesktopWallpaper { blur: 20.0, brightness: 0.7 };
    doc.beautify.chrome = WindowChrome::None;
    doc.beautify.aspect_ratio = AspectRatio::Free;
    doc.beautify.auto_balance = false;
    doc.beautify.padding = Padding { top: 30.0, right: 120.0, bottom: 30.0, left: 30.0, linked: false };
    doc.beautify.corner_radius = 30.0;
    save(&export(&mut r, &doc, &x1).expect("b4").1, "13-desktop-blur-perside-padding.png");

    // Crop + rotate (annotations remapped by history in the app; here none).
    let mut rot = Document::new(base.clone(), meta.clone());
    rot.image_transform = ImageTransform { crop: Some(IRect::new(220, 0, 700, 560)), rotation: Rotation::Deg90 };
    rot.beautify.background = gradient(Color::rgb(0x11, 0x99, 0x8E), Color::rgb(0x38, 0xEF, 0x7D), 90.0);
    save(&export(&mut r, &rot, &x1).expect("rot").1, "14-crop-rotate90.png");

    // Preview at reduced scale (same pipeline).
    let prev = r.render(&doc, &RenderOptions::preview(0.3)).expect("preview");
    save(&prev.image, "15-preview-0.3.png");

    bench();
}

fn bench() {
    println!("\n== 4K benchmark (3840x2160, scale_factor 2, X2 export = native) ==");
    let base = fake_screenshot(3840, 2160);
    let mut doc = Document::new(base, DocumentMetadata { scale_factor: 2.0, ..Default::default() });
    doc.beautify = BeautifySettings {
        background: Background::Wallpaper { name: "wallpaper-02".into() },
        inset: Inset { enabled: true, size: 12.0, color: InsetColor::Auto },
        chrome: WindowChrome::MacOs { dark: false, title: Some("Benchmark".into()) },
        ..BeautifySettings::default()
    };
    let s = 3.2; // fake_screenshot layout scale (3840 / 1200)
    let mut anns: Vec<Annotation> = effects_doc(RgbaImage::filled(1, 1, [0; 4])).annotations;
    for a in &mut anns {
        a.map_points(&|p| p * s);
        match &mut a.kind {
            AnnotationKind::Blur(b) => b.strength *= s,
            AnnotationKind::Pixelate(p) => p.block_size = (p.block_size as f32 * s) as u32,
            _ => {}
        }
    }
    anns.push(Annotation::new(AnnotationKind::Blur(BlurAnnotation { rect: Rect::new(0.0, 1400.0, 3840.0, 760.0), strength: 24.0, corner_radius: 0.0 })));
    anns.push(text(Point::new(900.0, 1500.0), "Big outlined title over blur", TextPreset::Title.style().scaled(3.0), None));
    doc.annotations = anns;
    let settings = ExportSettings::default();
    let mut r = Renderer::new();
    let _ = rs_render::layout_text("warm", &TextStyle::default(), None);

    let t = Instant::now();
    let (bytes, img) = export(&mut r, &doc, &settings).expect("export");
    println!("cold export (decode wallpaper, build base, blur, encode): {:?}  ({}x{}, {} KB)", t.elapsed(), img.width, img.height, bytes.len() / 1024);
    let t = Instant::now();
    let out = r.render(&doc, &RenderOptions::for_export(&doc, &settings)).expect("render");
    let render_warm = t.elapsed();
    let t = Instant::now();
    let png = encode(&out.image, ExportFormat::Png).expect("png");
    let enc = t.elapsed();
    println!("warm render: {render_warm:?}; PNG encode: {enc:?} ({} KB)", png.len() / 1024);
    // Annotation edit: move an arrow -> blur cache still valid, base cached.
    let mut d2 = doc.clone();
    if let Some(a) = d2.annotations.iter_mut().find(|a| matches!(a.kind, AnnotationKind::Arrow(_))) {
        a.translate(10.0, 0.0);
    }
    let t = Instant::now();
    let _ = r.render(&d2, &RenderOptions::for_export(&d2, &settings)).expect("render");
    println!("re-render after annotation move (full res): {:?}", t.elapsed());
    // Moving the big blur invalidates only that blur.
    if let Some(a) = d2.annotations.iter_mut().rev().find(|a| matches!(a.kind, AnnotationKind::Blur(_))) {
        a.translate(0.0, -20.0);
    }
    let t = Instant::now();
    let _ = r.render(&d2, &RenderOptions::for_export(&d2, &settings)).expect("render");
    println!("re-render after moving the 3840x760 blur (full res): {:?}", t.elapsed());
    for scale in [0.5f32, 0.25] {
        let o = RenderOptions::preview(scale);
        let _ = r.render(&doc, &o).expect("warm");
        let t = Instant::now();
        let n = 10;
        for i in 0..n {
            let mut d3 = doc.clone();
            if let Some(a) = d3.annotations.iter_mut().find(|a| matches!(a.kind, AnnotationKind::Arrow(_))) {
                a.translate(i as f32, 0.0);
            }
            let _ = r.render(&d3, &o).expect("preview");
        }
        println!("preview @{scale} (annotation drag, cached base): {:?}/frame", t.elapsed() / n);
    }
    let t = Instant::now();
    let mut d4 = doc.clone();
    d4.beautify.padding = Padding::uniform(60.0);
    let _ = r.render(&d4, &RenderOptions::preview(0.5)).expect("preview");
    println!("preview @0.5 after padding change (base rebuild): {:?}", t.elapsed());
    for f in [ExportFormat::Jpeg { quality: 90 }, ExportFormat::WebP] {
        let t = Instant::now();
        let b = encode(&out.image, f).expect("enc");
        println!("{f:?} encode: {:?} ({} KB)", t.elapsed(), b.len() / 1024);
    }
}
