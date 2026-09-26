//! The GPUI editor view: window chrome, toolbar, canvas element (preview
//! bitmap + live overlays + inline text editor), action bar and the side
//! panel (see `panel.rs`). Heavy work (rendering, export, OCR) runs off the
//! main thread.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    menu::ContextMenuExt as _,
    v_flex, ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rs_core::{
    AnnotationKind, Document, ExportFormat, ExportScale, ExportSettings, Point as CPoint, Rect as CRect, RgbaImage,
    ToolKind,
};
use rs_render::CanvasLayout;

use crate::icons::Ico;
use crate::model::{EditorModel, Interaction, Mods, ZOrder};
use crate::panel::PanelState;
use crate::render_worker::{PreviewFrame, PreviewResult, RenderWorker};
use crate::services::SensitiveMatch;
use crate::text_layout::{self, EditGeom};
use crate::*;

const ACCENT: u32 = 0x0A84FF;
use crate::fit::{MAX_ZOOM, MIN_ZOOM};
pub(crate) const TOOLBAR_H: f32 = 52.0;
pub(crate) const STATUS_H: f32 = 26.0;
/// Side panel width (pt).
pub(crate) const PANEL_W: f32 = 256.0;

/// Window title: "RapidSnap — W×H".
pub(crate) fn window_title(doc: &Document) -> String {
    let s = doc.content_size();
    match &doc.metadata.title {
        Some(t) => format!("{t} — {}×{}", s.width as u32, s.height as u32),
        None => format!("RapidSnap — {}×{}", s.width as u32, s.height as u32),
    }
}

pub(crate) fn default_file_stem(export: &ExportSettings, w: u32, h: u32, app: Option<&str>) -> String {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    rs_core::render_filename(
        &export.filename_template,
        &rs_core::FilenameContext {
            year: now.year(),
            month: now.month(),
            day: now.day(),
            hour: now.hour(),
            minute: now.minute(),
            second: now.second(),
            counter: export.counter,
            width: w,
            height: h,
            app_name: app.map(str::to_string),
        },
    )
}

/// Source app of a window capture (filename template `{app}`).
pub(crate) fn source_app(doc: &Document) -> Option<&str> {
    match &doc.metadata.source {
        rs_core::CaptureSource::Window { app_name: Some(a), .. } if !a.is_empty() => Some(a.as_str()),
        _ => None,
    }
}

fn dirs_desktop() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Desktop"))
}

/// Canvas geometry of the last paint (window px).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CanvasGeom {
    pub bounds: Bounds<Pixels>,
    /// Window position of canvas px (0, 0) at scale 1.
    pub origin: Point<Pixels>,
    /// Window px per canvas px (scale 1).
    pub k: f32,
}

/// Auto-redact review state.
pub(crate) struct RedactReview {
    pub items: Vec<(SensitiveMatch, bool)>,
    /// 0 solid, 1 blur, 2 pixelate.
    pub style: usize,
}

/// State of the drag-out handle.
#[derive(Default)]
struct DragOut {
    armed: bool,
    started: bool,
    ready: Option<(u64, PathBuf)>,
    exporting: bool,
}

