//! Debug aid: capture the editor's own window to a PNG (no Screen
//! Recording permission is needed for a process's own windows). Used by
//! the self-test when `RAPIDSNAP_EDITOR_SELFTEST_SHOT=1`.

use std::path::Path;

#[cfg(target_os = "macos")]
pub fn capture_window(window: &gpui_kit::Window, path: &Path) -> anyhow::Result<()> {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSView};
    use objc2_core_graphics::CGImage;
    use objc2_foundation::NSDictionary;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRectRaw {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCreateImage(bounds: CGRectRaw, list_option: u32, window_id: u32, image_option: u32) -> *mut CGImage;
    }
    let handle = HasWindowHandle::window_handle(window).map_err(|e| anyhow::anyhow!("window handle: {e}"))?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { anyhow::bail!("not an AppKit window") };
    // SAFETY: GPUI's live NSView; main thread.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let ns_window = view.window().ok_or_else(|| anyhow::anyhow!("no window"))?;
    let id = ns_window.windowNumber() as u32;
    // CGRectNull = infinite origin; kCGWindowListOptionIncludingWindow = 8;
    // kCGWindowImageBoundsIgnoreFraming = 1.
    let null = CGRectRaw { x: f64::INFINITY, y: f64::INFINITY, w: 0.0, h: 0.0 };
    // SAFETY: plain C call; returns a +1 CGImageRef or null.
    let raw = unsafe { CGWindowListCreateImage(null, 8, id, 1) };
    let Some(ptr) = std::ptr::NonNull::new(raw) else { anyhow::bail!("CGWindowListCreateImage returned null") };
    // SAFETY: +1 retained CF object from a Create function.
    let img = unsafe { objc2_core_foundation::CFRetained::from_raw(ptr) };
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &img);
    let dict = NSDictionary::new();
    // SAFETY: empty properties dictionary.
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &dict) }
        .ok_or_else(|| anyhow::anyhow!("PNG encoding failed"))?;
    std::fs::write(path, data.to_vec())?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn capture_window(_window: &gpui_kit::Window, _path: &Path) -> anyhow::Result<()> {
    anyhow::bail!("unsupported")
}
