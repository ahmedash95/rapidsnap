//! Capture flow glue: hotkey/menu → (delay) → overlay / direct capture →
//! Document → editor, or instant copy. Every rs-capture error ends in a
//! notice (or the welcome/permission window), never a crash.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use rs_capture::{CaptureError, Captured, DisplayInfo, WindowCaptureOptions, WindowInfo};
use rs_core::{CaptureRegion, CaptureSource, Document, Rect};
use rs_editor::PreferencesGlobal;

use crate::hud::{self, Tone};
use crate::overlay::{FrozenDisplay, OverlayMode, OverlayOutcome, OverlaySession, OverlayView};
use crate::platform;

/// Capture modes reachable from hotkeys / menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Region,
    Window,
    Fullscreen,
    RepeatLast,
}

/// The active overlay (at most one).
pub struct ActiveOverlay {
    pub session: Entity<OverlaySession>,
    pub windows: Vec<AnyWindowHandle>,
}
impl Global for ActiveOverlay {}

/// Options that only the self-test uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayOptions {
    /// If the freeze-frame fails, synthesize frames instead of falling back
    /// (lets the self-test run without Screen Recording permission).
    pub synthetic_on_error: bool,
    /// Skip the permission preflight.
    pub skip_permission_check: bool,
}

fn prefs(cx: &App) -> rs_core::Preferences {
    cx.try_global::<PreferencesGlobal>().map(|p| p.0.clone()).unwrap_or_default()
}

/// Entry point for capture events. `delay`: explicit seconds (Delayed
/// Capture menu) or `None` → the preference's default delay.
pub fn start(mode: CaptureMode, delay: Option<u32>, cx: &mut App) {
    if cx.has_global::<ActiveOverlay>() {
        // Pressing a capture hotkey while the overlay is up switches mode.
        let session = cx.global::<ActiveOverlay>().session.clone();
        session.update(cx, |s, cx| {
            let want = if mode == CaptureMode::Window { OverlayMode::Window } else { OverlayMode::Region };
            if s.mode != want {
                s.toggle_mode(cx);
            }
        });
        return;
    }
    // Fonts load while the user drags (editor needs them right after).
    crate::warm_up_for_editor(cx);
    let secs = delay.unwrap_or_else(|| prefs(cx).capture_delay.seconds());
    if secs > 0 {
        hud::countdown(cx, secs, move |cx| run(mode, cx));
    } else {
        run(mode, cx);
    }
}

fn run(mode: CaptureMode, cx: &mut App) {
    match mode {
        CaptureMode::Region => open_overlay(OverlayMode::Region, OverlayOptions::default(), cx),
        CaptureMode::Window => open_overlay(OverlayMode::Window, OverlayOptions::default(), cx),
        CaptureMode::Fullscreen => capture_fullscreen(cx),
        CaptureMode::RepeatLast => repeat_last(cx),
    }
}

/// Screen Recording preflight. Missing → welcome window + notice.
fn ensure_permission(cx: &mut App) -> bool {
    if rs_capture::has_screen_capture_permission() {
        return true;
    }
    log::warn!("Screen Recording permission missing");
    hud::show(
        cx,
        "Screen Recording permission needed",
        Some("Grant it in System Settings, then relaunch RapidSnap.".into()),
        Tone::Error,
        Duration::from_secs(3),
    );
    crate::welcome::open_welcome(cx);
    false
}

/// Handle a capture error: permission → welcome, cancel → nothing, other →
/// notice.
pub fn report_error(what: &str, e: &CaptureError, cx: &mut App) {
    match e {
        CaptureError::Cancelled => {}
        CaptureError::PermissionDenied => {
            hud::show(cx, "Screen Recording permission needed", None, Tone::Error, Duration::from_secs(3));
            crate::welcome::open_welcome(cx);
        }
        e => {
            log::warn!("{what} failed: {e}");
            hud::show(cx, format!("{what} failed"), Some(e.to_string()), Tone::Error, Duration::from_secs(3));
        }
    }
}