/// The editor root view (wrapped in gpui-component's `Root`).
pub struct EditorView {
    pub(crate) model: EditorModel,
    pub(crate) options: EditorOptions,
    focus_handle: FocusHandle,
    pub(crate) canvas_focus: FocusHandle,
    worker: RenderWorker,
    frame: Option<PreviewFrame>,
    requested: (u64, f32),
    /// Generation whose display doc / layout is cached.
    seen_generation: u64,
    pub(crate) layout1: CanvasLayout,
    display_doc: Document,
    zoom: f32,
    fit: bool,
    pan: Point<Pixels>,
    geom: Rc<Cell<CanvasGeom>>,
    pub(crate) edit_layout: Rc<RefCell<Option<EditGeom>>>,
    /// Caret blink phase (visible) and the task toggling it while editing.
    caret_on: bool,
    blink_task: Option<Task<()>>,
    /// (text, selection) the blink was last reset for.
    blink_key: Option<(String, Range<usize>)>,
    pub(crate) panel: PanelState,
    pub(crate) redact_review: Option<RedactReview>,
    pub(crate) busy: Option<String>,
    drag_out: DragOut,
    pressed: bool,
    last_title: String,
    pub(crate) last_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EditorView {
    pub fn new(doc: Document, options: EditorOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let canvas_focus = cx.focus_handle();
        let prefs = cx.try_global::<PreferencesGlobal>().map(|p| p.0.clone()).unwrap_or_default();
        let mut model = EditorModel::new(doc, prefs.tool_styles.clone());
        let initial_tool = if prefs.last_tool == ToolKind::Crop { ToolKind::Select } else { prefs.last_tool };
        model.set_tool(initial_tool);

        let (tx, rx) = smol::channel::unbounded::<PreviewResult>();
        let worker = RenderWorker::spawn(tx);
        if let Some(p) = &options.services.desktop_wallpaper {
            worker.set_desktop_wallpaper(Some(p.clone()));
        }
        // Receive rendered frames on the main thread.
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            while let Ok(res) = rx.recv().await {
                // Drain to the latest frame.
                let mut res = res;
                while let Ok(next) = rx.try_recv() {
                    res = next;
                }
                if this.update_in(cx, |this, window, cx| this.on_frame(res, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let doc_id = model.doc.id;
        let on_event = options.on_event.clone();
        let release_sub = cx.on_release(move |_, cx| {
            if let Some(cb) = on_event {
                cb(&EditorEvent::Closed { document: doc_id }, cx);
            }
        });

        let (panel, panel_subs) = PanelState::new(&model, &options, window, cx);
        let mut subs = vec![release_sub];
        subs.extend(panel_subs);
        let display_doc = model.display().0;
        let layout1 = rs_render::compute_layout(&display_doc, 1.0);
        canvas_focus.focus(window, cx);
        let title = window_title(&model.doc);
        let mut this = Self {
            model,
            options,
            focus_handle,
            canvas_focus,
            worker,
            frame: None,
            requested: (0, 0.0),
            seen_generation: 0,
            layout1,
            display_doc,
            zoom: 1.0,
            fit: true,
            pan: Point::default(),
            geom: Rc::new(Cell::new(CanvasGeom::default())),
            edit_layout: Rc::new(RefCell::new(None)),
            caret_on: true,
            blink_task: None,
            blink_key: None,
            panel,
            redact_review: None,
            busy: None,
            drag_out: DragOut::default(),
            pressed: false,
            last_title: title,
            last_error: None,
            _subscriptions: subs,
        };
        this.load_fonts(window, cx);
        this.panel.start_thumbnail_loading(&this.options, window, cx);
        if std::env::var("RAPIDSNAP_EDITOR_SELFTEST").is_ok_and(|v| v == "1") {
            crate::selftest::start(window, cx);
        }
        this
    }

    /// Current document (read-only).
    pub fn document(&self) -> &Document {
        &self.model.doc
    }

    /// The editor model (read-only).
    pub fn model(&self) -> &EditorModel {
        &self.model
    }

    fn emit(&self, ev: EditorEvent, cx: &mut App) {
        if let Some(cb) = &self.options.on_event {
            cb(&ev, cx);
        }
    }

    pub(crate) fn sf(&self) -> f32 {
        self.model.doc.metadata.scale_factor.max(0.01)
    }

    // ------------------------------------------------------------------
    // Change propagation
    // ------------------------------------------------------------------

    /// Call after any model mutation: syncs prefs, schedules a re-render.
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        if cx.try_global::<PreferencesGlobal>().is_some() {
            if self.model.styles_dirty {
                self.model.styles_dirty = false;
                let styles = self.model.styles.clone();
                cx.global_mut::<PreferencesGlobal>().0.tool_styles = styles;
            }
            let b = &self.model.doc.beautify;
            if cx.global::<PreferencesGlobal>().0.last_beautify != *b {
                let b = b.clone();
                cx.global_mut::<PreferencesGlobal>().0.last_beautify = b;
            }
            let tool = self.model.tool;
            if tool != ToolKind::Crop && cx.global::<PreferencesGlobal>().0.last_tool != tool {
                cx.global_mut::<PreferencesGlobal>().0.last_tool = tool;
            }
        }
        if let Some(e) = self.model.last_error.take() {
            log::warn!("editor: {e}");
            self.last_error = Some(e);
        }
        cx.notify();
    }

    pub(crate) fn push_recent_color(&mut self, c: rs_core::Color, cx: &mut Context<Self>) {
        if cx.try_global::<PreferencesGlobal>().is_some() {
            cx.global_mut::<PreferencesGlobal>().0.push_recent_color(c.with_alpha(1.0));
        }
    }

    pub(crate) fn prefs(&self, cx: &App) -> rs_core::Preferences {
        cx.try_global::<PreferencesGlobal>().map(|p| p.0.clone()).unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // Rendering pipeline
    // ------------------------------------------------------------------

    /// Canvas area (window pt), derived from the current window size so a
    /// resize is reflected in the same frame (the painted bounds lag one
    /// frame behind, which left "Fit" stale after the last resize step).
    fn viewport_size(&self, window: &Window) -> Size<Pixels> {
        let v = window.viewport_size();
        let w = v.width - px(PANEL_W);
        let h = v.height - px(TOOLBAR_H + STATUS_H);
        if w > px(10.) && h > px(10.) {
            return size(w, h);
        }
        self.geom.get().bounds.size
    }

    /// Fit zoom for the current viewport (see [`crate::fit`] for the rules).
    pub(crate) fn fit_zoom(&self, window: &Window) -> f32 {
        let v = self.viewport_size(window);
        let sf = self.sf();
        let cw = self.layout1.canvas_width.max(1) as f32 / sf;
        let ch = self.layout1.canvas_height.max(1) as f32 / sf;
        crate::fit::fit_zoom((f32::from(v.width), f32::from(v.height)), (cw, ch), sf)
    }

    /// Window px per content px.
    fn view_k(&self) -> f32 {
        self.zoom / self.sf()
    }

    /// Refresh the cached display document/layout and request a render if
    /// the model or the zoom changed.
    fn sync_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.seen_generation != self.model.generation {
            self.seen_generation = self.model.generation;
            let (d, _) = self.model.display();
            self.layout1 = rs_render::compute_layout(&d, 1.0);
            self.display_doc = d;
            self.panel.sync(&self.model, window, cx);
            let t = window_title(&self.model.doc);
            if t != self.last_title {
                window.set_window_title(&t);
                self.last_title = t;
            }
        }
        if self.fit {
            self.zoom = self.fit_zoom(window);
        }
        self.model.view_scale = self.view_k();
        let k = self.view_k();
        let wsf = window.scale_factor();
        let mut scale = (k * wsf).min(4.0);
        let side = self.layout1.canvas_width.max(self.layout1.canvas_height).max(1) as f32;
        scale = scale.min(8192.0 / side);
        let px_count = self.layout1.canvas_width as f32 * self.layout1.canvas_height as f32 * scale * scale;
        let interacting = !matches!(self.model.interaction, Interaction::None) || self.panel.slider_active;
        if interacting && px_count > 1.5e6 {
            scale *= (1.5e6 / px_count).sqrt().max(0.35);
        }
        // Quantize to avoid re-rendering for tiny zoom changes.
        let scale = (scale * 64.0).round().max(1.0) / 64.0;
        let key = (self.model.generation, scale);
        if key != self.requested {
            self.requested = key;
            let (doc, exclude) = self.model.display();
            self.worker.request_preview(doc, exclude, scale, self.model.generation);
        }
    }

    fn on_frame(&mut self, res: PreviewResult, window: &mut Window, cx: &mut Context<Self>) {
        match res {
            Ok(f) => {
                if self.frame.as_ref().is_some_and(|old| old.generation > f.generation) {
                    return;
                }
                log::trace!("preview {}x{} in {:.1} ms", f.width, f.height, f.millis);
                if let Some(old) = self.frame.replace(f) {
                    let _ = window.drop_image(old.image);
                }
                self.last_error = None;
            }
            Err(e) => {
                log::warn!("render failed: {e}");
                self.last_error = Some(e);
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Coordinates
    // ------------------------------------------------------------------

    fn to_content(&self, p: Point<Pixels>) -> CPoint {
        let g = self.geom.get();
        let k = g.k.max(1e-6);
        let c = CPoint::new(f32::from(p.x - g.origin.x) / k, f32::from(p.y - g.origin.y) / k);
        self.layout1.canvas_to_image(c)
    }

    pub(crate) fn to_window(&self, c: CPoint) -> Point<Pixels> {
        let g = self.geom.get();
        let q = self.layout1.image_to_canvas(c);
        point(g.origin.x + px(q.x * g.k), g.origin.y + px(q.y * g.k))
    }


    fn mods(m: &Modifiers) -> Mods {
        Mods { shift: m.shift, alt: m.alt, cmd: m.platform }
    }

    // ------------------------------------------------------------------
    // Pointer handling (registered from the canvas paint)
    // ------------------------------------------------------------------

    fn session_box_contains(&self, p: Point<Pixels>) -> bool {
        let Some(l) = self.edit_layout.borrow().clone() else { return false };
        let b = l.box_bounds();
        let b = Bounds::new(b.origin, size(b.size.width.max(px(8.)), b.size.height.max(px(8.))));
        b.dilate(px(6.)).contains(&p)
    }

    pub(crate) fn canvas_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.canvas_focus.focus(window, cx);
        self.pressed = true;
        // Clicks inside the text being edited move the caret / select.
        if self.model.editing.is_some() && self.session_box_contains(ev.position) {
            let idx = self.edit_layout.borrow().as_ref().map(|l| l.index_at(ev.position)).unwrap_or(0);
            let shift = ev.modifiers.shift;
            let clicks = ev.click_count;
            self.model.edit_text(|b| {
                if clicks >= 3 {
                    b.select_all();
                } else if clicks == 2 {
                    b.select_word_at(idx);
                } else {
                    b.set_cursor(idx, shift);
                }
            });
            if let Some(s) = &mut self.model.editing {
                s.selecting = clicks == 1;
            }
            self.changed(cx);
            return;
        }
        let p = self.to_content(ev.position);
        let was_eyedropper = self.model.eyedropper;
        if was_eyedropper {
            // Sample the composed canvas (background, padding, image and
            // annotations) when the preview is current; else the base image.
            self.model.canvas_sample = self
                .frame
                .as_ref()
                .filter(|f| f.generation == self.model.generation)
                .map(|f| (f.rgba.clone(), f.layout));
        }
        self.model.pointer_down(p, Self::mods(&ev.modifiers), ev.click_count);
        if was_eyedropper {
            self.model.canvas_sample = None;
        }
        if was_eyedropper {
            if let Some(c) = self.model.picked_color.take() {
                self.panel_color_picked(c, window, cx);
            }
            self.panel.eyedropper_target = None;
        }
        self.changed(cx);
    }

    pub(crate) fn canvas_mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pressed = ev.pressed_button == Some(MouseButton::Left);
        if let Some(s) = &self.model.editing
            && s.selecting
            && pressed
        {
            let idx = self.edit_layout.borrow().as_ref().map(|l| l.index_at(ev.position)).unwrap_or(0);
            self.model.edit_text(|b| b.set_cursor(idx, true));
            cx.notify();
            return;
        }
        let p = self.to_content(ev.position);
        let active = !matches!(self.model.interaction, Interaction::None);
        // `self.pressed` = we saw the mouse-down and no mouse-up yet (AppKit
        // delivers the up to this window even outside it). Synthetic hover
        // moves (pressed_button = None) during a drag must not end it.
        if active && !pressed && !self.pressed {
            // Button released outside the window: finish the interaction.
            self.model.pointer_up(p, Self::mods(&ev.modifiers));
            self.changed(cx);
            return;
        }
        let hovered_before = (self.model.hovered, self.model.hover.map(|h| (h.x as i32, h.y as i32)));
        self.model.pointer_move(p, Self::mods(&ev.modifiers));
        let _ = window;
        if active || hovered_before != (self.model.hovered, self.model.hover.map(|h| (h.x as i32, h.y as i32))) {
            if active {
                self.changed(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(crate) fn canvas_mouse_up(&mut self, ev: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.pressed = false;
        if let Some(s) = &mut self.model.editing {
            s.selecting = false;
        }
        if matches!(self.model.interaction, Interaction::None) {
            return;
        }
        let p = self.to_content(ev.position);
        self.model.pointer_up(p, Self::mods(&ev.modifiers));
        self.changed(cx);
    }

    fn canvas_right_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.canvas_focus.focus(window, cx);
        let p = self.to_content(ev.position);
        if let Some(id) = self.model.pick(p)
            && !self.model.selection.contains(&id)
        {
            self.model.commit_text();
            self.model.selection = vec![id];
            self.changed(cx);
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let d = ev.delta.pixel_delta(px(16.));
        if ev.modifiers.platform || ev.modifiers.control {
            let f = (1.0 + f32::from(d.y) * 0.01).clamp(0.5, 2.0);
            self.zoom_about(self.zoom * f, ev.position, window);
        } else {
            self.pan += d;
            self.clamp_pan(window);
        }
        cx.notify();
    }

    fn on_pinch(&mut self, ev: &PinchEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_about(self.zoom * (1.0 + ev.delta), ev.position, window);
        cx.notify();
    }

    fn zoom_about(&mut self, new_zoom: f32, anchor: Point<Pixels>, window: &Window) {
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let old = self.zoom.max(1e-4);
        let b = self.geom.get().bounds;
        let center = b.center();
        let r = new_zoom / old;
        let rel = anchor - center;
        self.pan = point(rel.x - (rel.x - self.pan.x) * r, rel.y - (rel.y - self.pan.y) * r);
        self.zoom = new_zoom;
        self.fit = false;
        self.clamp_pan(window);
    }

    fn clamp_pan(&mut self, window: &Window) {
        let v = self.viewport_size(window);
        let k = self.view_k();
        let dw = self.layout1.canvas_width as f32 * k;
        let dh = self.layout1.canvas_height as f32 * k;
        let mx = ((dw - f32::from(v.width)) / 2.0 + 60.0).max(0.0);
        let my = ((dh - f32::from(v.height)) / 2.0 + 60.0).max(0.0);
        self.pan = point(px(f32::from(self.pan.x).clamp(-mx, mx)), px(f32::from(self.pan.y).clamp(-my, my)));
    }

    pub(crate) fn set_zoom(&mut self, z: Option<f32>, window: &mut Window, cx: &mut Context<Self>) {
        match z {
            None => {
                self.fit = true;
                self.pan = Point::default();
            }
            Some(z) => {
                let c = self.geom.get().bounds.center();
                self.zoom_about(z, c, window);
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    pub(crate) fn set_tool(&mut self, t: ToolKind, window: &mut Window, cx: &mut Context<Self>) {
        self.model.set_tool(t);
        self.canvas_focus.focus(window, cx);
        self.changed(cx);
    }

    fn on_cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.redact_review.take().is_some() {
            cx.notify();
            return;
        }
        self.model.escape();
        self.changed(cx);
    }

    fn on_confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if self.model.crop.is_some() {
            self.model.apply_crop();
        } else if let [id] = self.model.selection.as_slice()
            && matches!(self.model.doc.get(*id).map(|a| &a.kind), Some(AnnotationKind::Text(_) | AnnotationKind::Callout(_)))
        {
            let id = *id;
            self.model.begin_edit(id);
        }
        self.changed(cx);
    }

    fn nudge(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.model.nudge(dx, dy);
        self.changed(cx);
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.model.undo();
        self.panel.sync(&self.model, window, cx);
        self.changed(cx);
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.model.redo();
        self.panel.sync(&self.model, window, cx);
        self.changed(cx);
    }

    /// ⌘C: copies the **image**, unless the user is working on annotations
    /// with the Select tool (then the selected annotations). A freshly drawn
    /// shape stays selected with its drawing tool active — ⌘C right after
    /// drawing must still copy the screenshot, not annotation JSON.
    fn canvas_copy(&mut self, _: &CanvasCopy, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.tool == ToolKind::Select && !self.model.selection.is_empty() && self.copy_annotations(window, cx) {
            return;
        }
        self.copy_image(window, cx);
    }

    /// Context menu "Copy": always the selected annotations.
    fn copy_selection_action(&mut self, _: &CopySelection, window: &mut Window, cx: &mut Context<Self>) {
        if !self.copy_annotations(window, cx) {
            window.push_notification("Select annotations to copy", cx);
        }
    }

    fn copy_annotations(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(json) = self.model.copy_selection() else { return false };
        cx.write_to_clipboard(ClipboardItem::new_string(json));
        let n = self.model.selection.len();
        window.push_notification(
            format!("Copied {n} annotation{} · paste into any editor with ⌘V", if n == 1 { "" } else { "s" }),
            cx,
        );
        true
    }

    fn canvas_cut(&mut self, _: &CanvasCut, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(json) = self.model.copy_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(json));
            self.model.delete_selection();
            self.changed(cx);
        }
    }

    fn canvas_paste(&mut self, _: &CanvasPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text())
            && self.model.paste(&text)
        {
            self.changed(cx);
            return;
        }
        window.push_notification("Nothing to paste (copy annotations with ⌘C first)", cx);
    }

    fn close(&mut self, _: &CloseEditor, window: &mut Window, _cx: &mut Context<Self>) {
        window.remove_window();
    }

    // ------------------------------------------------------------------
    // Text editing actions
    // ------------------------------------------------------------------

    fn text_op(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut crate::text_buffer::TextBuffer)) {
        self.model.edit_text(f);
        cx.notify();
    }

    fn text_vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(s) = &self.model.editing else { return };
        let cur = s.buffer.cursor();
        let target = self.edit_layout.borrow().as_ref().and_then(|l| l.vertical(cur, down));
        let len = s.buffer.text.len();
        let t = target.unwrap_or(if down { len } else { 0 });
        self.text_op(cx, |b| b.set_cursor(t, extend));
    }

    fn text_copy(&mut self, cut: bool, cx: &mut Context<Self>) {
        let Some(s) = &self.model.editing else { return };
        let t = s.buffer.selected_text().to_string();
        if t.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(t));
        if cut {
            self.text_op(cx, |b| b.replace(None, ""));
        }
    }

    fn text_paste(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
            // One line-break convention in the buffer (rs-render also breaks on a lone CR).
            let t = t.replace("\r\n", "\n").replace('\r', "\n");
            self.text_op(cx, |b| b.insert(&t));
        }
    }

    fn text_commit(&mut self, _: &TextCommit, _: &mut Window, cx: &mut Context<Self>) {
        self.model.commit_text();
        self.changed(cx);
    }

    // ------------------------------------------------------------------
    // Output: copy / save / project / drag-out / OCR / auto-redact
    // ------------------------------------------------------------------

    fn export_doc(&mut self) -> Document {
        self.model.commit_text();
        self.model.cancel_crop();
        self.model.doc.clone()
    }

    pub(crate) fn copy_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.export_doc();
        let prefs = self.prefs(cx);
        let settings = ExportSettings { format: ExportFormat::Png, ..prefs.export.clone() };
        let px_per_pt = settings.scale.factor();
        let rx = self.worker.export(doc.clone(), settings);
        let native_copy = self.options.services.copy_image.clone();
        self.busy = Some("Copying…".into());
        cx.notify();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let r = rx.recv().await.map_err(|e| e.to_string()).and_then(|r| r);
            // Native pasteboard (PNG + TIFF with the right DPI) off the main thread.
            let r = match (r, native_copy) {
                (Ok((bytes, img)), Some(copy)) => cx
                    .background_executor()
                    .spawn(async move { copy(&img, &bytes, px_per_pt).map(|_| (None, img)) })
                    .await,
                (Ok((bytes, img)), None) => Ok((Some(bytes), img)),
                (Err(e), _) => Err(e),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match r {
                    Ok((gpui_bytes, img)) => {
                        if let Some(bytes) = gpui_bytes {
                            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, bytes)));
                        }
                        this.emit(EditorEvent::Copied { document: doc.id }, cx);
                        this.add_to_history(&doc, img, cx);
                        if prefs.auto_close_on_copy {
                            window.remove_window();
                        } else {
                            window.push_notification("Copied to clipboard", cx);
                        }
                    }
                    Err(e) => this.last_error = Some(format!("Copy failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_to_history(&self, doc: &Document, img: RgbaImage, cx: &mut Context<Self>) {
        if let Some(f) = self.options.services.add_to_history.clone() {
            let doc = doc.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(e) = f(&doc, &img) {
                        log::warn!("history: {e}");
                    }
                })
                .detach();
        }
    }

