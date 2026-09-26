//! rs-capture live probe: permission, enumeration, every capture path,
//! wallpaper, clipboard round-trip, image import. Writes PNGs to
//! `$RS_CAPTURE_PROBE_OUT` (default: `$TMPDIR/rs-capture-probe`).
//!
//! `cargo run -p rs-capture --example probe`
//! `RS_CAPTURE_SKIP_PREFLIGHT=1` exercises the ScreenCaptureKit paths even
//! without permission (checks they fail cleanly instead of hanging).

use std::path::PathBuf;
use std::time::Instant;

use rs_capture::*;
use rs_core::{Point, Rect, RgbaImage};

fn save(dir: &std::path::Path, name: &str, cap: &Captured) {
    let path = dir.join(name);
    match encode_png(&cap.image, cap.scale_factor).and_then(|b| std::fs::write(&path, b).map_err(Into::into)) {
        Ok(()) => println!("    -> {} ({}x{} @{}x)", path.display(), cap.image.width, cap.image.height, cap.scale_factor),
        Err(e) => println!("    !! save failed: {e}"),
    }
}

/// Fraction of pixels that are not near-identical to the median-ish first
/// pixel — a crude "is there content beyond a flat wallpaper" signal.
fn variety(img: &RgbaImage) -> f32 {
    let mut distinct = std::collections::HashSet::new();
    for p in img.pixels.as_chunks::<4>().0.iter().step_by(97) {
        distinct.insert([p[0] >> 3, p[1] >> 3, p[2] >> 3]);
    }
    distinct.len() as f32
}