fn displays() -> Vec<DisplayInfo> {
    let d = platform::display_list();
    if !d.is_empty() {
        return d;
    }
    rs_capture::list_displays().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

/// Freeze all displays and show the selection overlay.
pub fn open_overlay(mode: OverlayMode, opts: OverlayOptions, cx: &mut App) {
    if cx.has_global::<ActiveOverlay>() {
        return;
    }
    if !opts.skip_permission_check && !ensure_permission(cx) {
        return;
    }
    let t0 = Instant::now();
    let infos = displays();
    if infos.is_empty() {
        hud::show(cx, "No displays found", None, Tone::Error, Duration::from_secs(2));
        return;
    }
    let cursor = platform::cursor_location();
    let frozen: Vec<FrozenDisplay> =
        infos.iter().cloned().map(|info| FrozenDisplay { info, capture: None, image: None }).collect();
    let session = cx.new(|_| {
        OverlaySession::new(frozen, mode, cursor, Box::new(on_overlay_done))
    });

    // Freeze-frame + window list in the background, concurrently with the
    // window creation below (overlay windows stay fully transparent until
    // the frames arrive, so they can't end up in the capture).
    let freeze = cx.background_executor().spawn(async move {
        let t = Instant::now();
        let r = rs_capture::capture_all_displays().map(|v| {
            v.into_iter()
                .filter_map(|(info, cap)| {
                    let img = crate::overlay::to_render_image(&cap.image)?;
                    Some((info, cap, img))
                })
                .collect::<Vec<_>>()
        });
        (r, t.elapsed())
    });
    let window_list = cx.background_executor().spawn(async move {
        match rs_capture::list_windows() {
            Ok(w) if !w.is_empty() => w,
            Ok(_) => platform::capturable_windows(),
            Err(e) => {
                log::debug!("list_windows: {e} (using CG fallback)");
                platform::capturable_windows()
            }
        }
    });

    let key_ix = infos.iter().position(|d| d.frame.contains(cursor)).unwrap_or(0);
    let mut windows = Vec::new();
    for (ix, info) in infos.iter().enumerate() {
        let s = session.clone();
        let res = cx.open_window(crate::overlay::overlay_window_options(info), move |window, cx| {
            if let Err(e) = platform::make_overlay_window(window, ix == key_ix) {
                log::warn!("overlay native tweaks failed: {e:#}");
            }
            cx.new(|cx| OverlayView::new(s, ix, window, cx))
        });
        match res {
            Ok(h) => {
                crate::hud::mark_transient(cx, h.window_id());
                windows.push(h.into());
            }
            Err(e) => log::error!("open overlay on display {}: {e:#}", info.id),
        }
    }
    if windows.is_empty() {
        hud::show(cx, "Could not open the capture overlay", None, Tone::Error, Duration::from_secs(3));
        return;
    }
    log::info!("overlay: {} window(s) opened in {:?}", windows.len(), t0.elapsed());
    cx.set_global(ActiveOverlay { session: session.clone(), windows });

    let weak = session.downgrade();
    cx.spawn(async move |cx| {
        let list = window_list.await;
        let _ = weak.update(cx, |s, cx| s.set_windows(list, cx));
        let (result, took) = freeze.await;
        log::info!("overlay: freeze-frame {} in {took:?} (hotkey→frame {:?})", if result.is_ok() { "ok" } else { "failed" }, t0.elapsed());
        cx.update(|cx| {
            if weak.upgrade().is_none_or(|s| s.read(cx).is_done()) {
                return;
            }
            let result = match result {
                Ok(f) if !f.is_empty() => Ok(f),
                Ok(_) => Err(CaptureError::NotFound),
                Err(e) => Err(e),
            };
            match result {
                Ok(frames) => {
                    let _ = weak.update(cx, |s, cx| s.set_frames(frames, cx));
                }
                Err(e) if opts.synthetic_on_error => {
                    log::warn!("overlay: no freeze-frame ({e}); using synthetic frames (self-test)");
                    let infos: Vec<DisplayInfo> =
                        weak.upgrade().map(|s| s.read(cx).displays.iter().map(|d| d.info.clone()).collect()).unwrap_or_default();
                    let frames = infos.into_iter().filter_map(synthetic_frame).collect();
                    let _ = weak.update(cx, |s, cx| s.set_frames(frames, cx));
                }
                Err(CaptureError::Cancelled) => {
                    let _ = weak.update(cx, |s, cx| s.cancel(cx));
                }
                Err(CaptureError::PermissionDenied) => {
                    let _ = weak.update(cx, |s, cx| s.cancel(cx));
                    report_error("Capture", &CaptureError::PermissionDenied, cx);
                }
                Err(e) => {
                    // Fast path unavailable: fall back to the system UI.
                    log::warn!("overlay: freeze-frame failed ({e}); falling back to screencapture -i");
                    let window_mode = weak.upgrade().is_some_and(|s| s.read(cx).mode == OverlayMode::Window);
                    let _ = weak.update(cx, |s, cx| s.cancel(cx));
                    system_capture_fallback(window_mode, cx);
                }
            }
        });
    })
    .detach();
}

/// A fake frame (gradient + grid) for the self-test.
fn synthetic_frame(info: DisplayInfo) -> Option<(DisplayInfo, Captured, Arc<RenderImage>)> {
    let s = info.scale_factor.max(1.0);
    let (w, h) = ((info.frame.width * s) as u32, (info.frame.height * s) as u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let grid = x % 100 == 0 || y % 100 == 0;
            px[i] = if grid { 255 } else { (x * 255 / w.max(1)) as u8 };
            px[i + 1] = if grid { 255 } else { (y * 255 / h.max(1)) as u8 };
            px[i + 2] = 160;
            px[i + 3] = 255;
        }
    }
    let image = rs_core::RgbaImage::new(w, h, px)?;
    let render = crate::overlay::to_render_image(&image)?;
    let cap = Captured {
        image,
        scale_factor: s,
        source: CaptureSource::Fullscreen { display_id: Some(info.id) },
        frame: Some(info.frame),
    };
    Some((info, cap, render))
}

