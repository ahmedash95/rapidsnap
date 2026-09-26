//! RapidSnap v2 — menu-bar app entry point.
//!
//! Startup: single-instance lock → accessory app (no Dock icon) → tray icon
//! + global hotkeys → events dispatched on GPUI's main thread (`tray.rs`).
//!
//! Welcome window on first launch / missing Screen Recording permission.
//!
//! Debug env vars:
//! * `RAPIDSNAP_SELFTEST=1` — end-to-end self-test (see [`selftest`]), exits.
//! * `RAPIDSNAP_DATA_DIR=/tmp/x` — isolated prefs/history (and lock).
//! * `RUST_LOG=debug` — verbose logging.
//! * `RAPIDSNAP_NO_WELCOME=1` — never open the welcome window at startup
//!   (measure the tray-only idle state without Screen Recording permission).


mod capture;
mod hud;
mod keys;
mod overlay;
mod platform;
mod prefs_window;
mod selftest;
mod services;
mod tray;
mod welcome;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::*;
use rs_core::Document;
use rs_editor::{EditorEvent, EditorOptions, PreferencesGlobal};

use crate::capture::CaptureMode;
use crate::hud::Tone;
use crate::platform::ActivationPolicy;
use crate::tray::AppEvent;

gpui_kit::actions!(
    rapidsnap,
    [CaptureRegionAction, CaptureWindowAction, CaptureFullscreenAction, OpenFileAction, OpenClipboardAction, PreferencesAction, AboutAction, QuitAction]
);

