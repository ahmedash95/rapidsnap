//! Menu-bar tray icon + global hotkeys, bridged into GPUI.
//!
//! Integration pattern (verified on macOS with GPUI's run loop):
//! * `tray-icon` / `muda` / `global-hotkey` deliver events on the main
//!   thread from inside AppKit's run loop — which GPUI owns — so they work
//!   without any extra event loop.
//! * Their `set_event_handler` callbacks are `Send + Sync` and have no access
//!   to GPUI's `App`, so they push an [`AppEvent`] into a `smol` channel.
//! * A task on GPUI's **foreground executor** (`cx.spawn`) awaits the
//!   channel and handles each event with `&mut App`. No polling, no latency.
//! * TrayIcon and GlobalHotKeyManager must be created on the main thread
//!   *after* the app finished launching (i.e. inside `Application::run`) and
//!   kept alive (we store them in a GPUI Global).

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::{App, Global};
use rs_core::{Hotkey, Hotkeys};
use rs_smart::history::HistoryEntry;
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{TrayIcon, TrayIconBuilder};
use uuid::Uuid;

/// Everything the app reacts to from outside GPUI (tray menu, hotkeys,
/// Dock / Finder "open").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppEvent {
    CaptureRegion,
    CaptureWindow,
    CaptureFullscreen,
    RepeatLastRegion,
    /// Region capture after a countdown (seconds).
    DelayedCapture(u32),
    OpenFile,
    OpenClipboard,
    OpenRecent(Uuid),
    ClearHistory,
    ToggleInstantCopy,
    Preferences,
    About,
    Quit,
}

/// The four hotkey-bindable capture modes, in display order.
pub const HOTKEY_MODES: [AppEvent; 4] =
    [AppEvent::CaptureRegion, AppEvent::CaptureWindow, AppEvent::CaptureFullscreen, AppEvent::RepeatLastRegion];

impl AppEvent {
    fn menu_id(self) -> String {
        match self {
            AppEvent::CaptureRegion => "capture-region".into(),
            AppEvent::CaptureWindow => "capture-window".into(),
            AppEvent::CaptureFullscreen => "capture-fullscreen".into(),
            AppEvent::RepeatLastRegion => "repeat-last".into(),
            AppEvent::DelayedCapture(s) => format!("delay:{s}"),
            AppEvent::OpenFile => "open-file".into(),
            AppEvent::OpenClipboard => "open-clipboard".into(),
            AppEvent::OpenRecent(id) => format!("recent:{id}"),
            AppEvent::ClearHistory => "clear-history".into(),
            AppEvent::ToggleInstantCopy => "instant-copy".into(),
            AppEvent::Preferences => "preferences".into(),
            AppEvent::About => "about".into(),
            AppEvent::Quit => "quit".into(),
        }
    }

    fn from_menu_id(id: &str) -> Option<AppEvent> {
        if let Some(s) = id.strip_prefix("delay:") {
            return s.parse().ok().map(AppEvent::DelayedCapture);
        }
        if let Some(s) = id.strip_prefix("recent:") {
            return Uuid::parse_str(s).ok().map(AppEvent::OpenRecent);
        }
        [
            AppEvent::CaptureRegion,
            AppEvent::CaptureWindow,
            AppEvent::CaptureFullscreen,
            AppEvent::RepeatLastRegion,
            AppEvent::OpenFile,
            AppEvent::OpenClipboard,
            AppEvent::ClearHistory,
            AppEvent::ToggleInstantCopy,
            AppEvent::Preferences,
            AppEvent::About,
            AppEvent::Quit,
        ]
        .into_iter()
        .find(|e| e.menu_id() == id)
    }

    /// Human label for hotkey-bindable modes.
    pub fn label(self) -> &'static str {
        match self {
            AppEvent::CaptureRegion => "Capture Region",
            AppEvent::CaptureWindow => "Capture Window",
            AppEvent::CaptureFullscreen => "Capture Full Screen",
            AppEvent::RepeatLastRegion => "Repeat Last Region",
            _ => "",
        }
    }
}

