//! Preferences window (General · Capture · Output · About).
//!
//! Reads and writes `PreferencesGlobal`; the app-level observer in `main.rs`
//! persists (debounced), re-registers hotkeys and syncs the tray menu.

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    separator::Separator,
    switch::Switch,
    tab::{Tab, TabBar},
    v_flex, ActiveTheme as _, IndexPath, Root, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rs_core::{CaptureDelay, ExportFormat, ExportScale, Preferences};
use rs_editor::PreferencesGlobal;

use crate::keys::{self, RecordOutcome};
use crate::platform::{self, ActivationPolicy};
use crate::tray::{self, AppEvent, TrayState, HOTKEY_MODES};

pub const TAB_GENERAL: usize = 0;
pub const TAB_CAPTURE: usize = 1;
pub const TAB_OUTPUT: usize = 2;
pub const TAB_ABOUT: usize = 3;

const FORMATS: [&str; 3] = ["PNG", "JPEG", "WebP"];
const SCALES: [&str; 2] = ["1× (points)", "2× (Retina pixels)"];
const QUALITIES: [(&str, u8); 5] =
    [("Maximum (100)", 100), ("High (90)", 90), ("Good (80)", 80), ("Medium (70)", 70), ("Small (60)", 60)];
const DELAYS: [&str; 4] = ["No delay", "3 seconds", "5 seconds", "10 seconds"];
const HISTORY: [(&str, u32); 5] = [("Off", 0), ("10 captures", 10), ("25 captures", 25), ("50 captures", 50), ("100 captures", 100)];

type StrSelect = Entity<SelectState<Vec<&'static str>>>;

pub struct PrefsView {
    tab: usize,
    format: StrSelect,
    scale: StrSelect,
    /// JPEG quality (shown while the default format is JPEG).
    quality: StrSelect,
    delay: StrSelect,
    history: StrSelect,
    template: Entity<InputState>,
    /// Mode whose shortcut is being recorded.
    recording: Option<AppEvent>,
    record_focus: FocusHandle,
    record_error: Option<String>,
    login_error: Option<String>,
    /// SMAppService state (refreshed on open, after toggling and whenever
    /// the window becomes active — e.g. back from System Settings).
    login_status: rs_smart::login_item::LoginItemStatus,
    _subs: Vec<Subscription>,
}

fn prefs(cx: &App) -> Preferences {
    cx.try_global::<PreferencesGlobal>().map(|p| p.0.clone()).unwrap_or_default()
}

fn update_prefs(cx: &mut App, f: impl FnOnce(&mut Preferences)) {
    if cx.has_global::<PreferencesGlobal>() {
        cx.update_global::<PreferencesGlobal, _>(|p, _| f(&mut p.0));
    }
}

fn format_ix(f: ExportFormat) -> usize {
    match f {
        ExportFormat::Png => 0,
        ExportFormat::Jpeg { .. } => 1,
        ExportFormat::WebP => 2,
    }
}

/// Closest JPEG quality preset (index into `QUALITIES`).
fn quality_ix(f: ExportFormat) -> usize {
    let q = match f {
        ExportFormat::Jpeg { quality } => quality,
        _ => 90,
    };
    QUALITIES
        .iter()
        .enumerate()
        .min_by_key(|(_, (_, v))| (i16::from(*v) - i16::from(q)).abs())
        .map(|(i, _)| i)
        .unwrap_or(1)
}

fn delay_ix(d: CaptureDelay) -> usize {
    match d {
        CaptureDelay::None => 0,
        CaptureDelay::S3 => 1,
        CaptureDelay::S5 => 2,
        CaptureDelay::S10 => 3,
    }
}

