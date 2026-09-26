//! The side-panel layout (compact, progressive disclosure). It only
//! *presents* the [`Spec`] built in `panel.rs`; all behavior comes from the
//! spec's closures.
//!
//! * Tab bar: Style · Beautify (+ the Beautify master switch on the right).
//! * Essential sections: compact header (title · "More" · switch), then
//!   label / control rows. Advanced rows (`Row::adv`) stay behind the
//!   section's "More".
//! * Advanced sections (`Sec::adv`) live in one "More options" disclosure at
//!   the bottom; each is a foldable row there.
//! * Arrange quick actions as a bottom bar.
//!
//! Geometry (pt, 4 pt rhythm): panel [`PANEL_W`] wide, 12 pt gutters,
//! 22 pt controls, 4 pt between rows, 12 pt between sections.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::panel_icons::PIco;
use crate::panel_spec::*;
use crate::view::{EditorView, PANEL_W};

/// Horizontal gutter.
const PAD_X: f32 = 12.0;
/// Content width (panel minus its 1 pt border and the gutters).
pub(crate) const CONTENT_W: f32 = PANEL_W - 1.0 - 2.0 * PAD_X;
/// Label column.
const LABEL_W: f32 = 56.0;
const COL_GAP: f32 = 6.0;
const ROW_GAP: f32 = 4.0;
const SEC_GAP: f32 = 12.0;
const TABS_H: f32 = 30.0;
const FOOTER_H: f32 = 36.0;
/// Color swatch diameter.
const SWATCH: f32 = 12.0;
/// Key in `PanelState::expanded` for the "More options" disclosure.
pub(crate) const MORE_KEY: &str = "__more";

const CTRL_W: f32 = CONTENT_W - LABEL_W - COL_GAP;

fn label_el(s: &SharedString, color: Hsla) -> Div {
    div()
        .w(px(LABEL_W))
        .flex_shrink_0()
        .text_size(px(FS_BODY))
        .line_height(px(16.))
        .text_color(color)
        .truncate()
        .child(s.clone())
}

fn line() -> Div {
    h_flex().w_full().min_h(px(CTL_H)).gap(px(COL_GAP)).items_center()
}

/// Measured panel heights (pt) of the last frame, for the self-test.
#[derive(Clone, Default)]
pub(crate) struct PanelMetrics {
    /// Scrollable body content height.
    pub body_h: Rc<Cell<f32>>,
    /// Fixed chrome (tab bar + bottom bar).
    pub chrome_h: Rc<Cell<f32>>,
}

impl EditorView {
    fn toggle_expanded(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if !self.panel.expanded.remove(id) {
            self.panel.expanded.insert(id);
        }
        cx.notify();
    }