fn close_overlay(cx: &mut App) -> Option<Entity<OverlaySession>> {
    let active = cx.remove_global::<ActiveOverlay>();
    for w in &active.windows {
        let id = w.window_id();
        let _ = w.update(cx, |_, window, _| window.remove_window());
        crate::hud::unmark_transient(cx, id);
    }
    Some(active.session)
}

fn on_overlay_done(outcome: OverlayOutcome, cx: &mut App) {
    if !cx.has_global::<ActiveOverlay>() {
        return;
    }
    let Some(session) = close_overlay(cx) else { return };
    let displays: Vec<FrozenDisplay> = session.update(cx, |s, _| std::mem::take(&mut s.displays));
    match outcome {
        OverlayOutcome::Cancelled => {
            if crate::hud::user_window_count(cx) == 0 {
                platform::deactivate_app();
            }
        }
        OverlayOutcome::Region { rect, display } => {
            let Some(main_id) = displays.get(display).map(|d| d.info.id) else { return };
            remember_region(rect, main_id, cx);
            let spans = displays.iter().filter(|d| d.info.frame.intersection(&rect).is_some()).count() > 1;
            let frozen: Vec<(DisplayInfo, Captured)> =
                displays.into_iter().filter_map(|d| Some((d.info, d.capture?))).collect();
            let covered = frozen.iter().filter(|(i, _)| i.frame.intersection(&rect).is_some()).count();
            if covered == 0 {
                // No freeze-frame: capture now that the overlay is gone.
                capture_async("Region capture", Duration::from_millis(150), cx, move || {
                    rs_capture::capture_region(rect, if spans { None } else { Some(main_id) })
                });
                return;
            }
            let r = if spans {
                rs_capture::crop_region_from_displays(&frozen, rect)
            } else {
                let (info, cap) = frozen.iter().find(|(i, _)| i.id == main_id).unwrap_or(&frozen[0]);
                rs_capture::crop_to_region(cap, info, rect)
            };
            match r {
                Ok(c) => deliver(c, cx),
                Err(e) => report_error("Crop", &e, cx),
            }
        }
        OverlayOutcome::Window { window } => capture_window(window, displays, cx),
    }
}

fn remember_region(rect: Rect, display_id: u32, cx: &mut App) {
    if cx.has_global::<PreferencesGlobal>() {
        cx.update_global::<PreferencesGlobal, _>(|p, _| {
            p.0.last_capture_region = Some(CaptureRegion { display_id: Some(display_id), rect });
        });
    }
}