    pub(crate) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.export_doc();
        let prefs = self.prefs(cx);
        let export = prefs.export.clone();
        let dir = export.default_folder.clone().or_else(dirs_desktop).unwrap_or_else(std::env::temp_dir);
        let l = rs_render::compute_layout(&doc, rs_render::RenderOptions::for_export(&doc, &export).scale);
        let name = format!("{}.{}", default_file_stem(&export, l.canvas_width, l.canvas_height, source_app(&doc)), export.format.extension());
        cx.activate(true);
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let Ok(Ok(Some(mut path))) = rx.await else { return };
            let mut settings = export.clone();
            // No / unknown extension → keep the preferred format and name the file after it,
            // so the bytes always match the extension.
            let known = path
                .extension()
                .map(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"))
                .unwrap_or(false);
            if !known {
                let mut os = path.clone().into_os_string();
                os.push(".");
                os.push(export.format.extension());
                path = std::path::PathBuf::from(os);
            }
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            settings.format = match ext.as_str() {
                "png" => ExportFormat::Png,
                "jpg" | "jpeg" => match export.format {
                    ExportFormat::Jpeg { quality } => ExportFormat::Jpeg { quality },
                    _ => ExportFormat::Jpeg { quality: 90 },
                },
                "webp" => ExportFormat::WebP,
                _ => export.format,
            };
            let Ok(rx) = this.update(cx, |this, cx| {
                this.busy = Some("Saving…".into());
                cx.notify();
                this.worker.export(doc.clone(), settings)
            }) else {
                return;
            };
            let r = rx.recv().await.map_err(|e| e.to_string()).and_then(|r| r);
            let target = path.clone();
            let r = match r {
                Ok((bytes, img)) => cx
                    .background_executor()
                    .spawn(async move { std::fs::write(&target, bytes).map(|_| img).map_err(|e| e.to_string()) })
                    .await,
                Err(e) => Err(e),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match r {
                    Ok(img) => {
                        this.emit(EditorEvent::Saved { document: doc.id, path: path.clone() }, cx);
                        this.add_to_history(&doc, img, cx);
                        if cx.try_global::<PreferencesGlobal>().is_some() {
                            { let c = &mut cx.global_mut::<PreferencesGlobal>().0.export.counter; *c = c.saturating_add(1); }
                        }
                        this.model.history.mark_saved();
                        window.push_notification(format!("Saved {}", path.display()), cx);
                    }
                    Err(e) => this.last_error = Some(format!("Save failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn save_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(save) = self.options.services.save_project.clone() else {
            window.push_notification("Project files are not available", cx);
            return;
        };
        let doc = self.export_doc();
        let prefs = self.prefs(cx);
        let dir = doc
            .metadata
            .project_path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .or(prefs.export.default_folder.clone())
            .or_else(dirs_desktop)
            .unwrap_or_else(std::env::temp_dir);
        let stem = doc
            .metadata
            .project_path
            .as_ref()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| default_file_stem(&prefs.export, 0, 0, source_app(&doc)));
        cx.activate(true);
        let rx = cx.prompt_for_new_path(&dir, Some(&format!("{stem}.rsnap")));
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let thumb_settings = ExportSettings { format: ExportFormat::Png, scale: ExportScale::X1, ..Default::default() };
            let Ok(rx) = this.update(cx, |this, _| this.worker.export(doc.clone(), thumb_settings)) else { return };
            let thumb = rx.recv().await.ok().and_then(|r| r.ok()).map(|(_, img)| thumbnail(&img, 512));
            let mut doc2 = doc.clone();
            doc2.metadata.project_path = Some(path.clone());
            let p2 = path.clone();
            let task = cx.background_executor().spawn(async move { save(&doc2, &p2, thumb.as_ref()) });
            let r = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match r {
                    Ok(()) => {
                        this.model.doc.metadata.project_path = Some(path.clone());
                        this.model.history.mark_saved();
                        window.push_notification(format!("Saved project {}", path.display()), cx);
                    }
                    Err(e) => this.last_error = Some(format!("Save project failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(load) = self.options.services.load_project.clone() else { return };
        cx.activate(true);
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        let options = self.options.clone();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let r = cx.background_executor().spawn(async move { load(&path) }).await;
            let _ = this.update_in(cx, |this, window, cx| match r {
                Ok(doc) => {
                    if let Err(e) = open_editor(cx, doc, options) {
                        this.last_error = Some(e.to_string());
                    }
                }
                Err(e) => {
                    window.push_notification(format!("Open failed: {e}"), cx);
                }
            });
        })
        .detach();
    }

    fn prepare_drag_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.model.generation;
        if self.drag_out.exporting || self.drag_out.ready.as_ref().is_some_and(|(g, _)| *g == generation) {
            return;
        }
        self.drag_out.exporting = true;
        let doc = self.model.doc.clone();
        let prefs = self.prefs(cx);
        let l = rs_render::compute_layout(&doc, rs_render::RenderOptions::for_export(&doc, &prefs.export).scale);
        let path = crate::dragout::drag_file_path(&default_file_stem(&prefs.export, l.canvas_width, l.canvas_height, source_app(&doc)));
        let rx = self.worker.export(doc, ExportSettings { format: ExportFormat::Png, ..prefs.export });
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let r = rx.recv().await.map_err(|e| e.to_string()).and_then(|r| r);
            let r = r.and_then(|(bytes, _)| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.drag_out.exporting = false;
                match r {
                    Ok(()) => {
                        this.drag_out.ready = Some((generation, path));
                        // Mouse still held on the handle → start now.
                        if this.drag_out.armed && !this.drag_out.started {
                            this.start_drag_out(window, cx);
                        }
                    }
                    Err(e) => this.last_error = Some(format!("Drag export failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_drag_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((g, path)) = self.drag_out.ready.clone() else { return };
        if g != self.model.generation {
            self.prepare_drag_file(window, cx);
            return;
        }
        self.drag_out.started = true;
        if let Err(e) = crate::dragout::start_file_drag(window, &path) {
            log::warn!("drag-out: {e}");
            crate::dragout::reveal_in_finder(&path);
        }
    }

    pub(crate) fn copy_text_ocr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ocr) = self.options.services.recognize_text.clone() else {
            window.push_notification("Text recognition is not available", cx);
            return;
        };
        let doc = self.export_doc();
        self.busy = Some("Recognizing text…".into());
        cx.notify();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let img = rs_render::content_image(&doc).map_err(|e| e.to_string())?;
                    ocr(&img)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match r {
                    Ok(lines) if !lines.is_empty() => {
                        let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        window.push_notification(format!("Copied {} line(s) of text", lines.len()), cx);
                    }
                    Ok(_) => window.push_notification("No text found", cx),
                    Err(e) => this.last_error = Some(format!("OCR failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn auto_redact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detect) = self.options.services.detect_sensitive.clone() else {
            window.push_notification("Auto-redact is not available", cx);
            return;
        };
        let doc = self.export_doc();
        self.busy = Some("Scanning for sensitive data…".into());
        cx.notify();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let img = rs_render::content_image(&doc).map_err(|e| e.to_string())?;
                    detect(&img)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match r {
                    Ok(items) if !items.is_empty() => {
                        this.redact_review = Some(RedactReview { items: items.into_iter().map(|m| (m, true)).collect(), style: 0 });
                    }
                    Ok(_) => window.push_notification("No sensitive data found", cx),
                    Err(e) => this.last_error = Some(format!("Auto-redact failed: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_redact_review(&mut self, cx: &mut Context<Self>) {
        let Some(rv) = self.redact_review.take() else { return };
        let sf = self.sf();
        let anns: Vec<rs_core::Annotation> = rv
            .items
            .iter()
            .filter(|(_, on)| *on)
            .map(|(m, _)| {
                let rect = m.bounds.outset(2.0 * sf);
                let kind = match rv.style {
                    1 => AnnotationKind::Blur(rs_core::BlurAnnotation { rect, strength: 10.0 * sf, corner_radius: 0.0 }),
                    2 => AnnotationKind::Pixelate(rs_core::PixelateAnnotation {
                        rect,
                        block_size: ((8.0 * sf) as u32).max(2),
                    }),
                    _ => AnnotationKind::Redact(rs_core::RedactAnnotation {
                        rect,
                        color: rs_core::Color::BLACK,
                        corner_radius: 2.0 * sf,
                    }),
                };
                rs_core::Annotation::new(kind)
            })
            .collect();
        if !anns.is_empty() {
            self.model.add_annotations(anns);
        }
        self.changed(cx);
    }

    fn load_fonts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut names = window.text_system().all_font_names();
        names.retain(|n| !n.starts_with('.'));
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        self.panel.set_fonts(names, window, cx);
    }
}

/// Downscale to fit `max` px (for project thumbnails).
fn thumbnail(img: &RgbaImage, max: u32) -> RgbaImage {
    let Some(src) = image::RgbaImage::from_raw(img.width, img.height, img.pixels.as_ref().clone()) else {
        return img.clone();
    };
    let t = image::imageops::thumbnail(&src, max.min(img.width), max.min(img.height) * img.height.max(1) / img.height.max(1));
    let k = (max as f32 / img.width.max(img.height).max(1) as f32).min(1.0);
    let (w, h) = (((img.width as f32 * k) as u32).max(1), ((img.height as f32 * k) as u32).max(1));
    let t = if t.width() != w || t.height() != h { image::imageops::thumbnail(&src, w, h) } else { t };
    RgbaImage::new(t.width(), t.height(), t.into_raw()).unwrap_or_else(|| img.clone())
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ----------------------------------------------------------------------
// IME / platform text input
// ----------------------------------------------------------------------

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let b = &self.model.editing.as_ref()?.buffer;
        let r = b.range_from_utf16(&range_utf16);
        let r = r.start.min(b.text.len())..r.end.min(b.text.len());
        adjusted.replace(b.range_to_utf16(&r));
        Some(b.text[r].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let b = &self.model.editing.as_ref()?.buffer;
        Some(UTF16Selection { range: b.range_to_utf16(&b.sel), reversed: b.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let b = &self.model.editing.as_ref()?.buffer;
        b.marked.as_ref().map(|m| b.range_to_utf16(m))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.model.editing {
            s.buffer.marked = None;
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = &self.model.editing else { return };
        let r = range_utf16.map(|r| s.buffer.range_from_utf16(&r));
        self.model.edit_text(|b| b.replace(r, text));
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = &self.model.editing else { return };
        let r = range_utf16.map(|r| s.buffer.range_from_utf16(&r));
        // New selection is relative to new_text (UTF-16 → UTF-8 within it).
        let sel = new_selected_range_utf16.map(|r| {
            let conv = |u: usize| {
                let mut u16s = 0;
                let mut u8s = 0;
                for ch in new_text.chars() {
                    if u16s >= u {
                        break;
                    }
                    u16s += ch.len_utf16();
                    u8s += ch.len_utf8();
                }
                u8s
            };
            conv(r.start)..conv(r.end)
        });
        self.model.edit_text(|b| b.replace_and_mark(r, new_text, sel));
        // Keep the annotation in sync even if the text is unchanged (mark only).
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let b = &self.model.editing.as_ref()?.buffer;
        let r = b.range_from_utf16(&range_utf16);
        let l = self.edit_layout.borrow();
        // Same rs-render geometry the caret / selection overlays use.
        Some(l.as_ref()?.bounds_for_range(r))
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let idx = self.edit_layout.borrow().as_ref()?.index_at(p);
        let b = &self.model.editing.as_ref()?.buffer;
        Some(b.to_utf16(idx))
    }
}

// ----------------------------------------------------------------------
// Canvas scene (built in render, painted in the canvas element)
// ----------------------------------------------------------------------

struct TextScene {
    /// The text box rs-render draws (content space).
    ann: rs_core::TextAnnotation,
    sel: Range<usize>,
    cursor: usize,
    marked: Option<Range<usize>>,
    caret_on: bool,
    /// Window px per content px (for the placeholder font size).
    k: f32,
}

struct CanvasScene {
    frame: Option<Arc<RenderImage>>,
    canvas_size1: (f32, f32),
    k: f32,
    pan: Point<Pixels>,
    layout1: CanvasLayout,
    content_size: rs_core::Size,
    selection: Vec<CRect>,
    handles: Vec<(CPoint, bool)>,
    /// Bounding box of a multi-selection (with handles in `handles`).
    multi_box: Option<CRect>,
    control_lines: Vec<(CPoint, CPoint)>,
    hover: Option<CRect>,
    marquee: Option<CRect>,
    guides: Vec<crate::snap::Guide>,
    crop: Option<CRect>,
    create_outline: Option<(CPoint, CPoint, bool)>,
    text: Option<TextScene>,
    text_hover: Option<(CPoint, f32)>,
    checker: bool,
    cursor: CursorStyle,
    geom: Rc<Cell<CanvasGeom>>,
    edit_layout: Rc<RefCell<Option<EditGeom>>>,
    focus: FocusHandle,
    entity: Entity<EditorView>,
    accent: Hsla,
}

impl EditorView {
    fn build_scene(&mut self, cx: &mut Context<Self>) -> CanvasScene {
        let m = &mut self.model;
        let k = self.zoom / m.doc.metadata.scale_factor.max(0.01);
        let mut selection = Vec::new();
        let mut handles = Vec::new();
        let mut control_lines = Vec::new();
        let editing_id = m.editing.as_ref().map(|s| s.id);
        let sel = m.selection.clone();
        for id in &sel {
            if Some(*id) == editing_id && matches!(m.editing.as_ref().map(|s| &s.annotation.kind), Some(AnnotationKind::Text(_))) {
                continue;
            }
            if let Some(a) = m.current(*id) {
                let b = m.bounds_of(&a);
                selection.push(b);
                if sel.len() == 1 && !matches!(m.interaction, Interaction::Move { .. }) && editing_id.is_none() {
                    for h in crate::hit::handles_for(&a, b) {
                        let round = !matches!(h.kind, crate::hit::HandleKind::Rect(_));
                        handles.push((h.pos, round));
                    }
                    if let AnnotationKind::Arrow(ar) = &a.kind
                        && let Some(c) = ar.control
                    {
                        let apex = crate::geom::quad_point(ar.start, c, ar.end, 0.5);
                        control_lines.push((ar.start.lerp(ar.end, 0.5), apex));
                    }
                    if let AnnotationKind::Magnifier(mg) = &a.kind {
                        control_lines.push((mg.source_center, mg.target_center));
                    }
                }
            }
        }
        let hover = match (m.hovered, &m.interaction) {
            (Some(id), Interaction::None) if !sel.contains(&id) => m.doc.get(id).cloned().map(|a| m.bounds_of(&a)),
            _ => None,
        };
        let marquee = match &m.interaction {
            Interaction::Marquee { start, current, .. } => Some(CRect::from_points(*start, *current)),
            _ => None,
        };
        let create_outline = match &m.interaction {
            Interaction::Create { tool, start, current, preview, .. } => match tool {
                ToolKind::Line | ToolKind::Arrow => Some((*start, *current, false)),
                ToolKind::Pen | ToolKind::Highlighter | ToolKind::Step | ToolKind::Magnifier => None,
                _ => preview.as_ref().map(|p| {
                    let b = p.geometric_bounds();
                    (b.origin(), CPoint::new(b.right(), b.bottom()), true)
                }),
            },
            _ => None,
        };
        let caret_on = self.caret_on;
        let text = m.editing.as_ref().and_then(|s| {
            let ann = match &s.annotation.kind {
                AnnotationKind::Text(t) => t.clone(),
                AnnotationKind::Callout(c) => rs_render::callout_text(c),
                _ => return None,
            };
            Some(TextScene {
                ann,
                sel: s.buffer.sel.clone(),
                cursor: s.buffer.cursor(),
                marked: s.buffer.marked.clone(),
                caret_on,
                k,
            })
        });
        let multi_box = if matches!(m.interaction, Interaction::Move { .. }) {
            None
        } else {
            m.multi_handles().map(|(u, hs)| {
                handles.extend(hs.into_iter().map(|(_, p)| (p, false)));
                u
            })
        };
        let text_hover = (m.tool == ToolKind::Text && m.editing.is_none() && matches!(m.interaction, Interaction::None))
            .then(|| m.hover.map(|h| (h, m.tool_style(ToolKind::Text).text.size * m.doc.metadata.scale_factor)))
            .flatten();
        let checker = !self.display_doc.beautify.enabled
            || matches!(self.display_doc.beautify.background, rs_core::Background::None);
        let cursor = if m.eyedropper {
            CursorStyle::Crosshair
        } else if m.editing.is_some() {
            CursorStyle::IBeam
        } else {
            match m.tool {
                ToolKind::Select => {
                    if m.hovered.is_some() || matches!(m.interaction, Interaction::Move { .. }) {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    }
                }
                ToolKind::Text => CursorStyle::IBeam,
                _ => CursorStyle::Crosshair,
            }
        };
        CanvasScene {
            frame: self.frame.as_ref().map(|f| f.image.clone()),
            canvas_size1: (self.layout1.canvas_width as f32, self.layout1.canvas_height as f32),
            k,
            pan: self.pan,
            layout1: self.layout1,
            content_size: self.display_doc.content_size(),
            selection,
            handles,
            multi_box,
            control_lines,
            hover,
            marquee,
            guides: m.guides.clone(),
            crop: m.crop,
            create_outline,
            text,
            text_hover,
            checker,
            cursor,
            geom: self.geom.clone(),
            edit_layout: self.edit_layout.clone(),
            focus: self.canvas_focus.clone(),
            entity: cx.entity(),
            accent: rgb(ACCENT).into(),
        }
    }
}

fn map_pt(g: &CanvasGeom, l: &CanvasLayout, c: CPoint) -> Point<Pixels> {
    let q = l.image_to_canvas(c);
    point(g.origin.x + px(q.x * g.k), g.origin.y + px(q.y * g.k))
}

fn map_rect(g: &CanvasGeom, l: &CanvasLayout, r: CRect) -> Bounds<Pixels> {
    let a = map_pt(g, l, r.origin());
    let b = map_pt(g, l, CPoint::new(r.right(), r.bottom()));
    Bounds::from_corners(point(a.x.min(b.x), a.y.min(b.y)), point(a.x.max(b.x), a.y.max(b.y)))
}

fn paint_line(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, w: f32, color: Hsla) {
    let mut pb = PathBuilder::stroke(px(w));
    pb.move_to(a);
    pb.line_to(b);
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

fn paint_canvas(bounds: Bounds<Pixels>, hitbox: Hitbox, s: CanvasScene, window: &mut Window, cx: &mut App) {
    // Geometry.
    let (cw, ch) = s.canvas_size1;
    let dw = cw * s.k;
    let dh = ch * s.k;
    let c = bounds.center();
    let origin = point(
        px((f32::from(c.x) - dw / 2.0 + f32::from(s.pan.x)).round()),
        px((f32::from(c.y) - dh / 2.0 + f32::from(s.pan.y)).round()),
    );
    let g = CanvasGeom { bounds, origin, k: s.k };
    s.geom.set(g);
    let l = &s.layout1;
    window.set_cursor_style(s.cursor, &hitbox);

    // Mouse listeners (window-level so drags continue outside the canvas).
    {
        let e = s.entity.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hb.is_hovered(window) {
                return;
            }
            match ev.button {
                MouseButton::Left => e.update(cx, |this, cx| this.canvas_mouse_down(ev, window, cx)),
                MouseButton::Right => e.update(cx, |this, cx| this.canvas_right_down(ev, window, cx)),
                _ => {}
            }
        });
        let e = s.entity.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                e.update(cx, |this, cx| this.canvas_mouse_move(ev, window, cx));
            }
        });
        let e = s.entity.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                e.update(cx, |this, cx| this.canvas_mouse_up(ev, window, cx));
            }
        });
        let e = s.entity.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                e.update(cx, |this, cx| this.on_scroll(ev, window, cx));
            }
        });
        let e = s.entity.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |ev: &PinchEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                e.update(cx, |this, cx| this.on_pinch(ev, window, cx));
            }
        });
    }

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let img_rect = Bounds::new(origin, size(px(dw), px(dh)));
        if s.checker {
            // Checkerboard under transparent canvases.
            let vis = img_rect.intersect(&bounds);
            let cell = 10.0f32;
            let x0 = f32::from(img_rect.origin.x);
            let y0 = f32::from(img_rect.origin.y);
            window.paint_quad(fill(vis, rgb(0xFFFFFF)));
            let (vx0, vy0) = (f32::from(vis.origin.x), f32::from(vis.origin.y));
            let (vx1, vy1) = (vx0 + f32::from(vis.size.width), vy0 + f32::from(vis.size.height));
            let i0 = ((vx0 - x0) / cell).floor() as i32;
            let j0 = ((vy0 - y0) / cell).floor() as i32;
            let i1 = ((vx1 - x0) / cell).ceil() as i32;
            let j1 = ((vy1 - y0) / cell).ceil() as i32;
            if (i1 - i0) * (j1 - j0) < 40_000 {
                for j in j0..j1 {
                    for i in i0..i1 {
                        if (i + j) % 2 == 0 {
                            continue;
                        }
                        let r = Bounds::new(point(px(x0 + i as f32 * cell), px(y0 + j as f32 * cell)), size(px(cell), px(cell)))
                            .intersect(&vis);
                        window.paint_quad(fill(r, rgb(0xDDDDDD)));
                    }
                }
            }
        }
        if let Some(img) = s.frame.clone() {
            let _ = window.paint_image(img_rect, img_rect, Corners::default(), img, 0, false);
        }

        let accent = s.accent;
        // Hover highlight.
        if let Some(h) = s.hover {
            window.paint_quad(outline(map_rect(&g, l, h).dilate(px(2.)), accent.opacity(0.6), BorderStyle::Solid));
        }
        // Selection boxes.
        for r in &s.selection {
            window.paint_quad(outline(map_rect(&g, l, *r).dilate(px(3.)), accent, BorderStyle::Solid));
        }
        if let Some(u) = s.multi_box {
            window.paint_quad(outline(map_rect(&g, l, u).dilate(px(6.)), accent.opacity(0.8), BorderStyle::Dashed));
        }
        for (a, b) in &s.control_lines {
            paint_line(window, map_pt(&g, l, *a), map_pt(&g, l, *b), 1.0, accent.opacity(0.7));
        }
        // Creation outline (instant feedback while the bitmap catches up).
        if let Some((a, b, is_rect)) = s.create_outline {
            if is_rect {
                window.paint_quad(outline(map_rect(&g, l, CRect::from_points(a, b)), accent.opacity(0.8), BorderStyle::Dashed));
            } else {
                paint_line(window, map_pt(&g, l, a), map_pt(&g, l, b), 1.0, accent.opacity(0.8));
            }
        }
        let content = map_rect(&g, l, CRect::new(0.0, 0.0, s.content_size.width, s.content_size.height));
        // Snap guides across the whole canvas (annotations can live over the
        // padding, and canvas edges/center are snap targets too).
        let canvas_px = Bounds::new(origin, size(px(dw), px(dh)));
        for gd in &s.guides {
            let color: Hsla = rgb(0xFF2D95).into();
            if gd.vertical {
                let x = map_pt(&g, l, CPoint::new(gd.pos, 0.0)).x;
                window.paint_quad(fill(Bounds::new(point(x, canvas_px.origin.y), size(px(1.), canvas_px.size.height)), color));
            } else {
                let y = map_pt(&g, l, CPoint::new(0.0, gd.pos)).y;
                window.paint_quad(fill(Bounds::new(point(canvas_px.origin.x, y), size(canvas_px.size.width, px(1.))), color));
            }
        }
        // Marquee.
        if let Some(r) = s.marquee {
            let b = map_rect(&g, l, r);
            window.paint_quad(quad(b, px(0.), accent.opacity(0.12), px(1.), accent, BorderStyle::Solid));
        }
        // Crop mode: dim outside, border, thirds, handles.
        if let Some(r) = s.crop {
            let b = map_rect(&g, l, r);
            let dim: Hsla = hsla(0., 0., 0., 0.55);
            let (x0, y0, x1, y1) = (content.origin.x, content.origin.y, content.origin.x + content.size.width, content.origin.y + content.size.height);
            let (bx0, by0, bx1, by1) = (b.origin.x, b.origin.y, b.origin.x + b.size.width, b.origin.y + b.size.height);
            for q in [
                Bounds::from_corners(point(x0, y0), point(x1, by0)),
                Bounds::from_corners(point(x0, by1), point(x1, y1)),
                Bounds::from_corners(point(x0, by0), point(bx0, by1)),
                Bounds::from_corners(point(bx1, by0), point(x1, by1)),
            ] {
                if q.size.width > px(0.) && q.size.height > px(0.) {
                    window.paint_quad(fill(q, dim));
                }
            }
            window.paint_quad(outline(b, white(), BorderStyle::Solid));
            for i in 1..3 {
                let fx = b.origin.x + b.size.width * (i as f32 / 3.0);
                let fy = b.origin.y + b.size.height * (i as f32 / 3.0);
                window.paint_quad(fill(Bounds::new(point(fx, by0), size(px(1.), b.size.height)), white().opacity(0.35)));
                window.paint_quad(fill(Bounds::new(point(bx0, fy), size(b.size.width, px(1.))), white().opacity(0.35)));
            }
            for h in crate::geom::RectHandle::ALL {
                let p = map_pt(&g, l, h.position(r));
                let hb = Bounds::new(point(p.x - px(5.), p.y - px(5.)), size(px(10.), px(10.)));
                window.paint_quad(quad(hb, px(2.), white(), px(1.), accent, BorderStyle::Solid));
            }
        }
        // Handles.
        for (p, round) in &s.handles {
            let p = map_pt(&g, l, *p);
            let hb = Bounds::new(point(p.x - px(5.), p.y - px(5.)), size(px(10.), px(10.)));
            window.paint_quad(quad(hb, if *round { px(5.) } else { px(2.) }, white(), px(1.5), accent, BorderStyle::Solid));
        }
        // Text tool hover preview: a caret where the text would start.
        if let Some((p, size_px)) = s.text_hover {
            let q = map_pt(&g, l, p);
            let h = px((size_px * s.k * 1.2).max(8.0));
            window.paint_quad(fill(Bounds::new(q, size(px(2.), h)), accent.opacity(0.8)));
        }
        // Inline text editor.
        if let Some(t) = s.text {
            paint_text_editor(&g, l, t, &s.focus, &s.entity, &s.edit_layout, bounds, window, cx);
        } else {
            *s.edit_layout.borrow_mut() = None;
        }
    });
}

