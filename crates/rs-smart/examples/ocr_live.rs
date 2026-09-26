//! Live OCR + auto-redact check (macOS Vision).
//!
//! `cargo run -p rs-smart --example ocr_live -- [out_dir]`
//! Renders a test image with cosmic-text, runs OCR + detection, draws word
//! boxes (green) and detections (red) and writes PNGs to `out_dir`.

use std::path::PathBuf;
use std::time::Instant;

use cosmic_text::{Attrs, Buffer, Color as CtColor, Family, FontSystem, Metrics, Shaping, SwashCache};
use rs_core::{Rect, RgbaImage};
use rs_smart::ocr::{self, OcrOptions};
use rs_smart::redact::{self, SensitiveKind};

const LINES: &[(&str, bool)] = &[
    ("Deploy notes - please do not share", false),
    ("Contact: jane.doe@example.com for access", false),
    ("Server IP 192.168.10.42 (staging)", false),
    ("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE", true),
    ("Card on file: 4242 4242 4242 4242 exp 12/34", false),
    ("Support hotline +49 30 12345678", false),
    ("Build 2026-09-25 finished in 2.35s", false),
];

fn render() -> image::RgbaImage {
    let (w, h) = (1400u32, 80 + LINES.len() as u32 * 70);
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    let mut fs = FontSystem::new();
    let mut cache = SwashCache::new();
    for (i, (text, mono)) in LINES.iter().enumerate() {
        let mut buf = Buffer::new(&mut fs, Metrics::new(34.0, 44.0));
        let mut b = buf.borrow_with(&mut fs);
        b.set_size(Some(w as f32 - 80.0), Some(60.0));
        let attrs = if *mono { Attrs::new().family(Family::Monospace) } else { Attrs::new() };
        b.set_text(text, &attrs, Shaping::Advanced, None);
        let oy = 40 + i as i32 * 70;
        b.draw(&mut cache, CtColor::rgb(20, 20, 30), |x, y, gw, gh, c| {
            let a = c.a() as u32;
            for dy in 0..gh as i32 {
                for dx in 0..gw as i32 {
                    let (px, py) = (40 + x + dx, oy + y + dy);
                    if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 || a == 0 {
                        continue;
                    }
                    let p = img.get_pixel_mut(px as u32, py as u32);
                    for k in 0..3 {
                        let src = [c.r(), c.g(), c.b()][k] as u32;
                        p.0[k] = ((src * a + p.0[k] as u32 * (255 - a)) / 255) as u8;
                    }
                }
            }
        });
    }
    img
}

fn stroke(img: &mut image::RgbaImage, r: Rect, col: [u8; 4], t: i32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (x0, y0, x1, y1) = (r.x as i32, r.y as i32, r.right() as i32, r.bottom() as i32);
    for y in y0 - t..=y1 + t {
        for x in x0 - t..=x1 + t {
            let edge = (x - x0).abs() < t || (x - x1).abs() < t || (y - y0).abs() < t || (y - y1).abs() < t;
            if edge && x >= 0 && y >= 0 && x < w && y < h {
                img.put_pixel(x as u32, y as u32, image::Rgba(col));
            }
        }
    }
}

fn main() {
    let out = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("rs-smart-ocr"));
    std::fs::create_dir_all(&out).expect("out dir");
    let img = render();
    img.save(out.join("ocr_input.png")).expect("save");
    let (w, h) = img.dimensions();
    let rs = RgbaImage::new(w, h, img.clone().into_raw()).expect("rgba");

    // Run off the main thread, as the app will.
    let t = Instant::now();
    let rs2 = rs.clone();
    let lines = std::thread::spawn(move || ocr::recognize_text(&rs2, &OcrOptions::for_redaction()))
        .join()
        .expect("thread")
        .expect("ocr");
    println!("OCR: {} lines in {:?}", lines.len(), t.elapsed());
    for l in &lines {
        println!("  [{:.2}] {:?} @ {:?} ({} words)", l.confidence, l.text, l.bounds, l.words.len());
    }
    let dets = redact::detect_sensitive(&lines, &SensitiveKind::ALL);
    println!("Detections:");
    for d in &dets {
        println!("  {:<16} {:<40} masked {:<40} @ {:?}", d.kind.label(), d.text, d.masked_text(), d.bounds);
    }
    let mut vis = img.clone();
    for l in &lines {
        for wd in &l.words {
            stroke(&mut vis, wd.bounds, [0, 180, 0, 255], 1);
        }
    }
    for d in &dets {
        stroke(&mut vis, d.bounds, [230, 0, 0, 255], 3);
    }
    vis.save(out.join("ocr_boxes.png")).expect("save");
    println!("plain text:\n{}", ocr::copy_text_from_image(&rs).expect("copy text"));
    println!("wrote {:?}", out);
}