fn capture_window(window: WindowInfo, displays: Vec<FrozenDisplay>, cx: &mut App) {
    let p = prefs(cx);
    let opts =
        WindowCaptureOptions {
            include_shadow: p.window_capture_shadow,
            transparent_background: p.window_capture_transparent,
        };
    let task = cx.background_executor().spawn({
        let window = window.clone();
        async move { rs_capture::capture_window(window.id, opts) }
    });
    cx.spawn(async move |cx| {
        let r = task.await;
        cx.update(|cx| match r {
            Ok(c) => deliver(c, cx),
            Err(CaptureError::PermissionDenied) => report_error("Window capture", &CaptureError::PermissionDenied, cx),
            Err(e) => {
                // Fallback: crop the window's frame from the freeze-frame.
                log::info!("capture_window failed ({e}); cropping from the freeze-frame");
                let best = displays
                    .iter()
                    .filter(|d| d.capture.is_some())
                    .max_by(|a, b| {
                        let area = |d: &FrozenDisplay| {
                            d.info.frame.intersection(&window.frame).map(|r| r.width * r.height).unwrap_or(0.)
                        };
                        area(a).total_cmp(&area(b))
                    });
                let cropped = best.and_then(|d| {
                    let r = d.info.frame.intersection(&window.frame)?;
                    rs_capture::crop_to_region(d.capture.as_ref()?, &d.info, r).ok()
                });
                match cropped {
                    Some(mut c) => {
                        c.source = CaptureSource::Window {
                            app_name: Some(window.app_name.clone()).filter(|s| !s.is_empty()),
                            title: Some(window.title.clone()).filter(|s| !s.is_empty()),
                        };
                        deliver(c, cx)
                    }
                    None => report_error("Window capture", &e, cx),
                }
            }
        });
    })
    .detach();
}

/// Run a blocking capture in the background after `delay`, then deliver.
fn capture_async(
    what: &'static str,
    delay: Duration,
    cx: &mut App,
    f: impl FnOnce() -> rs_capture::Result<Captured> + Send + 'static,
) {
    cx.spawn(async move |cx| {
        if !delay.is_zero() {
            cx.background_executor().timer(delay).await;
        }
        let r = cx.background_executor().spawn(async move { f() }).await;
        cx.update(|cx| match r {
            Ok(c) => deliver(c, cx),
            Err(e) => report_error(what, &e, cx),
        });
    })
    .detach();
}

fn system_capture_fallback(window_mode: bool, cx: &mut App) {
    capture_async("Capture", Duration::from_millis(100), cx, move || rs_capture::capture_interactive_system(window_mode));
}

// ---------------------------------------------------------------------------
// Direct captures
// ---------------------------------------------------------------------------

fn capture_fullscreen(cx: &mut App) {
    if !ensure_permission(cx) {
        return;
    }
    let cursor = platform::cursor_location();
    let ds = displays();
    let Some(d) = ds.iter().find(|d| d.frame.contains(cursor)).or_else(|| ds.iter().find(|d| d.is_main)).cloned() else {
        hud::show(cx, "No displays found", None, Tone::Error, Duration::from_secs(2));
        return;
    };
    capture_async("Full-screen capture", Duration::ZERO, cx, move || rs_capture::capture_display(d.id));
}

fn repeat_last(cx: &mut App) {
    let Some(region) = prefs(cx).last_capture_region else {
        hud::show(cx, "No previous region", Some("Capture a region first.".into()), Tone::Info, Duration::from_secs(2));
        return;
    };
    if !ensure_permission(cx) {
        return;
    }
    // The stored region comes from preferences.json (may be stale after a
    // display change, or hand-edited): clip it to the current displays so a
    // bogus rect can't turn into a gigantic capture buffer.
    let ds = displays();
    let display_id = region.display_id.filter(|id| ds.iter().any(|d| d.id == *id));
    let bounds: Vec<Rect> = match display_id {
        Some(id) => ds.iter().filter(|d| d.id == id).map(|d| d.frame).collect(),
        None => ds.iter().map(|d| d.frame).collect(),
    };
    let clipped = bounds.iter().filter_map(|f| f.intersection(&region.rect)).reduce(|a, b| a.union(&b));
    let Some(rect) = clipped.filter(|r| r.width >= 1.0 && r.height >= 1.0) else {
        hud::show(cx, "Previous region is off-screen", Some("Capture a region first.".into()), Tone::Info, Duration::from_secs(2));
        return;
    };
    capture_async("Repeat last region", Duration::ZERO, cx, move || rs_capture::capture_region(rect, display_id));
}

// ---------------------------------------------------------------------------
// Result delivery
// ---------------------------------------------------------------------------