impl PrefsView {
    pub fn new(tab: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let login_status = sync_login_status(cx);
        let p = prefs(cx);
        let select = |items: Vec<&'static str>, ix: usize, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| SelectState::new(items, Some(IndexPath::new(ix)), window, cx))
        };
        let format = select(FORMATS.to_vec(), format_ix(p.export.format), window, cx);
        let scale = select(SCALES.to_vec(), if p.export.scale == ExportScale::X1 { 0 } else { 1 }, window, cx);
        let quality = select(QUALITIES.iter().map(|(l, _)| *l).collect(), quality_ix(p.export.format), window, cx);
        let delay = select(DELAYS.to_vec(), delay_ix(p.capture_delay), window, cx);
        let hist_ix = HISTORY.iter().position(|(_, n)| *n == p.history_limit).unwrap_or(3);
        let history = select(HISTORY.iter().map(|(l, _)| *l).collect(), hist_ix, window, cx);
        let template = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("RapidSnap {date} at {time}")
                .default_value(p.export.filename_template.clone())
        });

        let subs = vec![
            cx.subscribe_in(&format, window, |_, _, ev: &SelectEvent<Vec<&'static str>>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else { return };
                let v = *v;
                update_prefs(cx, |p| {
                    let quality = match p.export.format {
                        ExportFormat::Jpeg { quality } => quality,
                        _ => 90,
                    };
                    p.export.format = match v {
                        "JPEG" => ExportFormat::Jpeg { quality },
                        "WebP" => ExportFormat::WebP,
                        _ => ExportFormat::Png,
                    };
                });
            }),
            cx.subscribe_in(&scale, window, |_, _, ev: &SelectEvent<Vec<&'static str>>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else { return };
                let x1 = *v == SCALES[0];
                update_prefs(cx, |p| p.export.scale = if x1 { ExportScale::X1 } else { ExportScale::X2 });
            }),
            cx.subscribe_in(&quality, window, |_, _, ev: &SelectEvent<Vec<&'static str>>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else { return };
                if let Some((_, q)) = QUALITIES.iter().find(|(l, _)| l == v) {
                    let quality = *q;
                    update_prefs(cx, |p| p.export.format = ExportFormat::Jpeg { quality });
                }
            }),
            cx.subscribe_in(&delay, window, |_, _, ev: &SelectEvent<Vec<&'static str>>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else { return };
                let d = match DELAYS.iter().position(|d| d == v) {
                    Some(1) => CaptureDelay::S3,
                    Some(2) => CaptureDelay::S5,
                    Some(3) => CaptureDelay::S10,
                    _ => CaptureDelay::None,
                };
                update_prefs(cx, |p| p.capture_delay = d);
            }),
            cx.subscribe_in(&history, window, |_, _, ev: &SelectEvent<Vec<&'static str>>, _, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else { return };
                if let Some((_, n)) = HISTORY.iter().find(|(l, _)| l == v) {
                    let n = *n;
                    update_prefs(cx, |p| p.history_limit = n);
                }
            }),
            cx.subscribe_in(&template, window, |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let t = state.read(cx).value().to_string();
                    update_prefs(cx, |p| p.export.filename_template = t);
                    cx.notify();
                    let _ = this;
                }
            }),
        ];
        let mut subs = subs;
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                let st = sync_login_status(cx);
                if st != this.login_status {
                    this.login_status = st;
                    cx.notify();
                }
            }
        }));
        // The editor (Export format/scale, counter) and the tray (delay, …)
        // write the same global while this window is open: keep the
        // stateful widgets in sync (plain switches re-read on render).
        subs.push(cx.observe_global_in::<PreferencesGlobal>(window, |this, window, cx| {
            this.sync_from_prefs(window, cx);
            cx.notify();
        }));
        let record_focus = cx.focus_handle();
        cx.on_focus_out(&record_focus, window, |this, _, _, cx| this.stop_recording(cx)).detach();
        cx.on_release(|this, cx| {
            if this.recording.is_some() {
                resume_hotkeys(cx);
            }
        })
        .detach();
        Self {
            tab,
            format,
            scale,
            quality,
            delay,
            history,
            template,
            recording: None,
            record_focus,
            record_error: None,
            login_error: None,
            login_status,
            _subs: subs,
        }
    }

    /// Push the current `PreferencesGlobal` values into the select / input
    /// widgets whose state would otherwise go stale.
    fn sync_from_prefs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = prefs(cx);
        let hist_ix = HISTORY.iter().position(|(_, n)| *n == p.history_limit).unwrap_or(3);
        let wanted = [
            (&self.format, format_ix(p.export.format)),
            (&self.scale, if p.export.scale == ExportScale::X1 { 0 } else { 1 }),
            (&self.quality, quality_ix(p.export.format)),
            (&self.delay, delay_ix(p.capture_delay)),
            (&self.history, hist_ix),
        ];
        for (select, ix) in wanted {
            let ix = IndexPath::new(ix);
            if select.read(cx).selected_index(cx) != Some(ix) {
                select.update(cx, |s, cx| s.set_selected_index(Some(ix), window, cx));
            }
        }
        if self.template.read(cx).value() != p.export.filename_template.as_str() {
            let t = p.export.filename_template.clone();
            self.template.update(cx, |s, cx| s.set_value(t, window, cx));
        }
    }

    pub fn set_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn start_recording(&mut self, mode: AppEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = Some(mode);
        self.record_error = None;
        if cx.has_global::<TrayState>() {
            cx.update_global::<TrayState, _>(|t, _| t.suspend());
        }
        self.record_focus.focus(window, cx);
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.take().is_some() {
            resume_hotkeys(cx);
            cx.notify();
        }
    }

    fn on_record_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let Some(mode) = self.recording else { return };
        match keys::record(ks) {
            RecordOutcome::Cancel => {}
            RecordOutcome::Clear => update_prefs(cx, |p| {
                if let Some(slot) = tray::hotkey_slot(&mut p.hotkeys, mode) {
                    *slot = None;
                }
            }),
            RecordOutcome::Hotkey(h) => {
                let p = prefs(cx);
                if let Some(other) =
                    HOTKEY_MODES.iter().find(|&&m| m != mode && tray::hotkey_for(&p.hotkeys, m) == Some(&h))
                {
                    self.record_error = Some(format!("{} is already used for {}", h.display(), other.label()));
                    cx.notify();
                    return;
                }
                update_prefs(cx, |p| {
                    if let Some(slot) = tray::hotkey_slot(&mut p.hotkeys, mode) {
                        *slot = Some(h);
                    }
                });
            }
            RecordOutcome::Invalid(msg) => {
                self.record_error = Some(msg);
                cx.notify();
                return;
            }
        }
        self.stop_recording(cx);
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        cx.activate(true);
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            cx.update(|cx| update_prefs(cx, |p| p.export.default_folder = Some(dir)));
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn set_login(&mut self, on: bool, cx: &mut Context<Self>) {
        use rs_smart::login_item::{self, LoginItemStatus};
        match login_item::set_launch_at_login(on) {
            Ok(()) => {
                self.login_error = None;
                self.login_status = login_item::login_item_status();
                if on && self.login_status == LoginItemStatus::NotFound {
                    self.login_error =
                        Some("macOS can’t find RapidSnap.app — move it to /Applications and try again.".into());
                }
                update_prefs(cx, |p| p.launch_at_login = on);
            }
            Err(e) => {
                log::warn!("launch at login: {e}");
                self.login_error = Some(if login_item::is_running_from_app_bundle() {
                    format!("Couldn’t change the login item: {e}")
                } else {
                    "Launch at login only works from the RapidSnap.app bundle.".into()
                });
            }
        }
        cx.notify();
    }
}