/// The hotkey assigned to `mode` in `hotkeys`.
pub fn hotkey_for(hotkeys: &Hotkeys, mode: AppEvent) -> Option<&Hotkey> {
    match mode {
        AppEvent::CaptureRegion => hotkeys.region.as_ref(),
        AppEvent::CaptureWindow => hotkeys.window.as_ref(),
        AppEvent::CaptureFullscreen => hotkeys.fullscreen.as_ref(),
        AppEvent::RepeatLastRegion => hotkeys.repeat_last.as_ref(),
        _ => None,
    }
}

/// Mutable slot of the hotkey for `mode`.
pub fn hotkey_slot(hotkeys: &mut Hotkeys, mode: AppEvent) -> Option<&mut Option<Hotkey>> {
    match mode {
        AppEvent::CaptureRegion => Some(&mut hotkeys.region),
        AppEvent::CaptureWindow => Some(&mut hotkeys.window),
        AppEvent::CaptureFullscreen => Some(&mut hotkeys.fullscreen),
        AppEvent::RepeatLastRegion => Some(&mut hotkeys.repeat_last),
        _ => None,
    }
}

/// Keeps the tray icon and hotkey manager alive; stored as a GPUI Global.
pub struct TrayState {
    pub tray: TrayIcon,
    pub manager: GlobalHotKeyManager,
    /// hotkey id → event
    bindings: HashMap<u32, AppEvent>,
    registered: Vec<HotKey>,
    /// The hotkeys the current registration was made from.
    pub current: Hotkeys,
    /// Per-mode registration error (e.g. combo taken by another app).
    pub errors: BTreeMap<String, String>,
    /// True while the shortcut recorder is active (hotkeys unregistered).
    pub suspended: bool,
    capture_items: Vec<(AppEvent, MenuItem)>,
    instant_copy: CheckMenuItem,
    recent: Submenu,
}

impl Global for TrayState {}

/// Convert a stored hotkey into a `global_hotkey::HotKey`.
pub fn to_global_hotkey(h: &Hotkey) -> anyhow::Result<HotKey> {
    let mut m = Modifiers::empty();
    if h.modifiers.cmd {
        m |= Modifiers::SUPER;
    }
    if h.modifiers.shift {
        m |= Modifiers::SHIFT;
    }
    if h.modifiers.alt {
        m |= Modifiers::ALT;
    }
    if h.modifiers.ctrl {
        m |= Modifiers::CONTROL;
    }
    let code = Code::from_str(&h.key).map_err(|e| anyhow::anyhow!("bad key {:?}: {e:?}", h.key))?;
    Ok(HotKey::new(Some(m), code))
}

fn load_tray_icon() -> anyhow::Result<tray_icon::Icon> {
    let path = rs_core::assets::asset_path("icons/tray-template@2x.png");
    let img = image::open(&path).map_err(|e| anyhow::anyhow!("{path:?}: {e}"))?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok(tray_icon::Icon::from_rgba(img.into_raw(), w, h)?)
}

fn item(e: AppEvent, label: &str) -> MenuItem {
    MenuItem::with_id(e.menu_id(), label, true, None)
}

