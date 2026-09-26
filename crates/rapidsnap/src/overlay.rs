//! Capture overlay: one borderless, topmost window per display showing the
//! frozen (dimmed) screen, with region selection (crosshair, live size,
//! loupe, arrow-key nudging) and window selection (hover highlight).
//!
//! State lives in one shared [`OverlaySession`] entity (global screen
//! points); every [`OverlayView`] renders its display's part of it. All input
//! (mouse from any overlay window, keys from the key window, synthetic input
//! from the self-test) goes through the same `pointer_*` / `key_*` methods.
//!
//! Window recipe (verified, CONTRACTS §9): `WindowKind::PopUp` +
//! `titlebar: None` + transparent background + display bounds, then
//! `platform::make_overlay_window` (borderless, level 1000, all Spaces) —
//! which also keeps the overlay out of tiling window managers (Amethyst
//! only manages normal-level standard windows).

use std::sync::Arc;

use gpui_kit::*;
use rs_capture::{Captured, DisplayInfo, WindowInfo};
use rs_core::{Point, Rect};

use crate::platform;

/// What the overlay selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMode {
    Region,
    Window,
}

/// One display's frozen frame.
pub struct FrozenDisplay {
    pub info: DisplayInfo,
    /// None → no frozen frame (live overlay; capture after it closes).
    pub capture: Option<Captured>,
    pub image: Option<Arc<RenderImage>>,
}

/// Result of an overlay session.
#[derive(Debug, Clone)]
pub enum OverlayOutcome {
    Cancelled,
    /// `rect` in global points (may span displays); `display` = the one it
    /// overlaps most.
    Region { rect: Rect, display: usize },
    Window { window: WindowInfo },
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    anchor: Point,
    current: Point,
    /// Display the drag started on; the selection is clamped to it.
    display: usize,
    /// Started with Enter (arrows resize, Enter confirms) instead of the mouse.
    keyboard: bool,
}

type DoneCallback = Box<dyn FnOnce(OverlayOutcome, &mut App)>;

/// Shared state of all overlay windows of one capture.
pub struct OverlaySession {
    pub displays: Vec<FrozenDisplay>,
    /// False until the freeze-frame arrived (windows render nothing but
    /// transparent pixels meanwhile, so a concurrent capture can't see them).
    pub frames_ready: bool,
    /// Front to back.
    pub windows: Vec<WindowInfo>,
    pub mode: OverlayMode,
    /// Global points.
    pub cursor: Point,
    drag: Option<Drag>,
    /// Space held during a drag → move the selection instead of resizing.
    space_held: bool,
    last_pointer: Point,
    hovered: Option<usize>,
    /// Display whose overlay window is currently key.
    pub key_display: Option<usize>,
    /// Outcome waiting for the freeze-frame (selection finished early).
    pending: Option<OverlayOutcome>,
    on_done: Option<DoneCallback>,
}

impl OverlaySession {
    pub fn new(displays: Vec<FrozenDisplay>, mode: OverlayMode, cursor: Point, on_done: DoneCallback) -> Self {
        let key_display = displays.iter().position(|d| d.info.frame.contains(cursor));
        Self {
            displays,
            frames_ready: false,
            windows: Vec::new(),
            mode,
            cursor,
            drag: None,
            space_held: false,
            last_pointer: cursor,
            hovered: None,
            key_display,
            pending: None,
            on_done: Some(on_done),
        }
    }

    pub fn is_done(&self) -> bool {
        self.on_done.is_none()
    }

    /// Install the freeze-frame (and the window list).
    pub fn set_frames(&mut self, frames: Vec<(DisplayInfo, Captured, Arc<RenderImage>)>, cx: &mut Context<Self>) {
        for (info, cap, image) in frames {
            if let Some(d) = self.displays.iter_mut().find(|d| d.info.id == info.id) {
                d.info = info;
                d.capture = Some(cap);
                d.image = Some(image);
            } else {
                log::warn!("freeze-frame for unknown display {}", info.id);
            }
        }
        self.frames_ready = true;
        self.update_hover();
        if let Some(outcome) = self.pending.take() {
            self.finish(outcome, cx);
        }
        cx.notify();
    }