/// Read the login-item state and mirror it into `launch_at_login` (when
/// running from the bundle): Enabled / RequiresApproval → on (registered;
/// approval shows a hint), NotRegistered / NotFound → off.
fn sync_login_status(cx: &mut App) -> rs_smart::login_item::LoginItemStatus {
    use rs_smart::login_item::{self, LoginItemStatus};
    let st = login_item::login_item_status();
    let registered = match st {
        LoginItemStatus::Enabled | LoginItemStatus::RequiresApproval => Some(true),
        LoginItemStatus::NotRegistered | LoginItemStatus::NotFound => Some(false),
        LoginItemStatus::Unavailable => None,
    };
    if let Some(on) = registered
        && prefs(cx).launch_at_login != on
    {
        update_prefs(cx, |p| p.launch_at_login = on);
    }
    st
}

fn resume_hotkeys(cx: &mut App) {
    let hk = prefs(cx).hotkeys;
    if cx.has_global::<TrayState>() {
        cx.update_global::<TrayState, _>(|t, _| t.resume(&hk));
    }
}

fn row(label: &'static str, hint: Option<&'static str>, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .w_full()
        .gap_4()
        .justify_between()
        .items_center()
        .py_1()
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().child(label))
                .when_some(hint, |d, h| d.child(div().text_xs().text_color(cx.theme().muted_foreground).child(h))),
        )
        .child(control)
}