/// Overlays for the text being edited. The glyphs themselves are in the
/// preview bitmap (rendered by rs-render, like committed text); only the
/// caret, selection, IME marked-text underline, box frame and the empty-text
/// placeholder are drawn here — all from rs-render's text geometry.
#[allow(clippy::too_many_arguments)]
fn paint_text_editor(
    g: &CanvasGeom,
    l: &CanvasLayout,
    t: TextScene,
    focus: &FocusHandle,
    entity: &Entity<EditorView>,
    edit_layout: &Rc<RefCell<Option<EditGeom>>>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let geom = EditGeom { ann: t.ann, layout1: *l, origin: g.origin, k: g.k };
    let text_box = geom.box_bounds();
    let caret0 = geom.caret(0);
    // Selection.
    for r in geom.selection_rects(t.sel.clone()) {
        window.paint_quad(fill(r, rgba(0x0A84FF55)));
    }
    // IME marked (composing) text: underline.
    if let Some(m) = t.marked.clone().filter(|m| !m.is_empty()) {
        let color = text_layout::to_hsla(geom.ann.style.color);
        for r in geom.selection_rects(m) {
            let y = r.origin.y + r.size.height - px(2.);
            window.paint_quad(fill(Bounds::new(point(r.origin.x, y), size(r.size.width, px(1.5))), color));
        }
    }
    // Box frame.
    let frame_w = text_box.size.width.max(caret0.size.height * 0.6);
    let frame = Bounds::new(text_box.origin, size(frame_w, text_box.size.height)).dilate(px(4.));
    window.paint_quad(quad(frame, px(3.), transparent_black(), px(1.), rgba(0x0A84FFAA), BorderStyle::Dashed));
    // Placeholder for empty text (GPUI text: it is never committed).
    if geom.ann.content.is_empty() {
        let style = &geom.ann.style;
        let font_size = px((style.size * t.k).max(1.0));
        let hint_color: Hsla = text_layout::to_hsla(style.color).opacity(0.35);
        let run = TextRun {
            len: "Type…".len(),
            font: text_layout::gpui_font(style),
            color: hint_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line("Type…".into(), font_size, &[run], None);
        let _ = line.paint(point(caret0.origin.x, caret0.origin.y), caret0.size.height, TextAlign::Left, None, window, cx);
    }
    // Blinking caret.
    if t.sel.is_empty() && t.caret_on && focus.is_focused(window) {
        let c = geom.caret(t.cursor);
        let w = c.size.width.max(px(2.));
        let x = c.origin.x + (c.size.width - w) / 2.0;
        window.paint_quad(fill(Bounds::new(point(x, c.origin.y), size(w, c.size.height)), rgb(ACCENT)));
    }
    window.handle_input(focus, ElementInputHandler::new(bounds, entity.clone()), cx);
    *edit_layout.borrow_mut() = Some(geom);
}

// ----------------------------------------------------------------------
// Render
// ----------------------------------------------------------------------

/// How the toolbar degrades at narrow widths (see `toolbar_tier`).
#[derive(Debug, Clone, Copy)]
struct ToolbarTier {
    fold_secondary: bool,
    save_label: bool,
    copy_label: bool,
    tool_gap: f32,
}

impl EditorView {
    /// Toolbar layout for the current window width: full → secondary
    /// actions folded into "…" → Save icon-only → Copy icon-only and tighter
    /// tool spacing. Widths are the measured button sizes (pt).
    fn toolbar_tier(&self, width: f32) -> ToolbarTier {
        let o = &self.options.services;
        let has_redact = o.detect_sensitive.is_some();
        let n_icons = [o.recognize_text.is_some(), o.load_project.is_some(), o.save_project.is_some()].iter().filter(|b| **b).count();
        const ICON: f32 = 24.0;
        const DRAG: f32 = 28.0;
        const LABELED: f32 = 66.0;
        const REDACT: f32 = 78.0;
        // Traffic lights, 16 tools, separator, undo/redo, the gaps around the
        // flexible spacer and the right padding.
        let left = |tool_gap: f32| 84.0 + 16.0 * ICON + 15.0 * tool_gap + 8.0 + 1.0 + 8.0 + 50.0 + 8.0 + 8.0 + 12.0;
        let right = |items: &[f32]| items.iter().sum::<f32>() + 8.0 * items.len().saturating_sub(1) as f32;
        let more: &[f32] = if has_redact || n_icons > 0 { &[ICON] } else { &[] };
        let mut full: Vec<f32> = if has_redact { vec![REDACT] } else { vec![] };
        full.extend(std::iter::repeat_n(ICON, n_icons));
        full.extend([DRAG, LABELED, LABELED]);
        let with = |tail: &[f32]| [more, tail].concat();
        let tier = |fold_secondary, save_label, copy_label, tool_gap| ToolbarTier { fold_secondary, save_label, copy_label, tool_gap };
        if width >= left(2.0) + right(&full) {
            tier(false, true, true, 2.0)
        } else if width >= left(2.0) + right(&with(&[DRAG, LABELED, LABELED])) {
            tier(true, true, true, 2.0)
        } else if width >= left(2.0) + right(&with(&[DRAG, ICON, LABELED])) {
            tier(true, false, true, 2.0)
        } else {
            tier(true, false, width >= left(0.0) + right(&with(&[DRAG, ICON, LABELED])), 0.0)
        }
    }

    fn tool_button(&self, t: ToolKind, icon: Ico, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let key = t.shortcut().to_ascii_uppercase();
        Button::new(SharedString::from(format!("tool-{}", t.name())))
            .icon(icon.icon())
            .ghost()
            .small()
            .selected(self.model.tool == t)
            .tooltip(format!("{} ({key})", t.name()))
            .on_click(cx.listener(move |this, _, window, cx| this.set_tool(t, window, cx)))
    }

    fn render_toolbar(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let tier = self.toolbar_tier(f32::from(window.viewport_size().width));
        use crate::panel_icons::PIco;
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let tools: [(ToolKind, Ico); 16] = [
            (ToolKind::Select, Ico::MousePointer2),
            (ToolKind::Rect, Ico::Square),
            (ToolKind::Ellipse, Ico::Circle),
            (ToolKind::Line, Ico::Minus),
            (ToolKind::Arrow, Ico::MoveUpRight),
            (ToolKind::Pen, Ico::PenLine),
            (ToolKind::Highlighter, Ico::Highlighter),
            (ToolKind::Step, Ico::CircleDot),
            (ToolKind::Callout, Ico::MessageSquareText),
            (ToolKind::Spotlight, Ico::Spotlight),
            (ToolKind::Magnifier, Ico::ZoomIn),
            (ToolKind::Blur, Ico::Droplets),
            (ToolKind::Pixelate, Ico::Grid3x3),
            (ToolKind::Redact, Ico::RectangleHorizontal),
            (ToolKind::Text, Ico::Type),
            (ToolKind::Crop, Ico::Crop),
        ];
        let can_undo = self.model.history.can_undo();
        let can_redo = self.model.history.can_redo();
        let has_ocr = self.options.services.recognize_text.is_some();
        let has_redact = self.options.services.detect_sensitive.is_some();
        let has_project = self.options.services.save_project.is_some();
        let has_open = self.options.services.load_project.is_some();
        let mut tool_row = h_flex().gap(px(tier.tool_gap));
        for (t, i) in tools {
            tool_row = tool_row.child(self.tool_button(t, i, cx));
        }
        h_flex()
            .id("toolbar")
            .h(px(TOOLBAR_H))
            .w_full()
            .pl(px(84.))
            .pr_3()
            .gap_2()
            .items_center()
            .bg(theme.title_bar)
            .border_b_1()
            .border_color(theme.border)
            .on_mouse_down(MouseButton::Left, |ev, window, _| {
                if ev.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
            .child(tool_row)
            .child(div().w(px(1.)).h(px(24.)).bg(theme.border))
            .child(
                h_flex()
                    .gap_0p5()
                    .child(
                        Button::new("undo")
                            .icon(Ico::Undo2.icon())
                            .ghost()
                            .small()
                            .disabled(!can_undo)
                            .tooltip("Undo (⌘Z)")
                            .on_click(cx.listener(|this, _, window, cx| this.undo(&Undo, window, cx))),
                    )
                    .child(
                        Button::new("redo")
                            .icon(Ico::Redo2.icon())
                            .ghost()
                            .small()
                            .disabled(!can_redo)
                            .tooltip("Redo (⇧⌘Z)")
                            .on_click(cx.listener(|this, _, window, cx| this.redo(&Redo, window, cx))),
                    ),
            )
            .child(div().flex_1().min_w_0())
            .when(tier.fold_secondary && (has_redact || has_ocr || has_open || has_project), |d| {
                let weak = cx.entity().downgrade();
                d.child(
                    Button::new("toolbar-more")
                        .icon(PIco::Ellipsis.icon())
                        .ghost()
                        .small()
                        .tooltip("More actions")
                        .dropdown_menu(move |menu, _, _| {
                            let item = |label: &str, icon: Ico, f: fn(&mut EditorView, &mut Window, &mut Context<EditorView>)| {
                                let weak = weak.clone();
                                PopupMenuItem::new(label.to_string()).icon(icon.icon()).on_click(move |_, window, cx| {
                                    let _ = weak.update(cx, |this, cx| f(this, window, cx));
                                })
                            };
                            let mut menu = menu;
                            if has_redact {
                                menu = menu.item(item("Auto-redact  ⇧⌘R", Ico::ShieldCheck, |t, w, cx| t.auto_redact(w, cx)));
                            }
                            if has_ocr {
                                menu = menu.item(item("Copy text  ⇧⌘T", Ico::ScanText, |t, w, cx| t.copy_text_ocr(w, cx)));
                            }
                            if has_open {
                                menu = menu.item(item("Open project…", Ico::FolderOpen, |t, w, cx| t.open_project(w, cx)));
                            }
                            if has_project {
                                menu = menu.item(item("Save project…  ⇧⌘S", Ico::FileArchive, |t, w, cx| t.save_project(w, cx)));
                            }
                            menu
                        }),
                )
            })
            .when(has_redact && !tier.fold_secondary, |d| {
                d.child(
                    Button::new("auto-redact")
                        .icon(Ico::ShieldCheck.icon())
                        .ghost()
                        .small()
                        .label("Redact")
                        .tooltip("Auto-redact emails, keys, cards, IPs, phones (⇧⌘R)")
                        .on_click(cx.listener(|this, _, window, cx| this.auto_redact(window, cx))),
                )
            })
            .when(has_ocr && !tier.fold_secondary, |d| {
                d.child(
                    Button::new("copy-text")
                        .icon(Ico::ScanText.icon())
                        .ghost()
                        .small()
                        .tooltip("Copy text from image (⇧⌘T)")
                        .on_click(cx.listener(|this, _, window, cx| this.copy_text_ocr(window, cx))),
                )
            })
            .when(has_open && !tier.fold_secondary, |d| {
                d.child(
                    Button::new("open-project")
                        .icon(Ico::FolderOpen.icon())
                        .ghost()
                        .small()
                        .tooltip("Open project (.rsnap)…")
                        .on_click(cx.listener(|this, _, window, cx| this.open_project(window, cx))),
                )
            })
            .when(has_project && !tier.fold_secondary, |d| {
                d.child(
                    Button::new("save-project")
                        .icon(Ico::FileArchive.icon())
                        .ghost()
                        .small()
                        .tooltip("Save project (.rsnap) — annotations stay editable (⇧⌘S)")
                        .on_click(cx.listener(|this, _, window, cx| this.save_project(window, cx))),
                )
            })
            .child(
                div()
                    .id("drag-out")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .cursor(CursorStyle::OpenHand)
                    .hover(|s| s.bg(theme.secondary_hover))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Drag into any app · click to reveal file").build(window, cx))
                    .child(Ico::GripVertical.icon().small())
                    .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                        if *hovered {
                            this.prepare_drag_file(window, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.drag_out.armed = true;
                            this.drag_out.started = false;
                            this.prepare_drag_file(window, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                        if this.drag_out.armed && !this.drag_out.started && ev.pressed_button == Some(MouseButton::Left) {
                            this.start_drag_out(window, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, _| {
                            if this.drag_out.armed && !this.drag_out.started
                                && let Some((_, p)) = &this.drag_out.ready
                            {
                                crate::dragout::reveal_in_finder(p);
                            }
                            this.drag_out.armed = false;
                        }),
                    ),
            )
            .child(
                Button::new("save")
                    .icon(Ico::Download.icon())
                    .small()
                    .when(tier.save_label, |b| b.label("Save"))
                    .tooltip("Save image (⌘S)")
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
            .child(
                Button::new("copy")
                    .icon(Ico::Copy.icon())
                    .primary()
                    .small()
                    .when(tier.copy_label, |b| b.label("Copy"))
                    .tooltip("Copy image to clipboard (⌘C)")
                    .on_click(cx.listener(|this, _, window, cx| this.copy_image(window, cx))),
            )
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let hint = if self.model.editing.is_some() {
            "⏎ new line · ⌘⏎ or Esc to finish · click outside to commit".to_string()
        } else if self.model.crop.is_some() {
            "Drag to crop · ⏎ apply · Esc cancel".to_string()
        } else if self.model.eyedropper {
            "Click the image to pick a color · Esc cancel".to_string()
        } else {
            match self.model.tool {
                ToolKind::Select => "Click to select · ⇧ multi · drag to move · ⌥-drag duplicates · ⌘ disables snapping".into(),
                ToolKind::Text => "Click to type · drag to create a wrapping box · double-click text to edit".into(),
                ToolKind::Arrow => "Drag to draw · ⇧ 45° · drag the middle handle to curve".into(),
                ToolKind::Step => "Click to place the next number".into(),
                ToolKind::Magnifier => "Drag from the area to magnify to where the loupe goes".into(),
                t => format!("{} · ⇧ constrain · ⌥ from center", t.name()),
            }
        };
        let size = self.model.doc.content_size();
        let zoom = self.zoom;
        h_flex()
            .h(px(STATUS_H))
            .w_full()
            .px_3()
            .gap_3()
            .items_center()
            .text_xs()
            .text_color(theme.muted_foreground)
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.title_bar)
            .child(format!("{}×{} px", size.width as u32, size.height as u32))
            .child(
                h_flex()
                    .gap_0p5()
                    .items_center()
                    .child(
                        Button::new("zoom-out")
                            .icon(Ico::ZoomOut.icon())
                            .ghost()
                            .xsmall()
                            .tooltip("Zoom out (⌘-)")
                            .on_click(cx.listener(|this, _, window, cx| this.set_zoom(Some(this.zoom / 1.25), window, cx))),
                    )
                    .child(
                        Button::new("zoom-label")
                            .ghost()
                            .xsmall()
                            .label(format!("{:.0}%", zoom * 100.0))
                            .tooltip("Actual size (⌘1)")
                            .on_click(cx.listener(|this, _, window, cx| this.set_zoom(Some(1.0), window, cx))),
                    )
                    .child(
                        Button::new("zoom-in")
                            .icon(Ico::ZoomIn.icon())
                            .ghost()
                            .xsmall()
                            .tooltip("Zoom in (⌘+)")
                            .on_click(cx.listener(|this, _, window, cx| this.set_zoom(Some(this.zoom * 1.25), window, cx))),
                    )
                    .child(
                        Button::new("zoom-fit")
                            .icon(Ico::Maximize.icon())
                            .ghost()
                            .xsmall()
                            .selected(self.fit)
                            .tooltip("Fit (⌘0)")
                            .on_click(cx.listener(|this, _, window, cx| this.set_zoom(None, window, cx))),
                    ),
            )
            .child(div().flex_1().overflow_hidden().child(hint))
            .when_some(self.busy.clone(), |d, b| d.child(div().text_color(theme.foreground).child(b)))
            .when_some(self.last_error.clone(), |d, e| d.child(div().text_color(rgb(0xFF5555)).child(e)))
    }

    fn render_redact_review(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rv = self.redact_review.as_ref()?;
        let theme = cx.theme().clone();
        let mut list = v_flex().gap_1();
        for (i, (m, on)) in rv.items.iter().enumerate() {
            list = list.child(
                Checkbox::new(SharedString::from(format!("redact-{i}")))
                    .label(format!("{} — {}", m.kind, m.text))
                    .checked(*on)
                    .on_click(cx.listener(move |this, v: &bool, _, cx| {
                        if let Some(rv) = &mut this.redact_review
                            && let Some(it) = rv.items.get_mut(i)
                        {
                            it.1 = *v;
                        }
                        cx.notify();
                    })),
            );
        }
        let style = rv.style;
        let mut styles = h_flex().gap_1();
        for (i, label) in ["Solid", "Blur", "Pixelate"].into_iter().enumerate() {
            styles = styles.child(
                Button::new(SharedString::from(format!("redact-style-{i}")))
                    .small()
                    .label(label)
                    .selected(style == i)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(rv) = &mut this.redact_review {
                            rv.style = i;
                        }
                        cx.notify();
                    })),
            );
        }
        let count = rv.items.iter().filter(|(_, on)| *on).count();
        Some(
            div()
                .absolute()
                .top(px(16.))
                .right(px(16.))
                .w(px(360.))
                .max_h(px(480.))
                .p_3()
                .rounded_lg()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Review sensitive data"))
                        .child(div().id("redact-list").max_h(px(300.)).overflow_y_scroll().child(list))
                        .child(styles)
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(Button::new("redact-cancel").small().label("Cancel").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.redact_review = None;
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    Button::new("redact-apply")
                                        .small()
                                        .primary()
                                        .label(format!("Redact {count}"))
                                        .on_click(cx.listener(|this, _, _, cx| this.apply_redact_review(cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let scene = self.build_scene(cx);
        let editing = self.model.editing.is_some();
        let _ = window;
        div()
            .id("canvas-area")
            .flex_1()
            .h_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x1E1E20))
            .key_context(if editing { TEXT_CONTEXT } else { CANVAS_CONTEXT })
            .track_focus(&self.canvas_focus)
            // Tools.
            .on_action(cx.listener(|t, _: &ToolSelect, w, cx| t.set_tool(ToolKind::Select, w, cx)))
            .on_action(cx.listener(|t, _: &ToolRect, w, cx| t.set_tool(ToolKind::Rect, w, cx)))
            .on_action(cx.listener(|t, _: &ToolEllipse, w, cx| t.set_tool(ToolKind::Ellipse, w, cx)))
            .on_action(cx.listener(|t, _: &ToolLine, w, cx| t.set_tool(ToolKind::Line, w, cx)))
            .on_action(cx.listener(|t, _: &ToolArrow, w, cx| t.set_tool(ToolKind::Arrow, w, cx)))
            .on_action(cx.listener(|t, _: &ToolPen, w, cx| t.set_tool(ToolKind::Pen, w, cx)))
            .on_action(cx.listener(|t, _: &ToolHighlighter, w, cx| t.set_tool(ToolKind::Highlighter, w, cx)))
            .on_action(cx.listener(|t, _: &ToolStep, w, cx| t.set_tool(ToolKind::Step, w, cx)))
            .on_action(cx.listener(|t, _: &ToolCallout, w, cx| t.set_tool(ToolKind::Callout, w, cx)))
            .on_action(cx.listener(|t, _: &ToolSpotlight, w, cx| t.set_tool(ToolKind::Spotlight, w, cx)))
            .on_action(cx.listener(|t, _: &ToolMagnifier, w, cx| t.set_tool(ToolKind::Magnifier, w, cx)))
            .on_action(cx.listener(|t, _: &ToolBlur, w, cx| t.set_tool(ToolKind::Blur, w, cx)))
            .on_action(cx.listener(|t, _: &ToolPixelate, w, cx| t.set_tool(ToolKind::Pixelate, w, cx)))
            .on_action(cx.listener(|t, _: &ToolRedact, w, cx| t.set_tool(ToolKind::Redact, w, cx)))
            .on_action(cx.listener(|t, _: &ToolText, w, cx| t.set_tool(ToolKind::Text, w, cx)))
            .on_action(cx.listener(|t, _: &ToolCrop, w, cx| t.set_tool(ToolKind::Crop, w, cx)))
            // Canvas commands.
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(|t, _: &DeleteSelection, _, cx| {
                t.model.delete_selection();
                t.changed(cx)
            }))
            .on_action(cx.listener(|t, _: &Duplicate, _, cx| {
                t.model.duplicate_selection();
                t.changed(cx)
            }))
            .on_action(cx.listener(|t, _: &SelectAll, _, cx| {
                t.model.select_all();
                t.changed(cx)
            }))
            .on_action(cx.listener(|t, _: &NudgeLeft, _, cx| t.nudge(-1.0, 0.0, cx)))
            .on_action(cx.listener(|t, _: &NudgeRight, _, cx| t.nudge(1.0, 0.0, cx)))
            .on_action(cx.listener(|t, _: &NudgeUp, _, cx| t.nudge(0.0, -1.0, cx)))
            .on_action(cx.listener(|t, _: &NudgeDown, _, cx| t.nudge(0.0, 1.0, cx)))
            .on_action(cx.listener(|t, _: &NudgeLeftBig, _, cx| t.nudge(-10.0, 0.0, cx)))
            .on_action(cx.listener(|t, _: &NudgeRightBig, _, cx| t.nudge(10.0, 0.0, cx)))
            .on_action(cx.listener(|t, _: &NudgeUpBig, _, cx| t.nudge(0.0, -10.0, cx)))
            .on_action(cx.listener(|t, _: &NudgeDownBig, _, cx| t.nudge(0.0, 10.0, cx)))
            .on_action(cx.listener(|t, _: &BringForward, _, cx| {
                t.model.zorder(ZOrder::Forward);
                t.changed(cx)
            }))
            .on_action(cx.listener(|t, _: &SendBackward, _, cx| {
                t.model.zorder(ZOrder::Backward);
                t.changed(cx)
            }))
            .on_action(cx.listener(|t, _: &BringToFront, _, cx| {
                t.model.zorder(ZOrder::Front);
                t.changed(cx)
            }))
            .on_action(cx.listener(|t, _: &SendToBack, _, cx| {
                t.model.zorder(ZOrder::Back);
                t.changed(cx)
            }))
            .on_action(cx.listener(Self::canvas_copy))
            .on_action(cx.listener(Self::copy_selection_action))
            .on_action(cx.listener(Self::canvas_cut))
            .on_action(cx.listener(Self::canvas_paste))
            // Text editing.
            .on_action(cx.listener(|t, _: &TextBackspace, _, cx| t.text_op(cx, |b| b.backspace())))
            .on_action(cx.listener(|t, _: &TextDeleteWordBack, _, cx| t.text_op(cx, |b| b.backspace_word())))
            .on_action(cx.listener(|t, _: &TextDelete, _, cx| t.text_op(cx, |b| b.delete_forward())))
            .on_action(cx.listener(|t, _: &TextLeft, _, cx| t.text_op(cx, |b| b.move_left(false))))
            .on_action(cx.listener(|t, _: &TextRight, _, cx| t.text_op(cx, |b| b.move_right(false))))
            .on_action(cx.listener(|t, _: &TextSelectLeft, _, cx| t.text_op(cx, |b| b.move_left(true))))
            .on_action(cx.listener(|t, _: &TextSelectRight, _, cx| t.text_op(cx, |b| b.move_right(true))))
            .on_action(cx.listener(|t, _: &TextWordLeft, _, cx| t.text_op(cx, |b| b.word_left(false))))
            .on_action(cx.listener(|t, _: &TextWordRight, _, cx| t.text_op(cx, |b| b.word_right(false))))
            .on_action(cx.listener(|t, _: &TextSelectWordLeft, _, cx| t.text_op(cx, |b| b.word_left(true))))
            .on_action(cx.listener(|t, _: &TextSelectWordRight, _, cx| t.text_op(cx, |b| b.word_right(true))))
            .on_action(cx.listener(|t, _: &TextHome, _, cx| t.text_op(cx, |b| b.home(false))))
            .on_action(cx.listener(|t, _: &TextEnd, _, cx| t.text_op(cx, |b| b.end(false))))
            .on_action(cx.listener(|t, _: &TextSelectHome, _, cx| t.text_op(cx, |b| b.home(true))))
            .on_action(cx.listener(|t, _: &TextSelectEnd, _, cx| t.text_op(cx, |b| b.end(true))))
            .on_action(cx.listener(|t, _: &TextDocStart, _, cx| t.text_op(cx, |b| b.set_cursor(0, false))))
            .on_action(cx.listener(|t, _: &TextDocEnd, _, cx| t.text_op(cx, |b| b.set_cursor(usize::MAX, false))))
            .on_action(cx.listener(|t, _: &TextSelectAll, _, cx| t.text_op(cx, |b| b.select_all())))
            .on_action(cx.listener(|t, _: &TextUp, _, cx| t.text_vertical(false, false, cx)))
            .on_action(cx.listener(|t, _: &TextDown, _, cx| t.text_vertical(true, false, cx)))
            .on_action(cx.listener(|t, _: &TextSelectUp, _, cx| t.text_vertical(false, true, cx)))
            .on_action(cx.listener(|t, _: &TextSelectDown, _, cx| t.text_vertical(true, true, cx)))
            .on_action(cx.listener(|t, _: &TextCopy, _, cx| t.text_copy(false, cx)))
            .on_action(cx.listener(|t, _: &TextCut, _, cx| t.text_copy(true, cx)))
            .on_action(cx.listener(|t, _: &TextPaste, _, cx| t.text_paste(cx)))
            .on_action(cx.listener(|t, _: &TextNewline, _, cx| t.text_op(cx, |b| b.insert("\n"))))
            .on_action(cx.listener(Self::text_commit))
            .on_action(cx.listener(|t, _: &TextUndo, _, cx| {
                t.text_op(cx, |b| {
                    b.undo();
                })
            }))
            .on_action(cx.listener(|t, _: &TextRedo, _, cx| {
                t.text_op(cx, |b| {
                    b.redo();
                })
            }))
            .on_action(cx.listener(|_, _: &TextCharacterPalette, w, _| w.show_character_palette()))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds, hitbox, window, cx| paint_canvas(bounds, hitbox, scene, window, cx),
                )
                .size_full(),
            )
            .children(self.render_redact_review(cx))
            .context_menu(|menu, _, _| {
                menu.menu("Bring to Front", Box::new(BringToFront))
                    .menu("Bring Forward", Box::new(BringForward))
                    .menu("Send Backward", Box::new(SendBackward))
                    .menu("Send to Back", Box::new(SendToBack))
                    .separator()
                    .menu("Duplicate", Box::new(Duplicate))
                    .menu("Copy", Box::new(CopySelection))
                    .menu("Paste", Box::new(CanvasPaste))
                    .separator()
                    .menu("Delete", Box::new(DeleteSelection))
            })
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_render(window, cx);
        self.sync_blink(cx);
        let theme = cx.theme().clone();
        let toolbar = self.render_toolbar(window, cx);
        let canvas = self.render_canvas(window, cx);
        let panel = self.render_panel(window, cx);
        let status = self.render_status(cx);
        v_flex()
            .id("rs-editor")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Copy, window, cx| this.copy_image(window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &SaveProject, window, cx| this.save_project(window, cx)))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| this.set_zoom(Some(this.zoom * 1.25), window, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| this.set_zoom(Some(this.zoom / 1.25), window, cx)))
            .on_action(cx.listener(|this, _: &ZoomFit, window, cx| this.set_zoom(None, window, cx)))
            .on_action(cx.listener(|this, _: &ZoomActual, window, cx| this.set_zoom(Some(1.0), window, cx)))
            .on_action(cx.listener(|this, _: &CopyText, window, cx| this.copy_text_ocr(window, cx)))
            .on_action(cx.listener(|this, _: &AutoRedact, window, cx| this.auto_redact(window, cx)))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(toolbar)
            .child(h_flex().flex_1().w_full().overflow_hidden().relative().child(canvas).child(panel))
            .child(status)
    }
}

// ----------------------------------------------------------------------
// Caret blink
// ----------------------------------------------------------------------

/// Standard macOS caret blink half-period.
pub(crate) const CARET_BLINK: Duration = Duration::from_millis(530);

impl EditorView {
    /// Restart the blink (caret visible) whenever the text or selection
    /// changes; stop the timer when no text is being edited.
    fn sync_blink(&mut self, cx: &mut Context<Self>) {
        let key = self.model.editing.as_ref().map(|s| (s.buffer.text.clone(), s.buffer.sel.clone()));
        if key == self.blink_key && (key.is_none() || self.blink_task.is_some()) {
            return;
        }
        let editing = key.is_some();
        self.blink_key = key;
        self.caret_on = true;
        if !editing {
            self.blink_task = None;
            return;
        }
        self.blink_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                let alive = this
                    .update(cx, |v, cx| {
                        if v.model.editing.is_none() {
                            return false;
                        }
                        v.caret_on = !v.caret_on;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
    }

    pub(crate) fn caret_on(&self) -> bool {
        self.caret_on
    }

    pub(crate) fn blinking(&self) -> bool {
        self.blink_task.is_some()
    }
}

/// Used by the self-test to poke at the view.
impl EditorView {
    pub(crate) fn zoom(&self) -> f32 {
        self.zoom
    }

    pub(crate) fn is_fit(&self) -> bool {
        self.fit
    }

    /// Canvas area size (window pt) as painted last frame.
    pub(crate) fn canvas_viewport(&self) -> (f32, f32) {
        let b = self.geom.get().bounds;
        (f32::from(b.size.width), f32::from(b.size.height))
    }

    /// Displayed canvas size (window pt) at the current zoom.
    pub(crate) fn canvas_display_size(&self) -> (f32, f32) {
        let k = self.view_k();
        (self.layout1.canvas_width as f32 * k, self.layout1.canvas_height as f32 * k)
    }

    pub(crate) fn model_mut(&mut self) -> &mut EditorModel {
        &mut self.model
    }

    pub(crate) fn frame_generation(&self) -> Option<u64> {
        self.frame.as_ref().map(|f| f.generation)
    }

    pub(crate) fn content_to_window(&self, c: CPoint) -> Point<Pixels> {
        self.to_window(c)
    }

    pub(crate) fn export_to_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(Result<(), String>, &mut Window, &mut App) + 'static,
    ) {
        let doc = self.export_doc();
        let rx = self.worker.export(doc, ExportSettings { format: ExportFormat::Png, ..Default::default() });
        cx.spawn_in(window, async move |_this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let r = rx.recv().await.map_err(|e| e.to_string()).and_then(|r| r);
            let r = r.and_then(|(bytes, _)| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
            let _ = cx.update(|window, cx| done(r, window, cx));
        })
        .detach();
    }

}

