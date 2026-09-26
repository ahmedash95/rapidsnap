//! # rs-editor — the RapidSnap editor window (GPUI + gpui-component)
//!
//! Public entry points (frozen signatures, see CONTRACTS.md §8):
//! * [`init`] — register actions + keybindings. Call once after `gpui_kit::init(cx)`.
//! * [`open_editor`] — open an editor window for a Document.
//! * [`PreferencesGlobal`] — the app-wide Preferences as a GPUI Global. The
//!   editor reads it (tool styles, export settings, auto-close) and writes it
//!   back (style memory, recent colors, presets, last beautify, export
//!   counter). The app observes and persists it.
//! * [`EditorOptions::services`] — optional app-injected services (OCR,
//!   auto-redact detection, project files, history, custom backgrounds).
//! * [`rgba_to_render_image`] — RgbaImage → GPUI `RenderImage`.
//!
//! Architecture:
//! * [`model::EditorModel`] — pure state machine (document, history,
//!   selection, tools, text sessions, crop). Unit-tested without GPUI.
//! * [`render_worker::RenderWorker`] — dedicated render thread, latest-wins.
//! * `view` — the GPUI window: toolbar, canvas element (bitmap + live
//!   overlays + inline text editor with IME), style/beautify panel.
//!
//! Set `RAPIDSNAP_EDITOR_SELFTEST=1` to drive a scripted self-test in every
//! opened editor (real GPUI events → export → quit); see `selftest.rs`.

mod debug_shot;
pub mod dragout;
pub mod fit;
pub mod gallery;
pub mod geom;
pub mod hit;
mod icons;
pub mod model;
mod panel;
mod panel_icons;
mod panel_layout;
mod panel_selftest;
mod panel_spec;
pub mod render_worker;
mod selftest;
pub mod services;
pub mod snap;
pub mod style_map;
mod text_layout;
pub mod text_buffer;
mod view;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Root;
use gpui_kit::*;
use rs_core::{Document, DocumentId, Preferences, RgbaImage};

pub use services::{CustomBackground, EditorServices, OcrTextLine, SensitiveMatch};
pub use view::EditorView;

/// Key context used by the editor's root element (for keybindings).
pub const KEY_CONTEXT: &str = "RsEditor";
/// Key context of the canvas when not editing text (tool letters, nudge…).
pub const CANVAS_CONTEXT: &str = "RsEditorCanvas";
/// Key context of the canvas while editing text inline.
pub const TEXT_CONTEXT: &str = "RsEditorText";

gpui_kit::actions!(rs_editor, [Copy, Save, Undo, Redo, CloseEditor]);

gpui_kit::actions!(
    rs_editor,
    [
        SaveProject,
        CopyText,
        AutoRedact,
        ZoomIn,
        ZoomOut,
        ZoomFit,
        ZoomActual,
        // canvas
        ToolSelect,
        ToolRect,
        ToolEllipse,
        ToolLine,
        ToolArrow,
        ToolPen,
        ToolHighlighter,
        ToolStep,
        ToolCallout,
        ToolSpotlight,
        ToolMagnifier,
        ToolBlur,
        ToolPixelate,
        ToolRedact,
        ToolText,
        ToolCrop,
        Cancel,
        Confirm,
        DeleteSelection,
        Duplicate,
        SelectAll,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        NudgeLeftBig,
        NudgeRightBig,
        NudgeUpBig,
        NudgeDownBig,
        BringForward,
        SendBackward,
        BringToFront,
        SendToBack,
        CanvasCopy,
        /// Copy the selected annotations (context menu; ⌘C copies the image unless the Select tool is active).
        CopySelection,
        CanvasCut,
        CanvasPaste,
        // text editing
        TextBackspace,
        TextDeleteWordBack,
        TextDelete,
        TextLeft,
        TextRight,
        TextUp,
        TextDown,
        TextSelectLeft,
        TextSelectRight,
        TextSelectUp,
        TextSelectDown,
        TextWordLeft,
        TextWordRight,
        TextSelectWordLeft,
        TextSelectWordRight,
        TextHome,
        TextEnd,
        TextSelectHome,
        TextSelectEnd,
        TextDocStart,
        TextDocEnd,
        TextSelectAll,
        TextCopy,
        TextCut,
        TextPaste,
        TextNewline,
        TextCommit,
        TextUndo,
        TextRedo,
        TextCharacterPalette,
    ]
);

/// App-wide preferences, shared between app and editor windows.
pub struct PreferencesGlobal(pub Preferences);

impl Global for PreferencesGlobal {}

/// Events the editor reports to the app.
#[derive(Debug, Clone)]
pub enum EditorEvent {
    /// Result was copied to the clipboard.
    Copied { document: DocumentId },
    /// Result was exported to a file.
    Saved { document: DocumentId, path: PathBuf },
    /// The editor window closed (the Document is dropped with it).
    Closed { document: DocumentId },
}