/// App menu + global shortcuts, visible while the app is `Regular`
/// (editor / preferences open).
fn install_app_menu(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-o", OpenFileAction, None),
        KeyBinding::new("cmd-shift-v", OpenClipboardAction, None),
        KeyBinding::new("cmd-,", PreferencesAction, None),
        KeyBinding::new("cmd-q", QuitAction, None),
    ]);
    cx.on_action(|_: &CaptureRegionAction, cx| handle_event(AppEvent::CaptureRegion, cx));
    cx.on_action(|_: &CaptureWindowAction, cx| handle_event(AppEvent::CaptureWindow, cx));
    cx.on_action(|_: &CaptureFullscreenAction, cx| handle_event(AppEvent::CaptureFullscreen, cx));
    cx.on_action(|_: &OpenFileAction, cx| handle_event(AppEvent::OpenFile, cx));
    cx.on_action(|_: &OpenClipboardAction, cx| handle_event(AppEvent::OpenClipboard, cx));
    cx.on_action(|_: &PreferencesAction, cx| handle_event(AppEvent::Preferences, cx));
    cx.on_action(|_: &AboutAction, cx| handle_event(AppEvent::About, cx));
    cx.on_action(|_: &QuitAction, cx| handle_event(AppEvent::Quit, cx));
    cx.set_menus([
        Menu::new("RapidSnap").items([
            MenuItem::action("About RapidSnap", AboutAction),
            MenuItem::separator(),
            MenuItem::action("Preferences…", PreferencesAction),
            MenuItem::separator(),
            MenuItem::action("Quit RapidSnap", QuitAction),
        ]),
        Menu::new("File").items([
            MenuItem::action("Capture Region", CaptureRegionAction),
            MenuItem::action("Capture Window", CaptureWindowAction),
            MenuItem::action("Capture Full Screen", CaptureFullscreenAction),
            MenuItem::separator(),
            MenuItem::action("Open File…", OpenFileAction),
            MenuItem::action("Open from Clipboard", OpenClipboardAction),
        ]),
    ]);
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Pre-release v2 builds kept data in `com.rapidsnap.v2`; move it over
    // once, before the instance lock creates the new data dir.
    rs_smart::paths::migrate_legacy_data_dir();

    let relaunched = std::env::var_os(platform::RELAUNCH_ENV).is_some();
    let Some(_lock) = platform::acquire_instance_lock(Duration::from_secs(if relaunched { 5 } else { 0 })) else {
        log::warn!("RapidSnap is already running (data dir {:?}); exiting", rs_smart::paths::data_dir());
        return;
    };

    let self_test = std::env::var_os("RAPIDSNAP_SELFTEST").is_some();

    // Files passed on the command line + Finder/Dock "open" events.
    let (open_tx, open_rx) = smol::channel::unbounded::<PathBuf>();
    for arg in std::env::args_os().skip(1) {
        let p = PathBuf::from(arg);
        if p.is_file() {
            let _ = open_tx.try_send(p);
        }
    }

    let app = gpui_kit::application();
    let url_tx = open_tx.clone();
    app.on_open_urls(move |urls| {
        for u in urls {
            match file_url_to_path(&u) {
                Some(p) => {
                    let _ = url_tx.try_send(p);
                }
                None => log::warn!("ignoring non-file URL {u}"),
            }
        }
    });
    app.on_reopen(|cx| {
        // Dock icon / Finder double-click while running: show preferences
        // if nothing else is open.
        if hud::user_window_count(cx) == 0 {
            prefs_window::open_preferences(prefs_window::TAB_GENERAL, cx);
        }
    });

    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        rs_editor::init(cx);
        install_app_menu(cx);
        services::init(cx);
        // GPUI sets Regular in applicationDidFinishLaunching; override now.
        platform::set_activation_policy(ActivationPolicy::Accessory);

        let prefs = rs_smart::prefs::load_preferences();
        let hotkeys = prefs.hotkeys.clone();
        let instant = prefs.instant_copy;
        let onboarding_done = prefs.onboarding_done;
        cx.set_global(PreferencesGlobal(prefs));
        cx.set_global(SaveState::default());
        cx.observe_global::<PreferencesGlobal>(on_prefs_changed).detach();
        // Any quit path (⌘Q, tray, Dock menu, logout) flushes a pending debounced save.
        cx.on_app_quit(|cx| {
            save_preferences_now(cx);
            async {}
        })
        .detach();

        // Back to accessory (no Dock icon) when the last user window closes.
        cx.on_window_closed(|cx, _| {
            if hud::user_window_count(cx) == 0 {
                platform::set_activation_policy(ActivationPolicy::Accessory);
            }
        })
        .detach();

        if let Err(e) = tray::install(cx, &hotkeys, instant, handle_event) {
            log::error!("tray/hotkey setup failed: {e:#}");
        } else {
            log::info!("tray icon + hotkeys installed");
        }
        capture::refresh_recent(cx);
        // Warm up ScreenCaptureKit (first hotkey freeze-frame) off the main
        // thread. It caches only the shareable-content snapshot (window /
        // display metadata), no pixel buffers. The font system (~tens of MB
        // of font tables) is *not* loaded at startup any more: it's warmed
        // when a capture starts / a document is opened (`warm_up_for_editor`),
        // which still leaves the whole overlay interaction for it to load.
        let can_capture = rs_capture::has_screen_capture_permission();
        cx.background_executor()
            .spawn(async move {
                if can_capture && let Err(e) = rs_capture::prepare_capture() {
                    log::warn!("prepare_capture: {e}");
                }
            })
            .detach();

        // Open-file requests (argv, Finder, Dock drops).
        cx.spawn(async move |cx| {
            while let Ok(path) = open_rx.recv().await {
                cx.update(|cx| open_path(&path, cx));
            }
        })
        .detach();

        if self_test {
            selftest::run(cx);
        } else if std::env::var_os(NO_WELCOME_ENV).is_some() {
            log::info!("{NO_WELCOME_ENV} set: staying tray-only");
        } else if !onboarding_done || !rs_capture::has_screen_capture_permission() {
            welcome::open_welcome(cx);
        }
    });
}

/// Load the renderer's font system in the background (idempotent, returns
/// immediately). Called when a capture starts or a document is about to be
/// opened, so the editor's first text render doesn't pay for it, while the
/// idle tray-only app stays small.
pub(crate) fn warm_up_for_editor(cx: &mut App) {
    rs_render::warm_up_fonts();
    services::prefetch_wallpaper(cx);
}

/// Debug/measurement knob: skip the startup welcome window.
const NO_WELCOME_ENV: &str = "RAPIDSNAP_NO_WELCOME";

/// `file:///a%20b.png` → `/a b.png`.
fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(out).ok()?;
    Some(PathBuf::from(s.trim_end_matches('/')))
}

