//! # rs-capture — screen capture, clipboard, image import
//!
//! ## Coordinate convention
//! Display and window frames and capture regions are in **global screen
//! points**, macOS "Quartz/CG global display" convention: origin at the
//! top-left of the *main* display, +y **down**, 1 unit = 1 point. (This is
//! what `CGDisplayBounds` / `CGWindowListCopyWindowInfo` report and what
//! GPUI's display bounds use.) Returned images are in physical pixels;
//! `Captured::scale_factor` = px per pt.
//!
//! ## Threading
//! All functions are **blocking** and callable from any thread. Call capture
//! functions from `cx.background_executor()` — never block GPUI's main
//! thread (captures take ~20–150 ms; the first ScreenCaptureKit call in a
//! process up to ~1 s, see [`prepare_capture`]). Every ScreenCaptureKit
//! completion is bridged with a timeout (6 s), so nothing can hang forever.
//!
//! ## Backends (macOS)
//! * Permission: `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`.
//!   Capture functions return `Err(PermissionDenied)` up front when the
//!   preflight fails (without permission macOS would hand back
//!   wallpaper-only frames).
//! * Enumeration: CoreGraphics (`CGGetActiveDisplayList`, display modes,
//!   `CGWindowListCopyWindowInfo`) — no permission needed.
//! * Capture: ScreenCaptureKit `SCScreenshotManager` (macOS 14+), sRGB
//!   output, own windows excluded; `screencapture` CLI only as fallback.
//! * Clipboard: `NSPasteboard` (PNG + TIFF). Import: `image` crate, ImageIO
//!   fallback (HEIC …).

use std::path::{Path, PathBuf};

use rs_core::{BeautifySettings, CaptureSource, Document, DocumentMetadata, Rect, RgbaImage};
use thiserror::Error;

#[cfg(target_os = "macos")]
mod macos;
mod png_meta;
mod region;

pub use png_meta::{dpi_to_scale, png_dpi};
pub use region::{
    RegionPart, RegionPlan, crop_region_from_displays, display_at_point, plan_region, point_to_pixel, sample_grid,
    sample_pixel, window_at_point,
};

/// Capture errors.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// Screen Recording permission not granted (System Settings → Privacy &
    /// Security → Screen & System Audio Recording).
    #[error("screen recording permission denied")]
    PermissionDenied,
    /// User cancelled (Esc in overlay / system UI).
    #[error("capture cancelled")]
    Cancelled,
    #[error("display or window not found")]
    NotFound,
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("clipboard: {0}")]
    Clipboard(String),
    #[error("clipboard has no image")]
    NoClipboardImage,
    #[error("platform error: {0}")]
    Platform(String),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = CaptureError> = std::result::Result<T, E>;

/// A display (monitor).
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    /// `CGDirectDisplayID`. Equals GPUI's `DisplayId` numeric value on macOS
    /// (`u32::from(display_id)`).
    pub id: u32,
    pub name: String,
    /// Global points (see crate docs).
    pub frame: Rect,
    /// Physical px per point (2.0 on Retina).
    pub scale_factor: f32,
    pub is_main: bool,
}

/// An on-screen window (for window capture / hover highlight).
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    /// `CGWindowID`.
    pub id: u32,
    pub owner_pid: i32,
    pub app_name: String,
    /// Window title (may be empty without Screen Recording permission).
    pub title: String,
    /// Global points.
    pub frame: Rect,
    /// CG window layer (0 = normal app windows).
    pub layer: i32,
    pub display_id: Option<u32>,
}

/// Window capture options.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowCaptureOptions {
    /// Include the system window shadow (transparent margins unless
    /// `transparent_background` is false, then flattened onto the desktop).
    pub include_shadow: bool,
    /// Keep the area outside the window's rounded corners transparent
    /// (otherwise it's filled with the desktop behind it).
    pub transparent_background: bool,
}

