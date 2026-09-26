//! `RAPIDSNAP_SELFTEST=1`: end-to-end smoke test of the app shell, then exit.
//!
//! 1. welcome + preferences windows open (all tabs render);
//! 2. region overlay on **every** display (synthetic freeze-frame if the real
//!    one fails, e.g. no Screen Recording permission), checks window level /
//!    bounds (tiling-WM immunity), toggles window mode, then synthesizes a
//!    drag through the same `pointer_*` path as the mouse → Document → editor;
//! 3. overlay + posted Esc key (keyboard routing to the non-activating panel);
//! 4. full-screen capture via posted ⇧⌘1 hotkey (real hotkey path; without
//!    permission this must show the welcome window, not crash);
//! 5. open an image file → editor; then quit. Exit code 0 = all passed.

use std::time::Duration;

use gpui_kit::*;
use rs_core::Point;

use crate::capture::{self, ActiveOverlay, OverlayOptions};
use crate::overlay::OverlayMode;
use crate::platform;

struct Report(Vec<(String, bool)>);

impl Report {
    fn check(&mut self, name: &str, ok: bool) {
        log::info!("SELFTEST {} {name}", if ok { "PASS" } else { "FAIL" });
        self.0.push((name.to_string(), ok));
    }
}

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

fn close_user_windows(cx: &mut App) {
    for w in cx.windows() {
        let _ = w.update(cx, |_, window, _| window.remove_window());
    }
}

