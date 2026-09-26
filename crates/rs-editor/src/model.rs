//! The editor model: document + history + selection + tool state machine,
//! text-edit sessions, crop mode. **No GPUI** — the view feeds it pointer
//! and keyboard input in content-space coordinates, so everything here is
//! unit-testable (and the self-test drives the same code paths).
//!
//! Annotations live in content space but are **not** confined to the image:
//! with beautify on they can be created, picked, moved, resized, snapped and
//! pasted anywhere on the output canvas (over the padding / inset / chrome /
//! background), i.e. at negative or beyond-the-image coordinates. See
//! [`EditorModel::canvas_rect`].

use std::collections::{BTreeMap, HashMap};

use rs_core::{
    zorder, Annotation, AnnotationClipboard, AnnotationId, AnnotationKind, BeautifySettings, Color, Command, Document,
    History, IRect, ImageTransform, Point, Rect, StepLabel, ToolKind, ToolStyle,
};
use rs_render::Renderer;

use crate::geom::{self, RectHandle};
use crate::hit::{self, DragMods, HandleKind};
use crate::snap::{self, Guide, SnapTargets};
use crate::style_map::{self, Sections, StyleEdit};
use crate::text_buffer::TextBuffer;

/// Keyboard modifiers relevant to canvas interaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    /// ⌘ — disables snapping while held.
    pub cmd: bool,
}

impl Mods {
    fn drag(self) -> DragMods {
        DragMods { shift: self.shift, alt: self.alt }
    }
}

/// Z-order operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZOrder {
    Front,
    Forward,
    Backward,
    Back,
}

/// In-progress pointer interaction.
#[derive(Debug, Clone, Default)]
pub enum Interaction {
    #[default]
    None,
    /// Drawing a new annotation with a tool.
    Create { tool: ToolKind, start: Point, current: Point, points: Vec<Point>, preview: Option<Annotation> },
    /// Moving the selection (⌥ = duplicate).
    Move { start: Point, originals: Vec<Annotation>, union: Rect, duplicate: bool, moved: bool },
    /// Dragging an edit handle of one annotation.
    Handle { handle: HandleKind, original: Annotation, bounds: Rect },
    /// Dragging a bounding-box handle of a multi-selection: every selected
    /// annotation scales proportionally from `union` (one undo step).
    MultiResize { handle: RectHandle, originals: Vec<Annotation>, union: Rect },
    /// Rubber-band selection.
    Marquee { start: Point, current: Point, additive: bool, base: Vec<AnnotationId> },
    /// Crop mode: new rect / move / resize.
    CropNew { start: Point },
    CropMove { start: Point, original: Rect },
    CropHandle { handle: RectHandle, original: Rect },
}

/// An inline text-editing session (Text annotations and callout text).
#[derive(Debug, Clone)]
pub struct TextSession {
    pub id: AnnotationId,
    /// `true` if the annotation doesn't exist in the document yet.
    pub is_new: bool,
    /// The annotation being edited (content kept in sync with `buffer`).
    pub annotation: Annotation,
    pub buffer: TextBuffer,
    /// Mouse is dragging a text selection.
    pub selecting: bool,
}

impl TextSession {
    /// Text box (content space): top-left + wrap width (None = auto).
    pub fn text_box(&self) -> (Point, Option<f32>) {
        match &self.annotation.kind {
            AnnotationKind::Text(t) => (t.position, t.width),
            AnnotationKind::Callout(c) => (
                Point::new(c.rect.x + c.padding, c.rect.y + c.padding),
                Some((c.rect.width - 2.0 * c.padding).max(1.0)),
            ),
            _ => (Point::ZERO, None),
        }
    }

    pub fn text_style(&self) -> Option<&rs_core::TextStyle> {
        match &self.annotation.kind {
            AnnotationKind::Text(t) => Some(&t.style),
            AnnotationKind::Callout(c) => Some(&c.text_style),
            _ => None,
        }
    }

    fn sync(&mut self) {
        let text = self.buffer.text.clone();
        match &mut self.annotation.kind {
            AnnotationKind::Text(t) => t.content = text,
            AnnotationKind::Callout(c) => c.text = text,
            _ => {}
        }
    }
}

/// Minimum drag (screen px) that counts as a drag rather than a click.
const CLICK_SLOP_PX: f32 = 4.0;
/// Pick tolerance (screen px).
const PICK_TOL_PX: f32 = 5.0;
/// Handle grab radius (screen px).
pub const HANDLE_RADIUS_PX: f32 = 7.0;
/// Snap threshold (screen px).
const SNAP_PX: f32 = 6.0;

/// The editor state machine. See module docs.
pub struct EditorModel {
    pub doc: Document,
    pub history: History,
    pub selection: Vec<AnnotationId>,
    pub tool: ToolKind,
    /// Per-tool style memory in pt (mirrors `Preferences::tool_styles`).
    pub styles: BTreeMap<ToolKind, ToolStyle>,
    /// Set when `styles` changed (the view writes them back to prefs).
    pub styles_dirty: bool,
    pub interaction: Interaction,
    /// Transient overrides while moving/resizing (not yet in history).
    transient: HashMap<AnnotationId, Annotation>,
    transient_added: Vec<Annotation>,
    pub guides: Vec<Guide>,
    pub editing: Option<TextSession>,
    /// Crop mode rect, in the *uncropped* content space (see [`Self::crop_transform`]).
    pub crop: Option<Rect>,
    /// Pointer position (content space) for hover previews.
    pub hover: Option<Point>,
    pub hovered: Option<AnnotationId>,
    /// Eyedropper armed: the next click samples the image.
    pub eyedropper: bool,
    /// Result of the last eyedropper click (taken by the view).
    pub picked_color: Option<Color>,
    /// Last eyedropper result (kept; for diagnostics / the self-test).
    pub last_picked: Option<Color>,
    /// The composed canvas (latest preview bitmap + its layout) the
    /// eyedropper samples, so picking works over the background / padding and
    /// picks what is visible. Set by the view; `None` → base image only.
    pub canvas_sample: Option<(rs_core::RgbaImage, rs_render::CanvasLayout)>,
    pub snapping: bool,
    /// Screen (logical) px per content px — set by the view.
    pub view_scale: f32,
    /// Bumped on every change that affects the rendered preview.
    pub generation: u64,
    pub last_error: Option<String>,
    measurer: Renderer,
    bounds_cache: HashMap<AnnotationId, (Annotation, Rect)>,
    paste_count: u32,
}

impl EditorModel {
    pub fn new(doc: Document, styles: BTreeMap<ToolKind, ToolStyle>) -> Self {
        Self {
            doc,
            history: History::new(),
            selection: Vec::new(),
            tool: ToolKind::Select,
            styles,
            styles_dirty: false,
            interaction: Interaction::None,
            transient: HashMap::new(),
            transient_added: Vec::new(),
            guides: Vec::new(),
            editing: None,
            crop: None,
            hover: None,
            hovered: None,
            eyedropper: false,
            picked_color: None,
            last_picked: None,
            canvas_sample: None,
            snapping: true,
            view_scale: 1.0,
            generation: 1,
            last_error: None,
            measurer: Renderer::new(),
            bounds_cache: HashMap::new(),
            paste_count: 0,
        }
    }