/// Dispatch a tray/hotkey event (main thread).
pub(crate) fn handle_event(ev: AppEvent, cx: &mut App) {
    log::info!("event: {ev:?}");
    match ev {
        AppEvent::CaptureRegion => capture::start(CaptureMode::Region, None, cx),
        AppEvent::CaptureWindow => capture::start(CaptureMode::Window, None, cx),
        AppEvent::CaptureFullscreen => capture::start(CaptureMode::Fullscreen, None, cx),
        AppEvent::RepeatLastRegion => capture::start(CaptureMode::RepeatLast, None, cx),
        AppEvent::DelayedCapture(s) => capture::start(CaptureMode::Region, Some(s), cx),
        AppEvent::OpenFile => open_file_dialog(cx),
        AppEvent::OpenClipboard => open_clipboard(cx),
        AppEvent::OpenRecent(id) => capture::open_recent(id, cx),
        AppEvent::ClearHistory => capture::clear_history(cx),
        AppEvent::ToggleInstantCopy => {
            cx.update_global::<PreferencesGlobal, _>(|p, _| p.0.instant_copy = !p.0.instant_copy);
            let on = cx.global::<PreferencesGlobal>().0.instant_copy;
            hud::show(
                cx,
                if on { "Instant copy mode on" } else { "Instant copy mode off" },
                on.then(|| "Captures go straight to the clipboard".to_string()),
                Tone::Info,
                Duration::from_millis(1400),
            );
        }
        AppEvent::Preferences => prefs_window::open_preferences(prefs_window::TAB_GENERAL, cx),
        AppEvent::About => prefs_window::open_preferences(prefs_window::TAB_ABOUT, cx),
        AppEvent::Quit => quit(cx),
    }
}

fn quit(cx: &mut App) {
    save_preferences_now(cx);
    cx.quit();
}

// ---------------------------------------------------------------------------
// Preferences: persistence (debounced) + side effects
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SaveState {
    generation: u64,
    last_saved: Option<rs_core::Preferences>,
}
impl Global for SaveState {}

fn on_prefs_changed(cx: &mut App) {
    let prefs = cx.global::<PreferencesGlobal>().0.clone();
    if cx.has_global::<tray::TrayState>() {
        cx.update_global::<tray::TrayState, _>(|t, _| {
            if !t.suspended && t.current != prefs.hotkeys {
                t.register_hotkeys(&prefs.hotkeys);
            }
            t.set_instant_copy(prefs.instant_copy);
        });
    }
    // Debounce: save 400 ms after the last change.
    let generation = {
        let s = cx.default_global::<SaveState>();
        if s.last_saved.as_ref() == Some(&prefs) {
            return;
        }
        s.generation += 1;
        s.generation
    };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_millis(400)).await;
        cx.update(|cx| {
            if cx.global::<SaveState>().generation == generation {
                save_preferences_async(cx);
            }
        });
    })
    .detach();
}

fn save_preferences_async(cx: &mut App) {
    let prefs = cx.global::<PreferencesGlobal>().0.clone();
    cx.global_mut::<SaveState>().last_saved = Some(prefs.clone());
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = rs_smart::prefs::save_preferences(&prefs) {
                log::warn!("saving preferences failed: {e}");
            }
        })
        .detach();
}

/// Save synchronously (quit / relaunch).
pub fn save_preferences_now(cx: &mut App) {
    let Some(prefs) = cx.try_global::<PreferencesGlobal>().map(|p| p.0.clone()) else { return };
    if cx.try_global::<SaveState>().and_then(|s| s.last_saved.as_ref()) == Some(&prefs) {
        return;
    }
    if let Err(e) = rs_smart::prefs::save_preferences(&prefs) {
        log::warn!("saving preferences failed: {e}");
    }
    cx.default_global::<SaveState>().last_saved = Some(prefs);
}

// ---------------------------------------------------------------------------
// Documents → editor
// ---------------------------------------------------------------------------

fn last_beautify(cx: &App) -> rs_core::BeautifySettings {
    cx.try_global::<PreferencesGlobal>().map(|p| p.0.last_beautify.clone()).unwrap_or_default()
}