/// A captured / imported image plus provenance.
#[derive(Debug, Clone)]
pub struct Captured {
    pub image: RgbaImage,
    /// Physical px per logical point (Retina 2.0; files/clipboard 1.0 unless
    /// DPI metadata says otherwise).
    pub scale_factor: f32,
    pub source: CaptureSource,
    /// Captured area in global points (None for files/clipboard).
    pub frame: Option<Rect>,
}

impl Captured {
    /// Build a new Document (created now) with the given beautify settings
    /// (usually `Preferences::last_beautify`).
    pub fn into_document(self, beautify: BeautifySettings) -> Document {
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let title = match &self.source {
            CaptureSource::File { path } => path.file_stem().map(|s| s.to_string_lossy().into_owned()),
            _ => None,
        };
        Document::new(
            self.image,
            DocumentMetadata {
                created_at_ms,
                source: self.source,
                scale_factor: self.scale_factor,
                title,
                project_path: None,
            },
        )
        .with_beautify(beautify)
    }
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// `CGPreflightScreenCaptureAccess()` — true if Screen Recording is granted.
/// Never prompts. Note: macOS caches the answer per process; after the user
/// grants access the app usually has to be relaunched to see `true`.
pub fn has_screen_capture_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::preflight()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// `CGRequestScreenCaptureAccess()` — shows the system prompt the first time
/// (and adds the app to the Screen Recording list); returns the current
/// grant state. Later calls don't prompt again — use
/// [`open_screen_recording_settings`].
pub fn request_screen_capture_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::request()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Open System Settings at Privacy → Screen Recording.
pub fn open_screen_recording_settings() -> Result<()> {
    std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn()?;
    Ok(())
}

/// Env var: set to `1` to skip the permission preflight gate (debugging
/// ScreenCaptureKit error paths from an unpermitted process).
pub const SKIP_PREFLIGHT_ENV: &str = "RS_CAPTURE_SKIP_PREFLIGHT";

#[cfg(target_os = "macos")]
fn ensure_permission() -> Result<()> {
    if macos::preflight() || std::env::var_os(SKIP_PREFLIGHT_ENV).is_some_and(|v| v == "1") {
        Ok(())
    } else {
        Err(CaptureError::PermissionDenied)
    }
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

/// All active displays, main display first. Frames in global points,
/// `scale_factor` per display (mixed DPI correct). Needs no permission.
/// Display names come from `NSScreen` and are refreshed whenever this is
/// called on the main thread (elsewhere the last known names, or
/// "Built-in Display" / "Display `id`", are used).
pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    #[cfg(target_os = "macos")]
    {
        macos::list_displays()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CaptureError::Unsupported("list_displays on this platform".into()))
    }
}

/// On-screen windows **front-to-back** (index 0 = top-most; the Vec order
/// is the z-order): layer 0 only, excluding RapidSnap's own windows,
/// invisible (alpha 0) windows, windows smaller than 40×40 pt, windows not
/// on any display, and system owners (Dock, Window Server, Control Centre…).
/// Needs no permission, but `title` is empty without Screen Recording.
/// Pair with [`window_at_point`] for hover highlighting.
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    #[cfg(target_os = "macos")]
    {
        macos::list_windows()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CaptureError::Unsupported("list_windows on this platform".into()))
    }
}