    pub fn set_windows(&mut self, windows: Vec<WindowInfo>, cx: &mut Context<Self>) {
        self.windows = windows;
        self.update_hover();
        cx.notify();
    }

    pub fn display_at(&self, p: Point) -> Option<usize> {
        self.displays.iter().position(|d| d.info.frame.contains(p))
    }

    /// Current selection (global pt), clamped to the union of all displays
    /// (may span displays) and snapped to the pixel grid of the display the
    /// drag started on. Also returns the display it overlaps most.
    pub fn selection(&self) -> Option<(Rect, usize)> {
        let d = self.drag?;
        let info = &self.displays.get(d.display)?.info;
        let s = info.scale_factor.max(1.0);
        let snap = |v: f32| (v * s).round() / s;
        let all = self.displays.iter().map(|d| d.info.frame).reduce(|a, b| a.union(&b))?;
        let clamp = |p: Point| Point::new(snap(p.x.clamp(all.x, all.right())), snap(p.y.clamp(all.y, all.bottom())));
        let rect = Rect::from_points(clamp(d.anchor), clamp(d.current));
        let area = |f: &Rect| rect.intersection(f).map(|r| r.width * r.height).unwrap_or(0.);
        let main = (0..self.displays.len())
            .max_by(|&a, &b| area(&self.displays[a].info.frame).total_cmp(&area(&self.displays[b].info.frame)))
            .unwrap_or(d.display);
        Some((rect, main))
    }

