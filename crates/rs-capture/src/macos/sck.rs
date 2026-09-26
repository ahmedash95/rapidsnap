//! ScreenCaptureKit capture (macOS 14+: `SCScreenshotManager`).
//!
//! Why SCK and not xcap / CGWindowListCreateImage: `CGWindowListCreateImage`
//! / `CGDisplayCreateImage` are obsoleted on macOS 15+ (and xcap uses them
//! for its macOS window path); SCK is the supported API, excludes our own
//! overlay/editor windows at the compositor level, returns native-Retina
//! frames, and can capture a single window with alpha + system shadow.
//!
//! Async → sync bridge: every SCK completion handler sends into a
//! `sync_channel(1)`; the caller blocks with `recv_timeout`, so a stuck
//! replayd / TCC prompt never hangs the app (late completions are dropped).

use std::collections::HashSet;
use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::Mutex;
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGDisplayBounds, CGImage, CGMainDisplayID, kCGColorSpaceSRGB};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCRunningApplication, SCScreenshotManager, SCShareableContent,
    SCStreamConfiguration, SCWindow,
};
use rs_core::{IRect, Rect, RgbaImage};

use super::cgimage::cgimage_to_rgba;
use crate::region::{RegionPlan, blit};
use crate::{CaptureError, Result};

/// First SCK call in a process can take ~1 s (connects to replayd).
const CONTENT_TIMEOUT: Duration = Duration::from_secs(6);
const SHOT_TIMEOUT: Duration = Duration::from_secs(6);
/// `kCVPixelFormatType_32BGRA`.
const PIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `SCStreamErrorUserDeclined`.
const SC_ERROR_USER_DECLINED: isize = -3801;

/// Shareable content snapshot. SCK model objects are immutable value
/// objects and safe to share across threads.
#[derive(Clone)]
pub(crate) struct Content(Retained<SCShareableContent>);
unsafe impl Send for Content {}
unsafe impl Sync for Content {}

struct Shot(CFRetained<CGImage>);

fn map_ns_error(err: *mut NSError, what: &str) -> CaptureError {
    match unsafe { Retained::retain(err) } {
        None => CaptureError::Platform(format!("{what} failed without an error")),
        Some(e) => {
            let code = e.code();
            let domain = e.domain().to_string();
            if code == SC_ERROR_USER_DECLINED && domain.contains("ScreenCaptureKit") {
                CaptureError::PermissionDenied
            } else {
                CaptureError::Platform(format!("{what}: {} ({domain} {code})", e.localizedDescription()))
            }
        }
    }
}

fn recv<T>(rx: Receiver<Result<T>>, timeout: Duration, what: &str) -> Result<T> {
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(RecvTimeoutError::Timeout) => Err(CaptureError::Platform(format!("{what} timed out after {timeout:?}"))),
        Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Platform(format!("{what}: completion handler dropped"))),
    }
}

/// Fetch on-screen shareable content (blocking, with timeout).
pub(crate) fn fetch_content() -> Result<Content> {
    // Make sure this process has a window-server connection (CLI tools).
    let _ = CGMainDisplayID();
    let (tx, rx) = sync_channel::<Result<Content>>(1);
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let r = match unsafe { Retained::retain(content) } {
            Some(c) => Ok(Content(c)),
            None => Err(map_ns_error(err, "SCShareableContent")),
        };
        let _ = tx.try_send(r);
    });
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            false, true, &block,
        )
    };
    recv(rx, CONTENT_TIMEOUT, "SCShareableContent")
}

// ---------------------------------------------------------------------------
// Content cache (for the latency-critical display / freeze-frame path)
// ---------------------------------------------------------------------------