/// Callback type for [`EditorEvent`]s.
pub type EditorEventHandler = Rc<dyn Fn(&EditorEvent, &mut App)>;

/// Options for [`open_editor`].
#[derive(Clone, Default)]
pub struct EditorOptions {
    /// Called on the main thread for every [`EditorEvent`].
    pub on_event: Option<EditorEventHandler>,
    /// App-injected services (OCR, projects, history, backgrounds…).
    pub services: EditorServices,
}

/// Register editor actions and keybindings. Call once at startup, after
/// `gpui_kit::init(cx)`.
pub fn init(cx: &mut App) {
    let root = Some(KEY_CONTEXT);
    let canvas = Some(CANVAS_CONTEXT);
    let text = Some(TEXT_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, root),
        KeyBinding::new("cmd-s", Save, root),
        KeyBinding::new("cmd-shift-s", SaveProject, root),
        KeyBinding::new("cmd-z", Undo, root),
        KeyBinding::new("cmd-shift-z", Redo, root),
        KeyBinding::new("cmd-w", CloseEditor, root),
        KeyBinding::new("cmd-=", ZoomIn, root),
        KeyBinding::new("cmd-+", ZoomIn, root),
        KeyBinding::new("cmd--", ZoomOut, root),
        KeyBinding::new("cmd-0", ZoomFit, root),
        KeyBinding::new("cmd-1", ZoomActual, root),
        KeyBinding::new("cmd-shift-t", CopyText, root),
        KeyBinding::new("cmd-shift-r", AutoRedact, root),
        // Canvas (not editing text).
        KeyBinding::new("v", ToolSelect, canvas),
        KeyBinding::new("r", ToolRect, canvas),
        KeyBinding::new("o", ToolEllipse, canvas),
        KeyBinding::new("l", ToolLine, canvas),
        KeyBinding::new("a", ToolArrow, canvas),
        KeyBinding::new("p", ToolPen, canvas),
        KeyBinding::new("h", ToolHighlighter, canvas),
        KeyBinding::new("n", ToolStep, canvas),
        KeyBinding::new("c", ToolCallout, canvas),
        KeyBinding::new("s", ToolSpotlight, canvas),
        KeyBinding::new("m", ToolMagnifier, canvas),
        KeyBinding::new("b", ToolBlur, canvas),
        KeyBinding::new("x", ToolPixelate, canvas),
        KeyBinding::new("d", ToolRedact, canvas),
        KeyBinding::new("t", ToolText, canvas),
        KeyBinding::new("k", ToolCrop, canvas),
        KeyBinding::new("escape", Cancel, canvas),
        KeyBinding::new("enter", Confirm, canvas),
        KeyBinding::new("backspace", DeleteSelection, canvas),
        KeyBinding::new("delete", DeleteSelection, canvas),
        KeyBinding::new("cmd-d", Duplicate, canvas),
        KeyBinding::new("cmd-a", SelectAll, canvas),
        KeyBinding::new("left", NudgeLeft, canvas),
        KeyBinding::new("right", NudgeRight, canvas),
        KeyBinding::new("up", NudgeUp, canvas),
        KeyBinding::new("down", NudgeDown, canvas),
        KeyBinding::new("shift-left", NudgeLeftBig, canvas),
        KeyBinding::new("shift-right", NudgeRightBig, canvas),
        KeyBinding::new("shift-up", NudgeUpBig, canvas),
        KeyBinding::new("shift-down", NudgeDownBig, canvas),
        KeyBinding::new("cmd-]", BringForward, canvas),
        KeyBinding::new("cmd-[", SendBackward, canvas),
        KeyBinding::new("alt-cmd-]", BringToFront, canvas),
        KeyBinding::new("alt-cmd-[", SendToBack, canvas),
        KeyBinding::new("cmd-c", CanvasCopy, canvas),
        KeyBinding::new("cmd-x", CanvasCut, canvas),
        KeyBinding::new("cmd-v", CanvasPaste, canvas),
        // Inline text editing.
        KeyBinding::new("backspace", TextBackspace, text),
        KeyBinding::new("shift-backspace", TextBackspace, text),
        KeyBinding::new("alt-backspace", TextDeleteWordBack, text),
        KeyBinding::new("delete", TextDelete, text),
        KeyBinding::new("left", TextLeft, text),
        KeyBinding::new("right", TextRight, text),
        KeyBinding::new("up", TextUp, text),
        KeyBinding::new("down", TextDown, text),
        KeyBinding::new("shift-left", TextSelectLeft, text),
        KeyBinding::new("shift-right", TextSelectRight, text),
        KeyBinding::new("shift-up", TextSelectUp, text),
        KeyBinding::new("shift-down", TextSelectDown, text),
        KeyBinding::new("alt-left", TextWordLeft, text),
        KeyBinding::new("alt-right", TextWordRight, text),
        KeyBinding::new("alt-shift-left", TextSelectWordLeft, text),
        KeyBinding::new("alt-shift-right", TextSelectWordRight, text),
        KeyBinding::new("cmd-left", TextHome, text),
        KeyBinding::new("cmd-right", TextEnd, text),
        KeyBinding::new("home", TextHome, text),
        KeyBinding::new("end", TextEnd, text),
        KeyBinding::new("cmd-shift-left", TextSelectHome, text),
        KeyBinding::new("cmd-shift-right", TextSelectEnd, text),
        KeyBinding::new("cmd-up", TextDocStart, text),
        KeyBinding::new("cmd-down", TextDocEnd, text),
        KeyBinding::new("cmd-a", TextSelectAll, text),
        KeyBinding::new("cmd-c", TextCopy, text),
        KeyBinding::new("cmd-x", TextCut, text),
        KeyBinding::new("cmd-v", TextPaste, text),
        KeyBinding::new("enter", TextNewline, text),
        KeyBinding::new("shift-enter", TextNewline, text),
        KeyBinding::new("cmd-enter", TextCommit, text),
        KeyBinding::new("escape", TextCommit, text),
        KeyBinding::new("cmd-z", TextUndo, text),
        KeyBinding::new("cmd-shift-z", TextRedo, text),
        KeyBinding::new("ctrl-cmd-space", TextCharacterPalette, text),
    ]);
}