    /// Keyboard/mouse hint for the overlay's hint bar (depends on the state).
    /// Drawn only on the overlay — never part of the capture, which is
    /// cropped from the frozen frame (or captured live with our windows
    /// excluded / closed).
    pub fn hint(&self) -> &'static str {
        match (self.mode, self.drag) {
            (OverlayMode::Window, _) => "Click a window to capture · Click the desktop: whole display · Space: region mode · Esc: cancel",
            (OverlayMode::Region, None) => {
                "Drag to select · Enter: keyboard select · ↑↓←→ nudge (⇧ ×10) · Space: window mode · Esc: cancel"
            }
            (OverlayMode::Region, Some(d)) if d.keyboard => {
                "↑↓←→ resize (⇧ ×10) · Hold Space + arrows: move · Enter: capture · Esc: cancel"
            }
            (OverlayMode::Region, Some(_)) => "Release to capture · Hold Space: move selection · Esc: cancel",
        }
    }

    pub fn hovered_window(&self) -> Option<&WindowInfo> {
        self.hovered.and_then(|i| self.windows.get(i))
    }

    fn update_hover(&mut self) {
        let c = self.cursor;
        self.hovered = self.windows.iter().position(|w| w.frame.contains(c));
    }

    // ---- input (shared by mouse, keyboard and the self-test) ----

    pub fn pointer_move(&mut self, p: Point, cx: &mut Context<Self>) {
        if self.is_done() {
            return;
        }
        let delta = p - self.last_pointer;
        self.last_pointer = p;
        self.cursor = p;
        if let Some(d) = &mut self.drag {
            if self.space_held {
                d.anchor = d.anchor + delta;
                d.current = d.current + delta;
            } else {
                d.current = p;
            }
        }
        if self.mode == OverlayMode::Window {
            self.update_hover();
        }
        cx.notify();
    }

    pub fn pointer_down(&mut self, p: Point, cx: &mut Context<Self>) {
        if self.is_done() {
            return;
        }
        self.cursor = p;
        self.last_pointer = p;
        if self.mode == OverlayMode::Region
            && let Some(display) = self.display_at(p) {
                self.drag = Some(Drag { anchor: p, current: p, display, keyboard: false });
            }
        cx.notify();
    }

    pub fn pointer_up(&mut self, p: Point, cx: &mut Context<Self>) {
        if self.is_done() {
            return;
        }
        match self.mode {
            OverlayMode::Region => {
                if let Some(d) = &mut self.drag
                    && !self.space_held {
                        d.current = p;
                    }
                self.confirm_selection(cx);
            }
            OverlayMode::Window => {
                self.cursor = p;
                self.update_hover();
                self.confirm_window(cx);
            }
        }
        cx.notify();
    }

    fn confirm_selection(&mut self, cx: &mut Context<Self>) {
        match self.selection() {
            // A click / tiny drag: ignore, keep the overlay open.
            Some((r, _)) if r.width < 2.0 || r.height < 2.0 => self.drag = None,
            Some((rect, display)) => self.finish(OverlayOutcome::Region { rect, display }, cx),
            None => {}
        }
    }

    fn confirm_window(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.hovered_window().cloned() {
            self.finish(OverlayOutcome::Window { window: w }, cx);
        } else if let Some(ix) = self.display_at(self.cursor) {
            // Clicked the desktop: capture that whole display.
            let rect = self.displays[ix].info.frame;
            self.finish(OverlayOutcome::Region { rect, display: ix }, cx);
        }
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.finish(OverlayOutcome::Cancelled, cx);
    }

    pub fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = match self.mode {
            OverlayMode::Region => OverlayMode::Window,
            OverlayMode::Window => OverlayMode::Region,
        };
        self.drag = None;
        self.update_hover();
        cx.notify();
    }

    /// Keyboard input. `key` = GPUI key name.
    pub fn key_down(&mut self, key: &str, shift: bool, cx: &mut Context<Self>) {
        if self.is_done() {
            return;
        }
        match key {
            "escape" => self.cancel(cx),
            "space" => {
                if self.drag.is_some() {
                    self.space_held = true;
                } else {
                    self.toggle_mode(cx);
                }
            }
            "enter" => match self.mode {
                OverlayMode::Region => {
                    if self.drag.is_some() {
                        self.confirm_selection(cx);
                    } else if let Some(display) = self.display_at(self.cursor) {
                        // Keyboard selection: Enter starts at the cursor,
                        // arrows extend, Enter confirms.
                        self.drag = Some(Drag { anchor: self.cursor, current: self.cursor, display, keyboard: true });
                    }
                }
                OverlayMode::Window => self.confirm_window(cx),
            },
            "left" | "right" | "up" | "down" => {
                let s = self.display_at(self.cursor).map(|i| self.displays[i].info.scale_factor).unwrap_or(1.0);
                let step = if shift { 10.0 } else { 1.0 } / s.max(1.0);
                let (dx, dy) = match key {
                    "left" => (-step, 0.0),
                    "right" => (step, 0.0),
                    "up" => (0.0, -step),
                    _ => (0.0, step),
                };
                let delta = Point::new(dx, dy);
                if let Some(d) = &mut self.drag {
                    if self.space_held {
                        d.anchor = d.anchor + delta;
                    }
                    d.current = d.current + delta;
                    self.cursor = d.current;
                } else {
                    self.cursor = self.cursor + delta;
                }
                self.last_pointer = self.cursor;
                platform::warp_cursor(self.cursor);
                if self.mode == OverlayMode::Window {
                    self.update_hover();
                }
            }
            _ => {}
        }
        cx.notify();
    }

    pub fn key_up(&mut self, key: &str, cx: &mut Context<Self>) {
        if key == "space" {
            self.space_held = false;
            cx.notify();
        }
    }

    fn finish(&mut self, outcome: OverlayOutcome, cx: &mut Context<Self>) {
        // A selection finished before the freeze-frame arrived waits for it
        // (crop must come from the frame, not a second capture).
        if !self.frames_ready && !matches!(outcome, OverlayOutcome::Cancelled) {
            self.pending = Some(outcome);
            return;
        }
        let Some(cb) = self.on_done.take() else { return };
        log::info!("overlay finished: {outcome:?}");
        // Deferred: we're inside an entity (and possibly window) update.
        cx.defer(move |cx| cb(outcome, cx));
    }
}

// ---------------------------------------------------------------------------
// View (one per display)
// ---------------------------------------------------------------------------

const DIM: u32 = 0x00000073;
const ACCENT: u32 = 0x0a84ff;
const LOUPE_CELLS: i32 = 15;
const LOUPE_CELL: f32 = 8.0;

pub struct OverlayView {
    session: Entity<OverlaySession>,
    ix: usize,
    focus: FocusHandle,
    _sub: Subscription,
}

impl OverlayView {
    pub fn new(session: Entity<OverlaySession>, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&session, |_, _, cx| cx.notify());
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { session, ix, focus, _sub: sub }
    }
}