fn section(title: &'static str, cx: &App) -> Div {
    div().pt_2().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().muted_foreground).child(title)
}

impl PrefsView {
    fn general(&mut self, cx: &mut Context<Self>) -> Div {
        let p = prefs(cx);
        v_flex()
            .gap_2()
            .child(section("STARTUP", cx))
            .child(row(
                "Launch at login",
                None,
                Switch::new("login").checked(p.launch_at_login).on_click(cx.listener(|this, v: &bool, _, cx| this.set_login(*v, cx))),
                cx,
            ))
            .when_some(self.login_error.clone(), |d, e| d.child(div().text_xs().text_color(rgb(0xff453a)).child(e)))
            .when(self.login_status == rs_smart::login_item::LoginItemStatus::Unavailable && self.login_error.is_none(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Available when running RapidSnap.app (not a bare dev binary)."),
                )
            })
            .when(self.login_status == rs_smart::login_item::LoginItemStatus::RequiresApproval, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .child(div().text_xs().text_color(rgb(0xff9f0a)).child("Approve RapidSnap in System Settings → Login Items."))
                        .child(Button::new("login-settings").xsmall().label("Open…").on_click(|_, _, _| {
                            if let Err(e) = rs_smart::login_item::open_login_items_settings() {
                                log::warn!("open login items settings: {e}");
                            }
                        })),
                )
            })
            .child(section("AFTER CAPTURE", cx))
            .child(row(
                "Instant copy mode",
                Some("Copy the (beautified) capture to the clipboard and skip the editor"),
                Switch::new("instant")
                    .checked(p.instant_copy)
                    .on_click(|v: &bool, _, cx| update_prefs(cx, |p| p.instant_copy = *v)),
                cx,
            ))
            .child(row(
                "Close editor after copying",
                None,
                Switch::new("autoclose")
                    .checked(p.auto_close_on_copy)
                    .on_click(|v: &bool, _, cx| update_prefs(cx, |p| p.auto_close_on_copy = *v)),
                cx,
            ))
            .child(row("Recent captures history", None, div().w(px(170.)).child(Select::new(&self.history).small()), cx))
            .child(section("ONBOARDING", cx))
            .child(row(
                "Welcome screen",
                Some("Screen Recording permission status and tips"),
                Button::new("welcome").small().label("Show Welcome…").on_click(|_, _, cx| crate::welcome::open_welcome(cx)),
                cx,
            ))
    }

    fn capture(&mut self, cx: &mut Context<Self>) -> Div {
        let p = prefs(cx);
        let errors = cx.try_global::<TrayState>().map(|t| t.errors.clone()).unwrap_or_default();
        let theme = cx.theme().clone();
        let mut list = v_flex().gap_1().child(section("GLOBAL SHORTCUTS", cx));
        for mode in HOTKEY_MODES {
            let recording = self.recording == Some(mode);
            let text = if recording {
                "Type shortcut…".to_string()
            } else {
                tray::hotkey_for(&p.hotkeys, mode).map(|h| h.display()).unwrap_or_else(|| "Record Shortcut".into())
            };
            let recorder = div()
                .id(SharedString::from(format!("rec-{}", mode.label())))
                .w(px(170.))
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_1()
                .border_color(if recording { theme.primary } else { theme.border })
                .bg(if recording { theme.accent } else { theme.background })
                .text_sm()
                .font_family("Menlo")
                .cursor_pointer()
                .when(recording, |d| d.track_focus(&self.record_focus))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.recording == Some(mode) {
                        this.stop_recording(cx);
                    } else {
                        this.start_recording(mode, window, cx);
                    }
                }))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    if this.recording.is_some() {
                        this.on_record_key(&ev.keystroke, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(text);
            let clear = Button::new(SharedString::from(format!("clear-{}", mode.label())))
                .ghost()
                .xsmall()
                .label("✕")
                .tooltip("Remove shortcut")
                .on_click(move |_, _, cx| {
                    update_prefs(cx, |p| {
                        if let Some(slot) = tray::hotkey_slot(&mut p.hotkeys, mode) {
                            *slot = None;
                        }
                    })
                });
            list = list.child(row(mode.label(), None, h_flex().gap_1().child(recorder).child(clear), cx));
            if let Some(err) = errors.get(mode.label()) {
                list = list.child(div().text_xs().text_color(rgb(0xff453a)).child(format!("⚠ {err}")));
            }
        }
        list.when_some(self.record_error.clone(), |d, e| d.child(div().text_xs().text_color(rgb(0xff9f0a)).child(e)))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Click a field and press the new combination · ⌫ clears · Esc cancels."),
            )
            .child(section("WINDOW CAPTURE", cx))
            .child(row(
                "Include window shadow",
                None,
                Switch::new("shadow")
                    .checked(p.window_capture_shadow)
                    .on_click(|v: &bool, _, cx| update_prefs(cx, |p| p.window_capture_shadow = *v)),
                cx,
            ))
            .child(row(
                "Transparent background",
                Some("Keep rounded corners see-through; off: opaque desktop pixels behind the corners"),
                Switch::new("transparent")
                    .checked(p.window_capture_transparent)
                    .on_click(|v: &bool, _, cx| update_prefs(cx, |p| p.window_capture_transparent = *v)),
                cx,
            ))
            .child(section("TIMING", cx))
            .child(row(
                "Default capture delay",
                Some("Applies to shortcuts and menu captures"),
                div().w(px(170.)).child(Select::new(&self.delay).small()),
                cx,
            ))
    }

    fn output(&mut self, cx: &mut Context<Self>) -> Div {
        let p = prefs(cx);
        let folder = p
            .export
            .default_folder
            .as_ref()
            .map(|f| f.display().to_string())
            .unwrap_or_else(|| "~/Desktop (default)".into());
        let now = chrono::Local::now();
        use chrono::{Datelike as _, Timelike as _};
        let preview = rs_core::render_filename(
            &p.export.filename_template,
            &rs_core::FilenameContext {
                year: now.year(),
                month: now.month(),
                day: now.day(),
                hour: now.hour(),
                minute: now.minute(),
                second: now.second(),
                counter: p.export.counter.saturating_add(1),
                width: 1440,
                height: 900,
                app_name: Some("Safari".into()),
            },
        );
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_2()
            .child(section("FORMAT", cx))
            .child(row("Default format", None, div().w(px(170.)).child(Select::new(&self.format).small()), cx))
            .when(matches!(p.export.format, ExportFormat::Jpeg { .. }), |d| {
                d.child(row("JPEG quality", None, div().w(px(170.)).child(Select::new(&self.quality).small()), cx))
            })
            .child(row("Scale", None, div().w(px(170.)).child(Select::new(&self.scale).small()), cx))
            .child(section("LOCATION", cx))
            .child(row(
                "Default folder",
                None,
                h_flex()
                    .gap_1()
                    .child(div().max_w(px(200.)).overflow_hidden().text_xs().text_color(muted).child(folder))
                    .child(Button::new("folder").small().label("Choose…").on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))))
                    .when(p.export.default_folder.is_some(), |d| {
                        d.child(
                            Button::new("folder-reset")
                                .ghost()
                                .small()
                                .label("Reset")
                                .on_click(|_, _, cx| update_prefs(cx, |p| p.export.default_folder = None)),
                        )
                    }),
                cx,
            ))
            .child(section("FILE NAME", cx))
            .child(Input::new(&self.template))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Tokens: {date} YYYY-MM-DD · {time} HH.MM.SS · {n} counter · {w} {h} size in px · {app} source app"),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(div().text_color(muted).child("Preview:"))
                    .child(div().font_family("Menlo").child(format!("{preview}.{}", p.export.format.extension()))),
            )
    }

    fn about(&mut self, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .items_center()
            .gap_3()
            .pt_6()
            .child(img(rs_core::assets::asset_path("icons/app-icon-256.png")).size(px(96.)))
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("RapidSnap"))
            .child(div().text_sm().text_color(muted).child(format!("Version {}", env!("CARGO_PKG_VERSION"))))
            .child(
                div()
                    .text_sm()
                    .text_center()
                    .text_color(muted)
                    .child("Fast screenshots with best-in-class annotations. Works fully offline."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("site").small().label("rapidsnap.app").on_click(|_, _, cx| cx.open_url("https://rapidsnap.app")))
                    .child(Button::new("issue").small().label("Report an Issue").on_click(|_, _, cx| {
                        cx.open_url("https://github.com/ahmedash95/rapidsnap-support/issues")
                    })),
            )
    }
}

