//! Self-test part for the side panel: walks the Style / Beautify states,
//! checks that the default content of each fits [`FIT_H`] of panel height
//! without scrolling, and (with `RAPIDSNAP_EDITOR_SELFTEST_SHOT=1`) captures
//! the window for each state plus the whole window at the minimum size.
//! Shots go to `$RAPIDSNAP_PANEL_SHOT_DIR` (default: the self-test output
//! dir) as `light-style-arrow.png`, `dark-beautify-more.png`, …
//! (`RAPIDSNAP_PANEL_DARK=1` switches to the dark appearance).

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::*;
use rs_core::{AnnotationKind, Background, ToolKind};

use crate::panel_layout::MORE_KEY;
use crate::view::EditorView;

/// Panel height (pt) the default content of every view must fit in.
const FIT_H: f32 = 500.0;

fn kind_of(k: &AnnotationKind) -> &'static str {
    match k {
        AnnotationKind::Arrow(_) => "arrow",
        AnnotationKind::Text(_) => "text",
        AnnotationKind::Rect(_) => "rect",
        AnnotationKind::Step(_) => "step",
        AnnotationKind::Blur(_) => "blur",
        _ => "",
    }
}

/// Returns `true` when every check passed.
pub(crate) async fn run(this: &WeakEntity<EditorView>, cx: &mut AsyncWindowContext, out: PathBuf) -> bool {
    let exec = cx.background_executor().clone();
    let shots = std::env::var("RAPIDSNAP_EDITOR_SELFTEST_SHOT").is_ok();
    let dir = std::env::var_os("RAPIDSNAP_PANEL_SHOT_DIR").map(PathBuf::from).unwrap_or(out);
    let _ = std::fs::create_dir_all(&dir);
    let dark = std::env::var("RAPIDSNAP_PANEL_DARK").is_ok();
    if dark {
        let _ = cx.update(|window, cx| gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, Some(window), cx));
    }
    let theme = if dark { "dark" } else { "light" };
    let pick = |cx: &mut AsyncWindowContext, kind: &'static str| {
        this.update_in(cx, |v, window, cx| {
            let id = v.document().annotations.iter().rev().find(|a| kind_of(&a.kind) == kind).map(|a| a.id);
            v.model_mut().set_tool(ToolKind::Select);
            v.model_mut().selection = id.into_iter().collect();
            v.panel.tab = 0;
            v.panel.expanded.clear();
            v.panel.sync(&v.model, window, cx);
            cx.notify();
        })
    };
    let mut ok = true;
    let mut report = Vec::new();
    macro_rules! shot {
        ($name:expr, $fit:expr) => {{
            exec.timer(Duration::from_millis(if shots { 450 } else { 120 })).await;
            let name: String = $name.into();
            let (body, chrome) = this
                .update(cx, |v, _| (v.panel.metrics.body_h.get(), v.panel.metrics.chrome_h.get()))
                .unwrap_or((0.0, 0.0));
            let total = body + chrome;
            if $fit && total > FIT_H {
                log::warn!("selftest: panel {name}: content {total:.0} pt > {FIT_H} pt");
                ok = false;
            }
            report.push(format!("{name}={total:.0}"));
            if shots {
                let p = dir.join(format!("{theme}-{name}.png"));
                let _ = cx.update(|window, _| match crate::debug_shot::capture_window(window, &p) {
                    Ok(()) => log::info!("selftest: panel shot {} ({:?})", p.display(), window.bounds().size),
                    Err(e) => log::warn!("selftest: panel shot failed: {e}"),
                });
            }
        }};
    }
    let saved_bg = this.update(cx, |v, _| v.document().beautify.clone()).ok();
    let saved_export = this.update(cx, |v, cx| v.prefs(cx).export.format).ok();

    // --- Style ------------------------------------------------------------
    for kind in ["arrow", "text", "rect", "step", "blur"] {
        let _ = pick(cx, kind);
        shot!(format!("style-{kind}"), true);
    }
    let _ = pick(cx, "arrow");
    let _ = this.update(cx, |v, cx| {
        v.panel.expanded.extend(["main", "arrow", "shadow", "presets", MORE_KEY]);
        cx.notify();
    });
    shot!("style-arrow-more", false);
    let _ = this.update(cx, |v, cx| {
        v.model_mut().selection.clear();
        v.model_mut().set_tool(ToolKind::Select);
        v.panel.expanded.clear();
        cx.notify();
    });
    shot!("style-empty", true);

    // --- Beautify ---------------------------------------------------------
    // Common case: beautify on with a wallpaper (the tallest background grid).
    let _ = this.update(cx, |v, cx| {
        let mut b = v.document().beautify.clone();
        b.enabled = true;
        b.background = Background::Wallpaper { name: rs_core::assets::BUNDLED_WALLPAPERS[0].into() };
        b.shadow.enabled = true;
        v.model_mut().set_beautify(b, None);
        v.panel.tab = 1;
        v.changed(cx);
    });
    shot!("beautify", true);
    let _ = this.update(cx, |v, cx| {
        let mut b = v.document().beautify.clone();
        b.enabled = true;
        b.background = rs_core::gradient_presets().remove(1).1;
        b.shadow.enabled = true;
        b.inset.enabled = true;
        b.chrome = rs_core::WindowChrome::MacOs { dark: false, title: None };
        if let Some(path) = rs_core::assets::wallpaper_path("wallpaper-01") {
            b.watermark = Some(rs_core::Watermark {
                content: rs_core::WatermarkContent::Image { path, width: 120.0 },
                position: rs_core::Corner::BottomRight,
                opacity: 0.8,
                margin: 16.0,
            });
        }
        if cx.try_global::<crate::PreferencesGlobal>().is_some() {
            cx.global_mut::<crate::PreferencesGlobal>().0.export.format = rs_core::ExportFormat::Jpeg { quality: 85 };
        }
        v.model_mut().set_beautify(b, None);
        v.panel.expanded.extend([MORE_KEY, "inset", "chrome", "watermark", "crop", "export"]);
        v.changed(cx);
    });
    shot!("beautify-more", false);
    if let Some(b) = saved_bg.clone() {
        let _ = this.update(cx, |v, cx| {
            v.model_mut().set_beautify(b, None);
            v.panel.expanded.clear();
            v.changed(cx);
        });
    }

    // --- Whole window at the minimum size and back ------------------------
    if shots {
        let before = cx.update(|window, _| window.bounds().size).ok();
        let (mw, mh) = crate::fit::MIN_WINDOW;
        // Fit mode, so the canvas follows the window like a fresh editor.
        let _ = this.update_in(cx, |v, window, cx| v.set_zoom(None, window, cx));
        let _ = cx.update(|window, _| window.resize(size(px(mw), px(mh))));
        let _ = pick(cx, "arrow");
        exec.timer(Duration::from_millis(400)).await;
        shot!("window-min-style", true);
        let _ = this.update(cx, |v, cx| {
            let mut b = v.document().beautify.clone();
            b.enabled = true;
            b.background = Background::Wallpaper { name: rs_core::assets::BUNDLED_WALLPAPERS[0].into() };
            b.shadow.enabled = true;
            v.model_mut().set_beautify(b, None);
            v.panel.tab = 1;
            v.changed(cx);
        });
        shot!("window-min-beautify", true);
        if let Some(b) = before {
            let _ = cx.update(|window, _| window.resize(b));
        }
        let _ = pick(cx, "arrow");
        exec.timer(Duration::from_millis(400)).await;
        shot!("window-normal", true);
    }
    log::info!("selftest: panel content heights (pt, tabs+body+bottom bar): {}", report.join(" "));

    // Background kinds sanity: None keeps the panel renderable.
    let _ = this.update(cx, |v, cx| {
        let mut b = v.document().beautify.clone();
        b.background = Background::None;
        v.model_mut().set_beautify(b, None);
        v.changed(cx);
    });
    exec.timer(Duration::from_millis(60)).await;
    if let Some(b) = saved_bg {
        let _ = this.update(cx, |v, cx| {
            v.model_mut().set_beautify(b, None);
            v.panel.tab = 0;
            v.changed(cx);
        });
    }
    // Export: JPEG keeps its quality across format switches.
    let jpeg_ok = this
        .update_in(cx, |v, window, cx| {
            if cx.try_global::<crate::PreferencesGlobal>().is_none() {
                return true;
            }
            cx.global_mut::<crate::PreferencesGlobal>().0.export.format = rs_core::ExportFormat::Jpeg { quality: 70 };
            v.panel.sync(&v.model, window, cx);
            v.panel.tab = 1;
            let spec = v.beautify_spec(cx);
            let fmt = spec.secs.iter().flat_map(|s| s.rows.iter()).find_map(|r| match &r.ctl {
                crate::panel_spec::Ctl::Seg(sg) if sg.id.as_ref() == "fmt" => Some(sg.on.clone()),
                _ => None,
            });
            let Some(on) = fmt else { return false };
            on(v, 0, window, cx);
            on(v, 1, window, cx);
            v.panel.tab = 0;
            cx.global::<crate::PreferencesGlobal>().0.export.format == rs_core::ExportFormat::Jpeg { quality: 70 }
        })
        .unwrap_or(false);
    // Color picker drags record a single recent color once settled.
    let before = this.update(cx, |v, cx| v.prefs(cx).recent_colors.len()).unwrap_or(0);
    for i in 0..12u8 {
        let _ = this.update_in(cx, |v, window, cx| {
            let c = rs_core::Color::rgb(10 + i * 7, 200 - i * 9, 90);
            v.apply_color(crate::panel::Col::Primary, c, window, cx);
            v.schedule_recent_color(c, window, cx);
        });
        exec.timer(Duration::from_millis(30)).await;
    }
    exec.timer(Duration::from_millis(900)).await;
    let after = this.update(cx, |v, cx| v.prefs(cx).recent_colors.len()).unwrap_or(0);
    let has_global = this.update(cx, |_, cx| cx.try_global::<crate::PreferencesGlobal>().is_some()).unwrap_or(false);
    let recent_ok = !has_global || after == (before + 1).min(rs_core::MAX_RECENT_COLORS);
    log::info!("selftest: panel jpeg quality kept={jpeg_ok}, picker drag recents {before} → {after} ok={recent_ok}");
    ok &= jpeg_ok && recent_ok;
    if let Some(f) = saved_export {
        let _ = this.update(cx, |_, cx| {
            if cx.try_global::<crate::PreferencesGlobal>().is_some() {
                cx.global_mut::<crate::PreferencesGlobal>().0.export.format = f;
            }
        });
    }
    log::info!("selftest: panel ok={ok}");
    ok
}
