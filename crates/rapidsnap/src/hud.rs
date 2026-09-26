//! Small floating, click-through notice window ("Copied to clipboard",
//! capture errors, delayed-capture countdown). One HUD at a time; showing a
//! new message replaces the text of the current one.

use std::time::Duration;

use gpui_kit::*;

use crate::platform;

/// Handles of transient windows (overlays, HUD) — they don't count as
/// "user windows" for the activation-policy dance.
#[derive(Default)]
pub struct TransientWindows(pub Vec<WindowId>);
impl Global for TransientWindows {}

pub fn mark_transient(cx: &mut App, id: WindowId) {
    cx.default_global::<TransientWindows>().0.push(id);
}

pub fn unmark_transient(cx: &mut App, id: WindowId) {
    cx.default_global::<TransientWindows>().0.retain(|w| *w != id);
}

/// Windows that are not overlays/HUDs (editor, preferences, welcome, …).
pub fn user_window_count(cx: &App) -> usize {
    let transient = cx.try_global::<TransientWindows>().map(|t| t.0.clone()).unwrap_or_default();
    cx.windows().iter().filter(|w| !transient.contains(&w.window_id())).count()
}

/// Kind of notice (affects the accent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Error,
}

pub struct HudView {
    text: SharedString,
    detail: Option<SharedString>,
    tone: Tone,
    /// Bumped on every new message; a close timer only fires for its own.
    generation: u64,
}

impl Render for HudView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let accent = match self.tone {
            Tone::Info => rgb(0x0a84ff),
            Tone::Success => rgb(0x30d158),
            Tone::Error => rgb(0xff453a),
        };
        div().size_full().flex().items_center().justify_center().child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .px_5()
                .py_3()
                .rounded_xl()
                .bg(rgba(0x1c1c1ee6))
                .border_1()
                .border_color(rgba(0xffffff26))
                .text_color(rgb(0xffffff))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().size(px(8.)).rounded_full().bg(accent))
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(self.text.clone())),
                )
                .children(self.detail.clone().map(|d| div().text_xs().text_color(rgba(0xffffffb3)).child(d))),
        )
    }
}

struct HudGlobal(Option<WindowHandle<HudView>>);
impl Global for HudGlobal {}

const HUD_W: f32 = 420.;
const HUD_H: f32 = 90.;

/// Show a notice for `duration` (replaces the current one).
pub fn show(cx: &mut App, text: impl Into<SharedString>, detail: Option<String>, tone: Tone, duration: Duration) {
    let text = text.into();
    let detail = detail.map(SharedString::from);
    log::info!("HUD: {text}{}", detail.as_ref().map(|d| format!(" — {d}")).unwrap_or_default());

    // Reuse the existing HUD window if still open.
    let existing = cx.try_global::<HudGlobal>().and_then(|g| g.0);
    let mut generation = 0;
    let reused = existing.and_then(|h| {
        h.update(cx, |v, _, cx| {
            v.text = text.clone();
            v.detail = detail.clone();
            v.tone = tone;
            v.generation += 1;
            generation = v.generation;
            cx.notify();
        })
        .ok()
        .map(|_| h)
    });
    let handle = match reused {
        Some(h) => h,
        None => match open(cx, text, detail, tone) {
            Some(h) => h,
            None => return,
        },
    };
    cx.set_global(HudGlobal(Some(handle)));

    cx.spawn(async move |cx| {
        cx.background_executor().timer(duration).await;
        cx.update(|cx| {
            let still_ours = handle.read_with(cx, |v, _| v.generation == generation).unwrap_or(false);
            if still_ours {
                close(cx);
            }
        });
    })
    .detach();
}

/// Close the HUD now.
pub fn close(cx: &mut App) {
    if let Some(h) = cx.try_global::<HudGlobal>().and_then(|g| g.0) {
        let id = h.window_id();
        let _ = h.update(cx, |_, window, _| window.remove_window());
        cx.set_global(HudGlobal(None));
        cx.defer(move |cx| unmark_transient(cx, id));
    }
}

fn open(cx: &mut App, text: SharedString, detail: Option<SharedString>, tone: Tone) -> Option<WindowHandle<HudView>> {
    // Display under the cursor, top-centre.
    let cursor = platform::cursor_location();
    let displays = platform::display_list();
    let d = displays
        .iter()
        .find(|d| d.frame.contains(cursor))
        .or_else(|| displays.iter().find(|d| d.is_main))
        .cloned();
    let (display_id, dw) = match &d {
        Some(d) => (Some(DisplayId::new(d.id as u64)), d.frame.width),
        None => (None, 1440.),
    };
    let bounds = Bounds::new(point(px((dw - HUD_W) / 2.), px(56.)), size(px(HUD_W), px(HUD_H)));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let result = cx.open_window(opts, |window, cx| {
        if let Err(e) = platform::make_hud_window(window) {
            log::warn!("hud native tweaks: {e:#}");
        }
        cx.new(|_| HudView { text, detail, tone, generation: 0 })
    });
    match result {
        Ok(h) => {
            mark_transient(cx, h.window_id());
            Some(h)
        }
        Err(e) => {
            log::error!("open HUD: {e:#}");
            None
        }
    }
}

/// Count down `seconds` (HUD + tray title), then run `then`.
pub fn countdown(cx: &mut App, seconds: u32, then: impl FnOnce(&mut App) + 'static) {
    cx.spawn(async move |cx| {
        for left in (1..=seconds).rev() {
            cx.update(|cx| {
                show(cx, format!("Capturing in {left}…"), None, Tone::Info, Duration::from_millis(1500));
                if let Some(t) = cx.try_global::<crate::tray::TrayState>() {
                    t.set_title(Some(&format!("{left}")));
                }
            });
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }
        cx.update(|cx| {
            if let Some(t) = cx.try_global::<crate::tray::TrayState>() {
                t.set_title(None);
            }
            close(cx);
        });
        // Let the HUD disappear before freezing the screen.
        cx.background_executor().timer(Duration::from_millis(120)).await;
        cx.update(then);
    })
    .detach();
}