/// Warm up ScreenCaptureKit (connects to the capture service and caches
/// the shareable-content snapshot) so the first hotkey capture is fast.
/// Blocking (can take ~0.2–1 s the first time); call once at startup from a
/// background thread, and again after display configuration changes
/// (optional — changes are detected automatically).
pub fn prepare_capture() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        ensure_permission()?;
        macos::sck::invalidate_cache();
        macos::sck::content_for_displays().map(|_| ())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Capture a whole display at native resolution (excluding RapidSnap's own
/// windows, e.g. the overlay).
pub fn capture_display(display_id: u32) -> Result<Captured> {
    let displays = list_displays()?;
    let d = displays.iter().find(|d| d.id == display_id).ok_or(CaptureError::NotFound)?;
    let mut cap = capture_region(d.frame, Some(display_id))?;
    cap.source = CaptureSource::Fullscreen { display_id: Some(display_id) };
    cap.frame = Some(d.frame);
    Ok(cap)
}

/// Capture every display (used to "freeze" the screen under the region
/// overlay: the overlay shows these images and the selection is cropped from
/// them with [`crop_to_region`] / [`crop_region_from_displays`], so the
/// result is pixel-identical to what the user saw).
///
/// All displays are captured concurrently from a cached ScreenCaptureKit
/// snapshot (see [`prepare_capture`]). Each image is at that display's own
/// scale; `Captured::frame` = display frame. Main display first.
pub fn capture_all_displays() -> Result<Vec<(DisplayInfo, Captured)>> {
    #[cfg(target_os = "macos")]
    {
        ensure_permission()?;
        let displays = list_displays()?;
        let content = macos::sck::content_for_displays()?;
        let images = match macos::sck::capture_displays(&content, &displays) {
            Ok(v) => v,
            Err(CaptureError::NotFound) => {
                // Stale snapshot (display hot-plugged mid-flight): refetch once.
                macos::sck::invalidate_cache();
                let content = macos::sck::content_for_displays()?;
                macos::sck::capture_displays(&content, &displays)?
            }
            Err(e) => return Err(e),
        };
        Ok(displays
            .into_iter()
            .zip(images)
            .map(|(d, image)| {
                let scale = if d.frame.width > 0.0 { image.width as f32 / d.frame.width } else { d.scale_factor };
                let cap = Captured {
                    image,
                    scale_factor: scale,
                    source: CaptureSource::Fullscreen { display_id: Some(d.id) },
                    frame: Some(d.frame),
                };
                (d, cap)
            })
            .collect())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CaptureError::Unsupported("capture_all_displays on this platform".into()))
    }
}

/// Capture a region in global points.
///
/// * `display_id = Some(id)`: the region is clipped to that display.
/// * `None`: the region may span several displays; every covered display is
///   captured and composited (mixed DPI → output at the highest scale, lower
///   DPI parts upscaled; uncovered gaps transparent).
///
/// The rect is snapped outward to whole output pixels (`Captured::frame` is
/// the snapped rect). Falls back to `/usr/sbin/screencapture -R` if
/// ScreenCaptureKit fails for a reason other than permission.
pub fn capture_region(rect: Rect, display_id: Option<u32>) -> Result<Captured> {
    let displays = list_displays()?;
    let plan = plan_region(rect, &displays, display_id)?;
    #[cfg(target_os = "macos")]
    {
        ensure_permission()?;
        let live = macos::sck::content_for_displays().and_then(|c| macos::sck::capture_plan(&plan, &c, None));
        let image = match live {
            Ok(img) => img,
            Err(CaptureError::PermissionDenied) => return Err(CaptureError::PermissionDenied),
            Err(e) => {
                log::warn!("ScreenCaptureKit region capture failed ({e}); falling back to screencapture");
                macos::sck::invalidate_cache();
                return capture_region_cli(plan.rect, plan.scale_factor);
            }
        };
        Ok(Captured { image, scale_factor: plan.scale_factor, source: CaptureSource::Region, frame: Some(plan.rect) })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = plan;
        Err(CaptureError::Unsupported("capture_region on this platform".into()))
    }
}

/// Fallback: `screencapture -x -R x,y,w,h` (global points). Includes our own
/// windows (hide the overlay first).
#[cfg(target_os = "macos")]
fn capture_region_cli(rect: Rect, scale: f32) -> Result<Captured> {
    let path = temp_png_path("region");
    let arg = format!("-R{},{},{},{}", rect.x, rect.y, rect.width, rect.height);
    let out = std::process::Command::new("/usr/sbin/screencapture").args(["-x", "-tpng", &arg]).arg(&path).output()?;
    if !out.status.success() || !path.exists() {
        let _ = std::fs::remove_file(&path);
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if msg.contains("could not create image") || !has_screen_capture_permission() {
            CaptureError::PermissionDenied
        } else {
            CaptureError::Platform(format!("screencapture failed: {msg}"))
        });
    }
    let cap = load_image_file(&path);
    let _ = std::fs::remove_file(&path);
    let mut cap = cap?;
    cap.scale_factor = if rect.width > 0.0 { cap.image.width as f32 / rect.width } else { scale };
    cap.source = CaptureSource::Region;
    cap.frame = Some(rect);
    Ok(cap)
}