/// Open an editor window for `doc`, bring it to front and focus it.
///
/// Must be called on the main thread (any `&mut App`). The caller is
/// responsible for the app activation policy (see CONTRACTS.md: the app
/// switches to `Regular` while editor windows are open). Multiple editor
/// windows can be open at once (each has its own model, history and render
/// thread).
pub fn open_editor(cx: &mut App, doc: Document, options: EditorOptions) -> anyhow::Result<WindowHandle<Root>> {
    let screen = cx.primary_display().map(|d| {
        let b = d.visible_bounds();
        (f32::from(b.size.width), f32::from(b.size.height))
    });
    let size = initial_window_size(&doc, screen);
    let title = view::window_title(&doc);
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(16.))),
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        window_min_size: Some(gpui_kit::size(px(fit::MIN_WINDOW.0), px(fit::MIN_WINDOW.1))),
        ..Default::default()
    };
    let handle = cx.open_window(window_options, |window, cx| {
        let view = cx.new(|cx| EditorView::new(doc, options, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })?;
    cx.activate(true);
    handle.update(cx, |_, window, _| window.activate_window())?;
    Ok(handle)
}

fn initial_window_size(doc: &Document, screen: Option<(f32, f32)>) -> Size<Pixels> {
    // Canvas (image + beautify frame) at actual size, in points.
    let l = rs_render::compute_layout(doc, 1.0);
    let sf = doc.metadata.scale_factor.max(0.01);
    let canvas_pt = (l.canvas_width as f32 / sf, l.canvas_height as f32 / sf);
    let chrome = fit::Chrome { panel_w: view::PANEL_W, bars_h: view::TOOLBAR_H + view::STATUS_H };
    let (w, h) = fit::initial_window_size(canvas_pt, chrome, screen);
    gpui_kit::size(px(w), px(h))
}

/// Convert straight-alpha RGBA to a GPUI `RenderImage` (GPUI wants BGRA,
/// straight alpha, one frame). O(pixels); do it once per preview update.
///
/// **Memory:** GPUI uploads each `RenderImage` into its sprite atlas; when
/// replacing a preview, call `window.drop_image(old)` (or `cx.drop_image`)
/// to free the previous one.
pub fn rgba_to_render_image(img: &RgbaImage) -> Arc<RenderImage> {
    let mut bgra = img.pixels.as_ref().clone();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(img.width, img.height, bgra).unwrap_or_else(|| image::RgbaImage::new(1, 1));
    Arc::new(RenderImage::new(vec![image::Frame::new(buf)]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_stem() {
        let s = super::view::default_file_stem(&rs_core::ExportSettings::default(), 10, 10, None);
        assert!(s.starts_with("RapidSnap 20"), "{s}");
    }

    #[test]
    fn render_image_conversion() {
        let img = rs_core::RgbaImage::filled(2, 2, [1, 2, 3, 4]);
        let ri = super::rgba_to_render_image(&img);
        assert_eq!(&ri.as_bytes(0).unwrap()[..4], &[3, 2, 1, 4]);
    }
}