struct Cached {
    content: Content,
    displays: Vec<(u32, [i64; 4])>,
    has_own_app: bool,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

fn display_signature() -> Vec<(u32, [i64; 4])> {
    super::active_display_ids()
        .unwrap_or_default()
        .into_iter()
        .map(|id| {
            let b = CGDisplayBounds(id);
            (id, [b.origin.x as i64, b.origin.y as i64, b.size.width as i64, b.size.height as i64])
        })
        .collect()
}

fn own_app(content: &Content) -> Option<Retained<SCRunningApplication>> {
    let pid = super::own_pid();
    unsafe { content.0.applications() }.iter().find(|a| unsafe { a.processID() } == pid)
}

/// Shareable content good enough for display captures: cached as long as
/// the display topology is unchanged and (if we own windows now) the
/// snapshot knows our app so its windows can be excluded.
pub(crate) fn content_for_displays() -> Result<Content> {
    let sig = display_signature();
    let own_windows = super::own_process_has_windows();
    if let Ok(g) = CACHE.lock()
        && let Some(c) = g.as_ref()
        && c.displays == sig
        && (c.has_own_app || !own_windows)
    {
        return Ok(c.content.clone());
    }
    let content = fetch_content()?;
    let has_own_app = own_app(&content).is_some();
    if let Ok(mut g) = CACHE.lock() {
        *g = Some(Cached { content: content.clone(), displays: sig, has_own_app });
    }
    Ok(content)
}

pub(crate) fn invalidate_cache() {
    if let Ok(mut g) = CACHE.lock() {
        *g = None;
    }
}

// ---------------------------------------------------------------------------
// Filters / configuration / screenshots
// ---------------------------------------------------------------------------

/// Filter for one display. `exclude`: None → exclude our own app (live, so
/// windows opened later are excluded too); Some(ids) → exclude exactly those
/// windows plus all of our own windows in the snapshot.
fn display_filter(content: &Content, display_id: u32, exclude: Option<&HashSet<u32>>) -> Result<Retained<SCContentFilter>> {
    let display = unsafe { content.0.displays() }
        .iter()
        .find(|d| unsafe { d.displayID() } == display_id)
        .ok_or(CaptureError::NotFound)?;
    let filter = match exclude {
        None => {
            let apps: Vec<Retained<SCRunningApplication>> = own_app(content).into_iter().collect();
            unsafe {
                SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&apps),
                    &NSArray::new(),
                )
            }
        }
        Some(ids) => {
            let pid = super::own_pid();
            let windows: Vec<Retained<SCWindow>> = unsafe { content.0.windows() }
                .iter()
                .filter(|w| unsafe {
                    ids.contains(&w.windowID()) || w.owningApplication().is_some_and(|a| a.processID() == pid)
                })
                .collect();
            unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&windows),
                )
            }
        }
    };
    Ok(filter)
}

fn make_config(width: u32, height: u32, source_rect: Option<Rect>) -> Retained<SCStreamConfiguration> {
    unsafe {
        let c = SCStreamConfiguration::new();
        c.setWidth(width.max(1) as usize);
        c.setHeight(height.max(1) as usize);
        if let Some(r) = source_rect {
            c.setSourceRect(CGRect::new(
                CGPoint::new(r.x as f64, r.y as f64),
                CGSize::new(r.width as f64, r.height as f64),
            ));
        }
        c.setShowsCursor(false);
        c.setPixelFormat(PIXEL_FORMAT_BGRA);
        c.setColorSpaceName(kCGColorSpaceSRGB);
        c.setCaptureResolution(SCCaptureResolutionType::Best);
        c
    }
}

fn start_shot(filter: &SCContentFilter, config: &SCStreamConfiguration) -> Receiver<Result<Shot>> {
    let (tx, rx) = sync_channel::<Result<Shot>>(1);
    let block = RcBlock::new(move |img: *mut CGImage, err: *mut NSError| {
        let r = match NonNull::new(img) {
            Some(p) => Ok(Shot(unsafe { CFRetained::retain(p) })),
            None => Err(map_ns_error(err, "SCScreenshotManager")),
        };
        let _ = tx.try_send(r);
    });
    unsafe { SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(filter, config, Some(&block)) };
    rx
}