    fn touch(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn sf(&self) -> f32 {
        self.doc.metadata.scale_factor.max(0.01)
    }

    fn tol(&self) -> f32 {
        PICK_TOL_PX / self.view_scale.max(1e-3)
    }

    pub fn handle_radius(&self) -> f32 {
        HANDLE_RADIUS_PX / self.view_scale.max(1e-3)
    }

    /// Remembered style (pt) for `tool`.
    pub fn tool_style(&self, tool: ToolKind) -> ToolStyle {
        self.styles.get(&tool).cloned().unwrap_or_else(|| ToolStyle::default_for(tool))
    }

    fn tool_style_px(&self, tool: ToolKind) -> ToolStyle {
        self.tool_style(tool).to_content_px(self.sf())
    }

    fn apply(&mut self, cmd: Command) -> bool {
        match self.history.apply(&mut self.doc, cmd) {
            Ok(()) => {
                self.touch();
                true
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
                false
            }
        }
    }

    fn apply_coalescing(&mut self, cmd: Command, key: String) -> bool {
        match self.history.apply_coalescing(&mut self.doc, cmd, key) {
            Ok(()) => {
                self.touch();
                true
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
                false
            }
        }
    }

    /// End a slider/nudge coalescing run.
    pub fn break_coalescing(&mut self) {
        self.history.break_coalescing();
    }

    /// Precise bounds (cached per annotation value).
    pub fn bounds_of(&mut self, a: &Annotation) -> Rect {
        if let Some((cached, r)) = self.bounds_cache.get(&a.id)
            && cached == a
        {
            return *r;
        }
        let r = self.measurer.annotation_bounds(a);
        if self.bounds_cache.len() > 512 {
            self.bounds_cache.clear();
        }
        self.bounds_cache.insert(a.id, (a.clone(), r));
        r
    }

    /// The annotation as currently displayed (transient override if any).
    pub fn current(&self, id: AnnotationId) -> Option<Annotation> {
        if let Some(s) = &self.editing
            && s.id == id
        {
            return Some(s.annotation.clone());
        }
        self.transient.get(&id).cloned().or_else(|| self.doc.get(id).cloned())
    }

    fn prune_selection(&mut self) {
        let doc = &self.doc;
        self.selection.retain(|id| doc.index_of(*id).is_some());
    }

    // ------------------------------------------------------------------
    // Tools
    // ------------------------------------------------------------------

    pub fn set_tool(&mut self, tool: ToolKind) {
        if self.tool == tool && tool != ToolKind::Crop {
            return;
        }
        self.commit_text();
        self.cancel_interaction();
        self.eyedropper = false;
        if self.crop.is_some() && tool != ToolKind::Crop {
            self.crop = None;
            self.touch();
        }
        if tool == ToolKind::Crop {
            self.enter_crop();
        }
        self.tool = tool;
    }

    /// Esc: cancel the current interaction / editing / crop, else deselect,
    /// else switch to the select tool.
    pub fn escape(&mut self) {
        if self.eyedropper {
            self.eyedropper = false;
            return;
        }
        if !matches!(self.interaction, Interaction::None) {
            self.cancel_interaction();
            return;
        }
        if self.editing.is_some() {
            self.commit_text();
            return;
        }
        if self.crop.is_some() {
            self.crop = None;
            self.tool = ToolKind::Select;
            self.touch();
            return;
        }
        if !self.selection.is_empty() {
            self.selection.clear();
            return;
        }
        self.tool = ToolKind::Select;
    }

    pub fn cancel_interaction(&mut self) {
        let had = !matches!(self.interaction, Interaction::None) || !self.transient.is_empty();
        self.interaction = Interaction::None;
        self.transient.clear();
        self.transient_added.clear();
        self.guides.clear();
        if had {
            self.touch();
        }
    }

    // ------------------------------------------------------------------
    // Pointer input (content space)
    // ------------------------------------------------------------------

    /// The whole output canvas (background + padding + frame) in content
    /// space — where annotations can be drawn. `None` when the canvas is
    /// exactly the image (beautify off, or crop mode). Independent of the
    /// annotations (the layout never depends on them).
    pub fn canvas_rect(&self) -> Option<Rect> {
        if self.crop.is_some() || !self.doc.beautify.enabled {
            return None;
        }
        let l = rs_render::compute_layout(&self.doc, 1.0);
        Some(l.canvas_rect_to_image(Rect::new(0.0, 0.0, l.canvas_width as f32, l.canvas_height as f32)))
    }

    /// Drawable area in content space: the canvas, or the image when the
    /// canvas is the image.
    pub fn drawable_rect(&self) -> Rect {
        self.canvas_rect().unwrap_or_else(|| {
            let c = if self.crop.is_some() { self.crop_content_size() } else { self.doc.content_size() };
            Rect::new(0.0, 0.0, c.width, c.height)
        })
    }

    fn snap_targets(&mut self, exclude: &[AnnotationId]) -> SnapTargets {
        let content = if self.crop.is_some() { self.crop_content_size() } else { self.doc.content_size() };
        let others: Vec<Annotation> =
            self.doc.annotations.iter().filter(|a| !a.hidden && !exclude.contains(&a.id)).cloned().collect();
        let rects: Vec<Rect> = others.iter().map(|a| self.bounds_of(a)).collect();
        SnapTargets::new(content, rects).with_canvas(self.canvas_rect())
    }

    fn snap_pt(&mut self, p: Point, mods: Mods, exclude: &[AnnotationId]) -> Point {
        if !self.snapping || mods.cmd {
            self.guides.clear();
            return p;
        }
        let t = self.snap_targets(exclude);
        let (q, g) = snap::snap_point(p, &t, SNAP_PX / self.view_scale.max(1e-3));
        self.guides = g;
        q
    }

    /// Handle of the single selected annotation under `p`.
    pub fn handle_at(&mut self, p: Point) -> Option<(AnnotationId, HandleKind)> {
        if self.selection.len() != 1 {
            return None;
        }
        let id = self.selection[0];
        let a = self.current(id)?;
        if a.locked {
            return None;
        }
        let b = self.bounds_of(&a);
        let hs = hit::handles_for(&a, b);
        let r = self.handle_radius();
        hit::handle_at(&hs, p, r).map(|h| (id, h))
    }

    /// Bounding box (content space) and handles of a multi-selection
    /// (≥ 2 unlocked annotations, not editing text). `None` otherwise.
    pub fn multi_handles(&mut self) -> Option<(Rect, Vec<(RectHandle, Point)>)> {
        if self.selection.len() < 2 || self.editing.is_some() {
            return None;
        }
        let ids = self.selection.clone();
        let mut union: Option<Rect> = None;
        let mut n = 0;
        for id in ids {
            let Some(a) = self.current(id) else { continue };
            if a.locked || a.hidden {
                continue;
            }
            n += 1;
            let b = self.bounds_of(&a);
            union = Some(union.map_or(b, |u| u.union(&b)));
        }
        let u = union.filter(|_| n >= 2)?;
        Some((u, RectHandle::ALL.iter().map(|h| (*h, h.position(u))).collect()))
    }

    /// Multi-selection bounding-box handle under `p`.
    pub fn multi_handle_at(&mut self, p: Point) -> Option<RectHandle> {
        let (_, hs) = self.multi_handles()?;
        let r = self.handle_radius();
        hs.iter()
            .map(|(h, q)| (*h, q.distance(p)))
            .filter(|(_, d)| *d <= r)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    }

    /// Topmost annotation at `p`.
    pub fn pick(&mut self, p: Point) -> Option<AnnotationId> {
        let anns = self.doc.annotations.clone();
        let tol = self.tol();
        let mut bounds = |a: &Annotation| self.bounds_of(a);
        hit::pick(&anns, p, tol, &mut bounds).map(|a| a.id)
    }

    pub fn pointer_down(&mut self, p: Point, mods: Mods, click_count: usize) {
        self.hover = Some(p);
        if self.eyedropper {
            self.picked_color = self.sample_color(p);
            self.last_picked = self.picked_color;
            self.eyedropper = false;
            return;
        }
        if self.crop.is_some() {
            self.crop_pointer_down(p);
            return;
        }
        // Clicking outside the text box being edited commits it.
        if self.editing.is_some() {
            self.commit_text();
            if self.tool == ToolKind::Text {
                return;
            }
        }
        // Double-click on text / callout → edit.
        if click_count >= 2
            && let Some(id) = self.pick(p)
            && let Some(a) = self.doc.get(id)
            && matches!(a.kind, AnnotationKind::Text(_) | AnnotationKind::Callout(_))
        {
            self.begin_edit(id);
            return;
        }
        // Bounding-box handles of a multi-selection.
        if let Some(handle) = self.multi_handle_at(p)
            && let Some((union, _)) = self.multi_handles()
        {
            let originals: Vec<Annotation> = self
                .doc
                .annotations
                .iter()
                .filter(|a| self.selection.contains(&a.id) && !a.locked && !a.hidden)
                .cloned()
                .collect();
            self.interaction = Interaction::MultiResize { handle, originals, union };
            return;
        }
        // Handles of the selected annotation work with every tool.
        if let Some((id, handle)) = self.handle_at(p)
            && let Some(original) = self.current(id)
        {
            let bounds = self.bounds_of(&original);
            self.interaction = Interaction::Handle { handle, original, bounds };
            return;
        }
        match self.tool {
            ToolKind::Select => self.select_down(p, mods),
            ToolKind::Crop => {}
            ToolKind::Text => {
                if let Some(id) = self.pick(p)
                    && matches!(self.doc.get(id).map(|a| &a.kind), Some(AnnotationKind::Text(_)))
                {
                    self.begin_edit(id);
                    return;
                }
                self.start_create(p, mods);
            }
            _ => self.start_create(p, mods),
        }
    }

    fn select_down(&mut self, p: Point, mods: Mods) {
        match self.pick(p) {
            Some(id) => {
                if mods.shift {
                    if let Some(ix) = self.selection.iter().position(|s| *s == id) {
                        self.selection.remove(ix);
                        return;
                    }
                    self.selection.push(id);
                } else if !self.selection.contains(&id) {
                    self.selection = vec![id];
                }
                self.start_move(p, mods.alt);
            }
            None => {
                let base = if mods.shift { self.selection.clone() } else { Vec::new() };
                if !mods.shift {
                    self.selection.clear();
                }
                self.interaction = Interaction::Marquee { start: p, current: p, additive: mods.shift, base };
            }
        }
    }

    fn start_move(&mut self, p: Point, duplicate: bool) {
        let originals: Vec<Annotation> = self
            .doc
            .annotations
            .iter()
            .filter(|a| self.selection.contains(&a.id) && !a.locked)
            .cloned()
            .collect();
        if originals.is_empty() {
            return;
        }
        let mut union: Option<Rect> = None;
        for a in &originals {
            let b = self.bounds_of(a);
            union = Some(union.map_or(b, |u| u.union(&b)));
        }
        self.interaction =
            Interaction::Move { start: p, originals, union: union.unwrap_or_default(), duplicate, moved: false };
    }

    fn start_create(&mut self, p: Point, mods: Mods) {
        let tool = self.tool;
        if !tool.creates_annotation() {
            return;
        }
        let p = self.snap_pt(p, mods, &[]);
        self.selection.clear();
        let mut interaction = Interaction::Create { tool, start: p, current: p, points: vec![p], preview: None };
        if let Interaction::Create { preview, .. } = &mut interaction {
            *preview = self.build_preview(tool, p, p, &[p], mods, false);
        }
        self.interaction = interaction;
        self.touch();
    }

    /// Build the annotation for a creation drag.
    fn build_preview(
        &self,
        tool: ToolKind,
        start: Point,
        current: Point,
        points: &[Point],
        mods: Mods,
        is_click: bool,
    ) -> Option<Annotation> {
        let st = self.tool_style_px(tool);
        let sf = self.sf();
        let rect_like = matches!(
            tool,
            ToolKind::Rect
                | ToolKind::Ellipse
                | ToolKind::Callout
                | ToolKind::Spotlight
                | ToolKind::Blur
                | ToolKind::Pixelate
                | ToolKind::Redact
                | ToolKind::Text
        );
        let (mut from, mut to) = (start, current);
        if rect_like {
            if mods.shift {
                to = geom::constrain_square(from, to);
            }
            if mods.alt {
                (from, to) = geom::from_center(start, to);
            }
        } else if matches!(tool, ToolKind::Line | ToolKind::Arrow) && mods.shift {
            to = geom::constrain_45(from, to);
        }
        let mut a = match tool {
            ToolKind::Callout if is_click => {
                let r = Rect::new(start.x, start.y, 180.0 * sf, 56.0 * sf);
                let mut a = st.create(tool, r.origin(), Point::new(r.right(), r.bottom()))?;
                if let AnnotationKind::Callout(c) = &mut a.kind {
                    c.tail = Point::new(r.x + r.width * 0.2, r.bottom() + 28.0 * sf);
                }
                a
            }
            ToolKind::Magnifier if is_click => {
                let r = 28.0 * sf;
                let zoom = st.magnifier_zoom.clamp(2.0, 4.0);
                let off = r * zoom * 1.3;
                let mut a = st.create(tool, start, Point::new(start.x + off, start.y - off))?;
                if let AnnotationKind::Magnifier(m) = &mut a.kind {
                    m.source_radius = r;
                }
                a
            }
            ToolKind::Step => st.create(tool, current, current)?,
            ToolKind::Text => st.create(tool, from, if is_click { from } else { to })?,
            _ => st.create(tool, from, to)?,
        };
        match &mut a.kind {
            AnnotationKind::Pen(pen) => {
                pen.points = if mods.shift { vec![start, current] } else { points.to_vec() };
            }
            AnnotationKind::Highlighter(h) => {
                h.points = if mods.shift { vec![start, current] } else { points.to_vec() };
            }
            AnnotationKind::Step(s) => s.value = self.doc.next_step_value(s.label),
            AnnotationKind::Text(t) => {
                // Text box starts at the click (top-left) — not normalized to the drag.
                t.position = Rect::from_points(from, to).origin();
                if is_click {
                    t.position = start;
                }
            }
            _ => {}
        }
        Some(a)
    }

    pub fn pointer_move(&mut self, p: Point, mods: Mods) {
        self.hover = Some(p);
        let slop = CLICK_SLOP_PX / self.view_scale.max(1e-3);
        let interaction = std::mem::take(&mut self.interaction);
        self.interaction = match interaction {
            Interaction::None => {
                self.hovered = if self.tool == ToolKind::Select && self.crop.is_none() { self.pick(p) } else { None };
                Interaction::None
            }
            Interaction::Create { tool, start, points, .. } => {
                let exclude: Vec<AnnotationId> = Vec::new();
                let snapped = if matches!(tool, ToolKind::Pen | ToolKind::Highlighter) {
                    p
                } else {
                    self.snap_pt(p, mods, &exclude)
                };
                let mut points = points;
                if matches!(tool, ToolKind::Pen | ToolKind::Highlighter)
                    && points.last().is_none_or(|l| l.distance(p) >= 1.5 / self.view_scale.max(1e-3))
                {
                    points.push(p);
                }
                let is_click = start.distance(snapped) < slop;
                let preview = self.build_preview(tool, start, snapped, &points, mods, is_click && tool == ToolKind::Text);
                self.touch();
                Interaction::Create { tool, start, current: snapped, points, preview }
            }
            Interaction::Move { start, originals, union, duplicate, moved } => {
                let mut d = p - start;
                let moved = moved || d.x.hypot(d.y) >= slop;
                if moved {
                    if self.snapping && !mods.cmd {
                        let ids: Vec<AnnotationId> = originals.iter().map(|a| a.id).collect();
                        let t = self.snap_targets(&ids);
                        let (dx, dy, g) =
                            snap::snap_rect(union.translate(d.x, d.y), &t, SNAP_PX / self.view_scale.max(1e-3));
                        d = d + Point::new(dx, dy);
                        self.guides = g;
                    } else {
                        self.guides.clear();
                    }
                    if mods.shift {
                        // Constrain to the dominant axis.
                        if d.x.abs() > d.y.abs() {
                            d.y = 0.0;
                        } else {
                            d.x = 0.0;
                        }
                    }
                    self.transient.clear();
                    self.transient_added.clear();
                    for o in &originals {
                        let mut a = o.clone();
                        a.translate(d.x, d.y);
                        if duplicate {
                            self.transient_added.push(Annotation { id: AnnotationId::new(), ..a });
                        } else {
                            self.transient.insert(a.id, a);
                        }
                    }
                    self.touch();
                }
                Interaction::Move { start, originals, union, duplicate, moved }
            }
            Interaction::Handle { handle, original, bounds } => {
                let q = self.snap_pt(p, mods, &[original.id]);
                let a = hit::apply_handle(&original, bounds, handle, q, mods.drag());
                self.transient.insert(a.id, a);
                self.touch();
                Interaction::Handle { handle, original, bounds }
            }
            Interaction::MultiResize { handle, originals, union } => {
                let ids: Vec<AnnotationId> = originals.iter().map(|a| a.id).collect();
                let q = self.snap_pt(p, mods, &ids);
                let mut to = geom::resize_rect(union, handle, q, mods.shift, mods.alt);
                // Never collapse (or flip) the selection.
                to.width = to.width.max(1.0);
                to.height = to.height.max(1.0);
                let scale_text = handle.is_corner();
                self.transient.clear();
                for o in &originals {
                    let a = hit::scale_annotation(o, union, to, scale_text);
                    self.transient.insert(a.id, a);
                }
                self.touch();
                Interaction::MultiResize { handle, originals, union }
            }
            Interaction::Marquee { start, additive, base, .. } => {
                let r = Rect::from_points(start, p);
                let anns = self.doc.annotations.clone();
                let mut bounds = |a: &Annotation| self.bounds_of(a);
                let hits = hit::marquee_select(&anns, r, &mut bounds);
                let mut sel = base.clone();
                for h in hits {
                    if !sel.contains(&h) {
                        sel.push(h);
                    }
                }
                self.selection = sel;
                Interaction::Marquee { start, current: p, additive, base }
            }
            Interaction::CropNew { start } => {
                let (w, h) = self.crop_bounds();
                let q = Point::new(p.x.clamp(0.0, w), p.y.clamp(0.0, h));
                let q = if mods.shift { geom::constrain_square(start, q) } else { q };
                self.crop = Some(Rect::from_points(start, q));
                Interaction::CropNew { start }
            }
            Interaction::CropMove { start, original } => {
                let (w, h) = self.crop_bounds();
                let d = p - start;
                let x = (original.x + d.x).clamp(0.0, (w - original.width).max(0.0));
                let y = (original.y + d.y).clamp(0.0, (h - original.height).max(0.0));
                self.crop = Some(Rect::new(x, y, original.width, original.height));
                Interaction::CropMove { start, original }
            }
            Interaction::CropHandle { handle, original } => {
                let (w, h) = self.crop_bounds();
                let q = Point::new(p.x.clamp(0.0, w), p.y.clamp(0.0, h));
                self.crop = Some(geom::resize_rect(original, handle, q, mods.shift, mods.alt));
                Interaction::CropHandle { handle, original }
            }
        };
    }

    pub fn pointer_up(&mut self, p: Point, mods: Mods) {
        let slop = CLICK_SLOP_PX / self.view_scale.max(1e-3);
        let interaction = std::mem::take(&mut self.interaction);
        self.guides.clear();
        match interaction {
            Interaction::None => {}
            Interaction::Create { tool, start, current, points, .. } => {
                let is_click = start.distance(current) < slop && points.len() <= 2;
                let preview = self.build_preview(tool, start, current, &points, mods, is_click);
                self.transient.clear();
                self.touch();
                let Some(a) = preview else { return };
                let keep = match tool {
                    ToolKind::Step | ToolKind::Magnifier | ToolKind::Callout | ToolKind::Text => true,
                    ToolKind::Pen | ToolKind::Highlighter => !is_click || points.len() > 1,
                    _ => !is_click,
                };
                if !keep {
                    return;
                }
                if tool == ToolKind::Text {
                    // Text is added on commit (empty text is never added).
                    self.editing = Some(TextSession {
                        id: a.id,
                        is_new: true,
                        annotation: a,
                        buffer: TextBuffer::new(""),
                        selecting: false,
                    });
                    self.selection.clear();
                    self.touch();
                    return;
                }
                let id = a.id;
                if self.apply(Command::Add(vec![a])) {
                    self.selection = vec![id];
                    if tool == ToolKind::Callout {
                        self.begin_edit(id);
                    }
                }
            }
            Interaction::Move { originals, duplicate, moved, .. } => {
                if moved {
                    let cmd = if duplicate {
                        let added = std::mem::take(&mut self.transient_added);
                        let ids: Vec<AnnotationId> = added.iter().map(|a| a.id).collect();
                        let ok = self.apply(Command::Add(added));
                        if ok {
                            self.selection = ids;
                        }
                        None
                    } else {
                        Some(Command::Batch(
                            originals
                                .iter()
                                .filter_map(|o| self.transient.get(&o.id).map(|n| Command::update(o.clone(), n.clone())))
                                .collect(),
                        ))
                    };
                    self.transient.clear();
                    if let Some(cmd) = cmd {
                        self.apply(cmd);
                    }
                    self.touch();
                }
            }
            Interaction::Handle { original, .. } => {
                if let Some(n) = self.transient.remove(&original.id) {
                    self.apply(Command::update(original, n));
                }
                self.transient.clear();
                self.touch();
            }
            Interaction::MultiResize { originals, .. } => {
                let cmds: Vec<Command> = originals
                    .iter()
                    .filter_map(|o| self.transient.get(&o.id).filter(|n| *n != o).map(|n| Command::update(o.clone(), n.clone())))
                    .collect();
                self.transient.clear();
                if !cmds.is_empty() {
                    self.apply(Command::Batch(cmds));
                }
                self.touch();
            }
            Interaction::Marquee { .. } => {}
            Interaction::CropNew { .. } | Interaction::CropMove { .. } | Interaction::CropHandle { .. } => {
                if let Some(r) = self.crop
                    && (r.width < 2.0 || r.height < 2.0)
                {
                    let (w, h) = self.crop_bounds();
                    self.crop = Some(Rect::new(0.0, 0.0, w, h));
                }
            }
        }
        let _ = p;
    }

    // ------------------------------------------------------------------
    // Keyboard commands
    // ------------------------------------------------------------------

    pub fn select_all(&mut self) {
        self.commit_text();
        self.selection = self.doc.annotations.iter().filter(|a| !a.locked && !a.hidden).map(|a| a.id).collect();
    }

    pub fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = std::mem::take(&mut self.selection);
        self.apply(Command::Remove(ids));
    }