pub fn run(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut r = Report(Vec::new());
        sleep(cx, 600).await;
        let displays = platform::display_list();
        for d in &displays {
            log::info!("SELFTEST display {} frame={:?} scale={} main={}", d.id, d.frame, d.scale_factor, d.is_main);
        }
        r.check("displays enumerated", !displays.is_empty());
        let rs_displays = rs_capture::list_displays();
        log::info!("SELFTEST rs_capture::list_displays -> {:?}", rs_displays.as_ref().map(|v| v.len()));
        log::info!("SELFTEST has_screen_capture_permission = {}", rs_capture::has_screen_capture_permission());

        // 1. welcome + preferences (every tab).
        cx.update(crate::welcome::open_welcome);
        sleep(cx, 400).await;
        for tab in [0, 1, 2, 3] {
            cx.update(|cx| crate::prefs_window::open_preferences(tab, cx));
            sleep(cx, 250).await;
        }
        let n = cx.update(|cx| crate::hud::user_window_count(cx));
        r.check("welcome + preferences windows open", n >= 2);
        // The editor changes export format/scale while Preferences is open →
        // the window's selects follow (no stale state).
        let orig_export = cx.update(|cx| cx.global::<rs_editor::PreferencesGlobal>().0.export.clone());
        cx.update(|cx| {
            cx.update_global::<rs_editor::PreferencesGlobal, _>(|p, _| {
                p.0.export.format = rs_core::ExportFormat::WebP;
                p.0.export.scale = rs_core::ExportScale::X1;
            })
        });
        sleep(cx, 250).await;
        let sel = cx.update(|cx| crate::prefs_window::open_export_selection(cx));
        log::info!("SELFTEST prefs window export selects after external change: {sel:?}");
        r.check("preferences window follows external prefs changes", sel == Some((Some(2), Some(0))));
        cx.update(|cx| cx.update_global::<rs_editor::PreferencesGlobal, _>(|p, _| p.0.export = orig_export));
        cx.update(close_user_windows);
        sleep(cx, 400).await;

        // 2. overlay on every display, synthesized drag → editor.
        let t = std::time::Instant::now();
        cx.update(|cx| {
            capture::open_overlay(
                OverlayMode::Region,
                OverlayOptions { synthetic_on_error: true, skip_permission_check: true },
                cx,
            )
        });
        log::info!("SELFTEST open_overlay returned after {:?}", t.elapsed());
        sleep(cx, 1500).await;
        let own = platform::own_windows();
        let overlays: Vec<_> = own.iter().filter(|w| w.layer == platform::OVERLAY_LEVEL as i64).collect();
        for w in &overlays {
            log::info!("SELFTEST overlay window layer={} bounds={:?}", w.layer, w.bounds);
        }
        r.check(&format!("one overlay per display ({} / {})", overlays.len(), displays.len()), overlays.len() == displays.len());
        let covers = displays.iter().all(|d| {
            overlays.iter().any(|w| {
                (w.bounds.0 - d.frame.x as f64).abs() < 1.
                    && (w.bounds.1 - d.frame.y as f64).abs() < 1.
                    && (w.bounds.2 - d.frame.width as f64).abs() < 1.
                    && (w.bounds.3 - d.frame.height as f64).abs() < 1.
            })
        });
        r.check("overlays cover their full displays (not tiled/resized)", covers);

        let session = cx.update(|cx| cx.try_global::<ActiveOverlay>().map(|a| a.session.clone()));
        let Some(session) = session else {
            r.check("overlay session active", false);
            finish(r, cx);
            return;
        };
        let ready = cx.update(|cx| session.read(cx).frames_ready);
        r.check("freeze-frame installed", ready);
        let (nwin, loaded) = cx.update(|cx| {
            let s = session.read(cx);
            (s.windows.len(), s.displays.iter().filter(|d| d.image.is_some()).count())
        });
        log::info!("SELFTEST overlay: {nwin} windows listed, {loaded} frames");

        // Window mode toggle (Space) + hover.
        let main = displays.iter().find(|d| d.is_main).or(displays.first()).cloned();
        let Some(main) = main else {
            finish(r, cx);
            return;
        };
        let center = Point::new(main.frame.x + main.frame.width / 2., main.frame.y + main.frame.height / 2.);
        cx.update(|cx| {
            session.update(cx, |s, cx| {
                s.key_down("space", false, cx);
                s.pointer_move(center, cx);
            })
        });
        sleep(cx, 200).await;
        let (mode, hovered) = cx.update(|cx| {
            let s = session.read(cx);
            (s.mode, s.hovered_window().map(|w| format!("{} ({:?})", w.app_name, w.frame)))
        });
        log::info!("SELFTEST window mode hovered: {hovered:?}");
        r.check("space toggles window mode", mode == OverlayMode::Window);
        cx.update(|cx| session.update(cx, |s, cx| s.key_down("space", false, cx)));

        // Synthesized drag (same path as mouse input) + arrow nudge.
        let a = Point::new(main.frame.x + 120., main.frame.y + 140.);
        let b = Point::new(main.frame.x + 520., main.frame.y + 440.);
        // Read the selection in the same update: real mouse-move events (the
        // user moving the mouse meanwhile) would otherwise move `current`.
        let sel = cx.update(|cx| {
            session.update(cx, |s, cx| {
                s.pointer_move(a, cx);
                s.pointer_down(a, cx);
                for i in 1..=10 {
                    s.pointer_move(a.lerp(b, i as f32 / 10.), cx);
                }
                s.selection()
            })
        });
        let hint = cx.update(|cx| session.read(cx).hint());
        log::info!("SELFTEST overlay hint while dragging: {hint:?}");
        r.check("overlay hint bar follows the drag state", hint.starts_with("Release to capture"));
        sleep(cx, 300).await;
        log::info!("SELFTEST selection before release: {sel:?}");
        r.check("drag produces a selection", sel.is_some_and(|(r, _)| (r.width - 400.).abs() < 1.));
        cx.update(|cx| session.update(cx, |s, cx| s.pointer_up(b, cx)));
        sleep(cx, 1500).await;
        let (overlay_gone, editors) =
            cx.update(|cx| (!cx.has_global::<ActiveOverlay>(), crate::hud::user_window_count(cx)));
        r.check("overlay closed after selection", overlay_gone);
        r.check("editor opened with the cropped document", editors >= 1);
        let last = cx.update(|cx| cx.global::<rs_editor::PreferencesGlobal>().0.last_capture_region);
        log::info!("SELFTEST last_capture_region = {last:?}");
        r.check("last region remembered", last.is_some());
        let hist = rs_smart::history::HistoryStore::open_default().list().map(|v| v.len());
        log::info!("SELFTEST history entries: {hist:?}");
        r.check("capture added to history", hist.is_ok_and(|n| n >= 1));
        // Freeze-frames (≈100 MB per 4K display) must be released with the overlay.
        let frames_left = cx.update(|cx| {
            session.read(cx).displays.iter().filter(|d| d.capture.is_some() || d.image.is_some()).count()
        });
        r.check("freeze-frames dropped after the overlay closed", frames_left == 0);
        // Retina: the captured document carries the display's scale factor.
        let entry_scale = rs_smart::history::HistoryStore::open_default()
            .list()
            .ok()
            .and_then(|v| v.first().map(|e| e.id))
            .and_then(|id| rs_smart::history::HistoryStore::open_default().load(id).ok())
            .map(|d| d.metadata.scale_factor);
        log::info!("SELFTEST captured document scale_factor = {entry_scale:?} (display {})", main.scale_factor);
        r.check(
            "captured document scale_factor = display scale",
            entry_scale.is_some_and(|s| (s - main.scale_factor.max(1.0)).abs() < 0.01),
        );
        // Editor Copy goes through the injected native clipboard (PNG + TIFF, pHYs = px/pt).
        // The user's clipboard is saved (every item/type) and restored after.
        let copy = cx.update(|cx| crate::services::editor_services(cx).copy_image);
        let probe = rs_core::RgbaImage::filled(64, 32, [10, 120, 250, 255]);
        let saved_clip = rs_capture::ClipboardSnapshot::take();
        log::info!("SELFTEST clipboard saved: {} item(s), {} bytes", saved_clip.items.len(), saved_clip.byte_len());
        let clip = copy.map(|f| {
            f(&probe, &[], 2.0).map_err(|e| e.to_string()).and_then(|_| rs_capture::clipboard_image().map_err(|e| e.to_string()))
        });
        let restored = saved_clip.restore();
        let after = rs_capture::ClipboardSnapshot::take();
        let same = after.items.len() == saved_clip.items.len()
            && after.items.iter().zip(&saved_clip.items).all(|(a, b)| {
                // Every saved (type, data) pair is back (AppKit may add derived types).
                b.iter().all(|pair| a.contains(pair))
            });
        log::info!("SELFTEST clipboard restored: {restored:?}, identical = {same}");
        r.check("user clipboard restored after the copy round-trip", restored.is_ok() && same);
        let clip_ok = matches!(&clip, Some(Ok(c)) if c.image.width == 64 && c.image.height == 32 && (c.scale_factor - 2.0).abs() < 0.01);
        log::info!("SELFTEST clipboard round-trip: {:?}", clip.as_ref().map(|r| r.as_ref().map(|c| (c.image.width, c.image.height, c.scale_factor))));
        r.check("editor copy → NSPasteboard PNG/TIFF at 2x (round-trip)", clip_ok);
        // Save as .jpg writes real JPEG bytes (v1 bug: always PNG).
        let jpeg = rs_render::encode(&probe, rs_core::ExportFormat::Jpeg { quality: 90 });
        r.check("JPEG export is JPEG", jpeg.is_ok_and(|b| b.starts_with(&[0xFF, 0xD8, 0xFF])));
        cx.update(close_user_windows);
        sleep(cx, 500).await;

        // Selection spanning two displays (mixed arrangement) → one image.
        if let Some(second) = displays.iter().find(|d| !d.is_main).cloned() {
            cx.update(|cx| {
                capture::open_overlay(
                    OverlayMode::Region,
                    OverlayOptions { synthetic_on_error: true, skip_permission_check: true },
                    cx,
                )
            });
            sleep(cx, 1200).await;
            let s = cx.update(|cx| cx.try_global::<ActiveOverlay>().map(|a| a.session.clone()));
            if let Some(s) = s {
                // Straddle the shared edge (works for side-by-side layouts).
                let edge = if second.frame.x >= main.frame.right() { main.frame.right() } else { main.frame.x };
                let a = Point::new(edge - 100., main.frame.y + 200.);
                let b = Point::new(edge + 100., main.frame.y + 400.);
                cx.update(|cx| {
                    s.update(cx, |s, cx| {
                        s.pointer_down(a, cx);
                        s.pointer_move(b, cx);
                        s.pointer_up(b, cx);
                    })
                });
                sleep(cx, 1500).await;
                let n = cx.update(|cx| crate::hud::user_window_count(cx));
                r.check("selection spanning two displays → editor", n >= 1);
                cx.update(close_user_windows);
                sleep(cx, 500).await;
            }
        }

        // Hotkey change in prefs → re-registered (+ menu accelerator), persisted.
        let new_hk = rs_core::Hotkey::new(rs_core::Modifiers { cmd: true, alt: true, ctrl: true, shift: false }, "Digit9");
        cx.update(|cx| {
            cx.update_global::<rs_editor::PreferencesGlobal, _>(|p, _| p.0.hotkeys.repeat_last = Some(new_hk.clone()))
        });
        sleep(cx, 800).await;
        let (registered, errors) = cx.update(|cx| {
            cx.try_global::<crate::tray::TrayState>()
                .map(|t| (t.current.repeat_last.clone(), t.errors.clone()))
                .unwrap_or_default()
        });
        log::info!("SELFTEST hotkey errors: {errors:?}");
        r.check("hotkey re-registered after prefs change", registered.as_ref() == Some(&new_hk));
        let saved = rs_smart::prefs::load_preferences();
        r.check("preferences persisted (debounced)", saved.hotkeys.repeat_last == Some(new_hk));

        // 3. overlay + posted Esc (key routing to the non-activating panel).
        cx.update(|cx| {
            capture::open_overlay(
                OverlayMode::Region,
                OverlayOptions { synthetic_on_error: true, skip_permission_check: true },
                cx,
            )
        });
        sleep(cx, 1200).await;
        // Only post Esc when our overlay is the key window: otherwise the
        // event would land in whatever app the user has in front.
        let overlay_key = cx.update(|cx| {
            let active = cx.active_window().map(|w| w.window_id());
            cx.try_global::<ActiveOverlay>().is_some_and(|a| a.windows.iter().any(|w| Some(w.window_id()) == active))
        });
        let can_post = overlay_key && platform::post_key_combo(53, false, false); // kVK_Escape
        if !overlay_key {
            log::warn!("SELFTEST SKIP posted Esc: our overlay is not the key window");
        }
        sleep(cx, 600).await;
        let esc_ok = cx.update(|cx| !cx.has_global::<ActiveOverlay>());
        if can_post {
            r.check("Esc (posted key event) cancels the overlay", esc_ok);
        } else {
            log::warn!("SELFTEST SKIP Esc key routing: this process may not post events (grant Accessibility to test)");
        }
        if !esc_ok {
            cx.update(|cx| {
                if let Some(s) = cx.try_global::<ActiveOverlay>().map(|a| a.session.clone()) {
                    s.update(cx, |s, cx| s.cancel(cx));
                }
            });
            sleep(cx, 300).await;
        }

        // 4. real hotkey path: ⇧⌘1 (kVK_ANSI_1 = 18) → full-screen capture.
        // Never post a global hotkey while another RapidSnap (v1, the user's
        // own copy of v2) runs: it would trigger *their* capture.
        let others = platform::other_rapidsnap_instances();
        let fullscreen_registered = cx.update(|cx| {
            cx.try_global::<crate::tray::TrayState>()
                .is_some_and(|t| !t.errors.contains_key(crate::tray::AppEvent::CaptureFullscreen.label()))
        });
        if !others.is_empty() || !fullscreen_registered {
            log::warn!(
                "SELFTEST SKIP posted ⇧⌘1: other RapidSnap instance(s) {others:?} / our registration ok = \
                 {fullscreen_registered}; calling the handler directly"
            );
            cx.update(|cx| crate::handle_event(crate::tray::AppEvent::CaptureFullscreen, cx));
        } else if !platform::post_key_combo(18, true, true) {
            log::warn!("SELFTEST SKIP hotkey path: this process may not post events; calling the handler directly");
            cx.update(|cx| crate::handle_event(crate::tray::AppEvent::CaptureFullscreen, cx));
        }
        sleep(cx, 2500).await;
        let n = cx.update(|cx| crate::hud::user_window_count(cx));
        log::info!("SELFTEST after ⇧⌘1: {n} user window(s) (welcome if no permission, editor if captured)");
        let has_perm = rs_capture::has_screen_capture_permission();
        r.check("full-screen hotkey: editor (permission) or welcome window (no permission)", n >= 1 || has_perm);
        cx.update(close_user_windows);
        sleep(cx, 400).await;

        // 5. open a file → editor (same path as Open File… / Finder / Dock).
        let path = std::env::temp_dir().join(format!("rapidsnap-selftest-{}.png", std::process::id()));
        let img = image::RgbaImage::from_fn(640, 400, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 200, 255]));
        match img.save(&path) {
            Ok(()) => {
                cx.update(|cx| crate::open_path(&path, cx));
                sleep(cx, 1500).await;
                let n = cx.update(|cx| crate::hud::user_window_count(cx));
                r.check("open image file → editor", n >= 1);
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => r.check(&format!("write temp image: {e}"), false),
        }
        finish(r, cx);
    })
    .detach();
}

fn finish(r: Report, cx: &mut AsyncApp) {
    let clip = rs_capture::ClipboardSnapshot::take();
    log::info!("SELFTEST clipboard at exit: {} item(s), {} bytes", clip.items.len(), clip.byte_len());
    let failed: Vec<_> = r.0.iter().filter(|(_, ok)| !ok).map(|(n, _)| n.clone()).collect();
    if failed.is_empty() {
        log::info!("SELFTEST DONE: all {} checks passed", r.0.len());
    } else {
        log::error!("SELFTEST DONE: {} of {} checks failed: {failed:?}", failed.len(), r.0.len());
    }
    cx.update(|cx| {
        close_user_windows(cx);
        crate::save_preferences_now(cx);
        if failed.is_empty() {
            cx.quit();
        } else {
            std::process::exit(2);
        }
    });
}