fn temp_png_path(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("rapidsnap-{tag}-{}-{n}.png", std::process::id()))
}

/// Capture a single window by `CGWindowID`.
///
/// * `transparent_background || include_shadow`: ScreenCaptureKit
///   single-window capture (window pixels only, even if covered by other
///   windows; area outside the rounded corners / around the shadow is
///   transparent). Fully transparent borders are trimmed. With
///   `include_shadow && !transparent_background` the result is flattened
///   onto the on-screen pixels behind the window (opaque, shadow kept).
/// * neither: the window's frame as composited on screen, with every
///   window **above** it (and RapidSnap's own windows) removed, so corners
///   show the desktop/windows behind it.
///
/// `Captured::frame` = window frame (expanded to the image when the shadow
/// makes it larger); source = `Window { app_name, title }`.
pub fn capture_window(window_id: u32, opts: WindowCaptureOptions) -> Result<Captured> {
    #[cfg(target_os = "macos")]
    {
        ensure_permission()?;
        let info = macos::find_window(window_id).ok_or(CaptureError::NotFound)?;
        let source = CaptureSource::Window {
            app_name: (!info.app_name.is_empty()).then(|| info.app_name.clone()),
            title: (!info.title.is_empty()).then(|| info.title.clone()),
        };
        let content = macos::sck::fetch_content()?;
        if opts.include_shadow || opts.transparent_background {
            let (img, scale) = macos::sck::capture_window_alpha(&content, window_id, opts.include_shadow)?;
            let (mut img, _) = region::trim_transparent(&img);
            let (w_pt, h_pt) = (img.width as f32 / scale, img.height as f32 / scale);
            let c = info.frame.center();
            let frame = if w_pt > info.frame.width + 0.5 || h_pt > info.frame.height + 0.5 {
                Rect::new(c.x - w_pt / 2.0, c.y - h_pt / 2.0, w_pt, h_pt)
            } else {
                info.frame
            };
            if !opts.transparent_background {
                // Shadow on an opaque background: flatten the shadowed window
                // onto what's behind it on screen (the window itself and
                // everything above it removed).
                let behind = list_displays().and_then(|displays| {
                    let plan = plan_region(frame, &displays, None)?;
                    let mut exclude: std::collections::HashSet<u32> =
                        macos::window_ids_above(window_id).into_iter().collect();
                    exclude.insert(window_id);
                    macos::sck::capture_plan(&plan, &content, Some(&exclude))
                });
                match behind {
                    Ok(bg) => img = region::composite_over(&img, &bg),
                    Err(e) => log::warn!("capture_window: background behind shadow unavailable ({e}); keeping alpha"),
                }
            }
            Ok(Captured { image: img, scale_factor: scale, source, frame: Some(frame) })
        } else {
            let displays = list_displays()?;
            let plan = plan_region(info.frame, &displays, None)?;
            let above: std::collections::HashSet<u32> = macos::window_ids_above(window_id).into_iter().collect();
            let img = macos::sck::capture_plan(&plan, &content, Some(&above))?;
            Ok(Captured { image: img, scale_factor: plan.scale_factor, source, frame: Some(plan.rect) })
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window_id, opts);
        Err(CaptureError::Unsupported("capture_window on this platform".into()))
    }
}