    pub fn duplicate_selection(&mut self) {
        let sf = self.sf();
        let dups: Vec<Annotation> = self
            .doc
            .annotations
            .iter()
            .filter(|a| self.selection.contains(&a.id))
            .map(|a| {
                let mut d = a.duplicate();
                d.translate(10.0 * sf, 10.0 * sf);
                d
            })
            .collect();
        if dups.is_empty() {
            return;
        }
        let ids = dups.iter().map(|a| a.id).collect();
        if self.apply(Command::Add(dups)) {
            self.selection = ids;
        }
    }

    /// Arrow-key nudge by (dx, dy) points (⇧ = 10 pt at the call site).
    pub fn nudge(&mut self, dx: f32, dy: f32) {
        if self.selection.is_empty() {
            return;
        }
        let sf = self.sf();
        let cmds: Vec<Command> = self
            .doc
            .annotations
            .iter()
            .filter(|a| self.selection.contains(&a.id) && !a.locked)
            .map(|a| {
                let mut n = a.clone();
                n.translate(dx * sf, dy * sf);
                Command::update(a.clone(), n)
            })
            .collect();
        self.apply_coalescing(Command::Batch(cmds), "nudge".into());
    }

    pub fn zorder(&mut self, op: ZOrder) {
        if self.selection.is_empty() {
            return;
        }
        let cmd = match op {
            ZOrder::Front => zorder::bring_to_front(&self.doc, &self.selection),
            ZOrder::Forward => zorder::bring_forward(&self.doc, &self.selection),
            ZOrder::Backward => zorder::send_backward(&self.doc, &self.selection),
            ZOrder::Back => zorder::send_to_back(&self.doc, &self.selection),
        };
        self.apply(cmd);
    }