/// Captured → Document → editor (or instant copy); adds it to history.
pub fn deliver(cap: Captured, cx: &mut App) {
    let p = prefs(cx);
    let doc = cap.into_document(p.last_beautify.clone());
    log::info!(
        "captured {}×{} px @{}x ({:?})",
        doc.base_image.width,
        doc.base_image.height,
        doc.metadata.scale_factor,
        doc.metadata.source
    );
    add_to_history(doc.clone(), p.history_limit, cx);
    if p.instant_copy {
        instant_copy(doc, p.export.clone(), cx);
    } else {
        crate::open_document(doc, cx);
    }
}

/// Instant mode: render with the current beautify settings → PNG →
/// clipboard, brief notice, no editor.
fn instant_copy(doc: Document, export: rs_core::ExportSettings, cx: &mut App) {
    let task = cx.background_executor().spawn(async move {
        let mut settings = export;
        settings.format = rs_core::ExportFormat::Png;
        let mut renderer = rs_render::Renderer::new();
        // Output px per point → PNG pHYs, so Retina captures paste at point size.
        let (img, scale) = match rs_render::export(&mut renderer, &doc, &settings) {
            Ok((_, img)) => (img, settings.scale.factor()),
            Err(e) => {
                // Renderer trouble → copy the plain capture.
                log::warn!("instant copy: render failed ({e}); copying the raw capture");
                (rs_render::content_image(&doc).map_err(|e| e.to_string())?, doc.metadata.scale_factor)
            }
        };
        rs_capture::copy_image_to_clipboard_with_scale(&img, scale).map_err(|e| e.to_string())
    });
    cx.spawn(async move |cx| {
        let r = task.await;
        cx.update(|cx| match r {
            Ok(()) => {
                hud::show(cx, "Copied to clipboard", None, Tone::Success, Duration::from_millis(1400));
                if crate::hud::user_window_count(cx) == 0 {
                    platform::deactivate_app();
                }
            }
            Err(e) => hud::show(cx, "Copy failed", Some(e), Tone::Error, Duration::from_secs(3)),
        });
    })
    .detach();
}

fn add_to_history(doc: Document, limit: u32, cx: &mut App) {
    if limit == 0 {
        return;
    }
    let index = cx.default_global::<crate::services::ServicesState>().history.clone();
    let task = cx.background_executor().spawn(async move {
        // The store downscales the thumbnail itself.
        let img = doc.base_image.clone();
        match index.add(&doc, &img, limit) {
            Ok(entries) => Some(entries),
            Err(e) => {
                log::warn!("history add: {e}");
                None
            }
        }
    });
    cx.spawn(async move |cx| {
        if let Some(entries) = task.await {
            cx.update(|cx| {
                if let Some(t) = cx.try_global::<crate::tray::TrayState>() {
                    t.set_recent(&entries);
                }
            });
        }
    })
    .detach();
}

/// Refresh the Recent Captures menu from disk.
pub fn refresh_recent(cx: &mut App) {
    let task = cx.background_executor().spawn(async move { rs_smart::history::HistoryStore::open_default().list() });
    cx.spawn(async move |cx| match task.await {
        Ok(entries) => cx.update(|cx| {
            if let Some(t) = cx.try_global::<crate::tray::TrayState>() {
                t.set_recent(&entries);
            }
        }),
        Err(e) => log::debug!("history list: {e}"),
    })
    .detach();
}

/// Open a history entry in the editor.
pub fn open_recent(id: uuid::Uuid, cx: &mut App) {
    crate::warm_up_for_editor(cx);
    let task = cx.background_executor().spawn(async move { rs_smart::history::HistoryStore::open_default().load(id) });
    cx.spawn(async move |cx| {
        let r = task.await;
        cx.update(|cx| match r {
            Ok(doc) => crate::open_document(doc, cx),
            Err(e) => {
                hud::show(cx, "Could not open capture", Some(e.to_string()), Tone::Error, Duration::from_secs(3));
                refresh_recent(cx);
            }
        });
    })
    .detach();
}

pub fn clear_history(cx: &mut App) {
    let task = cx.background_executor().spawn(async move { rs_smart::history::HistoryStore::open_default().clear() });
    cx.spawn(async move |cx| {
        if let Err(e) = task.await {
            log::warn!("clear history: {e}");
        }
        cx.update(refresh_recent);
    })
    .detach();
}