fn main() {
    let out: PathBuf = std::env::var_os("RS_CAPTURE_PROBE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rs-capture-probe"));
    std::fs::create_dir_all(&out).expect("create output dir");
    println!("output dir: {}", out.display());

    println!("\n== permission");
    println!("  preflight: {}", has_screen_capture_permission());

    println!("\n== displays");
    let t = Instant::now();
    let displays = list_displays();
    println!("  list_displays: {:?}", t.elapsed());
    let displays = match displays {
        Ok(d) => d,
        Err(e) => {
            println!("  ERROR {e}");
            vec![]
        }
    };
    let t = Instant::now();
    let _ = list_displays();
    println!("  list_displays (2nd): {:?}", t.elapsed());
    let t = Instant::now();
    let _ = std::thread::spawn(list_displays).join();
    println!("  list_displays (bg thread): {:?}", t.elapsed());
    for d in &displays {
        println!("  {:>10} {:<24} frame={:?} scale={} main={}", d.id, d.name, d.frame, d.scale_factor, d.is_main);
    }

    println!("\n== windows");
    let t = Instant::now();
    let windows = list_windows().unwrap_or_else(|e| {
        println!("  ERROR {e}");
        vec![]
    });
    println!("  list_windows: {:?}, {} windows (front-to-back)", t.elapsed(), windows.len());
    for w in windows.iter().take(15) {
        println!(
            "  id={:<6} pid={:<6} layer={} disp={:?} {:?} app={:?} title={:?}",
            w.id, w.owner_pid, w.layer, w.display_id, w.frame, w.app_name, w.title
        );
    }
    if let Some(main) = displays.first() {
        let c = main.frame.center();
        println!("  window_at_point(center of main) = {:?}", window_at_point(&windows, c).map(|w| (&w.app_name, w.id)));
    }

    println!("\n== wallpaper (main thread)");
    let t = Instant::now();
    match desktop_wallpaper_path(None) {
        Ok(p) => {
            println!("  {:?} ({:?})", p, t.elapsed());
            let t = Instant::now();
            let _ = desktop_wallpaper_path(None);
            println!("  2nd call (cached transcode): {:?}", t.elapsed());
            match load_image_file(&p) {
                Ok(c) => println!("  decoded {}x{}", c.image.width, c.image.height),
                Err(e) => println!("  decode ERROR {e}"),
            }
        }
        Err(e) => println!("  ERROR {e} ({:?})", t.elapsed()),
    }
    // Off-main-thread without a running main loop must time out cleanly.
    let t = Instant::now();
    let r = std::thread::spawn(|| desktop_wallpaper_path(None)).join().expect("thread");
    println!("  from background thread (no run loop): {:?} after {:?}", r.map(|_| "ok"), t.elapsed());

    println!("\n== captures");
    let t = Instant::now();
    println!("  prepare_capture: {:?} ({:?})", prepare_capture(), t.elapsed());

    let t = Instant::now();
    match capture_all_displays() {
        Ok(v) => {
            println!("  capture_all_displays: {:?}", t.elapsed());
            for (d, c) in &v {
                println!("    display {} variety={}", d.id, variety(&c.image));
                save(&out, &format!("all-{}.png", d.id), c);
            }
            if let Some((d, _)) = v.first() {
                let r = Rect::new(d.frame.x + 100.0, d.frame.y + 100.0, 400.0, 250.0);
                if let Ok(c) = crop_region_from_displays(&v, r) {
                    save(&out, "freeze-crop.png", &c);
                }
                println!("    sample_pixel(100,100) = {:?}", sample_pixel(&v, Point::new(d.frame.x + 100.0, d.frame.y + 100.0)));
            }
            // Second call measures the warm path.
            let t = Instant::now();
            let _ = capture_all_displays();
            println!("  capture_all_displays (warm): {:?}", t.elapsed());
        }
        Err(e) => println!("  capture_all_displays ERROR {e} ({:?})", t.elapsed()),
    }

    if let Some(d) = displays.first() {
        let t = Instant::now();
        match capture_display(d.id) {
            Ok(c) => {
                println!("  capture_display({}): {:?}", d.id, t.elapsed());
                save(&out, "display.png", &c);
            }
            Err(e) => println!("  capture_display ERROR {e} ({:?})", t.elapsed()),
        }
        let r = Rect::new(d.frame.x + 50.3, d.frame.y + 40.7, 300.2, 200.0);
        let t = Instant::now();
        match capture_region(r, None) {
            Ok(c) => {
                println!("  capture_region: {:?} frame={:?}", t.elapsed(), c.frame);
                save(&out, "region.png", &c);
            }
            Err(e) => println!("  capture_region ERROR {e} ({:?})", t.elapsed()),
        }
        if displays.len() > 1 {
            // Region straddling the first two displays' shared edge, if any.
            let a = &displays[0].frame;
            let b = &displays[1].frame;
            let u = a.union(b);
            let r = Rect::new(u.center().x - 200.0, u.center().y - 100.0, 400.0, 200.0);
            match capture_region(r, None) {
                Ok(c) => save(&out, "region-span.png", &c),
                Err(e) => println!("  spanning capture_region ERROR {e}"),
            }
        }
    }

    if let Some(w) = windows.first() {
        for (name, opts) in [
            ("window-opaque.png", WindowCaptureOptions::default()),
            ("window-alpha.png", WindowCaptureOptions { include_shadow: false, transparent_background: true }),
            ("window-shadow.png", WindowCaptureOptions { include_shadow: true, transparent_background: true }),
        ] {
            let t = Instant::now();
            match capture_window(w.id, opts) {
                Ok(c) => {
                    println!("  capture_window({}, {name}): {:?} frame={:?} src={:?}", w.id, t.elapsed(), c.frame, c.source);
                    save(&out, name, &c);
                }
                Err(e) => println!("  capture_window {name} ERROR {e} ({:?})", t.elapsed()),
            }
        }
    }
    println!("  capture_window(bogus) = {:?}", capture_window(u32::MAX - 1, Default::default()).map(|_| ()));

    println!("\n== clipboard round-trip");
    let mut px = Vec::new();
    for y in 0..40u32 {
        for x in 0..60u32 {
            px.extend_from_slice(&[(x * 4) as u8, (y * 6) as u8, 128, if x < 5 { 100 } else { 255 }]);
        }
    }
    let img = RgbaImage::new(60, 40, px).expect("img");
    let t = Instant::now();
    match copy_image_to_clipboard_with_scale(&img, 2.0) {
        Ok(()) => println!("  write ok ({:?})", t.elapsed()),
        Err(e) => println!("  write ERROR {e}"),
    }
    match clipboard_image() {
        Ok(c) => println!(
            "  read back {}x{} scale={} identical={}",
            c.image.width,
            c.image.height,
            c.scale_factor,
            c.image.pixels == img.pixels
        ),
        Err(e) => println!("  read ERROR {e}"),
    }
    let types = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", "clipboard info"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    println!("  pasteboard types (osascript): {types}");
    let file = out.join("clip-file.png");
    let _ = std::fs::write(&file, encode_png(&img, 1.0).unwrap_or_default());
    println!("  copy_file_to_clipboard: {:?}", copy_file_to_clipboard(&file));
    println!("  clipboard_image from file url: {:?}", clipboard_image().map(|c| (c.image.width, c.source)));
    println!("  copy_text_to_clipboard: {:?}", copy_text_to_clipboard("rs-capture probe"));
    println!("  clipboard_image with text only: {:?}", clipboard_image().map(|_| ()));

    println!("\n== import");
    for p in ["/System/Library/Desktop Pictures/Mac Blue.heic", "/System/Library/CoreServices/Finder.app/Contents/Resources/Finder.icns"] {
        let t = Instant::now();
        match load_image_file(std::path::Path::new(p)) {
            Ok(c) => println!("  {p}: {}x{} scale={} ({:?})", c.image.width, c.image.height, c.scale_factor, t.elapsed()),
            Err(e) => println!("  {p}: ERROR {e}"),
        }
    }
    println!("  garbage bytes: {:?}", load_image_bytes(b"not an image").map(|_| ()));
}
