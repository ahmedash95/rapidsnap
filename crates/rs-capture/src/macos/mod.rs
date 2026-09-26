//! macOS backend: CoreGraphics enumeration, ScreenCaptureKit capture,
//! NSPasteboard, ImageIO, NSWorkspace wallpaper.

pub(crate) mod cgimage;
pub(crate) mod imageio;
pub(crate) mod pasteboard;
pub(crate) mod sck;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_core_foundation::{CFArray, CFString, CGRect};
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayIsBuiltin, CGDisplayMode, CGError, CGGetActiveDisplayList,
    CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowOwnerPID,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use rs_core::Rect;

use crate::{CaptureError, DisplayInfo, Result, WindowInfo};

pub(crate) fn preflight() -> bool {
    CGPreflightScreenCaptureAccess()
}

pub(crate) fn request() -> bool {
    CGRequestScreenCaptureAccess()
}

pub(crate) fn cg_rect_to_rect(r: CGRect) -> Rect {
    Rect::new(r.origin.x as f32, r.origin.y as f32, r.size.width as f32, r.size.height as f32)
}

// ---------------------------------------------------------------------------
// Displays
// ---------------------------------------------------------------------------

pub(crate) fn active_display_ids() -> Result<Vec<u32>> {
    let mut ids = [0u32; 64];
    let mut n = 0u32;
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
    if err != CGError::Success {
        return Err(CaptureError::Platform(format!("CGGetActiveDisplayList failed: {}", err.0)));
    }
    Ok(ids[..(n as usize).min(ids.len())].to_vec())
}

/// Backing scale of a display: mode pixel width / point width (2.0 for
/// every HiDPI mode incl. "scaled" ones, 1.0 otherwise).
fn display_scale(id: u32) -> f32 {
    if let Some(mode) = CGDisplayCopyDisplayMode(id) {
        let px = CGDisplayMode::pixel_width(Some(&mode));
        let pt = CGDisplayMode::width(Some(&mode));
        if px > 0 && pt > 0 {
            return px as f32 / pt as f32;
        }
    }
    1.0
}

/// Display names come from NSScreen, which is main-thread-only; they're
/// cached whenever `list_displays` runs on the main thread.
static DISPLAY_NAMES: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

fn screen_number(screen: &NSScreen) -> Option<u32> {
    let desc = screen.deviceDescription();
    let key = NSString::from_str("NSScreenNumber");
    let obj: Retained<AnyObject> = desc.objectForKey(&key)?;
    obj.downcast_ref::<NSNumber>().map(|n| n.as_u32())
}

fn refresh_display_names(mtm: MainThreadMarker) {
    let mut map = HashMap::new();
    for screen in NSScreen::screens(mtm).iter() {
        if let Some(id) = screen_number(&screen) {
            map.insert(id, screen.localizedName().to_string());
        }
    }
    if let Ok(mut g) = DISPLAY_NAMES.lock() {
        *g = Some(map);
    }
}

fn display_name(id: u32) -> String {
    if let Ok(g) = DISPLAY_NAMES.lock()
        && let Some(name) = g.as_ref().and_then(|m| m.get(&id))
    {
        return name.clone();
    }
    if CGDisplayIsBuiltin(id) { "Built-in Display".to_owned() } else { format!("Display {id}") }
}

pub(crate) fn list_displays() -> Result<Vec<DisplayInfo>> {
    if let Some(mtm) = MainThreadMarker::new() {
        refresh_display_names(mtm);
    }
    displays_cg()
}

