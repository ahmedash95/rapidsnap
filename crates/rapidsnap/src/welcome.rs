//! Welcome / onboarding window: explains Screen Recording, requests it,
//! re-checks the status every second, offers Relaunch (macOS applies a new
//! grant only after restart) and Continue.

use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Root,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rs_editor::PreferencesGlobal;

use crate::platform::{self, ActivationPolicy};

pub struct WelcomeView {
    granted: bool,
    /// The user clicked "Grant Permission" in this session.
    requested: bool,
    /// Permission state at window open (to detect "granted but needs relaunch").
    initially_granted: bool,
    error: Option<String>,
}

impl WelcomeView {
    fn new(cx: &mut Context<Self>) -> Self {
        let granted = rs_capture::has_screen_capture_permission();
        // Live status re-check every second while the window is open.
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let now = cx.background_executor().spawn(async { rs_capture::has_screen_capture_permission() }).await;
                let alive = this
                    .update(cx, |v, cx| {
                        if v.granted != now {
                            log::info!("Screen Recording permission now {now}");
                            v.granted = now;
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Self { granted, requested: false, initially_granted: granted, error: None }
    }

    fn grant(&mut self, cx: &mut Context<Self>) {
        self.requested = true;
        // Shows the system prompt the first time; afterwards macOS only
        // allows toggling in System Settings, so open it as well.
        let ok = rs_capture::request_screen_capture_permission();
        log::info!("request_screen_capture_permission -> {ok}");
        if !ok
            && let Err(e) = rs_capture::open_screen_recording_settings() {
                self.error = Some(format!("Could not open System Settings: {e}"));
            }
        self.granted = ok || rs_capture::has_screen_capture_permission();
        cx.notify();
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.update_global::<PreferencesGlobal, _>(|p, _| p.0.onboarding_done = true);
        window.remove_window();
    }
}

fn hotkey_row(label: &str, hk: Option<&rs_core::Hotkey>, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .child(div().text_sm().child(label.to_string()))
        .child(
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(cx.theme().muted_foreground)
                .child(hk.map(|h| h.display()).unwrap_or_else(|| "—".into())),
        )
}

impl Render for WelcomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (dot, status) = if self.granted {
            (rgb(0x30d158), "Screen Recording: granted")
        } else {
            (rgb(0xff453a), "Screen Recording: not granted")
        };
        let needs_relaunch = (self.requested || !self.initially_granted) && self.granted;
        let hk = cx.try_global::<PreferencesGlobal>().map(|p| p.0.hotkeys.clone()).unwrap_or_default();
        let icon = rs_core::assets::asset_path("icons/app-icon-256.png");

        v_flex()
            .size_full()
            .items_center()
            .gap_4()
            .px_8()
            .pt(px(44.))
            .pb_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(img(icon).size(px(96.)))
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Welcome to RapidSnap"))
            .child(
                div()
                    .text_sm()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child("RapidSnap lives in your menu bar. To capture your screen it needs the Screen Recording permission — screenshots never leave your Mac."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(div().size(px(10.)).rounded_full().bg(dot))
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(status)),
            )
            .when(!self.granted, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .child("Click “Grant Permission”, enable RapidSnap under Privacy & Security → Screen & System Audio Recording, then relaunch."),
                )
            })
            .when(needs_relaunch, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .child("macOS applies the permission after RapidSnap restarts."),
                )
            })
            .when_some(self.error.clone(), |d, e| d.child(div().text_xs().text_color(rgb(0xff453a)).child(e)))
            .child(
                v_flex()
                    .w(px(300.))
                    .gap_1()
                    .child(hotkey_row("Capture Region", hk.region.as_ref(), cx))
                    .child(hotkey_row("Capture Window", hk.window.as_ref(), cx))
                    .child(hotkey_row("Capture Full Screen", hk.fullscreen.as_ref(), cx)),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_2()
                    .when(!self.granted, |d| {
                        d.child(
                            Button::new("grant")
                                .primary()
                                .label("Grant Permission")
                                .on_click(cx.listener(|this, _, _, cx| this.grant(cx))),
                        )
                    })
                    // macOS only applies a new Screen Recording grant to a fresh process.
                    .when(self.granted || self.requested, |d| {
                        d.child(
                            Button::new("relaunch")
                                .when(needs_relaunch, |b| b.primary())
                                .label("Quit & Reopen")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.update_global::<PreferencesGlobal, _>(|p, _| p.0.onboarding_done = true);
                                    crate::save_preferences_now(cx);
                                    match platform::relaunch() {
                                        Ok(()) => cx.quit(),
                                        Err(e) => {
                                            log::error!("relaunch: {e:#}");
                                            this.error = Some(format!("Could not relaunch: {e:#}. Quit and start RapidSnap again."));
                                            cx.notify();
                                        }
                                    }
                                })),
                        )
                    })
                    .child(
                        Button::new("continue")
                            .when(self.granted && !needs_relaunch, |b| b.primary())
                            .label("Continue")
                            .on_click(cx.listener(|this, _, window, cx| this.finish(window, cx))),
                    ),
            )
    }
}

struct WelcomeWindow(WindowHandle<Root>);
impl Global for WelcomeWindow {}

/// Open (or focus) the welcome window.
pub fn open_welcome(cx: &mut App) {
    if let Some(h) = cx.try_global::<WelcomeWindow>().map(|w| w.0)
        && h.update(cx, |_, w, _| {
            w.activate_window();
            platform::bring_to_front(w);
        }).is_ok() {
            cx.activate(true);
            return;
        }
    platform::set_activation_policy(ActivationPolicy::Regular);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(520.), px(600.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("Welcome to RapidSnap".into()), appears_transparent: true, ..Default::default() }),
        is_resizable: false,
        is_minimizable: false,
        kind: WindowKind::Normal,
        ..Default::default()
    };
    match cx.open_window(opts, |window, cx| {
        let view = cx.new(WelcomeView::new);
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(h) => {
            cx.set_global(WelcomeWindow(h));
            cx.activate(true);
            let _ = h.update(cx, |_, w, _| {
                w.activate_window();
                platform::bring_to_front(w);
            });
        }
        Err(e) => log::error!("open welcome: {e:#}"),
    }
}