    pub fn undo(&mut self) -> bool {
        self.commit_text();
        self.cancel_interaction();
        let r = self.history.undo(&mut self.doc).unwrap_or(false);
        if r {
            self.prune_selection();
            self.touch();
        }
        r
    }

    pub fn redo(&mut self) -> bool {
        self.commit_text();
        self.cancel_interaction();
        let r = self.history.redo(&mut self.doc).unwrap_or(false);
        if r {
            self.prune_selection();
            self.touch();
        }
        r
    }

    /// Clipboard JSON for the selected annotations (z-order), if any.
    pub fn copy_selection(&self) -> Option<String> {
        let sel: Vec<Annotation> =
            self.doc.annotations.iter().filter(|a| self.selection.contains(&a.id)).cloned().collect();
        (!sel.is_empty()).then(|| AnnotationClipboard::to_json(&sel))
    }

    /// Paste annotations from clipboard text. Returns false if the text is
    /// not a RapidSnap payload.
    pub fn paste(&mut self, text: &str) -> bool {
        let Some(mut anns) = AnnotationClipboard::from_json(text) else { return false };
        if anns.is_empty() {
            return false;
        }
        self.commit_text();
        // Clipboard JSON is untrusted: finite numbers, sane sizes, fresh ids
        // (the coordinate window covers the whole beautify canvas).
        let content = self.doc.content_size();
        let mut taken: std::collections::HashSet<AnnotationId> = self.doc.annotations.iter().map(|a| a.id).collect();
        Annotation::sanitize_all_with_margin(&mut anns, content, self.doc.annotation_margin(), &mut taken);
        // Pasting into the same document (same geometry exists) → offset.
        let overlaps = anns.iter().any(|a| self.doc.annotations.iter().any(|b| b.kind == a.kind));
        if overlaps {
            self.paste_count += 1;
            let d = 10.0 * self.sf() * self.paste_count as f32;
            anns.iter_mut().for_each(|a| a.translate(d, d));
        } else {
            self.paste_count = 0;
        }
        // Keep the pasted group on the canvas (image + padding; it may come
        // from a bigger screenshot, or the repeated paste offset may push it
        // out). Annotations over the padding stay where they are.
        let area = self.drawable_rect();
        let mut union: Option<Rect> = None;
        for a in &anns {
            let b = self.bounds_of(a);
            union = Some(union.map_or(b, |u| u.union(&b)));
        }
        if let Some(u) = union {
            let (dx, dy) = shift_into(u, area);
            if dx != 0.0 || dy != 0.0 {
                anns.iter_mut().for_each(|a| a.translate(dx, dy));
            }
        }
        // A group larger than the canvas (e.g. absurd coordinates): bring
        // every annotation that is still completely outside back in on its own.
        for a in anns.iter_mut() {
            let b = self.bounds_of(a);
            if !b.outset(0.5).intersects(&area) {
                let (dx, dy) = shift_into(b, area);
                a.translate(dx, dy);
            }
        }
        let ids = anns.iter().map(|a| a.id).collect();
        if self.apply(Command::Add(anns)) {
            self.selection = ids;
            true
        } else {
            false
        }
    }

    /// Add annotations as one undo step (auto-redact, API).
    pub fn add_annotations(&mut self, anns: Vec<Annotation>) -> bool {
        let ids = anns.iter().map(|a| a.id).collect();
        let ok = self.apply(Command::Add(anns));
        if ok {
            self.selection = ids;
        }
        ok
    }

    /// Renumber all step counters (per label style) by z-order.
    pub fn renumber_steps(&mut self) {
        let mut counters: HashMap<StepLabel, u32> = HashMap::new();
        let mut cmds = Vec::new();
        for a in &self.doc.annotations {
            if let AnnotationKind::Step(s) = &a.kind {
                let n = counters.entry(s.label).or_insert(0);
                *n += 1;
                if s.value != *n {
                    let mut b = a.clone();
                    if let AnnotationKind::Step(bs) = &mut b.kind {
                        bs.value = *n;
                    }
                    cmds.push(Command::update(a.clone(), b));
                }
            }
        }
        if !cmds.is_empty() {
            self.apply(Command::Batch(cmds));
        }
    }

    // ------------------------------------------------------------------
    // Style
    // ------------------------------------------------------------------

    /// What the style panel should show: (tool, style in pt, sections).
    pub fn panel_target(&self) -> (ToolKind, ToolStyle, Sections) {
        let sf = self.sf();
        if let Some(s) = &self.editing {
            let t = s.annotation.tool();
            return (t, style_map::style_of(&s.annotation, &self.tool_style(t), sf), Sections::for_tool(t));
        }
        if let Some(a) = self.selection.first().and_then(|id| self.current(*id)) {
            let t = a.tool();
            return (t, style_map::style_of(&a, &self.tool_style(t), sf), Sections::for_tool(t));
        }
        let t = self.tool;
        (t, self.tool_style(t), Sections::for_tool(t))
    }

    /// Apply a style edit to the text being edited, else the selection, and
    /// remember it for the relevant tool(s).
    pub fn apply_style(&mut self, e: &StyleEdit) {
        let sf = self.sf();
        let mut tools: Vec<ToolKind> = Vec::new();
        if let Some(s) = &mut self.editing {
            style_map::apply_to_annotation(&mut s.annotation, e, sf);
            tools.push(s.annotation.tool());
            self.touch();
        } else if !self.selection.is_empty() {
            let mut cmds = Vec::new();
            for a in self.doc.annotations.iter().filter(|a| self.selection.contains(&a.id)) {
                let mut n = a.clone();
                if style_map::apply_to_annotation(&mut n, e, sf) {
                    cmds.push(Command::update(a.clone(), n));
                }
                if !tools.contains(&a.tool()) {
                    tools.push(a.tool());
                }
            }
            if !cmds.is_empty() {
                let key = format!("style:{}:{:?}", e.key(), self.selection);
                self.apply_coalescing(Command::Batch(cmds), key);
            }
        } else if self.tool.creates_annotation() {
            tools.push(self.tool);
        }
        for t in tools {
            let mut st = self.tool_style(t);
            style_map::apply_to_tool_style(t, &mut st, e);
            self.styles.insert(t, st);
            self.styles_dirty = true;
        }
    }