/// Crop a display capture to `rect` (global points). `display` is the frame
/// the capture came from. Rounds outward to whole physical pixels.
/// (Single display; for selections spanning displays use
/// [`crop_region_from_displays`].)
pub fn crop_to_region(display_capture: &Captured, display: &DisplayInfo, rect: Rect) -> Result<Captured> {
    let s = display_capture.scale_factor;
    let img = &display_capture.image;
    let local = rect.translate(-display.frame.x, -display.frame.y);
    let x0 = (local.x * s).floor().max(0.0) as u32;
    let y0 = (local.y * s).floor().max(0.0) as u32;
    let x1 = ((local.right() * s).ceil().max(0.0) as u32).min(img.width);
    let y1 = ((local.bottom() * s).ceil().max(0.0) as u32).min(img.height);
    if x1 <= x0 || y1 <= y0 || !img.is_loaded() {
        return Err(CaptureError::NotFound);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in y0..y1 {
        let start = ((y * img.width + x0) * 4) as usize;
        out.extend_from_slice(&img.pixels[start..start + w as usize * 4]);
    }
    Ok(Captured {
        image: RgbaImage::new(w, h, out).ok_or(CaptureError::Platform("crop size".into()))?,
        scale_factor: s,
        source: CaptureSource::Region,
        frame: Some(rect),
    })
}

/// v1 behaviour / fallback: run the system `screencapture -i` UI
/// (`-i` interactive, `-x` no sound; `window == true` adds `-W` to start in
/// window mode). Blocks until the user finishes. Esc → `Cancelled`.
/// `scale_factor` comes from the PNG's pHYs (144 dpi → 2.0), defaulting to
/// 2.0 when absent.
pub fn capture_interactive_system(window: bool) -> Result<Captured> {
    let path = temp_png_path("interactive");
    let _ = std::fs::remove_file(&path);
    let mut cmd = std::process::Command::new("/usr/sbin/screencapture");
    cmd.arg("-i").arg("-x");
    if window {
        cmd.arg("-W");
    }
    let status = cmd.arg(&path).status()?;
    if !status.success() || !path.exists() {
        return Err(CaptureError::Cancelled);
    }
    let bytes = std::fs::read(&path);
    let _ = std::fs::remove_file(&path);
    let bytes = bytes?;
    let mut cap = load_image_bytes(&bytes)?;
    cap.source = if window { CaptureSource::Window { app_name: None, title: None } } else { CaptureSource::Region };
    cap.scale_factor = png_meta::png_dpi(&bytes).map(png_meta::dpi_to_scale).unwrap_or(2.0);
    Ok(cap)
}

/// Path of the current desktop wallpaper image for a display (NSWorkspace
/// `desktopImageURLForScreen:`; `None` = main display).
///
/// Callable from **any thread**: on the main thread the AppKit lookup is
/// direct; elsewhere it hops to the main queue (2 s timeout, so it needs a
/// running main run loop — GPUI has one). If the wallpaper is in a format
/// the `image` crate can't decode (HEIC — all of Apple's dynamic
/// wallpapers), it is transcoded once to a cached JPEG (≤ 3840 px) in the
/// temp dir and that path is returned — so prefer calling it from a
/// background thread.
pub fn desktop_wallpaper_path(display_id: Option<u32>) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut path = macos::desktop_wallpaper_url(display_id)?;
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("madesktop")) {
            // Aerial/"mobile asset" wallpapers: an XML plist pointing at a
            // thumbnail HEIC (fine for a blurred background).
            path = madesktop_thumbnail(&path).ok_or(CaptureError::NotFound)?;
        }
        if path.is_dir() || !path.exists() {
            return Err(CaptureError::NotFound);
        }
        if image::ImageFormat::from_path(&path).is_ok_and(|f| f.can_read()) {
            return Ok(path);
        }
        transcode_wallpaper(&path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = display_id;
        Err(CaptureError::Unsupported("desktop_wallpaper_path on this platform".into()))
    }
}