impl Render for PrefsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tab {
            TAB_CAPTURE => self.capture(cx),
            TAB_OUTPUT => self.output(cx),
            TAB_ABOUT => self.about(cx),
            _ => self.general(cx),
        };
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .pt(px(34.))
            .child(
                h_flex().w_full().justify_center().pb_2().child(
                    TabBar::new("prefs-tabs")
                        .selected_index(self.tab)
                        .on_click(cx.listener(|this, ix: &usize, _, cx| this.set_tab(*ix, cx)))
                        .child(Tab::new().label("General"))
                        .child(Tab::new().label("Capture"))
                        .child(Tab::new().label("Output"))
                        .child(Tab::new().label("About")),
                ),
            )
            .child(Separator::horizontal())
            .child(div().id("prefs-body").flex_1().overflow_y_scroll().px_6().py_3().child(body))
    }
}

struct PrefsWindow(WindowHandle<Root>, Entity<PrefsView>);
impl Global for PrefsWindow {}

/// Self-test probe: (format, scale) select indices of the open preferences
/// window, if any.
pub fn open_export_selection(cx: &App) -> Option<(Option<usize>, Option<usize>)> {
    let view = cx.try_global::<PrefsWindow>()?.1.read(cx);
    let ix = |s: &StrSelect| s.read(cx).selected_index(cx).map(|i| i.row);
    Some((ix(&view.format), ix(&view.scale)))
}