    // ------------------------------------------------------------------
    // Beautify & image transform
    // ------------------------------------------------------------------

    /// Replace the beautify settings (`coalesce` = slider key).
    pub fn set_beautify(&mut self, b: BeautifySettings, coalesce: Option<&str>) {
        let cmd = Command::set_beautify(self.doc.beautify.clone(), b);
        match coalesce {
            Some(k) => self.apply_coalescing(cmd, format!("beautify:{k}")),
            None => self.apply(cmd),
        };
    }

    pub fn set_transform(&mut self, t: ImageTransform) {
        let before = self.doc.image_transform;
        self.bounds_cache.clear();
        self.apply(Command::SetImageTransform { before, after: t });
    }

    pub fn rotate(&mut self, clockwise: bool) {
        self.commit_text();
        self.crop = None;
        if self.tool == ToolKind::Crop {
            self.tool = ToolKind::Select;
        }
        let mut t = self.doc.image_transform;
        t.rotation = if clockwise { t.rotation.cw() } else { t.rotation.ccw() };
        self.set_transform(t);
    }

    /// Transform used while cropping: current rotation, no crop.
    pub fn crop_transform(&self) -> ImageTransform {
        ImageTransform { crop: None, rotation: self.doc.image_transform.rotation }
    }

    fn crop_content_size(&self) -> rs_core::Size {
        let (w, h) = self.crop_transform().content_size(self.doc.base_image.width, self.doc.base_image.height);
        rs_core::Size::new(w as f32, h as f32)
    }

    fn crop_bounds(&self) -> (f32, f32) {
        let s = self.crop_content_size();
        (s.width, s.height)
    }

    fn map_content(&self, p: Point, from: &ImageTransform, to: &ImageTransform) -> Point {
        let (w, h) = (self.doc.base_image.width, self.doc.base_image.height);
        to.base_to_content(from.content_to_base(p, w, h), w, h)
    }

    fn enter_crop(&mut self) {
        self.commit_text();
        self.selection.clear();
        let cur = self.doc.image_transform;
        let t0 = self.crop_transform();
        let cs = self.doc.content_size();
        let a = self.map_content(Point::ZERO, &cur, &t0);
        let b = self.map_content(Point::new(cs.width, cs.height), &cur, &t0);
        self.crop = Some(Rect::from_points(a, b));
        self.touch();
    }

    fn crop_pointer_down(&mut self, p: Point) {
        let Some(r) = self.crop else { return };
        let hr = self.handle_radius();
        if let Some(h) = RectHandle::ALL.into_iter().find(|h| h.position(r).distance(p) <= hr) {
            self.interaction = Interaction::CropHandle { handle: h, original: r };
        } else if r.contains(p) {
            self.interaction = Interaction::CropMove { start: p, original: r };
        } else {
            self.crop = Some(Rect::from_points(p, p));
            self.interaction = Interaction::CropNew { start: p };
        }
    }

    /// Apply the crop rect (crop mode) → `Command::SetImageTransform`.
    pub fn apply_crop(&mut self) {
        let Some(r) = self.crop.take() else { return };
        self.tool = ToolKind::Select;
        let t0 = self.crop_transform();
        let (w, h) = (self.doc.base_image.width, self.doc.base_image.height);
        let a = t0.content_to_base(r.origin(), w, h);
        let b = t0.content_to_base(Point::new(r.right(), r.bottom()), w, h);
        let br = Rect::from_points(a, b);
        let x0 = br.x.round().max(0.0) as u32;
        let y0 = br.y.round().max(0.0) as u32;
        let x1 = (br.right().round().max(0.0) as u32).min(w);
        let y1 = (br.bottom().round().max(0.0) as u32).min(h);
        let crop = (x1 > x0 && y1 > y0).then(|| IRect::new(x0, y0, x1 - x0, y1 - y0));
        let crop = crop.filter(|c| *c != IRect::new(0, 0, w, h));
        let after = ImageTransform { crop, rotation: t0.rotation };
        self.set_transform(after);
        self.touch();
    }

    pub fn cancel_crop(&mut self) {
        if self.crop.take().is_some() {
            self.tool = ToolKind::Select;
            self.touch();
        }
    }

    pub fn reset_crop(&mut self) {
        let t = ImageTransform { crop: None, ..self.doc.image_transform };
        self.crop = None;
        self.set_transform(t);
    }

    // ------------------------------------------------------------------
    // Eyedropper
    // ------------------------------------------------------------------

    /// Color under content point `p` (of the display doc): the composed
    /// canvas (background, frame, image and annotations — works over the
    /// padding) when the view provided it, else the base-image pixel.
    pub fn sample_color(&self, p: Point) -> Option<Color> {
        if let Some((img, l)) = &self.canvas_sample {
            let q = l.image_to_canvas(p);
            if q.x >= 0.0
                && q.y >= 0.0
                && let Some([r, g, b, a]) = img.pixel(q.x as u32, q.y as u32)
                && a > 0
            {
                return Some(Color::rgba(r, g, b, a));
            }
        }
        let t = if self.crop.is_some() { self.crop_transform() } else { self.doc.image_transform };
        let img = &self.doc.base_image;
        let b = t.content_to_base(p, img.width, img.height);
        if b.x < 0.0 || b.y < 0.0 {
            return None;
        }
        let [r, g, bl, a] = img.pixel(b.x as u32, b.y as u32)?;
        Some(Color::rgba(r, g, bl, a))
    }

    // ------------------------------------------------------------------
    // Text editing sessions
    // ------------------------------------------------------------------

    /// Start editing an existing Text / Callout annotation.
    pub fn begin_edit(&mut self, id: AnnotationId) {
        self.commit_text();
        self.cancel_interaction();
        let Some(a) = self.doc.get(id).cloned() else { return };
        let text = match &a.kind {
            AnnotationKind::Text(t) => t.content.clone(),
            AnnotationKind::Callout(c) => c.text.clone(),
            _ => return,
        };
        let mut buffer = TextBuffer::new(text);
        buffer.select_all();
        self.selection = vec![id];
        self.editing = Some(TextSession { id, is_new: false, annotation: a, buffer, selecting: false });
        self.touch();
    }

    /// Start a new text at `p` (optionally fixed `width`) — same as a click
    /// with the Text tool.
    pub fn begin_new_text(&mut self, p: Point, width: Option<f32>) {
        self.commit_text();
        let st = self.tool_style_px(ToolKind::Text);
        let to = Point::new(p.x + width.unwrap_or(0.0), p.y);
        let Some(mut a) = st.create(ToolKind::Text, p, to) else { return };
        if let AnnotationKind::Text(t) = &mut a.kind {
            t.position = p;
            t.width = width;
        }
        self.editing = Some(TextSession { id: a.id, is_new: true, annotation: a, buffer: TextBuffer::new(""), selecting: false });
        self.touch();
    }

    /// Mutate the edit buffer (keeps the annotation in sync).
    pub fn edit_text(&mut self, f: impl FnOnce(&mut TextBuffer)) {
        if let Some(s) = &mut self.editing {
            let before = s.buffer.text.clone();
            f(&mut s.buffer);
            if s.buffer.text != before {
                s.sync();
                self.touch();
            }
        }
    }

    /// Commit the text session: add (new, non-empty), update (changed),
    /// remove (emptied Text), or drop (new, empty).
    pub fn commit_text(&mut self) {
        let Some(mut s) = self.editing.take() else { return };
        s.sync();
        let empty = s.buffer.text.trim().is_empty();
        let is_text = matches!(s.annotation.kind, AnnotationKind::Text(_));
        if s.is_new {
            if !empty {
                let id = s.id;
                if self.apply(Command::Add(vec![s.annotation])) {
                    self.selection = vec![id];
                }
            }
        } else if let Some(orig) = self.doc.get(s.id).cloned() {
            if empty && is_text {
                self.apply(Command::Remove(vec![s.id]));
                self.selection.retain(|x| *x != s.id);
            } else if orig != s.annotation {
                self.apply(Command::update(orig, s.annotation));
            }
        }
        self.touch();
    }

    /// Discard the session's changes.
    pub fn cancel_text(&mut self) {
        if self.editing.take().is_some() {
            self.touch();
        }
    }

    // ------------------------------------------------------------------
    // Display
    // ------------------------------------------------------------------

    /// The document to render right now (history state + transient edits +
    /// creation preview + text session + crop mode) and ids to exclude.
    pub fn display(&self) -> (Document, Vec<AnnotationId>) {
        let mut d = self.doc.clone();
        let exclude = Vec::new();
        if !self.transient.is_empty() {
            for a in d.annotations.iter_mut() {
                if let Some(t) = self.transient.get(&a.id) {
                    *a = t.clone();
                }
            }
        }
        d.annotations.extend(self.transient_added.iter().cloned());
        if let Interaction::Create { preview: Some(p), .. } = &self.interaction
            && p.tool() != ToolKind::Text
        {
            d.annotations.push(p.clone());
        }
        // The text being edited is rendered by rs-render like committed text
        // (the view only overlays caret / selection / IME marks), so nothing
        // jumps on commit.
        if let Some(s) = &self.editing {
            let a = s.annotation.clone();
            match d.index_of(a.id) {
                Some(ix) => d.annotations[ix] = a,
                None => d.annotations.push(a),
            }
        }
        if self.crop.is_some() {
            let t0 = self.crop_transform();
            let cur = self.doc.image_transform;
            rs_core::remap_annotations(&mut d.annotations, &cur, &t0, d.base_image.width, d.base_image.height);
            d.image_transform = t0;
            d.beautify.enabled = false;
        }
        (d, exclude)
    }
}