/// Displays from CoreGraphics only (cached names; no AppKit) — cheap.
pub(crate) fn displays_cg() -> Result<Vec<DisplayInfo>> {
    let main = CGMainDisplayID();
    let mut out: Vec<DisplayInfo> = active_display_ids()?
        .into_iter()
        .map(|id| DisplayInfo {
            id,
            name: display_name(id),
            frame: cg_rect_to_rect(CGDisplayBounds(id)),
            scale_factor: display_scale(id),
            is_main: id == main,
        })
        .filter(|d| !d.frame.is_empty())
        .collect();
    out.sort_by_key(|d| !d.is_main);
    if out.is_empty() {
        return Err(CaptureError::NotFound);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Raw on-screen window record (front-to-back order).
#[derive(Debug, Clone)]
pub(crate) struct RawWindow {
    pub info: WindowInfo,
    pub alpha: f32,
}

fn key(s: &'static CFString) -> &'static NSString {
    // CFString and NSString are toll-free bridged.
    unsafe { &*(s as *const CFString as *const NSString) }
}

fn dict_number(d: &NSDictionary<NSString, AnyObject>, k: &NSString) -> Option<Retained<NSNumber>> {
    let obj = d.objectForKey(k)?;
    obj.downcast::<NSNumber>().ok()
}

fn dict_string(d: &NSDictionary<NSString, AnyObject>, k: &NSString) -> Option<String> {
    let obj = d.objectForKey(k)?;
    obj.downcast::<NSString>().ok().map(|s| s.to_string())
}

fn dict_bounds(d: &NSDictionary<NSString, AnyObject>) -> Option<Rect> {
    let obj = d.objectForKey(key(unsafe { kCGWindowBounds }))?;
    let b = obj.downcast::<NSDictionary>().ok()?;
    // SAFETY: kCGWindowBounds is a CFDictionary<CFString, CFNumber>.
    let b: &NSDictionary<NSString, AnyObject> = unsafe { &*(Retained::as_ptr(&b) as *const NSDictionary<NSString, AnyObject>) };
    let n = |k: &str| dict_number(b, &NSString::from_str(k)).map(|v| v.as_f64() as f32);
    Some(Rect::new(n("X")?, n("Y")?, n("Width")?, n("Height")?))
}

/// All on-screen windows, front-to-back, straight from the window server
/// (no filtering besides "on screen" and "not a desktop element"). Needs no
/// permission; titles are empty without Screen Recording.
pub(crate) fn raw_windows(option: CGWindowListOption, relative_to: u32) -> Result<Vec<RawWindow>> {
    let arr: objc2_core_foundation::CFRetained<CFArray> = CGWindowListCopyWindowInfo(option, relative_to)
        .ok_or(CaptureError::Platform("CGWindowListCopyWindowInfo returned NULL".into()))?;
    // SAFETY: CFArray<CFDictionary> is toll-free bridged to NSArray<NSDictionary>.
    let arr: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*(&*arr as *const CFArray as *const NSArray<NSDictionary<NSString, AnyObject>>) };
    let displays = displays_cg().unwrap_or_default();
    let mut out = Vec::with_capacity(arr.count());
    for d in arr.iter() {
        let Some(id) = dict_number(&d, key(unsafe { kCGWindowNumber })) else { continue };
        let Some(frame) = dict_bounds(&d) else { continue };
        let pid = dict_number(&d, key(unsafe { kCGWindowOwnerPID })).map(|n| n.as_i32()).unwrap_or(-1);
        let layer = dict_number(&d, key(unsafe { kCGWindowLayer })).map(|n| n.as_i32()).unwrap_or(0);
        let alpha = dict_number(&d, key(unsafe { kCGWindowAlpha })).map(|n| n.as_f64() as f32).unwrap_or(1.0);
        let app_name = dict_string(&d, key(unsafe { kCGWindowOwnerName })).unwrap_or_default();
        let title = dict_string(&d, key(unsafe { kCGWindowName })).unwrap_or_default();
        let display_id = best_display(&displays, &frame);
        out.push(RawWindow {
            info: WindowInfo { id: id.as_u32(), owner_pid: pid, app_name, title, frame, layer, display_id },
            alpha,
        });
    }
    Ok(out)
}

/// Display with the largest overlap with `frame`.
pub(crate) fn best_display(displays: &[DisplayInfo], frame: &Rect) -> Option<u32> {
    displays
        .iter()
        .filter_map(|d| d.frame.intersection(frame).map(|i| (d.id, i.width * i.height)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

pub(crate) fn own_pid() -> i32 {
    std::process::id() as i32
}

/// Apps whose layer-0 windows are never useful capture targets.
const IGNORED_OWNERS: &[&str] = &["Window Server", "Dock", "Control Centre", "Control Center", "Notification Center", "Notification Centre", "WindowManager", "Screenshot"];

/// Minimum window size (points) to be offered as a capture target.
pub(crate) const MIN_WINDOW_SIDE: f32 = 40.0;

pub(crate) fn list_windows() -> Result<Vec<WindowInfo>> {
    let raw = raw_windows(CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements, 0)?;
    let pid = own_pid();
    Ok(raw
        .into_iter()
        .filter(|w| {
            let i = &w.info;
            i.layer == 0
                && i.owner_pid != pid
                && w.alpha > 0.01
                && i.frame.width >= MIN_WINDOW_SIDE
                && i.frame.height >= MIN_WINDOW_SIDE
                && i.display_id.is_some()
                && !i.app_name.is_empty()
                && !IGNORED_OWNERS.contains(&i.app_name.as_str())
        })
        .map(|w| w.info)
        .collect())
}

/// IDs of on-screen windows above `window_id` (front-to-back).
pub(crate) fn window_ids_above(window_id: u32) -> Vec<u32> {
    raw_windows(CGWindowListOption::OptionOnScreenAboveWindow, window_id)
        .map(|v| v.into_iter().map(|w| w.info.id).collect())
        .unwrap_or_default()
}

pub(crate) fn find_window(window_id: u32) -> Option<WindowInfo> {
    raw_windows(CGWindowListOption::OptionIncludingWindow, window_id)
        .ok()?
        .into_iter()
        .find(|w| w.info.id == window_id)
        .map(|w| w.info)
}

/// Does this process currently have any on-screen window?
pub(crate) fn own_process_has_windows() -> bool {
    let pid = own_pid();
    raw_windows(CGWindowListOption::OptionOnScreenOnly, 0)
        .map(|v| v.iter().any(|w| w.info.owner_pid == pid))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Desktop wallpaper
// ---------------------------------------------------------------------------

fn wallpaper_url_main(mtm: MainThreadMarker, display_id: Option<u32>) -> Result<PathBuf> {
    let screens = NSScreen::screens(mtm);
    let target = display_id.unwrap_or_else(|| CGMainDisplayID());
    let screen = screens
        .iter()
        .find(|s| screen_number(s) == Some(target))
        .or_else(|| NSScreen::mainScreen(mtm))
        .ok_or(CaptureError::NotFound)?;
    let url = NSWorkspace::sharedWorkspace().desktopImageURLForScreen(&screen).ok_or(CaptureError::NotFound)?;
    let path = url.path().ok_or(CaptureError::NotFound)?;
    Ok(PathBuf::from(path.to_string()))
}

/// Resolve the wallpaper file URL. On the main thread: direct. Elsewhere:
/// hop to the main queue (async + timeout — never deadlocks, fails with
/// `Platform` if the main run loop isn't serviced within 2 s).
pub(crate) fn desktop_wallpaper_url(display_id: Option<u32>) -> Result<PathBuf> {
    if let Some(mtm) = MainThreadMarker::new() {
        return wallpaper_url_main(mtm, display_id);
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    dispatch2::DispatchQueue::main().exec_async(move || {
        let r = match MainThreadMarker::new() {
            Some(mtm) => wallpaper_url_main(mtm, display_id),
            None => Err(CaptureError::Platform("main queue not on main thread".into())),
        };
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(2))
        .map_err(|_| CaptureError::Platform("timed out waiting for the main thread (NSWorkspace)".into()))?
}