/// Open (or focus) the preferences window on `tab`.
pub fn open_preferences(tab: usize, cx: &mut App) {
    if let Some(PrefsWindow(h, view)) = cx.try_global::<PrefsWindow>().map(|w| PrefsWindow(w.0, w.1.clone()))
        && h.update(cx, |_, w, _| w.activate_window()).is_ok() {
            view.update(cx, |v, cx| v.set_tab(tab, cx));
            cx.activate(true);
            let _ = h.update(cx, |_, w, _| platform::bring_to_front(w));
            return;
        }
    platform::set_activation_policy(ActivationPolicy::Regular);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(600.), px(560.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("RapidSnap Preferences".into()), appears_transparent: true, ..Default::default() }),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    let mut view_entity = None;
    let res = cx.open_window(opts, |window, cx| {
        let view = cx.new(|cx| PrefsView::new(tab, window, cx));
        view_entity = Some(view.clone());
        cx.new(|cx| Root::new(view, window, cx))
    });
    match (res, view_entity) {
        (Ok(h), Some(v)) => {
            cx.set_global(PrefsWindow(h, v));
            cx.activate(true);
            let _ = h.update(cx, |_, w, _| {
                w.activate_window();
                platform::bring_to_front(w);
            });
        }
        (Err(e), _) => log::error!("open preferences: {e:#}"),
        _ => {}
    }
}
