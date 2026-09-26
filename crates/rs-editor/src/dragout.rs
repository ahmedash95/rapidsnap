//! Drag the exported image out of the editor into other apps.
//!
//! GPUI only supports drags *within* a window, so on macOS we start a native
//! `NSDraggingSession` on GPUI's NSView with a file URL (a PNG written to a
//! temp file). The AppKit call is deferred to the next main-queue turn
//! (CONTRACTS §9: never call AppKit from inside a GPUI callback).

use std::path::{Path, PathBuf};

/// Temp file path used for drag-out / reveal.
pub fn drag_file_path(stem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("rapidsnap-drag");
    let _ = std::fs::create_dir_all(&dir);
    let safe: String = stem.chars().map(|c| if c == '/' || c == ':' { '-' } else { c }).collect();
    dir.join(format!("{safe}.png"))
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::path::Path;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource,
        NSEvent, NSImage, NSView,
    };
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RsEditorDragSource"]
        struct DragSource;

        unsafe impl NSObjectProtocol for DragSource {}

        unsafe impl NSDraggingSource for DragSource {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn source_mask(&self, _session: &NSDraggingSession, _context: NSDraggingContext) -> NSDragOperation {
                NSDragOperation::Copy
            }
        }
    );

    impl DragSource {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    thread_local! {
        static SOURCE: RefCell<Option<Retained<DragSource>>> = const { RefCell::new(None) };
    }

    struct MainOnly {
        view: Retained<NSView>,
        event: Retained<NSEvent>,
        path: String,
    }
    // SAFETY: only used inside the main-queue block.
    unsafe impl Send for MainOnly {}

    pub fn start(window: &gpui_kit::Window, path: &Path) -> anyhow::Result<()> {
        let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("not on main thread"))?;
        let handle = HasWindowHandle::window_handle(window).map_err(|e| anyhow::anyhow!("window handle: {e}"))?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else { anyhow::bail!("not an AppKit window") };
        // SAFETY: GPUI's live NSView; main thread.
        let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
        let event = NSApplication::sharedApplication(mtm)
            .currentEvent()
            .ok_or_else(|| anyhow::anyhow!("no current event"))?;
        let job = MainOnly { view: view.retain(), event, path: path.to_string_lossy().into_owned() };
        dispatch2::DispatchQueue::main().exec_async(move || {
            let job = job;
            let Some(mtm) = MainThreadMarker::new() else { return };
            let ns_path = NSString::from_str(&job.path);
            let url = NSURL::fileURLWithPath(&ns_path);
            let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), ProtocolObject::from_ref(&*url));
            let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &ns_path);
            let loc = job.view.convertPoint_fromView(job.event.locationInWindow(), None);
            let (w, h) = match &image {
                Some(img) => {
                    let s = img.size();
                    let k = (160.0 / s.width.max(s.height).max(1.0)).min(1.0);
                    (s.width * k, s.height * k)
                }
                None => (64.0, 64.0),
            };
            let frame = NSRect::new(NSPoint::new(loc.x - w / 2.0, loc.y - h / 2.0), NSSize::new(w, h));
            let contents: Option<&AnyObject> = image.as_deref().map(|i| i.as_ref());
            // SAFETY: contents is an NSImage (or nil), frame in view coordinates.
            unsafe { item.setDraggingFrame_contents(frame, contents) };
            let source = DragSource::new(mtm);
            let items = NSArray::from_retained_slice(&[item]);
            let _session =
                job.view.beginDraggingSessionWithItems_event_source(&items, &job.event, ProtocolObject::from_ref(&*source));
            SOURCE.with(|s| *s.borrow_mut() = Some(source));
        });
        Ok(())
    }
}

/// Start a native file drag of `path` from the window (macOS). Must be
/// called while the mouse button is down (from a mouse-move handler).
pub fn start_file_drag(window: &gpui_kit::Window, path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        mac::start(window, path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, path);
        anyhow::bail!("drag-out is only implemented on macOS")
    }
}

/// Reveal a file in Finder (fallback for drag-out).
pub fn reveal_in_finder(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}