/// `thumbnailPath` from an XML `.madesktop` plist.
#[cfg(target_os = "macos")]
fn madesktop_thumbnail(path: &Path) -> Option<PathBuf> {
    let xml = std::fs::read_to_string(path).ok()?;
    let after_key = &xml[xml.find("<key>thumbnailPath</key>")? + "<key>thumbnailPath</key>".len()..];
    let start = after_key.find("<string>")? + "<string>".len();
    let end = after_key[start..].find("</string>")? + start;
    let p = after_key[start..end].trim().replace("&amp;", "&").replace("&apos;", "'").replace("&quot;", "\"");
    let p = PathBuf::from(p);
    p.exists().then_some(p)
}

#[cfg(target_os = "macos")]
fn transcode_wallpaper(path: &Path) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(path)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    let out = std::env::temp_dir().join(format!("rapidsnap-wallpaper-{:016x}.jpg", h.finish()));
    if out.exists() {
        return Ok(out);
    }
    let cap = load_image_file(path)?;
    let img = image::RgbaImage::from_raw(cap.image.width, cap.image.height, cap.image.pixels.as_ref().clone())
        .ok_or(CaptureError::Platform("wallpaper buffer".into()))?;
    let mut img = image::DynamicImage::ImageRgba8(img);
    const MAX_SIDE: u32 = 3840;
    if img.width().max(img.height()) > MAX_SIDE {
        img = img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle);
    }
    let rgb = img.to_rgb8();
    let tmp = out.with_extension("jpg.tmp");
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, 90).encode_image(&rgb)?;
    }
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Lower-case file extensions accepted by [`load_image_file`]
/// (HEIC/HEIF via ImageIO on macOS).
pub const SUPPORTED_IMPORT_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif", "heic", "heif"];

/// Upper bound on decoded image side (px) — guards against decompression
/// bombs; larger images return `Unsupported`.
pub const MAX_IMPORT_SIDE: u32 = 32_768;

/// Decode an image file into a `Captured` (source File; scale from PNG pHYs
/// — 144 dpi → 2.0 — else 1.0).
pub fn load_image_file(path: &Path) -> Result<Captured> {
    let bytes = std::fs::read(path)?;
    let mut cap = load_image_bytes(&bytes)?;
    cap.source = CaptureSource::File { path: path.to_path_buf() };
    Ok(cap)
}