fn shot_to_rgba(rx: Receiver<Result<Shot>>) -> Result<RgbaImage> {
    let shot = recv(rx, SHOT_TIMEOUT, "SCScreenshotManager")?;
    cgimage_to_rgba(&shot.0)
}

/// Capture every part of a region plan (in parallel) and compose them.
/// `exclude`: see [`display_filter`].
pub(crate) fn capture_plan(plan: &RegionPlan, content: &Content, exclude: Option<&HashSet<u32>>) -> Result<RgbaImage> {
    let mut pending = Vec::with_capacity(plan.parts.len());
    for part in &plan.parts {
        let filter = display_filter(content, part.display_id, exclude)?;
        let cfg = make_config(part.dest.width, part.dest.height, Some(part.source_rect));
        let rx = start_shot(&filter, &cfg);
        pending.push((part.dest, rx, filter, cfg));
    }
    let mut out = vec![0u8; plan.width as usize * plan.height as usize * 4];
    let mut first_err = None;
    for (dest, rx, _filter, _cfg) in pending {
        match shot_to_rgba(rx) {
            Ok(img) => blit(&img, IRect::new(0, 0, img.width, img.height).to_rect(), &mut out, plan.width, dest),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    RgbaImage::new(plan.width, plan.height, out).ok_or(CaptureError::Platform("region size".into()))
}

/// Capture whole displays in parallel. Each entry: (display id, image).
pub(crate) fn capture_displays(content: &Content, displays: &[crate::DisplayInfo]) -> Result<Vec<RgbaImage>> {
    let mut pending = Vec::with_capacity(displays.len());
    for d in displays {
        let filter = display_filter(content, d.id, None)?;
        let w = (d.frame.width * d.scale_factor).round() as u32;
        let h = (d.frame.height * d.scale_factor).round() as u32;
        let cfg = make_config(w, h, Some(Rect::new(0.0, 0.0, d.frame.width, d.frame.height)));
        let rx = start_shot(&filter, &cfg);
        pending.push((rx, filter, cfg));
    }
    // Wait for all shots, then convert in parallel (vImage is fast, but
    // 5K frames are ~60 MB each).
    let mut shots = Vec::with_capacity(pending.len());
    for (rx, _f, _c) in pending {
        shots.push(recv(rx, SHOT_TIMEOUT, "SCScreenshotManager")?);
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = shots.iter().map(|shot| s.spawn(move || cgimage_to_rgba(&shot.0))).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(CaptureError::Platform("conversion thread panicked".into()))))
            .collect()
    })
}

/// Single-window capture with alpha (`desktopIndependentWindow`).
/// Returns (image, px/pt).
pub(crate) fn capture_window_alpha(content: &Content, window_id: u32, include_shadow: bool) -> Result<(RgbaImage, f32)> {
    let win = unsafe { content.0.windows() }
        .iter()
        .find(|w| unsafe { w.windowID() } == window_id)
        .ok_or(CaptureError::NotFound)?;
    let filter = unsafe { SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &win) };
    let mut scale = unsafe { filter.pointPixelScale() };
    if !(scale.is_finite() && scale > 0.0) {
        scale = 2.0;
    }
    let mut rect = unsafe { filter.contentRect() };
    if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
        rect = unsafe { win.frame() };
    }
    let w = (rect.size.width * scale as f64).round() as u32;
    let h = (rect.size.height * scale as f64).round() as u32;
    let cfg = make_config(w, h, None);
    unsafe {
        cfg.setIgnoreShadowsSingleWindow(!include_shadow);
        cfg.setShouldBeOpaque(false);
        cfg.setIgnoreGlobalClipSingleWindow(true);
    }
    let img = shot_to_rgba(start_shot(&filter, &cfg))?;
    Ok((img, scale))
}