/// Open an editor window for `doc` (switches to Regular activation policy).
pub fn open_document(doc: Document, cx: &mut App) {
    warm_up_for_editor(cx);
    platform::set_activation_policy(ActivationPolicy::Regular);
    let opts = EditorOptions {
        on_event: Some(std::rc::Rc::new(|ev: &EditorEvent, cx: &mut App| {
            log::info!("editor event: {ev:?}");
            match ev {
                EditorEvent::Saved { path, .. } => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    hud::show(cx, "Saved", name, Tone::Success, Duration::from_millis(1400));
                }
                // The editor adds copies to the history (background) → refresh the menu soon.
                EditorEvent::Copied { .. } | EditorEvent::Closed { .. } => {
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(Duration::from_millis(800)).await;
                        cx.update(capture::refresh_recent);
                    })
                    .detach();
                }
            }
        })),
        services: services::editor_services(cx),
    };
    match rs_editor::open_editor(cx, doc, opts) {
        Ok(h) => {
            // Accessory app → Regular just now (and maybe triggered by a hotkey
            // while another app is frontmost): make sure the editor really
            // comes to the front and is key.
            let _ = h.update(cx, |_, window, _| platform::bring_to_front(window));
        }
        Err(e) => {
            log::error!("open editor: {e:#}");
            hud::show(cx, "Could not open the editor", Some(format!("{e:#}")), Tone::Error, Duration::from_secs(3));
            if hud::user_window_count(cx) == 0 {
                platform::set_activation_policy(ActivationPolicy::Accessory);
            }
        }
    }
}

pub(crate) fn open_path(path: &Path, cx: &mut App) {
    log::info!("open {path:?}");
    warm_up_for_editor(cx);
    // Decoding big images blocks: do it in the background.
    let path = path.to_path_buf();
    let beautify = last_beautify(cx);
    let task = cx.background_executor().spawn({
        let path = path.clone();
        async move {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext == rs_smart::project::PROJECT_EXTENSION {
                rs_smart::project::load_project(&path).map_err(|e| e.to_string())
            } else if rs_capture::SUPPORTED_IMPORT_EXTENSIONS.contains(&ext.as_str()) {
                rs_capture::load_image_file(&path).map(|c| c.into_document(beautify)).map_err(|e| e.to_string())
            } else {
                Err(format!("unsupported file type “.{ext}”"))
            }
        }
    });
    cx.spawn(async move |cx| {
        let r = task.await;
        cx.update(|cx| match r {
            Ok(doc) => open_document(doc, cx),
            Err(e) => {
                log::warn!("open {path:?}: {e}");
                hud::show(cx, "Could not open file", Some(e), Tone::Error, Duration::from_secs(3));
            }
        });
    })
    .detach();
}

fn open_file_dialog(cx: &mut App) {
    // The panel only comes to front if the app is active.
    cx.activate(true);
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Open".into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else { return };
        let Some(path): Option<PathBuf> = paths.into_iter().next() else { return };
        cx.update(|cx| open_path(&path, cx));
    })
    .detach();
}

fn open_clipboard(cx: &mut App) {
    warm_up_for_editor(cx);
    let task = cx.background_executor().spawn(async move { rs_capture::clipboard_image() });
    cx.spawn(async move |cx| {
        let r = task.await;
        cx.update(|cx| match r {
            Ok(cap) => open_document(cap.into_document(last_beautify(cx)), cx),
            Err(rs_capture::CaptureError::NoClipboardImage) => {
                hud::show(cx, "No image on the clipboard", None, Tone::Info, Duration::from_secs(2))
            }
            Err(e) => hud::show(cx, "Open from clipboard failed", Some(e.to_string()), Tone::Error, Duration::from_secs(3)),
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::file_url_to_path;
    use std::path::PathBuf;

    #[test]
    fn file_urls() {
        assert_eq!(file_url_to_path("file:///tmp/a%20b.png"), Some(PathBuf::from("/tmp/a b.png")));
        assert_eq!(file_url_to_path("file://localhost/x.rsnap"), Some(PathBuf::from("/x.rsnap")));
        assert_eq!(file_url_to_path("https://x"), None);
    }
}