/// Create tray + hotkeys and start forwarding events. `on_event` runs on the
/// GPUI main thread for every event.
pub fn install(
    cx: &mut App,
    hotkeys: &Hotkeys,
    instant_copy: bool,
    on_event: impl Fn(AppEvent, &mut App) + 'static,
) -> anyhow::Result<()> {
    let (tx, rx) = smol::channel::unbounded::<AppEvent>();

    // --- tray menu ---
    let capture_items: Vec<(AppEvent, MenuItem)> = HOTKEY_MODES.iter().map(|&e| (e, item(e, e.label()))).collect();
    let delayed = Submenu::with_items(
        "Delayed Capture",
        true,
        &[
            &item(AppEvent::DelayedCapture(3), "3 Seconds"),
            &item(AppEvent::DelayedCapture(5), "5 Seconds"),
            &item(AppEvent::DelayedCapture(10), "10 Seconds"),
        ],
    )?;
    let open_accel = keys_accel("KeyO", true);
    let open_file = MenuItem::with_id(AppEvent::OpenFile.menu_id(), "Open File…", true, open_accel);
    let recent = Submenu::new("Recent Captures", true);
    let instant =
        CheckMenuItem::with_id(AppEvent::ToggleInstantCopy.menu_id(), "Instant Copy Mode", true, instant_copy, None);
    let prefs_item =
        MenuItem::with_id(AppEvent::Preferences.menu_id(), "Preferences…", true, keys_accel("Comma", true));
    let quit_item = MenuItem::with_id(AppEvent::Quit.menu_id(), "Quit RapidSnap", true, keys_accel("KeyQ", true));

    let menu = Menu::new();
    let mut entries: Vec<&dyn IsMenuItem> = capture_items.iter().map(|(_, i)| i as &dyn IsMenuItem).collect();
    let sep1 = PredefinedMenuItem::separator();
    let sep2 = PredefinedMenuItem::separator();
    let sep3 = PredefinedMenuItem::separator();
    let sep4 = PredefinedMenuItem::separator();
    // Same shortcut as the app menu binding (⇧⌘V, main.rs).
    let clipboard = MenuItem::with_id(
        AppEvent::OpenClipboard.menu_id(),
        "Open from Clipboard",
        true,
        crate::keys::accelerator(&Hotkey::new(rs_core::Modifiers { cmd: true, shift: true, ..Default::default() }, "KeyV")),
    );
    let about = item(AppEvent::About, "About RapidSnap");
    entries.extend([
        &delayed as &dyn IsMenuItem,
        &sep1,
        &open_file,
        &clipboard,
        &recent,
        &sep2,
        &instant,
        &sep3,
        &prefs_item,
        &about,
        &sep4,
        &quit_item,
    ]);
    menu.append_items(&entries)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(load_tray_icon()?)
        .with_icon_as_template(true)
        .with_tooltip("RapidSnap")
        .build()?;

    let menu_tx = tx;
    MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
        if let Some(e) = AppEvent::from_menu_id(ev.id.as_ref()) {
            let _ = menu_tx.try_send(e);
        }
    }));

    // --- global hotkeys ---
    let manager = GlobalHotKeyManager::new()?;
    let mut state = TrayState {
        tray,
        manager,
        bindings: HashMap::new(),
        registered: Vec::new(),
        current: hotkeys.clone(),
        errors: BTreeMap::new(),
        suspended: false,
        capture_items,
        instant_copy: instant,
        recent,
    };
    state.register_hotkeys(hotkeys);
    state.set_recent(&[]);

    let (hk_id_tx, hk_rx) = smol::channel::unbounded::<u32>();
    GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
        if ev.state == HotKeyState::Pressed {
            let _ = hk_id_tx.try_send(ev.id);
        }
    }));
    cx.set_global(state);

    // --- forward into GPUI (foreground executor) ---
    let on_event = std::rc::Rc::new(on_event);
    let on_menu = on_event.clone();
    cx.spawn(async move |cx| {
        while let Ok(e) = rx.recv().await {
            log::debug!("tray/app event: {e:?}");
            cx.update(|cx| on_menu(e, cx));
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(id) = hk_rx.recv().await {
            cx.update(|cx| {
                let ev = cx.try_global::<TrayState>().filter(|s| !s.suspended).and_then(|s| s.bindings.get(&id).copied());
                log::debug!("hotkey {id} -> {ev:?}");
                if let Some(e) = ev {
                    on_event(e, cx);
                }
            });
        }
    })
    .detach();
    Ok(())
}

fn keys_accel(code: &str, cmd: bool) -> Option<tray_icon::menu::accelerator::Accelerator> {
    crate::keys::accelerator(&Hotkey::new(rs_core::Modifiers { cmd, ..Default::default() }, code))
}