impl Focusable for OverlayView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn abs(r: Rect) -> Div {
    div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.width.max(0.))).h(px(r.height.max(0.)))
}

/// Four dim bands around `hole` (local coords) covering `w`×`h`.
fn dim_around(hole: Option<Rect>, w: f32, h: f32) -> Vec<Div> {
    let dim = |r: Rect| abs(r).bg(rgba(DIM));
    match hole {
        None => vec![dim(Rect::new(0., 0., w, h))],
        Some(r) => vec![
            dim(Rect::new(0., 0., w, r.y.max(0.))),
            dim(Rect::new(0., r.bottom(), w, (h - r.bottom()).max(0.))),
            dim(Rect::new(0., r.y, r.x.max(0.), r.height)),
            dim(Rect::new(r.right(), r.y, (w - r.right()).max(0.), r.height)),
        ],
    }
}

fn pill(text: String) -> Div {
    div()
        .px_2()
        .py(px(3.))
        .rounded_md()
        .bg(rgba(0x000000cc))
        .text_color(rgb(0xffffff))
        .text_xs()
        .font_family("Menlo")
        .child(text)
}

/// Sample a (2k+1)² block of pixels around `p` (global pt) from a frozen
/// display. Returns row-major RGBA + centre pixel coords.
type LoupeSample = (Vec<[u8; 4]>, (i64, i64));

fn sample(d: &FrozenDisplay, p: Point) -> Option<LoupeSample> {
    let cap = d.capture.as_ref()?;
    let img = &cap.image;
    if !img.is_loaded() || d.info.frame.width <= 0. {
        return None;
    }
    let s = img.width as f32 / d.info.frame.width;
    let cx = ((p.x - d.info.frame.x) * s).floor() as i64;
    let cy = ((p.y - d.info.frame.y) * s).floor() as i64;
    let k = (LOUPE_CELLS / 2) as i64;
    let mut out = Vec::with_capacity((LOUPE_CELLS * LOUPE_CELLS) as usize);
    for y in cy - k..=cy + k {
        for x in cx - k..=cx + k {
            let px = if x >= 0 && y >= 0 && (x as u32) < img.width && (y as u32) < img.height {
                let i = ((y as u32 * img.width + x as u32) * 4) as usize;
                [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2], 255]
            } else {
                [0, 0, 0, 0]
            };
            out.push(px);
        }
    }
    Some((out, (cx, cy)))
}

