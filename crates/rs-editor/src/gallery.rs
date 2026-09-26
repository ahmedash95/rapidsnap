//! Compile-checked usage of the gpui-component widgets RapidSnap needs, at the
//! pinned rev. Exists so import paths / constructor signatures in
//! CONTRACTS.md §9 are verified by the compiler. Open it for a visual check
//! with `rs_editor::gallery::open_gallery(cx)`. The editor owner may delete
//! or extend it (keep it compiling if you keep it).

use gpui_kit::component::{
    button::{Button, ButtonGroup, ButtonVariants as _, Toggle},
    checkbox::Checkbox,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    input::{Input, InputEvent, InputState, NumberInput},
    label::Label,
    radio::Radio,
    select::{Select, SelectState},
    separator::Separator,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    tab::{Tab, TabBar},
    v_flex, ActiveTheme as _, IconName, IndexPath, Root, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Widget gallery view.
pub struct Gallery {
    slider: Entity<SliderState>,
    color: Entity<ColorPickerState>,
    input: Entity<InputState>,
    number: Entity<InputState>,
    select: Entity<SelectState<Vec<String>>>,
    tab: usize,
    on: bool,
    log: Vec<String>,
    _subs: Vec<Subscription>,
}

impl Gallery {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let slider = cx.new(|_| SliderState::new().min(0.0).max(100.0).step(1.0).default_value(40.0));
        let color = cx.new(|cx| ColorPickerState::new(window, cx));
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type…"));
        let number = cx.new(|cx| InputState::new(window, cx).default_value("12"));
        let select = cx.new(|cx| {
            SelectState::new(
                vec!["Free".to_string(), "1:1".to_string(), "16:9".to_string()],
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let subs = vec![
            cx.subscribe_in(&slider, window, |this, _, ev: &SliderEvent, _, cx| {
                if let SliderEvent::Change(v) = ev {
                    this.log.push(format!("slider {}", v.end()));
                    cx.notify();
                }
            }),
            cx.subscribe_in(&color, window, |this, _, ev: &ColorPickerEvent, _, cx| {
                let ColorPickerEvent::Change(c) = ev;
                this.log.push(format!("color {c:?}"));
                cx.notify();
            }),
            cx.subscribe_in(&input, window, |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.log.push(format!("input {}", state.read(cx).value()));
                    cx.notify();
                }
            }),
        ];
        Self { slider, color, input, number, select, tab: 0, on: false, log: Vec::new(), _subs: subs }
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .p_4()
            .pt(px(36.))
            .gap_3()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                TabBar::new("tabs")
                    .selected_index(self.tab)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.tab = *ix;
                        cx.notify();
                    }))
                    .child(Tab::new().label("Annotate"))
                    .child(Tab::new().label("Beautify")),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("primary").primary().label("Primary").icon(IconName::Check))
                    .child(Button::new("ghost").ghost().small().label("Ghost").tooltip("A tooltip"))
                    .child(
                        ButtonGroup::new("group")
                            .child(Button::new("a").label("A"))
                            .child(Button::new("b").label("B")),
                    )
                    .child(Toggle::new("toggle").label("Bold").checked(self.on)),
            )
            .child(
                Switch::new("switch").label("Switch").checked(self.on).on_click(cx.listener(
                    |this, v: &bool, _, cx| {
                        this.on = *v;
                        cx.notify();
                    },
                )),
            )
            .child(Checkbox::new("check").label("Checkbox").checked(self.on))
            .child(Radio::new("radio").label("Radio").checked(!self.on))
            .child(Slider::new(&self.slider))
            .child(ColorPicker::new(&self.color))
            .child(Input::new(&self.input))
            .child(NumberInput::new(&self.number))
            .child(Select::new(&self.select))
            .child(Separator::horizontal())
            .child(Label::new("Label"))
            .when(!self.log.is_empty(), |d| d.child(div().text_xs().child(self.log.join(" · "))))
    }
}

/// Open the gallery in its own window (debug aid).
pub fn open_gallery(cx: &mut App) -> anyhow::Result<WindowHandle<Root>> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(640.), px(720.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("Widgets".into()), appears_transparent: true, ..Default::default() }),
        ..Default::default()
    };
    cx.open_window(opts, |window, cx| {
        let view = cx.new(|cx| Gallery::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
}
