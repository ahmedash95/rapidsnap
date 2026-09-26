//! macOS platform glue: activation policy, native window tweaks (overlay /
//! HUD), CoreGraphics display + window lists (no permission needed), cursor
//! location, single-instance lock, relaunch.
//!
//! Every AppKit call that may fire delegate callbacks into GPUI is deferred
//! to the next main-queue turn (see CONTRACTS.md §9).

use rs_capture::{DisplayInfo, WindowInfo};
use rs_core::{Point, Rect};

/// App activation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationPolicy {
    /// Dock icon + app menu (while editor/preferences windows are open).
    Regular,
    /// Menu-bar only, no Dock icon (idle).
    Accessory,
}

/// Set the NSApplication activation policy. GPUI forces `Regular` in
/// `applicationDidFinishLaunching`, so call this *inside* `Application::run`.
/// Must be called on the main thread (no-op otherwise).
#[cfg(target_os = "macos")]
pub fn set_activation_policy(policy: ActivationPolicy) {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("set_activation_policy called off the main thread");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let p = match policy {
        ActivationPolicy::Regular => NSApplicationActivationPolicy::Regular,
        ActivationPolicy::Accessory => NSApplicationActivationPolicy::Accessory,
    };
    if app.activationPolicy() != p {
        app.setActivationPolicy(p);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_activation_policy(_policy: ActivationPolicy) {}

/// Give focus back to the previously active app (after an overlay capture
/// that didn't open any window). Deferred to the next main-queue turn.
#[cfg(target_os = "macos")]
pub fn deactivate_app() {
    dispatch2::DispatchQueue::main().exec_async(|| {
        use objc2_app_kit::NSApplication;
        use objc2_foundation::MainThreadMarker;
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).deactivate();
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn deactivate_app() {}

#[cfg(target_os = "macos")]
fn ns_window_of(window: &gpui_kit::Window) -> anyhow::Result<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).map_err(|e| anyhow::anyhow!("window handle: {e}"))?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        anyhow::bail!("not an AppKit window");
    };
    // SAFETY: GPUI hands out its live GPUIView (an NSView); we're on the main thread.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    view.window().ok_or_else(|| anyhow::anyhow!("view has no window"))
}

#[cfg(target_os = "macos")]
struct MainOnly(objc2::rc::Retained<objc2_app_kit::NSWindow>);
// SAFETY: only dereferenced inside blocks running on the main queue.
#[cfg(target_os = "macos")]
unsafe impl Send for MainOnly {}

/// Level for overlays (kCGScreenSaverWindowLevel): above menu bar & Dock.
pub const OVERLAY_LEVEL: isize = 1000;

/// Turn a GPUI `PopUp` window into a capture overlay: borderless (keeping
/// `NonactivatingPanel` so it can be key without activating the app),
/// non-opaque, no shadow, level 1000, on all Spaces, covering its whole
/// screen (incl. menu bar). `make_key`: make this the key window (the one
/// under the cursor), others are just ordered front.
///
/// Deferred to the next main-queue turn (see module docs).
#[cfg(target_os = "macos")]
pub fn make_overlay_window(window: &gpui_kit::Window, make_key: bool) -> anyhow::Result<()> {
    use objc2_app_kit::{NSColor, NSWindowCollectionBehavior, NSWindowStyleMask};
    let w = MainOnly(ns_window_of(window)?);
    dispatch2::DispatchQueue::main().exec_async(move || {
        // Bind the whole wrapper (edition-2024 closures would otherwise
        // capture only the non-Send field `w.0`).
        let w = w;
        let w = &w.0;
        w.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
        w.setOpaque(false);
        w.setHasShadow(false);
        w.setBackgroundColor(Some(&NSColor::clearColor()));
        w.setLevel(OVERLAY_LEVEL);
        w.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // Titled windows are constrained below the menu bar at creation.
        if let Some(screen) = w.screen() {
            w.setFrame_display(screen.frame(), true);
        }
        if make_key {
            w.makeKeyAndOrderFront(None);
        } else {
            w.orderFrontRegardless();
        }
    });
    Ok(())
}

/// Bring a freshly opened user window (editor / preferences / welcome) to the
/// front and make it key. An accessory app that just switched to `Regular`
/// (or was triggered by a global hotkey while another app is active) is not
/// reliably activated by GPUI's synchronous `activate` — macOS 14's
/// cooperative activation can ignore it, leaving the window behind the
/// frontmost app. Deferred to the next main-queue turn (after the policy
/// change took effect): activate the app, order the window front regardless
/// of activation, make it key.
#[cfg(target_os = "macos")]
pub fn bring_to_front(window: &gpui_kit::Window) {
    let Ok(w) = ns_window_of(window) else { return };
    let w = MainOnly(w);
    dispatch2::DispatchQueue::main().exec_async(move || {
        let w = w;
        use objc2_app_kit::NSApplication;
        use objc2_foundation::MainThreadMarker;
        if let Some(mtm) = MainThreadMarker::new() {
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }
        w.0.orderFrontRegardless();
        w.0.makeKeyAndOrderFront(None);
    });
}

#[cfg(not(target_os = "macos"))]
pub fn bring_to_front(_window: &gpui_kit::Window) {}

/// Make the overlay window key (e.g. the cursor moved to another display).
#[cfg(target_os = "macos")]
pub fn make_key_window(window: &gpui_kit::Window) {
    if let Ok(w) = ns_window_of(window) {
        let w = MainOnly(w);
        dispatch2::DispatchQueue::main().exec_async(move || {
            let w = w;
            if !w.0.isKeyWindow() {
                w.0.makeKeyAndOrderFront(None);
            }
        });
    }
}

/// HUD / notice window: borderless, click-through, above everything,
/// never key.
#[cfg(target_os = "macos")]
pub fn make_hud_window(window: &gpui_kit::Window) -> anyhow::Result<()> {
    use objc2_app_kit::{NSColor, NSWindowCollectionBehavior, NSWindowStyleMask};
    let w = MainOnly(ns_window_of(window)?);
    dispatch2::DispatchQueue::main().exec_async(move || {
        let w = w;
        let w = &w.0;
        let frame = w.frame();
        w.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
        w.setOpaque(false);
        w.setHasShadow(false);
        w.setBackgroundColor(Some(&NSColor::clearColor()));
        w.setIgnoresMouseEvents(true);
        w.setLevel(OVERLAY_LEVEL + 1);
        w.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        w.setFrame_display(frame, true);
        w.orderFrontRegardless();
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn make_overlay_window(_window: &gpui_kit::Window, _make_key: bool) -> anyhow::Result<()> {
    Ok(())
}
#[cfg(not(target_os = "macos"))]
pub fn make_key_window(_window: &gpui_kit::Window) {}
#[cfg(not(target_os = "macos"))]
pub fn make_hud_window(_window: &gpui_kit::Window) -> anyhow::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Displays / cursor (CoreGraphics, no permission needed)
// ---------------------------------------------------------------------------

/// Active displays in global points (CG global coordinates), with scale
/// factors. Fallback for `rs_capture::list_displays` (and fast: no SCK).
#[cfg(target_os = "macos")]
pub fn display_list() -> Vec<DisplayInfo> {
    use objc2_core_graphics::{
        CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGGetActiveDisplayList, CGMainDisplayID,
    };
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: valid buffer of 16 ids and a valid count pointer.
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if err.0 != 0 {
        log::warn!("CGGetActiveDisplayList failed: {}", err.0);
        return Vec::new();
    }
    let main = CGMainDisplayID();
    ids[..count as usize]
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let b = CGDisplayBounds(id);
            let mode = CGDisplayCopyDisplayMode(id);
            let scale = mode
                .as_deref()
                .map(|m| {
                    let pw = CGDisplayMode::pixel_width(Some(m)) as f64;
                    let w = CGDisplayMode::width(Some(m)) as f64;
                    if w > 0.0 { (pw / w) as f32 } else { 1.0 }
                })
                .unwrap_or(1.0);
            DisplayInfo {
                id,
                name: format!("Display {}", i + 1),
                frame: Rect::new(b.origin.x as f32, b.origin.y as f32, b.size.width as f32, b.size.height as f32),
                scale_factor: scale.max(1.0),
                is_main: id == main,
            }
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn display_list() -> Vec<DisplayInfo> {
    Vec::new()
}

/// Current mouse location in global points (origin top-left of main display).
#[cfg(target_os = "macos")]
pub fn cursor_location() -> Point {
    use objc2_core_graphics::CGEvent;
    let ev = CGEvent::new(None);
    let p = CGEvent::location(ev.as_deref());
    Point::new(p.x as f32, p.y as f32)
}

#[cfg(not(target_os = "macos"))]
pub fn cursor_location() -> Point {
    Point::ZERO
}

/// Move the mouse cursor (global points). Used for arrow-key nudging.
#[cfg(target_os = "macos")]
pub fn warp_cursor(p: Point) {
    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::{CGAssociateMouseAndMouseCursorPosition, CGWarpMouseCursorPosition};
    let _ = CGWarpMouseCursorPosition(CGPoint { x: p.x as f64, y: p.y as f64 });
    // Warping suppresses mouse events briefly; re-associate immediately.
    let _ = CGAssociateMouseAndMouseCursorPosition(true);
}

#[cfg(not(target_os = "macos"))]
pub fn warp_cursor(_p: Point) {}

// ---------------------------------------------------------------------------
// Window list
// ---------------------------------------------------------------------------

/// One entry of `CGWindowListCopyWindowInfo`.
#[derive(Debug, Clone)]
pub struct WindowListEntry {
    pub id: u32,
    pub owner_pid: i64,
    pub owner_name: String,
    pub title: String,
    pub layer: i64,
    pub alpha: f64,
    pub bounds: (f64, f64, f64, f64),
}

/// Windows owned by this process (on-screen), via CoreGraphics.
pub fn own_windows() -> Vec<WindowListEntry> {
    let pid = std::process::id() as i64;
    all_windows().into_iter().filter(|w| w.owner_pid == pid).collect()
}

/// All on-screen windows (any owner), front to back. Needs no permission
/// for id/owner/bounds/layer (titles need Screen Recording).
#[cfg(target_os = "macos")]
pub fn all_windows() -> Vec<WindowListEntry> {
    use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};

    let Some(list): Option<CFRetained<CFArray>> = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    ) else {
        return Vec::new();
    };
    // SAFETY: CGWindowListCopyWindowInfo returns an array of CFDictionary<CFString, CFType>.
    let list: CFRetained<CFArray<CFDictionary<CFString, CFType>>> = unsafe { CFRetained::cast_unchecked(list) };
    let num = |d: &CFDictionary<CFString, CFType>, k: &str| -> Option<f64> {
        d.get(&CFString::from_str(k))?.downcast_ref::<CFNumber>()?.as_f64()
    };
    let string = |d: &CFDictionary<CFString, CFType>, k: &str| -> String {
        d.get(&CFString::from_str(k)).and_then(|v| v.downcast_ref::<CFString>().map(|s| s.to_string())).unwrap_or_default()
    };
    let mut out = Vec::new();
    for d in list.iter() {
        let bounds = d
            .get(&CFString::from_str("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>().ok())
            .map(|b| {
                // SAFETY: kCGWindowBounds is a CFDictionary<CFString, CFNumber>.
                let b: CFRetained<CFDictionary<CFString, CFType>> = unsafe { CFRetained::cast_unchecked(b) };
                let f = |k: &str| {
                    b.get(&CFString::from_str(k))
                        .and_then(|v| v.downcast_ref::<CFNumber>().and_then(|n| n.as_f64()))
                        .unwrap_or(0.0)
                };
                (f("X"), f("Y"), f("Width"), f("Height"))
            })
            .unwrap_or_default();
        out.push(WindowListEntry {
            id: num(&d, "kCGWindowNumber").unwrap_or(0.0) as u32,
            owner_pid: num(&d, "kCGWindowOwnerPID").unwrap_or(-1.0) as i64,
            owner_name: string(&d, "kCGWindowOwnerName"),
            title: string(&d, "kCGWindowName"),
            layer: num(&d, "kCGWindowLayer").unwrap_or(0.0) as i64,
            alpha: num(&d, "kCGWindowAlpha").unwrap_or(1.0),
            bounds,
        });
    }
    out
}

#[cfg(not(target_os = "macos"))]
pub fn all_windows() -> Vec<WindowListEntry> {
    Vec::new()
}

/// Fallback for `rs_capture::list_windows`: normal (layer 0), visible,
/// non-tiny windows of other processes, front to back.
pub fn capturable_windows() -> Vec<WindowInfo> {
    let pid = std::process::id() as i64;
    all_windows()
        .into_iter()
        .filter(|w| w.owner_pid != pid && w.layer == 0 && w.alpha > 0.01 && w.bounds.2 >= 40.0 && w.bounds.3 >= 40.0)
        .map(|w| WindowInfo {
            id: w.id,
            owner_pid: w.owner_pid as i32,
            app_name: w.owner_name,
            title: w.title,
            frame: Rect::new(w.bounds.0 as f32, w.bounds.1 as f32, w.bounds.2 as f32, w.bounds.3 as f32),
            layer: w.layer as i32,
            display_id: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Single instance / relaunch
// ---------------------------------------------------------------------------

/// Holds an exclusive `flock` on `<data dir>/rapidsnap.lock` for the whole
/// process lifetime.
pub struct InstanceLock {
    #[allow(dead_code)]
    file: std::fs::File,
}

/// Try to become the single running instance (per data dir). Retries for up
/// to `wait` (used after "Relaunch", while the old process is exiting).
#[cfg(unix)]
pub fn acquire_instance_lock(wait: std::time::Duration) -> Option<InstanceLock> {
    use std::os::fd::AsRawFd;
    let dir = rs_smart::paths::data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("create data dir {dir:?}: {e}");
    }
    let path = dir.join(rs_smart::paths::LOCK_FILE_NAME);
    let file = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            // Can't lock → don't block startup.
            log::warn!("open lock file {path:?}: {e}");
            return std::fs::File::open("/dev/null").ok().map(|file| InstanceLock { file });
        }
    };
    let deadline = std::time::Instant::now() + wait;
    loop {
        // SAFETY: valid fd owned by `file`.
        let r = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if r == 0 {
            return Some(InstanceLock { file });
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(not(unix))]
pub fn acquire_instance_lock(_wait: std::time::Duration) -> Option<InstanceLock> {
    std::fs::File::open(std::env::current_exe().ok()?).ok().map(|file| InstanceLock { file })
}

/// Env var set on a relaunched process (makes it wait for the lock).
pub const RELAUNCH_ENV: &str = "RAPIDSNAP_RELAUNCHED";

/// Spawn a detached helper that starts RapidSnap again once this process
/// has exited (the caller quits right after). Inside a `.app` uses `open`
/// so macOS re-reads TCC permissions for the bundle.
pub fn relaunch() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let exe_s = exe.to_string_lossy().to_string();
    let bundle = exe_s.find(".app/Contents/MacOS/").map(|i| exe_s[..i + 4].to_string());
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    // Wait (≤ 10 s) for this process to exit so the instance lock is free,
    // then start again. `open` launches through LaunchServices, which does
    // not inherit our environment → pass the relevant vars explicitly.
    let wait = format!("i=0; while kill -0 {} 2>/dev/null && [ $i -lt 100 ]; do sleep 0.1; i=$((i+1)); done", std::process::id());
    let cmd = match bundle {
        Some(b) => {
            let mut envs = format!(" --env {RELAUNCH_ENV}=1");
            for var in [rs_smart::paths::DATA_DIR_ENV, "RUST_LOG"] {
                if let Ok(v) = std::env::var(var) {
                    envs.push_str(&format!(" --env {}", quote(&format!("{var}={v}"))));
                }
            }
            format!("{wait}; /usr/bin/open -n{envs} {}", quote(&b))
        }
        None => format!("{wait}; {} >/dev/null 2>&1 &", quote(&exe_s)),
    };
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .env(RELAUNCH_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// Other running RapidSnap processes (v1 and v2 share the bundle id
/// `io.github.ahmedash95.rapidsnap` / name "RapidSnap"; a renamed user copy,
/// a dev `rapidsnap` binary, …): see [`is_other_rapidsnap`]. This process is
/// excluded by pid, so the result never contains ourselves even though our own
/// bundle id / name match.
/// The self-test must not post global hotkeys while one runs — the event
/// would trigger *their* capture.
#[cfg(target_os = "macos")]
pub fn other_rapidsnap_instances() -> Vec<String> {
    let own = std::process::id() as i32;
    let apps = objc2_app_kit::NSWorkspace::sharedWorkspace().runningApplications();
    apps.iter()
        .filter_map(|a| {
            let pid = a.processIdentifier();
            let id = a.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default();
            let name = a.localizedName().map(|s| s.to_string()).unwrap_or_default();
            is_other_rapidsnap(own, pid, &id, &name).then(|| format!("{name} ({id}, pid {pid})"))
        })
        .collect()
}

/// Whether the running app (`pid`, bundle id, localized name) is a RapidSnap
/// other than this process (`own_pid`): compared by pid only, never by name
/// or bundle id (v1, v2 and this process can all share them). Matches any
/// bundle id or name containing "rapidsnap" (case-insensitive), except Apple
/// helpers named after the app (e.g. "AutoFill (RapidSnap)").
pub fn is_other_rapidsnap(own_pid: i32, pid: i32, bundle_id: &str, name: &str) -> bool {
    pid != own_pid
        && pid > 0
        && !bundle_id.starts_with("com.apple.")
        && (bundle_id.to_ascii_lowercase().contains("rapidsnap") || name.to_ascii_lowercase().contains("rapidsnap"))
}

#[cfg(not(target_os = "macos"))]
pub fn other_rapidsnap_instances() -> Vec<String> {
    Vec::new()
}

/// Debug helper: synthesize a key press (down+up) with ⌘/⇧ via CGEventPost
/// (HID tap). Used by the self-test. Requires the responsible process (e.g.
/// the terminal) to have Accessibility access, otherwise macOS drops them.
/// Returns whether posting is permitted (`CGPreflightPostEventAccess`).
#[cfg(target_os = "macos")]
pub fn post_key_combo(keycode: u16, cmd: bool, shift: bool) -> bool {
    use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation, CGPreflightPostEventAccess};
    let allowed = CGPreflightPostEventAccess();
    log::info!("CGPreflightPostEventAccess = {allowed}");
    if !allowed {
        // macOS would drop them anyway; don't even try.
        return false;
    }
    let mut flags = CGEventFlags::empty();
    if cmd {
        flags |= CGEventFlags::MaskCommand;
    }
    if shift {
        flags |= CGEventFlags::MaskShift;
    }
    for down in [true, false] {
        if let Some(ev) = CGEvent::new_keyboard_event(None, keycode, down) {
            CGEvent::set_flags(Some(&ev), flags);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        }
    }
    allowed
}

#[cfg(not(target_os = "macos"))]
pub fn post_key_combo(_keycode: u16, _cmd: bool, _shift: bool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::is_other_rapidsnap;

    #[test]
    fn other_instance_detection_is_by_pid() {
        let own = 100;
        // Ourselves: never "another instance", whatever our id / name.
        assert!(!is_other_rapidsnap(own, own, rs_smart::paths::APP_ID, "RapidSnap"));
        assert!(!is_other_rapidsnap(own, own, "", "rapidsnap"));
        // v1 / a bundled v2 (same bundle id), a user copy, a dev binary.
        assert!(is_other_rapidsnap(own, 200, "io.github.ahmedash95.rapidsnap", "RapidSnap"));
        assert!(is_other_rapidsnap(own, 201, "com.rapidsnap.v2", "RapidSnap-Preview"));
        assert!(is_other_rapidsnap(own, 202, "", "rapidsnap"));
        // Unrelated apps and Apple helpers named after the app.
        assert!(!is_other_rapidsnap(own, 300, "com.apple.Safari", "Safari"));
        assert!(!is_other_rapidsnap(own, 301, "com.apple.AutoFill", "AutoFill (RapidSnap)"));
        assert!(!is_other_rapidsnap(own, -1, "io.github.ahmedash95.rapidsnap", "RapidSnap"));
    }
}