/// Smallest translation that moves rect `r` inside `area`; when `r` is
/// larger than the area its top-left goes to the area's top-left.
fn shift_into(r: Rect, area: Rect) -> (f32, f32) {
    let axis = |lo: f32, len: f32, min: f32, max_len: f32| -> f32 {
        if !(lo.is_finite() && len.is_finite() && min.is_finite() && max_len.is_finite()) {
            return 0.0;
        }
        let lo = lo - min;
        if len >= max_len || lo < 0.0 {
            -lo
        } else if lo + len > max_len {
            max_len - (lo + len)
        } else {
            0.0
        }
    };
    (axis(r.x, r.width, area.x, area.width), axis(r.y, r.height, area.y, area.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::{DocumentMetadata, RgbaImage};

    fn model() -> EditorModel {
        let doc = Document::new(RgbaImage::filled(400, 300, [200, 10, 10, 255]), DocumentMetadata::default())
            .with_beautify(BeautifySettings::disabled());
        let mut m = EditorModel::new(doc, BTreeMap::new());
        m.view_scale = 1.0;
        m.snapping = false;
        m
    }

    fn drag(m: &mut EditorModel, a: Point, b: Point, mods: Mods) {
        m.pointer_down(a, mods, 1);
        m.pointer_move(a.lerp(b, 0.5), mods);
        m.pointer_move(b, mods);
        m.pointer_up(b, mods);
    }

    #[test]
    fn draw_rect_select_move_undo_redo() {
        let mut m = model();
        m.set_tool(ToolKind::Rect);
        drag(&mut m, Point::new(10.0, 10.0), Point::new(110.0, 60.0), Mods::default());
        assert_eq!(m.doc.annotations.len(), 1);
        let id = m.doc.annotations[0].id;
        assert_eq!(m.selection, vec![id]);
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(10.0, 10.0, 100.0, 50.0));

        // A click (no drag) with the rect tool adds nothing.
        m.pointer_down(Point::new(200.0, 200.0), Mods::default(), 1);
        m.pointer_up(Point::new(201.0, 200.0), Mods::default());
        assert_eq!(m.doc.annotations.len(), 1);

        // Move with the select tool (click on the stroke).
        m.set_tool(ToolKind::Select);
        drag(&mut m, Point::new(10.0, 30.0), Point::new(30.0, 50.0), Mods::default());
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(30.0, 30.0));
        assert_eq!(m.history.undo_len(), 2);

        assert!(m.undo());
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(10.0, 10.0));
        assert!(m.undo());
        assert!(m.doc.annotations.is_empty());
        assert!(m.selection.is_empty());
        assert!(m.redo());
        assert!(m.redo());
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(30.0, 30.0));
    }

    #[test]
    fn constrain_and_from_center() {
        let mut m = model();
        m.set_tool(ToolKind::Ellipse);
        drag(&mut m, Point::new(100.0, 100.0), Point::new(150.0, 120.0), Mods { shift: true, alt: true, cmd: false });
        let b = m.doc.annotations[0].geometric_bounds();
        assert_eq!(b, Rect::new(50.0, 50.0, 100.0, 100.0));
    }

    #[test]
    fn resize_via_handle_is_one_undo_step() {
        let mut m = model();
        m.set_tool(ToolKind::Rect);
        drag(&mut m, Point::new(10.0, 10.0), Point::new(110.0, 60.0), Mods::default());
        // Bottom-right handle (stroke-outset bounds ≈ rect ± 2).
        let br = {
            let a = m.doc.annotations[0].clone();
            let b = m.bounds_of(&a);
            let _ = b;
            Point::new(110.0, 60.0)
        };
        drag(&mut m, br, Point::new(210.0, 160.0), Mods::default());
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(10.0, 10.0, 200.0, 150.0));
        assert_eq!(m.history.undo_len(), 2);
        m.undo();
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(10.0, 10.0, 100.0, 50.0));
    }

    #[test]
    fn marquee_and_shift_click() {
        let mut m = model();
        m.set_tool(ToolKind::Redact);
        drag(&mut m, Point::new(10.0, 10.0), Point::new(40.0, 40.0), Mods::default());
        drag(&mut m, Point::new(100.0, 10.0), Point::new(140.0, 40.0), Mods::default());
        drag(&mut m, Point::new(10.0, 200.0), Point::new(40.0, 240.0), Mods::default());
        m.set_tool(ToolKind::Select);
        drag(&mut m, Point::new(0.0, 0.0), Point::new(200.0, 100.0), Mods::default());
        assert_eq!(m.selection.len(), 2);
        m.pointer_down(Point::new(20.0, 220.0), Mods { shift: true, ..Default::default() }, 1);
        m.pointer_up(Point::new(20.0, 220.0), Mods::default());
        assert_eq!(m.selection.len(), 3);
        m.pointer_down(Point::new(20.0, 220.0), Mods { shift: true, ..Default::default() }, 1);
        m.pointer_up(Point::new(20.0, 220.0), Mods::default());
        assert_eq!(m.selection.len(), 2);
        m.delete_selection();
        assert_eq!(m.doc.annotations.len(), 1);
        m.undo();
        assert_eq!(m.doc.annotations.len(), 3);
    }

    #[test]
    fn duplicate_nudge_zorder_copy_paste() {
        let mut m = model();
        m.set_tool(ToolKind::Redact);
        drag(&mut m, Point::new(10.0, 10.0), Point::new(40.0, 40.0), Mods::default());
        let first = m.doc.annotations[0].id;
        m.duplicate_selection();
        assert_eq!(m.doc.annotations.len(), 2);
        let dup = m.selection[0];
        assert_ne!(dup, first);
        m.nudge(1.0, 0.0);
        m.nudge(1.0, 0.0);
        let x = m.doc.get(dup).unwrap().geometric_bounds().x;
        assert_eq!(x, 22.0);
        let undo_before = m.history.undo_len();
        m.break_coalescing();
        m.nudge(0.0, 10.0);
        assert_eq!(m.history.undo_len(), undo_before + 1);
        m.zorder(ZOrder::Back);
        assert_eq!(m.doc.annotations[0].id, dup);
        m.zorder(ZOrder::Front);
        assert_eq!(m.doc.annotations[1].id, dup);
        let json = m.copy_selection().unwrap();
        assert!(m.paste(&json));
        assert_eq!(m.doc.annotations.len(), 3);
        assert!(!m.paste("not json"));
    }

    #[test]
    fn alt_drag_duplicates() {
        let mut m = model();
        m.set_tool(ToolKind::Redact);
        drag(&mut m, Point::new(10.0, 10.0), Point::new(40.0, 40.0), Mods::default());
        m.set_tool(ToolKind::Select);
        drag(&mut m, Point::new(20.0, 20.0), Point::new(120.0, 20.0), Mods { alt: true, ..Default::default() });
        assert_eq!(m.doc.annotations.len(), 2);
        assert_eq!(m.doc.annotations[0].geometric_bounds().x, 10.0);
        assert_eq!(m.doc.annotations[1].geometric_bounds().x, 110.0);
    }

    #[test]
    fn text_session_lifecycle() {
        let mut m = model();
        m.set_tool(ToolKind::Text);
        // Click → session, nothing in doc yet.
        m.pointer_down(Point::new(50.0, 50.0), Mods::default(), 1);
        m.pointer_up(Point::new(50.0, 50.0), Mods::default());
        assert!(m.editing.is_some());
        assert!(m.doc.annotations.is_empty());
        m.edit_text(|b| b.insert("Hello"));
        // Display shows the session annotation, rendered by rs-render (not excluded).
        let (d, ex) = m.display();
        assert_eq!(d.annotations.len(), 1);
        assert!(ex.is_empty());
        m.commit_text();
        assert_eq!(m.doc.annotations.len(), 1);
        let id = m.doc.annotations[0].id;
        // Empty new text is dropped.
        m.pointer_down(Point::new(200.0, 200.0), Mods::default(), 1);
        m.pointer_up(Point::new(200.0, 200.0), Mods::default());
        assert!(m.editing.is_some());
        m.escape();
        assert_eq!(m.doc.annotations.len(), 1);
        // Double-click to re-edit; clearing the text removes it.
        m.set_tool(ToolKind::Select);
        m.pointer_down(Point::new(55.0, 55.0), Mods::default(), 2);
        assert_eq!(m.editing.as_ref().map(|s| s.id), Some(id));
        m.edit_text(|b| {
            b.select_all();
            b.backspace();
        });
        m.commit_text();
        assert!(m.doc.annotations.is_empty());
        m.undo();
        assert_eq!(m.doc.annotations.len(), 1);
    }

    #[test]
    fn text_drag_creates_fixed_width_box() {
        let mut m = model();
        m.set_tool(ToolKind::Text);
        drag(&mut m, Point::new(20.0, 20.0), Point::new(220.0, 60.0), Mods::default());
        let s = m.editing.as_ref().unwrap();
        let AnnotationKind::Text(t) = &s.annotation.kind else { panic!() };
        assert_eq!(t.width, Some(200.0));
        assert_eq!(t.position, Point::new(20.0, 20.0));
    }

    #[test]
    fn steps_auto_increment_and_renumber() {
        let mut m = model();
        m.set_tool(ToolKind::Step);
        for x in [20.0, 60.0, 100.0] {
            m.pointer_down(Point::new(x, 20.0), Mods::default(), 1);
            m.pointer_up(Point::new(x, 20.0), Mods::default());
        }
        let vals: Vec<u32> = m
            .doc
            .annotations
            .iter()
            .filter_map(|a| if let AnnotationKind::Step(s) = &a.kind { Some(s.value) } else { None })
            .collect();
        assert_eq!(vals, vec![1, 2, 3]);
        m.selection = vec![m.doc.annotations[1].id];
        m.delete_selection();
        m.renumber_steps();
        let vals: Vec<u32> = m
            .doc
            .annotations
            .iter()
            .filter_map(|a| if let AnnotationKind::Step(s) = &a.kind { Some(s.value) } else { None })
            .collect();
        assert_eq!(vals, vec![1, 2]);
    }

    #[test]
    fn style_edits_apply_to_selection_and_memory() {
        let mut m = model();
        m.set_tool(ToolKind::Arrow);
        m.apply_style(&StyleEdit::StrokeWidth(9.0));
        assert_eq!(m.tool_style(ToolKind::Arrow).stroke.width, 9.0);
        drag(&mut m, Point::new(10.0, 10.0), Point::new(100.0, 100.0), Mods::default());
        let AnnotationKind::Arrow(a) = &m.doc.annotations[0].kind else { panic!() };
        assert_eq!(a.stroke.width, 9.0);
        m.apply_style(&StyleEdit::Color(Color::BLUE));
        m.apply_style(&StyleEdit::Color(Color::GREEN));
        // Two color edits coalesce into one undo step.
        assert_eq!(m.history.undo_len(), 2);
        let AnnotationKind::Arrow(a) = &m.doc.annotations[0].kind else { panic!() };
        assert_eq!(a.stroke.color, Color::GREEN);
        assert!(m.styles_dirty);
        let (tool, st, sec) = m.panel_target();
        assert_eq!(tool, ToolKind::Arrow);
        assert_eq!(st.stroke.color, Color::GREEN);
        assert!(sec.arrow);
    }

    #[test]
    fn crop_and_rotate() {
        let mut m = model();
        m.set_tool(ToolKind::Redact);
        drag(&mut m, Point::new(100.0, 100.0), Point::new(150.0, 150.0), Mods::default());
        m.set_tool(ToolKind::Crop);
        assert_eq!(m.crop, Some(Rect::new(0.0, 0.0, 400.0, 300.0)));
        // Drag a new crop rect from outside... start inside moves, so use a handle.
        drag(&mut m, Point::new(0.0, 0.0), Point::new(50.0, 50.0), Mods::default());
        assert_eq!(m.crop, Some(Rect::new(50.0, 50.0, 350.0, 250.0)));
        m.apply_crop();
        assert_eq!(m.doc.content_size(), rs_core::Size::new(350.0, 250.0));
        // Annotation stays on the same pixels.
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(50.0, 50.0));
        m.rotate(true);
        assert_eq!(m.doc.content_size(), rs_core::Size::new(250.0, 350.0));
        m.undo();
        m.undo();
        assert_eq!(m.doc.content_size(), rs_core::Size::new(400.0, 300.0));
        // Re-entering crop mode after a crop shows the crop rect in full-image space.
        m.redo();
        m.set_tool(ToolKind::Crop);
        assert_eq!(m.crop, Some(Rect::new(50.0, 50.0, 350.0, 250.0)));
        let (d, _) = m.display();
        assert_eq!(d.content_size(), rs_core::Size::new(400.0, 300.0));
        m.escape();
        assert!(m.crop.is_none());
    }

    #[test]
    fn eyedropper_samples_base_image() {
        let mut m = model();
        m.eyedropper = true;
        m.pointer_down(Point::new(5.0, 5.0), Mods::default(), 1);
        assert_eq!(m.picked_color, Some(Color::rgba(200, 10, 10, 255)));
        assert!(!m.eyedropper);
        assert!(m.doc.annotations.is_empty());
    }

    #[test]
    fn snapping_to_image_edge() {
        let mut m = model();
        m.snapping = true;
        m.set_tool(ToolKind::Redact);
        drag(&mut m, Point::new(3.0, 3.0), Point::new(40.0, 40.0), Mods::default());
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(0.0, 0.0));
        // ⌘ disables snapping.
        drag(&mut m, Point::new(103.0, 3.0), Point::new(140.0, 40.0), Mods { cmd: true, ..Default::default() });
        assert_eq!(m.doc.annotations[1].geometric_bounds().origin(), Point::new(103.0, 3.0));
    }

    #[test]
    fn pen_and_callout_and_magnifier() {
        let mut m = model();
        m.set_tool(ToolKind::Pen);
        m.pointer_down(Point::new(10.0, 10.0), Mods::default(), 1);
        for i in 1..20 {
            m.pointer_move(Point::new(10.0 + i as f32 * 5.0, 10.0 + (i as f32).sin() * 5.0), Mods::default());
        }
        m.pointer_up(Point::new(110.0, 10.0), Mods::default());
        let AnnotationKind::Pen(p) = &m.doc.annotations[0].kind else { panic!() };
        assert!(p.points.len() > 10);
        m.set_tool(ToolKind::Callout);
        m.pointer_down(Point::new(50.0, 100.0), Mods::default(), 1);
        m.pointer_up(Point::new(50.0, 100.0), Mods::default());
        assert!(matches!(m.doc.annotations[1].kind, AnnotationKind::Callout(_)));
        assert!(m.editing.is_some(), "callout starts text editing");
        m.edit_text(|b| b.insert("Note"));
        m.commit_text();
        let AnnotationKind::Callout(c) = &m.doc.annotations[1].kind else { panic!() };
        assert_eq!(c.text, "Note");
        m.set_tool(ToolKind::Magnifier);
        m.pointer_down(Point::new(200.0, 200.0), Mods::default(), 1);
        m.pointer_up(Point::new(200.0, 200.0), Mods::default());
        assert!(matches!(m.doc.annotations[2].kind, AnnotationKind::Magnifier(_)));
    }

    #[test]
    fn multi_selection_resize_scales_all_one_undo_step() {
        let mut m = model();
        m.set_tool(ToolKind::Redact);
        drag(&mut m, Point::new(10.0, 10.0), Point::new(50.0, 50.0), Mods::default());
        drag(&mut m, Point::new(60.0, 60.0), Point::new(110.0, 110.0), Mods::default());
        m.set_tool(ToolKind::Arrow);
        drag(&mut m, Point::new(10.0, 100.0), Point::new(60.0, 100.0), Mods::default());
        let arrow_w = match &m.doc.annotations[2].kind {
            AnnotationKind::Arrow(a) => a.stroke.width,
            _ => 0.0,
        };
        m.set_tool(ToolKind::Select);
        m.selection = m.doc.annotations[..2].iter().map(|a| a.id).collect();
        let (u, hs) = m.multi_handles().unwrap();
        assert_eq!(u, Rect::new(10.0, 10.0, 100.0, 100.0));
        assert_eq!(hs.len(), 8);
        let undo0 = m.history.undo_len();
        // Drag bottom-right from (110,110) to (210,210): ×2 from (10,10).
        drag(&mut m, Point::new(110.0, 110.0), Point::new(210.0, 210.0), Mods::default());
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(10.0, 10.0, 80.0, 80.0));
        assert_eq!(m.doc.annotations[1].geometric_bounds(), Rect::new(110.0, 110.0, 100.0, 100.0));
        assert_eq!(m.history.undo_len(), undo0 + 1, "single undo step");
        assert!(m.undo());
        assert_eq!(m.doc.annotations[1].geometric_bounds(), Rect::new(60.0, 60.0, 50.0, 50.0));
        // With an arrow: stroke width unchanged, endpoints scaled.
        m.selection = m.doc.annotations.iter().map(|a| a.id).collect();
        let (u, _) = m.multi_handles().unwrap();
        let right = RectHandle::Right.position(u);
        drag(&mut m, right, Point::new(right.x + u.width, right.y), Mods::default());
        let AnnotationKind::Arrow(a) = &m.doc.annotations[2].kind else { panic!() };
        assert_eq!(a.stroke.width, arrow_w);
        assert!((a.end.x - a.start.x - 100.0).abs() < 1.0, "{a:?}");
        // Single selection: no multi handles.
        m.selection.truncate(1);
        assert!(m.multi_handles().is_none());
    }

    #[test]
    fn paste_is_sanitized_and_kept_in_bounds() {
        let mut m = model();
        let mut a = Annotation::new(AnnotationKind::Redact(rs_core::RedactAnnotation {
            rect: Rect::new(5000.0, 5000.0, 40.0, -30.0),
            color: Color::BLACK,
            corner_radius: 1e30,
        }));
        a.opacity = 7.0;
        let mut b = a.clone();
        b.kind = AnnotationKind::Redact(rs_core::RedactAnnotation {
            rect: Rect::new(-1e12, 1.0, 10.0, 10.0),
            color: Color::BLACK,
            corner_radius: 0.0,
        });
        let json = AnnotationClipboard::to_json(&[a, b]);
        assert!(m.paste(&json));
        assert_eq!(m.doc.annotations.len(), 2);
        assert_ne!(m.doc.annotations[0].id, m.doc.annotations[1].id, "duplicate ids replaced");
        let content = Rect::new(0.0, 0.0, 400.0, 300.0);
        for a in &m.doc.annotations {
            let g = a.geometric_bounds();
            assert!(g.x.is_finite() && g.y.is_finite() && g.width >= 0.0 && g.height >= 0.0, "{g:?}");
            assert!(a.opacity <= 1.0);
            assert!(content.outset(0.5).intersects(&g.outset(0.5)), "in bounds {g:?}");
            if let AnnotationKind::Redact(r) = &a.kind {
                assert!(r.corner_radius <= rs_core::sanitize::limits::MAX_CORNER_RADIUS);
            }
        }
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(shift_into(Rect::new(390.0, -5.0, 20.0, 10.0), area), (-10.0, 5.0));
        assert_eq!(shift_into(Rect::new(10.0, 10.0, 20.0, 10.0), area), (0.0, 0.0));
        // Canvas area with padding: negative coordinates are inside.
        let canvas = Rect::new(-50.0, -50.0, 500.0, 400.0);
        assert_eq!(shift_into(Rect::new(-40.0, -45.0, 20.0, 10.0), canvas), (0.0, 0.0));
        assert_eq!(shift_into(Rect::new(-60.0, 345.0, 20.0, 10.0), canvas), (10.0, -5.0));
    }

    /// 400×300 image (sf 1) with beautify on: 40 pt padding, no inset /
    /// chrome / shadow / aspect → canvas = (-40, -40, 480×380) content px.
    fn padded_model() -> EditorModel {
        let mut b = BeautifySettings {
            enabled: true,
            auto_balance: false,
            padding: rs_core::Padding::uniform(40.0),
            ..Default::default()
        };
        b.inset.enabled = false;
        b.chrome = rs_core::WindowChrome::None;
        b.shadow.enabled = false;
        b.aspect_ratio = rs_core::AspectRatio::Free;
        b.corner_radius = 12.0;
        b.background = rs_core::Background::Solid { color: Color::rgb(0, 0, 255) };
        b.watermark = None;
        let doc = Document::new(RgbaImage::filled(400, 300, [200, 10, 10, 255]), DocumentMetadata::default()).with_beautify(b);
        let mut m = EditorModel::new(doc, BTreeMap::new());
        m.view_scale = 1.0;
        m.snapping = false;
        m
    }

    #[test]
    fn canvas_rect_covers_padding() {
        let m = padded_model();
        assert_eq!(m.canvas_rect(), Some(Rect::new(-40.0, -40.0, 480.0, 380.0)));
        assert_eq!(m.drawable_rect(), Rect::new(-40.0, -40.0, 480.0, 380.0));
        assert_eq!(model().canvas_rect(), None, "beautify off: canvas == image");
        assert_eq!(model().drawable_rect(), Rect::new(0.0, 0.0, 400.0, 300.0));
    }

    #[test]
    fn create_pick_move_resize_in_padding() {
        let mut m = padded_model();
        // Rect drawn entirely in the top-left padding.
        m.set_tool(ToolKind::Rect);
        drag(&mut m, Point::new(-35.0, -35.0), Point::new(-5.0, -10.0), Mods::default());
        assert_eq!(m.doc.annotations.len(), 1);
        let id = m.doc.annotations[0].id;
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(-35.0, -35.0, 30.0, 25.0));
        // Picked on its stroke (outside the image).
        assert_eq!(m.pick(Point::new(-35.0, -20.0)), Some(id));
        // Resize via the bottom-right handle, into the image and beyond.
        m.set_tool(ToolKind::Select);
        m.selection = vec![id];
        drag(&mut m, Point::new(-5.0, -10.0), Point::new(20.0, 15.0), Mods::default());
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(-35.0, -35.0, 55.0, 50.0));
        // Move it to the bottom-right padding (beyond the image size).
        drag(&mut m, Point::new(-35.0, -20.0), Point::new(385.0, 305.0), Mods::default());
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(385.0, 290.0));
        // Nudge further out: no clamping.
        m.nudge(10.0, 10.0);
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(395.0, 300.0));
        assert!(m.undo() && m.undo() && m.undo());
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(-35.0, -35.0, 30.0, 25.0));
        assert!(m.redo());
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(-35.0, -35.0, 55.0, 50.0));
    }

    #[test]
    fn move_from_image_into_padding_and_marquee_there() {
        let mut m = padded_model();
        m.set_tool(ToolKind::Redact);
        drag(&mut m, Point::new(100.0, 100.0), Point::new(140.0, 130.0), Mods::default());
        m.set_tool(ToolKind::Select);
        drag(&mut m, Point::new(120.0, 110.0), Point::new(120.0 - 125.0, 110.0 - 128.0), Mods::default());
        let g = m.doc.annotations[0].geometric_bounds();
        assert_eq!(g, Rect::new(-25.0, -28.0, 40.0, 30.0));
        // Marquee selection starting in the padding picks it up.
        m.selection.clear();
        drag(&mut m, Point::new(-39.0, -39.0), Point::new(30.0, 10.0), Mods::default());
        assert_eq!(m.selection, vec![m.doc.annotations[0].id]);
        // Every other tool too: step, arrow, pen, text in the padding.
        m.set_tool(ToolKind::Step);
        m.pointer_down(Point::new(420.0, -20.0), Mods::default(), 1);
        m.pointer_up(Point::new(420.0, -20.0), Mods::default());
        m.set_tool(ToolKind::Arrow);
        drag(&mut m, Point::new(-30.0, 320.0), Point::new(60.0, 330.0), Mods::default());
        m.set_tool(ToolKind::Text);
        m.pointer_down(Point::new(10.0, 310.0), Mods::default(), 1);
        m.pointer_up(Point::new(10.0, 310.0), Mods::default());
        m.edit_text(|b| b.insert("Note"));
        m.commit_text();
        assert_eq!(m.doc.annotations.len(), 4);
        let AnnotationKind::Text(t) = &m.doc.annotations[3].kind else { panic!() };
        assert_eq!(t.position, Point::new(10.0, 310.0));
        let tid = m.doc.annotations[3].id;
        assert_eq!(m.pick(Point::new(14.0, 316.0)), Some(tid));
        // Double-click the text in the padding re-edits it.
        m.set_tool(ToolKind::Select);
        m.pointer_down(Point::new(14.0, 316.0), Mods::default(), 2);
        assert_eq!(m.editing.as_ref().map(|s| s.id), Some(tid));
        m.commit_text();
    }

    #[test]
    fn snapping_to_canvas_edges_in_padding() {
        let mut m = padded_model();
        m.snapping = true;
        m.set_tool(ToolKind::Redact);
        // Starts 2 px from the canvas' top-left corner (-40, -40).
        drag(&mut m, Point::new(-38.0, -37.0), Point::new(-10.0, -15.0), Mods::default());
        assert_eq!(m.doc.annotations[0].geometric_bounds().origin(), Point::new(-40.0, -40.0));
        // Ends 3 px from the canvas' right edge (440) and bottom (340).
        drag(&mut m, Point::new(405.0, 305.0), Point::new(437.0, 337.0), Mods::default());
        let g = m.doc.annotations[1].geometric_bounds();
        assert_eq!((g.right(), g.bottom()), (440.0, 340.0));
        // Moving a shape snaps its edge to the canvas edge too.
        m.set_tool(ToolKind::Select);
        drag(&mut m, Point::new(420.0, 320.0), Point::new(422.0, 318.5), Mods::default());
        let g = m.doc.annotations[1].geometric_bounds();
        assert_eq!((g.right(), g.bottom()), (440.0, 340.0));
    }

    #[test]
    fn paste_keeps_padding_annotations() {
        let mut m = padded_model();
        let a = Annotation::new(AnnotationKind::Redact(rs_core::RedactAnnotation {
            rect: Rect::new(-30.0, -30.0, 20.0, 20.0),
            color: Color::BLACK,
            corner_radius: 0.0,
        }));
        assert!(m.paste(&AnnotationClipboard::to_json(&[a])));
        assert_eq!(m.doc.annotations[0].geometric_bounds(), Rect::new(-30.0, -30.0, 20.0, 20.0));
        // Far outside the canvas → brought back onto the canvas (not the image).
        let b = Annotation::new(AnnotationKind::Redact(rs_core::RedactAnnotation {
            rect: Rect::new(-900.0, 100.0, 20.0, 20.0),
            color: Color::BLACK,
            corner_radius: 0.0,
        }));
        assert!(m.paste(&AnnotationClipboard::to_json(&[b])));
        assert_eq!(m.doc.annotations[1].geometric_bounds(), Rect::new(-40.0, 100.0, 20.0, 20.0));
    }

    #[test]
    fn eyedropper_samples_composed_canvas_over_padding() {
        let mut m = padded_model();
        let (d, _) = m.display();
        let out = rs_render::Renderer::new().render(&d, &rs_render::RenderOptions::preview(0.5)).unwrap();
        m.canvas_sample = Some((out.image, out.layout));
        m.eyedropper = true;
        m.pointer_down(Point::new(-20.0, -20.0), Mods::default(), 1);
        assert_eq!(m.picked_color, Some(Color::rgb(0, 0, 255)), "background color in the padding");
        m.eyedropper = true;
        m.pointer_down(Point::new(200.0, 150.0), Mods::default(), 1);
        assert_eq!(m.picked_color, Some(Color::rgba(200, 10, 10, 255)));
        assert!(m.doc.annotations.is_empty());
    }
}