fn loupe(cells: Vec<[u8; 4]>, center: (i64, i64), at: Point) -> Div {
    let side = LOUPE_CELLS as f32 * LOUPE_CELL;
    let mid = cells[(LOUPE_CELLS * LOUPE_CELLS / 2) as usize];
    let hex = format!("#{:02X}{:02X}{:02X}", mid[0], mid[1], mid[2]);
    let grid = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let o = bounds.origin;
            for (i, c) in cells.iter().enumerate() {
                let (col, row) = ((i as i32 % LOUPE_CELLS) as f32, (i as i32 / LOUPE_CELLS) as f32);
                let color = Hsla::from(Rgba {
                    r: c[0] as f32 / 255.,
                    g: c[1] as f32 / 255.,
                    b: c[2] as f32 / 255.,
                    a: if c[3] == 0 { 1.0 } else { c[3] as f32 / 255. },
                });
                let cell = Bounds::new(
                    point(o.x + px(col * LOUPE_CELL), o.y + px(row * LOUPE_CELL)),
                    size(px(LOUPE_CELL), px(LOUPE_CELL)),
                );
                window.paint_quad(fill(cell, color));
                // Pixel grid.
                window.paint_quad(
                    outline(cell, Hsla::from(rgba(0x00000030)), BorderStyle::Solid).border_widths(px(0.5)),
                );
            }
            let k = (LOUPE_CELLS / 2) as f32 * LOUPE_CELL;
            let c = Bounds::new(point(o.x + px(k), o.y + px(k)), size(px(LOUPE_CELL), px(LOUPE_CELL)));
            window.paint_quad(outline(c, Hsla::from(rgb(0xffffff)), BorderStyle::Solid).border_widths(px(1.5)));
        },
    )
    .size(px(side));
    div()
        .absolute()
        .left(px(at.x))
        .top(px(at.y))
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .child(
            div()
                .size(px(side + 4.))
                .p(px(2.))
                .rounded_lg()
                .overflow_hidden()
                .bg(rgb(0xffffff))
                .shadow_lg()
                .child(grid),
        )
        .child(pill(format!("{hex}  {},{}", center.0, center.1)))
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.session.read(cx);
        let Some(disp) = s.displays.get(self.ix) else { return div().id(("overlay", self.ix)).size_full() };
        let frame = disp.info.frame;
        let origin = Point::new(frame.x, frame.y);
        let (w, h) = (frame.width, frame.height);
        let to_local = |r: Rect| r.translate(-origin.x, -origin.y);
        let cursor_here = frame.contains(s.cursor);
        let mut root = div()
            .id(("overlay", self.ix))
            .size_full()
            .relative()
            .key_context("RsOverlay")
            .track_focus(&self.focus)
            .cursor(if s.mode == OverlayMode::Region { CursorStyle::Crosshair } else { CursorStyle::PointingHand });

        // Input: window-level listeners so drags leaving the window (or
        // starting before the frame arrived) are still seen.
        let session = self.session.clone();
        let ix = self.ix;
        root = root.child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let to_global = move |p: gpui_kit::Point<Pixels>| Point::new(origin.x + p.x.as_f32(), origin.y + p.y.as_f32());
                    let sm = session.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let p = to_global(ev.position);
                        sm.update(cx, |s, cx| {
                            if s.key_display != Some(ix) && s.drag.is_none() && s.display_at(p) == Some(ix) {
                                s.key_display = Some(ix);
                                platform::make_key_window(window);
                            }
                            s.pointer_move(p, cx)
                        });
                    });
                    let sd = session.clone();
                    window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                            sd.update(cx, |s, cx| s.pointer_down(to_global(ev.position), cx));
                        } else if phase == DispatchPhase::Bubble && ev.button == MouseButton::Right {
                            sd.update(cx, |s, cx| s.cancel(cx));
                        }
                    });
                    let su = session.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                            su.update(cx, |s, cx| s.pointer_up(to_global(ev.position), cx));
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        );
        root = root
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let key = ev.keystroke.key.clone();
                let shift = ev.keystroke.modifiers.shift;
                this.session.update(cx, |s, cx| s.key_down(&key, shift, cx));
                cx.stop_propagation();
            }))
            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| {
                let key = ev.keystroke.key.clone();
                this.session.update(cx, |s, cx| s.key_up(&key, cx));
            }));

        // Until the freeze-frame arrives, render (almost) nothing: alpha 1/255
        // so AppKit still routes clicks here instead of passing them through
        // fully transparent pixels to the windows below.
        if !s.frames_ready {
            return root.bg(rgba(0x00000001));
        }

        if let Some(image) = &disp.image {
            root = root.child(img(image.clone()).absolute().top_0().left_0().w(px(w)).h(px(h)).object_fit(ObjectFit::Fill));
        }

        match s.mode {
            OverlayMode::Region => {
                let sel = s.selection().and_then(|(r, _)| r.intersection(&frame)).map(to_local);
                root = root.children(dim_around(sel, w, h));
                if let Some(r) = sel {
                    root = root.child(abs(r).border_1().border_color(rgb(0xffffff)));
                    // Handles-looking corner marks (visual only).
                    for c in r.corners() {
                        root = root.child(
                            abs(Rect::new(c.x - 3., c.y - 3., 6., 6.))
                                .bg(rgb(0xffffff))
                                .border_1()
                                .border_color(rgb(ACCENT))
                                .rounded_sm(),
                        );
                    }
                    if let Some((full, dix)) = s.selection() {
                        let sc = s.displays[dix].info.scale_factor;
                        let label = format!(
                            "{} × {}",
                            (full.width * sc).round() as u32,
                            (full.height * sc).round() as u32
                        );
                        let (lx, ly) =
                            if r.bottom() + 28. < h { (r.x, r.bottom() + 6.) } else { (r.x + 4., (r.y - 26.).max(4.)) };
                        root = root.child(div().absolute().left(px(lx)).top(px(ly)).child(pill(label)));
                    }
                } else if cursor_here {
                    // Crosshair guides through the cursor.
                    let c = s.cursor - origin;
                    root = root
                        .child(abs(Rect::new(0., c.y, w, 1.)).bg(rgba(0xffffff4d)))
                        .child(abs(Rect::new(c.x, 0., 1., h)).bg(rgba(0xffffff4d)));
                }
                if cursor_here
                    && let Some((cells, center)) = sample(disp, s.cursor) {
                        let side = LOUPE_CELLS as f32 * LOUPE_CELL + 4.;
                        let c = s.cursor - origin;
                        let lx = if c.x + 24. + side < w { c.x + 24. } else { c.x - 24. - side };
                        let ly = if c.y + 24. + side + 30. < h { c.y + 24. } else { c.y - 24. - side - 30. };
                        root = root.child(loupe(cells, center, Point::new(lx, ly)));
                    }
            }
            OverlayMode::Window => {
                let hovered = s.hovered_window();
                let hole = hovered.and_then(|w| w.frame.intersection(&frame)).map(to_local);
                root = root.children(dim_around(hole, w, h));
                if let (Some(r), Some(win)) = (hole, hovered) {
                    root = root.child(abs(r).bg(rgba(0x0a84ff2e)).border_2().border_color(rgb(ACCENT)));
                    let name = if win.title.is_empty() { win.app_name.clone() } else { format!("{} — {}", win.app_name, win.title) };
                    let sc = disp.info.scale_factor;
                    let label = format!(
                        "{name}  {} × {}",
                        (win.frame.width * sc).round() as u32,
                        (win.frame.height * sc).round() as u32
                    );
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(r.x))
                            .top(px(r.y))
                            .w(px(r.width))
                            .h(px(r.height))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(pill(label)),
                    );
                }
            }
        }

        if cursor_here {
            // Hint bar at the bottom of the display under the cursor; moves
            // to the top when the selection / cursor is down there so it
            // never hides what's being selected.
            const BAND: f32 = 96.;
            let sel_local = s.selection().and_then(|(r, _)| r.intersection(&frame)).map(to_local);
            let low = |y: f32| y > h - BAND;
            let at_top = low((s.cursor - origin).y) || sel_local.is_some_and(|r| low(r.bottom()));
            let bar = div().absolute().left_0().w(px(w)).flex().justify_center().child(pill(s.hint().into()));
            root = root.child(if at_top { bar.top(px(48.)) } else { bar.bottom(px(48.)) });
        }
        let _ = window;
        root
    }
}

