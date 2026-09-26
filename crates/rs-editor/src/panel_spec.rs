//! Declarative description of the side panel + the shared UI primitives.
//!
//! `panel.rs` builds a [`Spec`] (every section / row / control with its
//! behavior closure) from the model; `panel_layout.rs` presents it. Behavior
//! therefore lives in exactly one place.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    color_picker::ColorPicker,
    h_flex,
    input::{Input, InputState},
    select::Select,
    slider::Slider,
    switch::Switch,
    tooltip::Tooltip,
    v_flex, ActiveTheme as _, Selectable as _, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rs_core::Color;

use crate::icons::Ico;
use crate::panel::{Col, Sl};
use crate::panel_icons::{AnyIco, PIco};
use crate::text_layout::to_hsla;
use crate::view::EditorView;

pub(crate) type ActFn = Rc<dyn Fn(&mut EditorView, &mut Window, &mut Context<EditorView>)>;
pub(crate) type SegFn = Rc<dyn Fn(&mut EditorView, usize, &mut Window, &mut Context<EditorView>)>;
pub(crate) type ToggleFn = Rc<dyn Fn(&mut EditorView, bool, &mut Window, &mut Context<EditorView>)>;

pub(crate) fn act(f: impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) + 'static) -> ActFn {
    Rc::new(f)
}
pub(crate) fn seg_fn(f: impl Fn(&mut EditorView, usize, &mut Window, &mut Context<EditorView>) + 'static) -> SegFn {
    Rc::new(f)
}
pub(crate) fn tog_fn(f: impl Fn(&mut EditorView, bool, &mut Window, &mut Context<EditorView>) + 'static) -> ToggleFn {
    Rc::new(f)
}

// ----------------------------------------------------------------------
// Spec
// ----------------------------------------------------------------------

/// What a segment shows besides its label.
#[derive(Clone)]
pub(crate) enum Vis {
    None,
    Icon(AnyIco),
    /// "Aa" drawn at this weight.
    Weight(u16),
}

#[derive(Clone)]
pub(crate) struct Opt {
    pub label: SharedString,
    pub vis: Vis,
    /// Tooltip (defaults to the label for visual-only segments).
    pub tip: Option<SharedString>,
}

impl Opt {
    pub fn text(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), vis: Vis::None, tip: None }
    }
    pub fn icon(label: impl Into<SharedString>, ico: impl Into<AnyIco>) -> Self {
        Self { label: label.into(), vis: Vis::Icon(ico.into()), tip: None }
    }
    pub fn vis(label: impl Into<SharedString>, vis: Vis) -> Self {
        Self { label: label.into(), vis, tip: None }
    }
    pub fn tip(mut self, tip: impl Into<SharedString>) -> Self {
        self.tip = Some(tip.into());
        self
    }
}

#[derive(Clone)]
pub(crate) struct Seg {
    pub id: SharedString,
    pub opts: Vec<Opt>,
    pub sel: Option<usize>,
    pub on: SegFn,
}

#[derive(Clone)]
pub(crate) struct Tog {
    pub id: SharedString,
    pub on_: bool,
    pub on: ToggleFn,
}

#[derive(Clone)]
pub(crate) struct Btn {
    pub id: SharedString,
    pub label: Option<SharedString>,
    pub icon: Option<AnyIco>,
    pub tip: Option<SharedString>,
    pub primary: bool,
    pub selected: bool,
    pub on: ActFn,
}