fn decode_with_image_crate(bytes: &[u8]) -> Result<image::DynamicImage> {
    use image::ImageDecoder as _;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let (w, h) = decoder.dimensions();
    if w > MAX_IMPORT_SIDE || h > MAX_IMPORT_SIDE {
        return Err(CaptureError::Unsupported(format!("image too large: {w}x{h}")));
    }
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Decode encoded image bytes (format sniffed; EXIF orientation applied;
/// ImageIO fallback on macOS for HEIC and anything the `image` crate
/// rejects). Scale factor from PNG pHYs, else 1.0.
pub fn load_image_bytes(bytes: &[u8]) -> Result<Captured> {
    #[allow(unused_mut)]
    let mut scale = png_meta::png_dpi(bytes).map(png_meta::dpi_to_scale).unwrap_or(1.0);
    let image = match decode_with_image_crate(bytes) {
        Ok(img) => {
            let img = img.to_rgba8();
            let (w, h) = img.dimensions();
            RgbaImage::new(w, h, img.into_raw()).ok_or(CaptureError::Platform("decode size".into()))?
        }
        Err(e @ CaptureError::Unsupported(_)) => return Err(e),
        Err(e) => {
            #[cfg(target_os = "macos")]
            {
                match macos::imageio::decode(bytes) {
                    Ok(d) => {
                        if let Some(dpi) = d.dpi {
                            scale = png_meta::dpi_to_scale(dpi);
                        }
                        apply_orientation(d.image, d.orientation)?
                    }
                    Err(_) => return Err(e),
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                return Err(e);
            }
        }
    };
    Ok(Captured { image, scale_factor: scale, source: CaptureSource::Unknown, frame: None })
}

#[cfg(target_os = "macos")]
fn apply_orientation(img: RgbaImage, exif: u8) -> Result<RgbaImage> {
    let Some(o) = image::metadata::Orientation::from_exif(exif) else { return Ok(img) };
    if o == image::metadata::Orientation::NoTransforms {
        return Ok(img);
    }
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.pixels.as_ref().clone())
        .ok_or(CaptureError::Platform("orientation buffer".into()))?;
    let mut d = image::DynamicImage::ImageRgba8(buf);
    d.apply_orientation(o);
    let d = d.to_rgba8();
    let (w, h) = d.dimensions();
    RgbaImage::new(w, h, d.into_raw()).ok_or(CaptureError::Platform("orientation size".into()))
}

/// Encode as PNG (fast compression) with a pHYs chunk for `scale_factor`
/// (> 1 → `scale × 72` dpi so apps paste Retina captures at point size).
pub fn encode_png(img: &RgbaImage, scale_factor: f32) -> Result<Vec<u8>> {
    use image::ImageEncoder as _;
    if !img.is_loaded() {
        return Err(CaptureError::Unsupported("image pixels not loaded".into()));
    }
    let mut png = Vec::with_capacity(img.pixels.len() / 3);
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(&img.pixels, img.width, img.height, image::ExtendedColorType::Rgba8)?;
    Ok(png_meta::with_scale(png, scale_factor))
}

// ---------------------------------------------------------------------------
// Clipboard. macOS: NSPasteboard directly (PNG + TIFF representations —
// pastes correctly into browsers, Slack, Figma, Keynote, Mail, Preview).
// Other platforms: arboard. All functions work from any thread.
// (GPUI's `cx.write_to_clipboard` is an alternative inside GPUI code, but it
// writes PNG only.)
// ---------------------------------------------------------------------------

/// Read an image from the clipboard: PNG, TIFF, JPEG/HEIC/WebP/GIF data, or
/// a copied image *file* (Finder ⌘C → source `File`). `Err(NoClipboardImage)`
/// if none. Scale from PNG pHYs (else 1.0).
pub fn clipboard_image() -> Result<Captured> {
    #[cfg(target_os = "macos")]
    {
        match macos::pasteboard::read_image() {
            Some(macos::pasteboard::PasteboardImage::Data(bytes)) => {
                let mut cap = load_image_bytes(&bytes)?;
                cap.source = CaptureSource::Clipboard;
                Ok(cap)
            }
            Some(macos::pasteboard::PasteboardImage::File(path)) => load_image_file(&path),
            None => Err(CaptureError::NoClipboardImage),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut cb = arboard::Clipboard::new().map_err(|e| CaptureError::Clipboard(e.to_string()))?;
        let img = cb.get_image().map_err(|e| match e {
            arboard::Error::ContentNotAvailable => CaptureError::NoClipboardImage,
            e => CaptureError::Clipboard(e.to_string()),
        })?;
        let (w, h) = (img.width as u32, img.height as u32);
        Ok(Captured {
            image: RgbaImage::new(w, h, img.bytes.into_owned())
                .ok_or(CaptureError::Clipboard("unexpected clipboard image layout".into()))?,
            scale_factor: 1.0,
            source: CaptureSource::Clipboard,
            frame: None,
        })
    }
}

/// Put an image on the clipboard (PNG + TIFF, 72 dpi). For Retina captures
/// prefer [`copy_image_to_clipboard_with_scale`].
pub fn copy_image_to_clipboard(img: &RgbaImage) -> Result<()> {
    copy_image_to_clipboard_with_scale(img, 1.0)
}

/// Put an image on the clipboard tagged with `scale_factor × 72` dpi, so a
/// 2× capture pastes at its point size in apps that honour DPI (Pages,
/// Keynote, Notes) while pixel-based apps still get every pixel.
pub fn copy_image_to_clipboard_with_scale(img: &RgbaImage, scale_factor: f32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let png = encode_png(img, scale_factor)?;
        macos::pasteboard::write_png(&png)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = scale_factor;
        let mut cb = arboard::Clipboard::new().map_err(|e| CaptureError::Clipboard(e.to_string()))?;
        cb.set_image(arboard::ImageData {
            width: img.width as usize,
            height: img.height as usize,
            bytes: std::borrow::Cow::Borrowed(&img.pixels),
        })
        .map_err(|e| CaptureError::Clipboard(e.to_string()))
    }
}

/// Put already-encoded PNG bytes on the clipboard (e.g. `rs_render::export`
/// output — avoids re-encoding). A TIFF representation is added too.
pub fn copy_png_to_clipboard(png: &[u8]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::pasteboard::write_png(png)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let cap = load_image_bytes(png)?;
        copy_image_to_clipboard(&cap.image)
    }
}

/// Put a file reference on the clipboard (pasting into Finder copies the
/// file; Mail/Slack attach it). macOS only.
pub fn copy_file_to_clipboard(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::pasteboard::write_file_url(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err(CaptureError::Unsupported("copy_file_to_clipboard on this platform".into()))
    }
}

/// Put text on the clipboard (e.g. OCR result).
pub fn copy_text_to_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::pasteboard::write_text(text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut cb = arboard::Clipboard::new().map_err(|e| CaptureError::Clipboard(e.to_string()))?;
        cb.set_text(text.to_owned()).map_err(|e| CaptureError::Clipboard(e.to_string()))
    }
}