/// Window options for a full-display overlay on `display`.
pub fn overlay_window_options(display: &DisplayInfo) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(display.frame.width), px(display.frame.height)),
        ))),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(DisplayId::new(display.id as u64)),
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// Straight RGBA → GPUI RenderImage (BGRA), multi-threaded (big frames).
pub fn to_render_image(img: &rs_core::RgbaImage) -> Option<Arc<RenderImage>> {
    let mut bgra: Vec<u8> = img.pixels.as_ref().clone();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    let chunk = (bgra.len() / 4).div_ceil(threads) * 4;
    if chunk > 0 {
        std::thread::scope(|s| {
            for part in bgra.chunks_mut(chunk) {
                s.spawn(move || {
                    for p in part.as_chunks_mut::<4>().0 {
                        p.swap(0, 2);
                    }
                });
            }
        });
    }
    let buf = image::RgbaImage::from_raw(img.width, img.height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit's prelude exports its own `test` attribute.
    use super::{Drag, OverlayMode, OverlaySession};
    use rs_core::Point;

    #[test]
    fn hint_follows_state() {
        let mut s = OverlaySession::new(Vec::new(), OverlayMode::Region, Point::ZERO, Box::new(|_, _| {}));
        assert!(s.hint().starts_with("Drag to select"));
        assert!(s.hint().contains("Space: window mode") && s.hint().contains("Esc: cancel"));
        s.drag = Some(Drag { anchor: Point::ZERO, current: Point::ZERO, display: 0, keyboard: false });
        assert!(s.hint().starts_with("Release to capture"));
        s.drag = Some(Drag { anchor: Point::ZERO, current: Point::ZERO, display: 0, keyboard: true });
        assert!(s.hint().contains("Enter: capture"));
        s.mode = OverlayMode::Window;
        assert!(s.hint().contains("Space: region mode"));
    }
}