    fn empty_state(&self, title: &SharedString, body: &SharedString, t: &Tok) -> AnyElement {
        v_flex()
            .w_full()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .py(px(24.))
            .child(
                div()
                    .size(px(32.))
                    .rounded_full()
                    .bg(t.track)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::icons::Ico::MousePointer2.icon().size(px(15.)).text_color(t.muted_fg)),
            )
            .child(div().text_size(px(FS_TITLE)).font_weight(FontWeight::SEMIBOLD).child(title.clone()))
            .child(div().text_size(px(FS_SMALL)).line_height(px(15.)).text_color(t.muted_fg).text_center().child(body.clone()))
            .into_any_element()
    }

    /// One label / control row. `open`: the section's advanced content is
    /// revealed (the color row then also shows recents + eyedropper).
    fn row_el(&self, r: &Row, open: bool, t: &Tok, cx: &mut Context<Self>) -> AnyElement {
        let has_label = !r.label.is_empty();
        let lc = t.muted_fg;
        match &r.ctl {
            Ctl::Slider { sl, value } => {
                line().child(label_el(&r.label, lc)).child(self.ui_slider(*sl)).child(value_box(value, t)).into_any_element()
            }
            Ctl::Seg(s) => {
                if has_label {
                    line().child(label_el(&r.label, lc)).child(self.ui_seg(s, CTL_H, t, cx)).into_any_element()
                } else {
                    self.ui_seg(s, CTL_H, t, cx)
                }
            }
            Ctl::SegPlus(s, b) => line()
                .child(label_el(&r.label, lc))
                .child(div().flex_1().min_w_0().child(self.ui_seg(s, CTL_H, t, cx)))
                .child(self.ui_btn(b, true, cx))
                .into_any_element(),
            Ctl::Toggle(tg) => line()
                .child(div().flex_1().text_size(px(FS_BODY)).text_color(lc).child(r.label.clone()))
                .child(self.ui_switch(tg, cx))
                .into_any_element(),
            Ctl::Color { col, cur, eyedropper } => {
                let (pal, recent) = self.palette_lists(cx);
                // Picker (20 pt) + gap, then the palette across the rest.
                let sw_w = CTRL_W - 20. - COL_GAP;
                let n = pal.len();
                v_flex()
                    .w_full()
                    .gap(px(ROW_GAP))
                    .child(
                        line().child(label_el(&r.label, lc)).child(
                            h_flex()
                                .flex_1()
                                .gap(px(COL_GAP))
                                .items_center()
                                .child(self.ui_picker(*col))
                                .child(self.ui_swatches(*col, &pal, *cur, n, SWATCH, sw_w, t, cx)),
                        ),
                    )
                    .when(open && (!recent.is_empty() || *eyedropper), |d| {
                        d.child(
                            h_flex()
                                .w_full()
                                .gap(px(COL_GAP))
                                .items_center()
                                .child(
                                    div()
                                        .w(px(LABEL_W))
                                        .flex_shrink_0()
                                        .text_size(px(FS_SMALL))
                                        .text_color(t.faint_fg)
                                        .when(!recent.is_empty(), |d| d.child("Recent")),
                                )
                                .child(
                                    div()
                                        .w(px(20.))
                                        .flex_shrink_0()
                                        .flex()
                                        .justify_center()
                                        .when(*eyedropper, |d| d.child(self.ui_eyedropper(*col, cx))),
                                )
                                .when(!recent.is_empty(), |d| d.child(self.ui_swatches(*col, &recent[..recent.len().min(n)], *cur, n, SWATCH, sw_w, t, cx)))
                                .when(recent.is_empty(), |d| {
                                    d.child(div().text_size(px(FS_SMALL)).text_color(t.faint_fg).child("Pick from the image"))
                                }),
                        )
                    })
                    .into_any_element()
            }
            Ctl::Font => line().child(label_el(&r.label, lc)).child(self.ui_font()).into_any_element(),
            Ctl::Input(st) => line().when(has_label, |d| d.child(label_el(&r.label, lc))).child(self.ui_input(st)).into_any_element(),
            Ctl::InputAction(st, b) => line().child(self.ui_input(st)).child(self.ui_btn(b, false, cx)).into_any_element(),
            Ctl::Buttons(bs) => {
                let icon_only = bs.iter().all(|b| b.label.is_none());
                let mut row = h_flex().flex_1().min_w_0().gap(px(4.)).flex_wrap();
                for b in bs {
                    row = row.child(self.ui_btn(b, false, cx).when(icon_only, |b| b.outline()));
                }
                line().when(has_label, |d| d.child(label_el(&r.label, lc))).child(row).into_any_element()
            }
            Ctl::Tiles(tiles) => self.ui_tiles(tiles, 6, CONTENT_W, 1.6, 4., t, cx),
            Ctl::Chips(c) => self.ui_chips(c, t, cx),
            Ctl::Note(s) => self.ui_note(s, t),
        }
    }

    pub(crate) fn render_panel_layout(&mut self, spec: Spec, cx: &mut Context<Self>) -> AnyElement {
        let t = tok(cx);
        let tab = self.panel.tab;

        // --- Tab bar ------------------------------------------------------
        let mut tabs = h_flex().flex_shrink_0().px(px(PAD_X)).h(px(TABS_H)).gap(px(16.)).items_end().border_b_1().border_color(t.hair);
        for (i, label) in ["Style", "Beautify"].into_iter().enumerate() {
            let sel = tab == i;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("panel-tab-{i}")))
                    .h_full()
                    .flex()
                    .items_center()
                    .border_b_2()
                    .border_color(if sel { t.fg } else { hsla(0., 0., 0., 0.) })
                    .cursor_pointer()
                    .text_size(px(FS_TITLE))
                    .font_weight(if sel { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(if sel { t.fg } else { t.muted_fg })
                    .hover(|d| d.text_color(t.fg))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_panel_tab(i, cx))),
            );
        }
        if let Some(tg) = &spec.master {
            tabs = tabs.child(div().flex_1()).child(
                h_flex()
                    .id("panel-master")
                    .h_full()
                    .items_center()
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Beautify on / off").build(window, cx))
                    .child(self.ui_switch(tg, cx)),
            );
        }

        // --- Body ---------------------------------------------------------
        let mut body = v_flex().w_full().px(px(PAD_X)).pt(px(8.)).pb(px(10.)).gap(px(SEC_GAP));
        if let Some(h) = &spec.head {
            body = body.child(
                h_flex()
                    .h(px(18.))
                    .gap(px(6.))
                    .items_baseline()
                    .child(div().text_size(px(FS_TITLE)).font_weight(FontWeight::SEMIBOLD).child(h.title.clone()))
                    .child(div().text_size(px(FS_SMALL)).text_color(t.muted_fg).child(h.subtitle.clone())),
            );
        }
        if let Some((title, b)) = &spec.empty {
            body = body.child(self.empty_state(title, b, &t));
        }
        let (essential, extra): (Vec<Sec>, Vec<Sec>) = spec.secs.into_iter().partition(|s| !s.adv);
        for s in &essential {
            body = body.child(self.section_el(s, &t, cx));
        }
        if !extra.is_empty() {
            let open = self.panel.expanded.contains(MORE_KEY);
            let mut more = v_flex().w_full().child(
                h_flex()
                    .id("panel-more-options")
                    .h(px(22.))
                    .gap(px(6.))
                    .items_center()
                    .cursor_pointer()
                    .border_t_1()
                    .border_color(t.hair)
                    .text_size(px(FS_BODY))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.muted_fg)
                    .hover(|d| d.text_color(t.fg))
                    .child(div().flex_1().child("More options"))
                    .child(if open { PIco::ChevronDown } else { PIco::ChevronRight }.icon().size(px(12.)).text_color(t.faint_fg))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_expanded(MORE_KEY, cx))),
            );
            if open {
                for s in &extra {
                    more = more.child(self.folded_section_el(s, &t, cx));
                }
            }
            body = body.child(more);
        }
        let metrics = self.panel.metrics.clone();
        let body_h = metrics.body_h.clone();
        body = body.relative().child(
            canvas(move |b, _, _| body_h.set(f32::from(b.size.height)), |_, _, _, _| {}).absolute().top_0().left_0().size_full(),
        );

        let mut chrome_h = TABS_H;
        let mut shell = v_flex()
            .w(px(PANEL_W))
            .flex_shrink_0()
            .h_full()
            .bg(t.bg)
            .border_l_1()
            .border_color(t.border)
            .text_color(t.fg)
            .child(tabs)
            .child(div().id("panel-scroll").flex_1().overflow_y_scroll().child(body));
        if let Some(f) = &spec.footer {
            // Arrange as a calm bottom toolbar.
            let mut bar = h_flex()
                .flex_shrink_0()
                .h(px(FOOTER_H))
                .px(px(PAD_X - 4.))
                .items_center()
                .justify_between()
                .border_t_1()
                .border_color(t.hair);
            for r in &f.rows {
                if let Ctl::Buttons(bs) = &r.ctl {
                    for b in bs {
                        bar = bar.child(self.ui_btn(b, true, cx));
                    }
                }
            }
            shell = shell.child(bar);
            chrome_h += FOOTER_H;
        }
        metrics.chrome_h.set(chrome_h);
        shell.into_any_element()
    }

    /// Essential section: compact header, visible rows, advanced rows once
    /// the header's "More" is on.
    fn section_el(&self, s: &Sec, t: &Tok, cx: &mut Context<Self>) -> AnyElement {
        let id = s.id;
        let open = self.panel.expanded.contains(id);
        let n_adv = s.rows.iter().filter(|r| r.adv).count();
        let header = h_flex()
            .h(px(18.))
            .gap(px(8.))
            .items_center()
            .child(div().flex_1().text_size(px(FS_BODY)).font_weight(FontWeight::SEMIBOLD).child(s.title.clone()))
            .when(n_adv > 0, |d| {
                d.child(
                    h_flex()
                        .id(SharedString::from(format!("sec-more-{id}")))
                        .gap(px(2.))
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(FS_SMALL))
                        .text_color(t.accent)
                        .hover(|d| d.opacity(0.8))
                        .child(if open { "Less" } else { "More" })
                        .child(if open { PIco::ChevronDown } else { PIco::ChevronRight }.icon().size(px(11.)).text_color(t.accent))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(id, cx))),
                )
            })
            .when_some(s.toggle.as_ref(), |d, tg| d.child(self.ui_switch(tg, cx)));
        let mut col = v_flex().w_full().gap(px(ROW_GAP)).child(header);
        for r in s.rows.iter().filter(|r| !r.adv || open) {
            col = col.child(self.row_el(r, open, t, cx));
        }
        col.into_any_element()
    }

    /// Section inside "More options": one row until opened, then all rows.
    fn folded_section_el(&self, s: &Sec, t: &Tok, cx: &mut Context<Self>) -> AnyElement {
        let id = s.id;
        let open = self.panel.expanded.contains(id);
        let title = h_flex()
            .h(px(28.))
            .gap(px(8.))
            .items_center()
            .child(
                h_flex()
                    .id(SharedString::from(format!("fold-{id}")))
                    .flex_1()
                    .h_full()
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .child(s.icon.icon().size(px(13.)).text_color(t.muted_fg))
                    .child(div().flex_1().text_size(px(FS_BODY)).child(s.title.clone()))
                    .child(if open { PIco::ChevronDown } else { PIco::ChevronRight }.icon().size(px(12.)).text_color(t.faint_fg))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(id, cx))),
            )
            .when_some(s.toggle.as_ref(), |d, tg| d.child(self.ui_switch(tg, cx)));
        let mut col = v_flex().w_full().border_t_1().border_color(t.hair).child(title);
        if open && !s.rows.is_empty() {
            let mut rows = v_flex().gap(px(ROW_GAP)).pb(px(10.));
            for r in &s.rows {
                rows = rows.child(self.row_el(r, true, t, cx));
            }
            col = col.child(rows);
        }
        col.into_any_element()
    }
}