impl Btn {
    pub fn new(id: impl Into<SharedString>, on: ActFn) -> Self {
        Self { id: id.into(), label: None, icon: None, tip: None, primary: false, selected: false, on }
    }
    pub fn label(mut self, l: impl Into<SharedString>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn icon(mut self, i: impl Into<AnyIco>) -> Self {
        self.icon = Some(i.into());
        self
    }
    pub fn tip(mut self, t: impl Into<SharedString>) -> Self {
        self.tip = Some(t.into());
        self
    }
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
}

#[derive(Clone)]
pub(crate) enum TileVis {
    Gradient(Color, Color, f32),
    Image(Option<Arc<RenderImage>>),
    Add,
}

#[derive(Clone)]
pub(crate) struct Tile {
    pub id: SharedString,
    pub tip: SharedString,
    pub vis: TileVis,
    pub selected: bool,
    pub on: ActFn,
    pub on_remove: Option<ActFn>,
}

#[derive(Clone)]
pub(crate) struct Chip {
    pub id: SharedString,
    pub label: SharedString,
    pub on: ActFn,
    pub on_remove: ActFn,
}

#[derive(Clone)]
pub(crate) enum Ctl {
    Slider { sl: Sl, value: SharedString },
    Seg(Seg),
    /// Segmented control + trailing icon toggle (e.g. weight + italic).
    SegPlus(Seg, Btn),
    Toggle(Tog),
    Color { col: Col, cur: Color, eyedropper: bool },
    Font,
    Input(Entity<InputState>),
    InputAction(Entity<InputState>, Btn),
    Buttons(Vec<Btn>),
    Tiles(Vec<Tile>),
    Chips(Vec<Chip>),
    Note(SharedString),
}

#[derive(Clone)]
pub(crate) struct Row {
    pub label: SharedString,
    pub ctl: Ctl,
    /// Hidden until the section's "More" is opened.
    pub adv: bool,
}

impl Row {
    pub fn new(label: impl Into<SharedString>, ctl: Ctl) -> Self {
        Self { label: label.into(), ctl, adv: false }
    }
    pub fn adv(mut self) -> Self {
        self.adv = true;
        self
    }
}

#[derive(Clone)]
pub(crate) struct Sec {
    pub id: &'static str,
    pub title: SharedString,
    pub icon: AnyIco,
    /// Section-level on/off (rows are only present while on).
    pub toggle: Option<Tog>,
    pub rows: Vec<Row>,
    /// Whole section folded into the "More options" list.
    pub adv: bool,
}

impl Sec {
    pub fn new(id: &'static str, title: impl Into<SharedString>, icon: impl Into<AnyIco>) -> Self {
        Self { id, title: title.into(), icon: icon.into(), toggle: None, rows: Vec::new(), adv: false }
    }
    pub fn row(mut self, r: Row) -> Self {
        self.rows.push(r);
        self
    }
    pub fn rows(mut self, r: impl IntoIterator<Item = Row>) -> Self {
        self.rows.extend(r);
        self
    }
    pub fn toggle(mut self, t: Tog) -> Self {
        self.toggle = Some(t);
        self
    }
    pub fn adv(mut self) -> Self {
        self.adv = true;
        self
    }
}


#[derive(Clone)]
pub(crate) struct Head {
    pub title: SharedString,
    pub subtitle: SharedString,
}

#[derive(Clone, Default)]
pub(crate) struct Spec {
    pub head: Option<Head>,
    /// Master on/off shown at the right of the tab bar (Beautify).
    pub master: Option<Tog>,
    /// Empty state (title, body).
    pub empty: Option<(SharedString, SharedString)>,
    pub secs: Vec<Sec>,
    /// Quick actions shown as a bottom bar (Arrange).
    pub footer: Option<Sec>,
}

// ----------------------------------------------------------------------
// Design tokens (4 pt grid)
// ----------------------------------------------------------------------

/// Type scale (px): labels/values, small captions, titles.
pub(crate) const FS_BODY: f32 = 12.0;
pub(crate) const FS_SMALL: f32 = 11.0;
pub(crate) const FS_TITLE: f32 = 13.0;
/// The one control height.
pub(crate) const CTL_H: f32 = 22.0;

#[derive(Clone, Copy)]
pub(crate) struct Tok {
    pub dark: bool,
    pub bg: Hsla,
    pub fg: Hsla,
    pub muted_fg: Hsla,
    pub faint_fg: Hsla,
    pub border: Hsla,
    pub hair: Hsla,
    pub track: Hsla,
    pub thumb: Hsla,
    pub hover: Hsla,
    pub card: Hsla,
    pub accent: Hsla,
}

pub(crate) fn tok(cx: &App) -> Tok {
    let t = cx.theme();
    let dark = t.is_dark();
    let accent: Hsla = if dark { rgb(0x0A84FF).into() } else { rgb(0x007AFF).into() };
    Tok {
        dark,
        bg: t.background,
        fg: t.foreground,
        muted_fg: t.muted_foreground,
        faint_fg: t.muted_foreground.opacity(0.7),
        border: t.border,
        hair: t.border.opacity(0.7),
        track: if dark { hsla(0., 0., 1., 0.07) } else { hsla(0., 0., 0., 0.055) },
        thumb: if dark { hsla(0., 0., 1., 0.16) } else { hsla(0., 0., 1., 1.) },
        hover: if dark { hsla(0., 0., 1., 0.06) } else { hsla(0., 0., 0., 0.04) },
        card: if dark { hsla(0., 0., 1., 0.045) } else { hsla(0., 0., 1., 1.) },
        accent,
    }
}

// ----------------------------------------------------------------------
// Primitives
// ----------------------------------------------------------------------

fn vis_el(v: &Vis, size: f32, color: Hsla) -> Option<AnyElement> {
    match v {
        Vis::None => None,
        Vis::Icon(i) => Some(i.icon().size(px(size)).text_color(color).into_any_element()),
        Vis::Weight(w) => Some(
            div()
                .text_size(px(size * 0.85))
                .line_height(px(size))
                .font_weight(gpui_kit::FontWeight(*w as f32))
                .text_color(color)
                .child("Aa")
                .into_any_element(),
        ),
    }
}

impl EditorView {
    /// Segmented control of height `h`. Up to 6 options share one row of
    /// equal segments (visual instead of text when present); more options
    /// wrap onto two rows whose segments size to their text.
    pub(crate) fn ui_seg(&self, s: &Seg, h: f32, t: &Tok, cx: &mut Context<Self>) -> AnyElement {
        let per_row = if s.opts.len() > 6 { s.opts.len().div_ceil(2) } else { s.opts.len().max(1) };
        let multi = per_row < s.opts.len();
        let mut track = v_flex().w_full().p(px(2.)).gap(px(2.)).rounded(px(6.)).bg(t.track);
        for (r, chunk) in s.opts.chunks(per_row).enumerate() {
            let mut row = h_flex().w_full().h(px(h - 4.)).gap(px(2.));
            for (j, o) in chunk.iter().enumerate() {
                let i = r * per_row + j;
                let sel = s.sel == Some(i);
                let on = s.on.clone();
                let fg = if sel { t.fg } else { t.muted_fg };
                let visual = vis_el(&o.vis, 14., fg);
                let tip = o.tip.clone().or_else(|| visual.is_some().then(|| o.label.clone()));
                let mut c = div()
                    .id(SharedString::from(format!("{}-{i}", s.id)))
                    .flex()
                    .h_full()
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .px(px(if multi { 5. } else { 0. }))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(FS_SMALL))
                    .text_color(fg)
                    .when(sel, |d| {
                        d.bg(t.thumb).shadow(vec![BoxShadow {
                            color: hsla(0., 0., 0., if t.dark { 0.3 } else { 0.12 }),
                            offset: point(px(0.), px(0.5)),
                            blur_radius: px(1.5),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                    })
                    .when(!sel, |d| d.hover(|d| d.bg(t.hover)))
                    .on_click(cx.listener(move |this, _, window, cx| on(this, i, window, cx)));
                c = if multi { c.flex_auto() } else { c.flex_1() };
                c = match visual {
                    Some(v) => c.child(v),
                    None => c.child(div().truncate().child(o.label.clone())),
                };
                if let Some(tip) = tip {
                    c = c.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
                }
                row = row.child(c);
            }
            track = track.child(row);
        }
        track.into_any_element()
    }

    pub(crate) fn ui_slider(&self, sl: Sl) -> AnyElement {
        let s = self.panel.slider_state(sl);
        // Inset by half a thumb so the knob never overhangs the column.
        // The slider draws 24 pt tall; center it in the 22 pt control row.
        div()
            .flex_1()
            .min_w_0()
            .h(px(CTL_H))
            .flex()
            .items_center()
            .px(px(6.))
            .when_some(s, |d, s| d.child(div().w_full().child(Slider::new(&s))))
            .into_any_element()
    }

    pub(crate) fn ui_switch(&self, tg: &Tog, cx: &mut Context<Self>) -> AnyElement {
        let on = tg.on.clone();
        Switch::new(tg.id.clone())
            .small()
            .checked(tg.on_)
            .on_click(cx.listener(move |this, v: &bool, window, cx| on(this, *v, window, cx)))
            .into_any_element()
    }

    /// `ghost`: borderless (icon buttons in rows / the arrange bar).
    pub(crate) fn ui_btn(&self, b: &Btn, ghost: bool, cx: &mut Context<Self>) -> Button {
        let on = b.on.clone();
        let mut btn = Button::new(b.id.clone()).xsmall();
        btn = if b.label.is_some() { btn.h(px(CTL_H)).px(px(8.)) } else { btn.size(px(CTL_H + 2.)) };
        if let Some(i) = b.icon {
            btn = btn.icon(i.icon());
        }
        if let Some(l) = &b.label {
            btn = btn.label(l.clone());
        }
        if let Some(t) = &b.tip {
            btn = btn.tooltip(t.clone());
        }
        if b.primary {
            btn = btn.primary();
        } else if ghost {
            btn = btn.ghost();
        }
        btn.selected(b.selected).on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
    }

    pub(crate) fn ui_picker(&self, col: Col) -> AnyElement {
        match self.panel.picker_state(col) {
            Some(p) => div()
                .flex_shrink_0()
                .child(ColorPicker::new(&p).anchor(Anchor::TopRight).with_size(gpui_kit::component::Size::XSmall))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    pub(crate) fn ui_eyedropper(&self, col: Col, cx: &mut Context<Self>) -> AnyElement {
        Button::new(SharedString::from(format!("eyedrop-{col:?}")))
            .icon(Ico::Pipette.icon())
            .ghost()
            .xsmall()
            .selected(self.panel.eyedropper_target == Some(col))
            .tooltip("Pick a color from the screenshot")
            .on_click(cx.listener(move |this, _, window, cx| {
                this.model.eyedropper = true;
                this.panel.eyedropper_target = Some(col);
                this.canvas_focus.focus(window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// Palette colors (default palette) and recent colors (deduplicated
    /// against the palette), each as its own list.
    pub(crate) fn palette_lists(&self, cx: &App) -> (Vec<Color>, Vec<Color>) {
        let pal: Vec<Color> = rs_core::DEFAULT_PALETTE.to_vec();
        let mut recent = Vec::new();
        for c in self.prefs(cx).recent_colors {
            let c = c.with_alpha(1.0);
            if !pal.contains(&c) && !recent.contains(&c) {
                recent.push(c);
            }
        }
        (pal, recent)
    }

    /// One row of `cols` swatches spread over exactly `width`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ui_swatches(
        &self,
        col: Col,
        colors: &[Color],
        cur: Color,
        cols: usize,
        swatch: f32,
        width: f32,
        t: &Tok,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cols = cols.max(2);
        let gap = ((width - cols as f32 * swatch) / (cols as f32 - 1.)).max(2.);
        let mut grid = v_flex().gap(px(gap.min(6.)));
        for (r, chunk) in colors.chunks(cols).enumerate() {
            let mut row = h_flex().gap(px(gap));
            for (j, c) in chunk.iter().enumerate() {
                let c = *c;
                let selected = c.r == cur.r && c.g == cur.g && c.b == cur.b;
                let sum = c.r as u32 + c.g as u32 + c.b as u32;
                let light = sum > 690;
                row = row.child(
                    div()
                        .id(SharedString::from(format!("sw-{col:?}-{r}-{j}")))
                        .size(px(swatch))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(to_hsla(c.with_alpha(1.0)))
                        .when(light || (t.dark && sum < 40), |d| d.border_1().border_color(t.border))
                        .when(selected, |d| {
                            d.border_2().border_color(t.bg).shadow(vec![BoxShadow {
                                color: t.accent,
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(1.5),
                                inset: false,
                            }])
                        })
                        .cursor_pointer()
                        .hover(|d| d.opacity(0.85))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.model.break_coalescing();
                            this.set_color(col, c, window, cx);
                        })),
                );
            }
            grid = grid.child(row);
        }
        grid.into_any_element()
    }

    /// Thumbnail grid exactly `width` wide.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ui_tiles(&self, tiles: &[Tile], cols: usize, width: f32, ratio: f32, gap: f32, t: &Tok, cx: &mut Context<Self>) -> AnyElement {
        let cols = cols.max(1);
        let w = ((width - gap * (cols as f32 - 1.)) / cols as f32).floor();
        let h = (w / ratio).round();
        let mut grid = v_flex().gap(px(gap));
        for chunk in tiles.chunks(cols) {
            let mut row = h_flex().gap(px(gap));
            for tl in chunk {
                let on = tl.on.clone();
                let tip = tl.tip.clone();
                let mut d = div()
                    .id(tl.id.clone())
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)));
                d = match &tl.vis {
                    TileVis::Gradient(a, b, angle) => d.bg(linear_gradient(
                        angle + 90.0,
                        linear_color_stop(to_hsla(*a), 0.0),
                        linear_color_stop(to_hsla(*b), 1.0),
                    )),
                    TileVis::Image(img_) => {
                        d.bg(t.track).when_some(img_.clone(), |d, i| d.child(img(i).size_full().object_fit(ObjectFit::Cover)))
                    }
                    TileVis::Add => d
                        .border_1()
                        .border_color(t.border)
                        .bg(t.track)
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|d| d.bg(t.hover))
                        .child(PIco::ImagePlus.icon().size(px(13.)).text_color(t.muted_fg)),
                };
                // Selection ring drawn on top (inside) so it's never clipped.
                d = d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(4.))
                        .border_color(if tl.selected { t.accent } else { hsla(0., 0., 0., 0.08) })
                        .when(tl.selected, |d| d.border_2())
                        .when(!tl.selected && !matches!(tl.vis, TileVis::Add), |d| d.border_1()),
                );
                if let Some(rm) = tl.on_remove.clone() {
                    d = d.child(
                        div().absolute().top(px(1.)).right(px(1.)).child(
                            Button::new(SharedString::from(format!("{}-rm", tl.id)))
                                .icon(Ico::X.icon())
                                .xsmall()
                                .size(px(14.))
                                .tooltip("Remove from library")
                                .on_click(cx.listener(move |this, _, window, cx| rm(this, window, cx))),
                        ),
                    );
                }
                row = row.child(d);
            }
            grid = grid.child(row);
        }
        grid.into_any_element()
    }

    pub(crate) fn ui_font(&self) -> AnyElement {
        div().flex_1().min_w_0().child(Select::new(&self.panel.font_select).xsmall().h(px(CTL_H)).menu_max_h(px(320.))).into_any_element()
    }

    pub(crate) fn ui_input(&self, st: &Entity<InputState>) -> AnyElement {
        div().flex_1().min_w_0().child(Input::new(st).xsmall().h(px(CTL_H))).into_any_element()
    }

    pub(crate) fn ui_chips(&self, chips: &[Chip], t: &Tok, cx: &mut Context<Self>) -> AnyElement {
        let mut wrap = h_flex().w_full().flex_wrap().gap(px(4.));
        for c in chips {
            let on = c.on.clone();
            let rm = c.on_remove.clone();
            wrap = wrap.child(
                h_flex()
                    .h(px(CTL_H))
                    .pl(px(8.))
                    .pr(px(2.))
                    .gap(px(2.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card)
                    .child(
                        div()
                            .id(c.id.clone())
                            .text_size(px(FS_SMALL))
                            .cursor_pointer()
                            .hover(|d| d.text_color(t.accent))
                            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
                            .child(c.label.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}-del", c.id)))
                            .icon(Ico::X.icon())
                            .ghost()
                            .xsmall()
                            .size(px(16.))
                            .tooltip("Delete preset")
                            .on_click(cx.listener(move |this, _, window, cx| rm(this, window, cx))),
                    ),
            );
        }
        wrap.into_any_element()
    }

    pub(crate) fn ui_note(&self, s: &SharedString, t: &Tok) -> AnyElement {
        div().text_size(px(FS_SMALL)).line_height(px(15.)).text_color(t.muted_fg).child(s.clone()).into_any_element()
    }
}

/// Right-aligned numeric readout of a slider.
pub(crate) fn value_box(v: &SharedString, t: &Tok) -> Div {
    div()
        .flex_shrink_0()
        .w(px(34.))
        .flex()
        .items_center()
        .justify_end()
        .text_size(px(FS_SMALL))
        .text_color(t.muted_fg)
        .child(v.clone())
}