/// A copy of everything on the clipboard (every item, every type), taken
/// with [`ClipboardSnapshot::take`] and put back with
/// [`ClipboardSnapshot::restore`]. Used by self-tests so a copy round-trip
/// check doesn't clobber the user's clipboard.
#[derive(Debug, Clone, Default)]
pub struct ClipboardSnapshot {
    /// Per pasteboard item: (UTI, bytes).
    pub items: Vec<Vec<(String, Vec<u8>)>>,
}

impl ClipboardSnapshot {
    /// Read the whole clipboard (macOS; empty elsewhere).
    pub fn take() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self { items: macos::pasteboard::snapshot() }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::default()
        }
    }

    /// Replace the clipboard with the snapshot.
    pub fn restore(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            macos::pasteboard::restore(&self.items)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(CaptureError::NotImplemented("ClipboardSnapshot::restore"))
        }
    }

    /// Total bytes held.
    pub fn byte_len(&self) -> usize {
        self.items.iter().flatten().map(|(_, d)| d.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_png_bytes() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let cap = load_image_bytes(&png).unwrap();
        assert_eq!((cap.image.width, cap.image.height), (3, 2));
        let doc = cap.into_document(BeautifySettings::default());
        assert_eq!(doc.content_size(), rs_core::Size::new(3.0, 2.0));
    }

    #[test]
    fn png_phys_sets_scale_and_clipboard_encoding_roundtrips() {
        let img = RgbaImage::filled(4, 3, [9, 8, 7, 200]);
        let png = encode_png(&img, 2.0).unwrap();
        let cap = load_image_bytes(&png).unwrap();
        assert_eq!(cap.scale_factor, 2.0);
        assert_eq!(cap.image.pixels, img.pixels);
        assert_eq!(load_image_bytes(&encode_png(&img, 1.0).unwrap()).unwrap().scale_factor, 1.0);
        assert!(load_image_bytes(b"nope").is_err());
    }

    #[test]
    fn crop_region() {
        let cap = Captured {
            image: RgbaImage::filled(200, 100, [5, 5, 5, 255]),
            scale_factor: 2.0,
            source: CaptureSource::Fullscreen { display_id: Some(1) },
            frame: None,
        };
        let d = DisplayInfo {
            id: 1,
            name: "x".into(),
            frame: Rect::new(100.0, 0.0, 100.0, 50.0),
            scale_factor: 2.0,
            is_main: false,
        };
        let c = crop_to_region(&cap, &d, Rect::new(110.0, 10.0, 20.0, 5.0)).unwrap();
        assert_eq!((c.image.width, c.image.height), (40, 10));
    }
}