impl TrayState {
    /// (Re-)register hotkeys from preferences. Failures (e.g. combo taken by
    /// another app, or duplicated between modes) are recorded in `errors`.
    pub fn register_hotkeys(&mut self, hotkeys: &Hotkeys) {
        self.unregister_all();
        self.current = hotkeys.clone();
        self.errors.clear();
        for mode in HOTKEY_MODES {
            let hk = hotkey_for(hotkeys, mode);
            // Menu accelerator (display only): shown only once the hotkey is
            // really ours — a combo held by another app (e.g. RapidSnap v1)
            // must not be advertised next to our menu item.
            let menu_item = self.capture_items.iter().find(|(e, _)| *e == mode).map(|(_, mi)| mi.clone());
            let show = |shown: Option<&Hotkey>| {
                if let Some(mi) = &menu_item {
                    let _ = mi.set_accelerator(shown.and_then(crate::keys::accelerator));
                }
            };
            show(None);
            let Some(hk) = hk else { continue };
            if self.registered.iter().any(|g| to_global_hotkey(hk).is_ok_and(|n| n.id() == g.id())) {
                self.errors.insert(mode.label().into(), format!("{} is already used by another mode", hk.display()));
                continue;
            }
            match to_global_hotkey(hk).and_then(|g| self.manager.register(g).map(|_| g).map_err(Into::into)) {
                Ok(g) => {
                    log::info!("registered hotkey {} for {mode:?}", hk.display());
                    self.bindings.insert(g.id(), mode);
                    self.registered.push(g);
                    show(Some(hk));
                }
                Err(e) => {
                    log::warn!("hotkey {} for {mode:?} failed: {e}", hk.display());
                    self.errors
                        .insert(mode.label().into(), format!("{} could not be registered (in use by another app?)", hk.display()));
                }
            }
        }
    }

    fn unregister_all(&mut self) {
        if !self.registered.is_empty() {
            let _ = self.manager.unregister_all(&self.registered);
        }
        self.registered.clear();
        self.bindings.clear();
    }

    /// Temporarily release all hotkeys (shortcut recorder active) so the
    /// combination reaches our window instead of triggering a capture.
    pub fn suspend(&mut self) {
        self.suspended = true;
        self.unregister_all();
    }

    /// Re-register after [`suspend`](Self::suspend).
    pub fn resume(&mut self, hotkeys: &Hotkeys) {
        self.suspended = false;
        self.register_hotkeys(hotkeys);
    }

    /// Sync the "Instant Copy Mode" check mark.
    pub fn set_instant_copy(&self, on: bool) {
        if self.instant_copy.is_checked() != on {
            self.instant_copy.set_checked(on);
        }
    }

    /// Countdown / status text next to the tray icon (None = icon only).
    pub fn set_title(&self, title: Option<&str>) {
        self.tray.set_title(title);
    }

    /// Rebuild the "Recent Captures" submenu (newest first).
    pub fn set_recent(&self, entries: &[HistoryEntry]) {
        while self.recent.remove_at(0).is_some() {}
        if entries.is_empty() {
            let _ = self.recent.append(&MenuItem::new("No Recent Captures", false, None));
            return;
        }
        for e in entries.iter().take(10) {
            let _ = self.recent.append(&item(AppEvent::OpenRecent(e.id), &recent_label(e)));
        }
        let _ = self.recent.append(&PredefinedMenuItem::separator());
        let _ = self.recent.append(&item(AppEvent::ClearHistory, "Clear History"));
    }
}

fn recent_label(e: &HistoryEntry) -> String {
    use chrono::TimeZone as _;
    let when = chrono::Local
        .timestamp_millis_opt(e.created_at_ms)
        .single()
        .map(|t| t.format("%b %-d, %H:%M").to_string())
        .unwrap_or_default();
    let what = match &e.source {
        rs_core::CaptureSource::Window { app_name: Some(a), .. } if !a.is_empty() => a.clone(),
        rs_core::CaptureSource::Window { .. } => "Window".into(),
        rs_core::CaptureSource::Fullscreen { .. } => "Full Screen".into(),
        rs_core::CaptureSource::Region => "Region".into(),
        rs_core::CaptureSource::File { path } => {
            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "File".into())
        }
        rs_core::CaptureSource::Clipboard => "Clipboard".into(),
        _ => "Capture".into(),
    };
    format!("{what} — {}×{} — {when}", e.width, e.height)
}
