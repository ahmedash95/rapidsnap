//! Right side panel: Style (context-sensitive for the current tool /
//! selection / text being edited) and Beautify (every BeautifySettings
//! option), with live preview through the model.
//!
//! Structure: this file owns the widget state, the event handlers and the
//! *spec builders* ([`EditorView::style_spec`] / [`EditorView::beautify_spec`])
//! that describe every control and its behavior once. `panel_layout.rs`
//! presents that spec; shared primitives live in `panel_spec.rs`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{
    color_picker::{ColorPickerEvent, ColorPickerState},
    input::{InputEvent, InputState},
    select::{SelectEvent, SelectState},
    slider::{SliderEvent, SliderState},
    IndexPath,
};
use gpui_kit::*;
use rs_core::{
    ArrowHead, AspectRatio, Background, BeautifySettings, Color, Corner, DashStyle, FillMode, FontFamily, FontWeight,
    InsetColor, LineCap, Padding, SpotlightShape, StepLabel, StylePreset, TextAlign, TextPreset, ToolKind, Watermark,
    WatermarkContent, WindowChrome,
};

use crate::icons::Ico;
use crate::model::EditorModel;
use crate::panel_icons::PIco;
use crate::panel_layout::PanelMetrics;
use crate::panel_spec::*;
use crate::services::CustomBackground;
use crate::style_map::{self, StyleEdit};
use crate::text_layout::{from_hsla, to_hsla};
use crate::view::EditorView;
use crate::{EditorOptions, PreferencesGlobal};

/// Panel sliders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Sl {
    StrokeWidth,
    Opacity,
    CornerRadius,
    ShadowBlur,
    HighlighterWidth,
    StepRadius,
    SpotDim,
    MagZoom,
    BlurStrength,
    PixelBlock,
    CalloutPadding,
    FontSize,
    OutlineWidth,
    BPadding,
    BPadTop,
    BPadRight,
    BPadBottom,
    BPadLeft,
    BInset,
    BCorner,
    BShadowSize,
    BShadowOpacity,
    BShadowY,
    BShadowX,
    BDeskBlur,
    BDeskBright,
    BGradAngle,
    BWmOpacity,
    BWmSize,
    BWmWidth,
    JpegQuality,
}

impl Sl {
    const ALL: [Sl; 31] = [
        Sl::StrokeWidth,
        Sl::Opacity,
        Sl::CornerRadius,
        Sl::ShadowBlur,
        Sl::HighlighterWidth,
        Sl::StepRadius,
        Sl::SpotDim,
        Sl::MagZoom,
        Sl::BlurStrength,
        Sl::PixelBlock,
        Sl::CalloutPadding,
        Sl::FontSize,
        Sl::OutlineWidth,
        Sl::BPadding,
        Sl::BPadTop,
        Sl::BPadRight,
        Sl::BPadBottom,
        Sl::BPadLeft,
        Sl::BInset,
        Sl::BCorner,
        Sl::BShadowSize,
        Sl::BShadowOpacity,
        Sl::BShadowY,
        Sl::BShadowX,
        Sl::BDeskBlur,
        Sl::BDeskBright,
        Sl::BGradAngle,
        Sl::BWmOpacity,
        Sl::BWmSize,
        Sl::BWmWidth,
        Sl::JpegQuality,
    ];

    /// (min, max, step)
    fn range(self) -> (f32, f32, f32) {
        match self {
            Sl::StrokeWidth => (1.0, 30.0, 0.5),
            Sl::Opacity => (0.05, 1.0, 0.01),
            Sl::CornerRadius => (0.0, 60.0, 1.0),
            Sl::ShadowBlur => (1.0, 40.0, 1.0),
            Sl::HighlighterWidth => (4.0, 60.0, 1.0),
            Sl::StepRadius => (8.0, 40.0, 1.0),
            Sl::SpotDim => (0.1, 0.95, 0.01),
            Sl::MagZoom => (2.0, 4.0, 0.1),
            Sl::BlurStrength => (2.0, 40.0, 1.0),
            Sl::PixelBlock => (4.0, 64.0, 1.0),
            Sl::CalloutPadding => (2.0, 40.0, 1.0),
            Sl::FontSize => (8.0, 200.0, 1.0),
            Sl::OutlineWidth => (0.5, 12.0, 0.5),
            Sl::BPadding | Sl::BPadTop | Sl::BPadRight | Sl::BPadBottom | Sl::BPadLeft => (0.0, 300.0, 1.0),
            Sl::BInset => (0.0, 120.0, 1.0),
            Sl::BCorner => (0.0, 100.0, 1.0),
            Sl::BShadowSize => (0.0, 80.0, 1.0),
            Sl::BShadowOpacity => (0.0, 1.0, 0.01),
            Sl::BShadowY | Sl::BShadowX => (-40.0, 40.0, 1.0),
            Sl::BDeskBlur => (0.0, 60.0, 1.0),
            Sl::BDeskBright => (0.3, 1.5, 0.01),
            Sl::BGradAngle => (0.0, 360.0, 1.0),
            Sl::BWmOpacity => (0.05, 1.0, 0.01),
            Sl::BWmSize => (8.0, 72.0, 1.0),
            Sl::BWmWidth => (16.0, 600.0, 1.0),
            Sl::JpegQuality => (10.0, 100.0, 1.0),
        }
    }

    fn is_beautify(self) -> bool {
        matches!(
            self,
            Sl::BPadding
                | Sl::BPadTop
                | Sl::BPadRight
                | Sl::BPadBottom
                | Sl::BPadLeft
                | Sl::BInset
                | Sl::BCorner
                | Sl::BShadowSize
                | Sl::BShadowOpacity
                | Sl::BShadowY
                | Sl::BShadowX
                | Sl::BWmWidth
                | Sl::BDeskBlur
                | Sl::BDeskBright
                | Sl::BGradAngle
                | Sl::BWmOpacity
                | Sl::BWmSize
        )
    }
}

/// Color targets (one color picker each).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Col {
    Primary,
    Fill,
    StepText,
    TextBg,
    BgSolid,
    GradA,
    GradB,
    Inset,
    Watermark,
}

impl Col {
    const ALL: [Col; 9] =
        [Col::Primary, Col::Fill, Col::StepText, Col::TextBg, Col::BgSolid, Col::GradA, Col::GradB, Col::Inset, Col::Watermark];
}

pub(crate) struct PanelState {
    pub tab: usize,
    sliders: HashMap<Sl, Entity<SliderState>>,
    pickers: HashMap<Col, Entity<ColorPickerState>>,
    pub(crate) font_select: Entity<SelectState<Vec<String>>>,
    fonts: Vec<String>,
    preset_name: Entity<InputState>,
    chrome_text: Entity<InputState>,
    watermark_text: Entity<InputState>,
    pub eyedropper_target: Option<Col>,
    thumbs: HashMap<String, Arc<RenderImage>>,
    pub custom_bgs: Vec<CustomBackground>,
    per_side: bool,
    pub slider_active: bool,
    inset_cache: std::cell::Cell<Option<(rs_core::ImageTransform, Color)>>,
    /// Sections whose advanced rows are revealed (plus
    /// `panel_layout::MORE_KEY` for the "More options" disclosure).
    pub expanded: HashSet<&'static str>,
    /// Measured heights of the last frame (self-test).
    pub metrics: PanelMetrics,
    /// Last JPEG quality (kept while another format is selected).
    pub jpeg_quality: u8,
    /// Bumped on every color-picker change; a recent color is only
    /// recorded once the picker has been still for a moment (commit).
    recent_gen: u64,
}

fn gradient_stops(b: &Background) -> Option<(Color, Color, f32)> {
    match b {
        Background::LinearGradient { stops, angle_deg } => Some((
            stops.first().map(|s| s.color).unwrap_or(Color::WHITE),
            stops.last().map(|s| s.color).unwrap_or(Color::BLACK),
            *angle_deg,
        )),
        Background::RadialGradient { stops, .. } => Some((
            stops.first().map(|s| s.color).unwrap_or(Color::WHITE),
            stops.last().map(|s| s.color).unwrap_or(Color::BLACK),
            0.0,
        )),
        _ => None,
    }
}

impl PanelState {
    pub fn new(
        model: &EditorModel,
        _options: &EditorOptions,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) -> (Self, Vec<Subscription>) {
        let mut subs = Vec::new();
        let mut sliders = HashMap::new();
        for sl in Sl::ALL {
            let (min, max, step) = sl.range();
            let st = cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(min));
            subs.push(cx.subscribe_in(&st, window, move |this: &mut EditorView, _, ev: &SliderEvent, window, cx| {
                match ev {
                    SliderEvent::Change(v) => {
                        this.panel.slider_active = true;
                        this.on_slider(sl, v.end(), window, cx);
                    }
                    SliderEvent::Release(_) => {
                        this.panel.slider_active = false;
                        this.model.break_coalescing();
                        this.changed(cx);
                    }
                }
            }));
            sliders.insert(sl, st);
        }
        let mut pickers = HashMap::new();
        for col in Col::ALL {
            let st = cx.new(|cx| ColorPickerState::new(window, cx).default_value(to_hsla(Color::RED)));
            subs.push(cx.subscribe_in(&st, window, move |this: &mut EditorView, _, ev: &ColorPickerEvent, window, cx| {
                let ColorPickerEvent::Change(c) = ev;
                if let Some(c) = c {
                    let c = from_hsla(*c);
                    this.apply_color(col, c, window, cx);
                    this.schedule_recent_color(c, window, cx);
                }
            }));
            pickers.insert(col, st);
        }
        let font_select = cx.new(|cx| SelectState::new(vec!["System".to_string()], Some(IndexPath::default()), window, cx).searchable(true));
        subs.push(cx.subscribe_in(
            &font_select,
            window,
            |this: &mut EditorView, _, ev: &SelectEvent<Vec<String>>, _, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                let fam = match name.as_str() {
                    "System" => FontFamily::System,
                    "Monospace" => FontFamily::Monospace,
                    "Serif" => FontFamily::Serif,
                    n => FontFamily::Named(n.to_string()),
                };
                this.model.apply_style(&StyleEdit::FontFamily(fam));
                this.changed(cx);
            },
        ));
        let preset_name = cx.new(|cx| InputState::new(window, cx).placeholder("Preset name"));
        let chrome_text = cx.new(|cx| InputState::new(window, cx).placeholder("Title / URL"));
        subs.push(cx.subscribe_in(&chrome_text, window, |this: &mut EditorView, st, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                let v = st.read(cx).value().to_string();
                let mut b = this.model.doc.beautify.clone();
                b.chrome = match b.chrome {
                    WindowChrome::MacOs { dark, .. } => WindowChrome::MacOs { dark, title: (!v.is_empty()).then_some(v) },
                    WindowChrome::Browser { dark, .. } => WindowChrome::Browser { dark, url: v },
                    WindowChrome::None => WindowChrome::None,
                };
                this.model.set_beautify(b, Some("chrome_text"));
                this.changed(cx);
            }
        }));
        let watermark_text = cx.new(|cx| InputState::new(window, cx).placeholder("Watermark text"));
        subs.push(cx.subscribe_in(&watermark_text, window, |this: &mut EditorView, st, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                let v = st.read(cx).value().to_string();
                let mut b = this.model.doc.beautify.clone();
                if let Some(w) = &mut b.watermark
                    && let WatermarkContent::Text { text, .. } = &mut w.content
                {
                    *text = v;
                }
                this.model.set_beautify(b, Some("wm_text"));
                this.changed(cx);
            }
        }));
        let mut p = Self {
            tab: 0,
            sliders,
            pickers,
            font_select,
            fonts: Vec::new(),
            preset_name,
            chrome_text,
            watermark_text,
            eyedropper_target: None,
            thumbs: HashMap::new(),
            custom_bgs: Vec::new(),
            per_side: !model.doc.beautify.padding.linked,
            slider_active: false,
            inset_cache: std::cell::Cell::new(None),
            expanded: HashSet::new(),
            metrics: PanelMetrics::default(),
            jpeg_quality: 90,
            recent_gen: 0,
        };
        p.sync(model, window, cx);
        (p, subs)
    }

    pub fn set_fonts(&mut self, names: Vec<String>, window: &mut Window, cx: &mut Context<EditorView>) {
        let mut items = vec!["System".to_string(), "Monospace".to_string(), "Serif".to_string()];
        items.extend(names);
        self.fonts = items.clone();
        self.font_select.update(cx, |s, cx| s.set_items(items, window, cx));
    }

    fn jpeg_quality_from(&self, cx: &App) -> Option<u8> {
        match cx.try_global::<PreferencesGlobal>()?.0.export.format {
            rs_core::ExportFormat::Jpeg { quality } => Some(quality.clamp(10, 100)),
            _ => None,
        }
    }

    pub(crate) fn slider_state(&self, sl: Sl) -> Option<Entity<SliderState>> {
        self.sliders.get(&sl).cloned()
    }

    pub(crate) fn picker_state(&self, col: Col) -> Option<Entity<ColorPickerState>> {
        self.pickers.get(&col).cloned()
    }

    fn set_slider(&self, sl: Sl, v: f32, window: &mut Window, cx: &mut Context<EditorView>) {
        if let Some(s) = self.sliders.get(&sl) {
            let (min, max, _) = sl.range();
            let v = v.clamp(min, max);
            s.update(cx, |s, cx| {
                if (s.value().end() - v).abs() > 1e-4 {
                    s.set_value(v, window, cx)
                }
            });
        }
    }

    fn set_picker(&self, col: Col, c: Color, window: &mut Window, cx: &mut Context<EditorView>) {
        if let Some(p) = self.pickers.get(&col) {
            let h = to_hsla(c);
            p.update(cx, |p, cx| {
                if p.value() != Some(h) {
                    p.set_value(h, window, cx)
                }
            });
        }
    }

    /// Push model values into the widgets (after selection/tool/undo change).
    pub fn sync(&mut self, model: &EditorModel, window: &mut Window, cx: &mut Context<EditorView>) {
        let (tool, st, _) = model.panel_target();
        self.set_slider(Sl::StrokeWidth, st.stroke.width, window, cx);
        self.set_slider(Sl::Opacity, st.opacity, window, cx);
        self.set_slider(Sl::CornerRadius, st.corner_radius, window, cx);
        let shadow = if tool == ToolKind::Text { st.text.shadow } else { st.shadow };
        self.set_slider(Sl::ShadowBlur, shadow.map(|s| s.blur).unwrap_or(8.0), window, cx);
        self.set_slider(Sl::HighlighterWidth, st.highlighter_width, window, cx);
        self.set_slider(Sl::StepRadius, st.step_radius, window, cx);
        self.set_slider(Sl::SpotDim, st.spotlight_dim.alpha_f32(), window, cx);
        self.set_slider(Sl::MagZoom, st.magnifier_zoom, window, cx);
        self.set_slider(Sl::BlurStrength, st.blur_strength, window, cx);
        self.set_slider(Sl::PixelBlock, st.pixelate_block as f32, window, cx);
        self.set_slider(Sl::CalloutPadding, st.callout_padding, window, cx);
        self.set_slider(Sl::FontSize, st.text.size, window, cx);
        if let rs_core::TextBackground::Outline { width, .. } = st.text.background {
            self.set_slider(Sl::OutlineWidth, width, window, cx);
        }
        self.set_picker(Col::Primary, style_map::primary_color(tool, &st), window, cx);
        self.set_picker(Col::Fill, st.fill, window, cx);
        self.set_picker(Col::StepText, st.step_text_color, window, cx);
        self.set_picker(Col::TextBg, style_map::text_bg_color(&st.text.background), window, cx);
        // Font select.
        let fam = match &st.text.family {
            FontFamily::System => "System".to_string(),
            FontFamily::Monospace => "Monospace".to_string(),
            FontFamily::Serif => "Serif".to_string(),
            FontFamily::Named(n) => n.clone(),
        };
        self.font_select.update(cx, |s, cx| {
            if s.selected_value() != Some(&fam) {
                s.set_selected_value(&fam, window, cx);
            }
        });
        // Beautify.
        let b = &model.doc.beautify;
        self.set_slider(Sl::BPadding, b.padding.top, window, cx);
        self.set_slider(Sl::BPadTop, b.padding.top, window, cx);
        self.set_slider(Sl::BPadRight, b.padding.right, window, cx);
        self.set_slider(Sl::BPadBottom, b.padding.bottom, window, cx);
        self.set_slider(Sl::BPadLeft, b.padding.left, window, cx);
        self.set_slider(Sl::BInset, b.inset.size, window, cx);
        self.set_slider(Sl::BCorner, b.corner_radius, window, cx);
        self.set_slider(Sl::BShadowSize, b.shadow.size, window, cx);
        self.set_slider(Sl::BShadowOpacity, b.shadow.opacity, window, cx);
        self.set_slider(Sl::BShadowY, b.shadow.offset_y, window, cx);
        self.set_slider(Sl::BShadowX, b.shadow.offset_x, window, cx);
        if let Background::DesktopWallpaper { blur, brightness } = b.background {
            self.set_slider(Sl::BDeskBlur, blur, window, cx);
            self.set_slider(Sl::BDeskBright, brightness, window, cx);
        }
        if let Some((a, c, angle)) = gradient_stops(&b.background) {
            self.set_picker(Col::GradA, a, window, cx);
            self.set_picker(Col::GradB, c, window, cx);
            self.set_slider(Sl::BGradAngle, angle, window, cx);
        }
        if let Background::Solid { color } = b.background {
            self.set_picker(Col::BgSolid, color, window, cx);
        }
        if let InsetColor::Custom(c) = b.inset.color {
            self.set_picker(Col::Inset, c, window, cx);
        }
        if let Some(w) = &b.watermark {
            self.set_slider(Sl::BWmOpacity, w.opacity, window, cx);
            match &w.content {
                WatermarkContent::Text { style, .. } => {
                    self.set_slider(Sl::BWmSize, style.size, window, cx);
                    self.set_picker(Col::Watermark, style.color, window, cx);
                }
                WatermarkContent::Image { width, .. } => self.set_slider(Sl::BWmWidth, *width, window, cx),
            }
        }
        if let Some(q) = self.jpeg_quality_from(cx) {
            self.jpeg_quality = q;
        }
        self.set_slider(Sl::JpegQuality, self.jpeg_quality as f32, window, cx);
    }

    /// Decode wallpaper / custom background thumbnails in the background.
    pub fn start_thumbnail_loading(&mut self, options: &EditorOptions, window: &mut Window, cx: &mut Context<EditorView>) {
        if let Some(list) = &options.services.list_backgrounds {
            self.custom_bgs = list();
        }
        let mut jobs: Vec<(String, PathBuf)> = rs_core::assets::BUNDLED_WALLPAPERS
            .iter()
            .filter_map(|n| rs_core::assets::wallpaper_path(n).map(|p| (format!("wp:{n}"), p)))
            .collect();
        jobs.extend(self.custom_bgs.iter().map(|c| (format!("custom:{}", c.path.display()), c.path.clone())));
        load_thumbs(jobs, window, cx);
    }
}

/// Global cache so multiple editor windows don't re-decode the 6000×4000 wallpapers.
fn thumb_cache() -> &'static std::sync::Mutex<HashMap<String, Arc<RenderImage>>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<RenderImage>>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn load_thumbs(jobs: Vec<(String, PathBuf)>, window: &mut Window, cx: &mut Context<EditorView>) {
    let task = cx.background_executor().spawn(async move {
        let mut out = Vec::new();
        for (key, path) in jobs {
            if let Some(hit) = thumb_cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
                out.push((key, hit));
                continue;
            }
            let Ok(img) = image::open(&path) else { continue };
            let t = img.thumbnail(180, 120).to_rgba8();
            let (w, h) = (t.width(), t.height());
            let Some(rgba) = rs_core::RgbaImage::new(w, h, t.into_raw()) else { continue };
            let ri = crate::rgba_to_render_image(&rgba);
            if let Ok(mut c) = thumb_cache().lock() {
                c.insert(key.clone(), ri.clone());
            }
            out.push((key, ri));
        }
        out
    });
    cx.spawn_in(window, async move |this: WeakEntity<EditorView>, cx: &mut AsyncWindowContext| {
        let thumbs = task.await;
        let _ = this.update(cx, |this, cx| {
            this.panel.thumbs.extend(thumbs);
            cx.notify();
        });
    })
    .detach();
}

// ----------------------------------------------------------------------
// Event handlers (on the view)
// ----------------------------------------------------------------------

impl EditorView {
    fn on_slider(&mut self, sl: Sl, v: f32, _window: &mut Window, cx: &mut Context<Self>) {
        if sl == Sl::JpegQuality {
            let q = v.round().clamp(10.0, 100.0) as u8;
            self.panel.jpeg_quality = q;
            if cx.try_global::<PreferencesGlobal>().is_some() {
                cx.global_mut::<PreferencesGlobal>().0.export.format = rs_core::ExportFormat::Jpeg { quality: q };
            }
            cx.notify();
            return;
        }
        if sl.is_beautify() {
            let mut b = self.model.doc.beautify.clone();
            match sl {
                Sl::BPadding => b.padding = Padding::uniform(v.round()),
                Sl::BPadTop => b.padding = Padding { top: v.round(), linked: false, ..b.padding },
                Sl::BPadRight => b.padding = Padding { right: v.round(), linked: false, ..b.padding },
                Sl::BPadBottom => b.padding = Padding { bottom: v.round(), linked: false, ..b.padding },
                Sl::BPadLeft => b.padding = Padding { left: v.round(), linked: false, ..b.padding },
                Sl::BInset => b.inset.size = v.round(),
                Sl::BCorner => b.corner_radius = v.round(),
                Sl::BShadowSize => b.shadow.size = v.round(),
                Sl::BShadowOpacity => b.shadow.opacity = v,
                Sl::BShadowY => b.shadow.offset_y = v.round(),
                Sl::BShadowX => b.shadow.offset_x = v.round(),
                Sl::BWmWidth => {
                    if let Some(w) = &mut b.watermark
                        && let WatermarkContent::Image { width, .. } = &mut w.content
                    {
                        *width = v.round();
                    }
                }
                Sl::BDeskBlur | Sl::BDeskBright => {
                    if let Background::DesktopWallpaper { blur, brightness } = &mut b.background {
                        if sl == Sl::BDeskBlur {
                            *blur = v.round();
                        } else {
                            *brightness = v;
                        }
                    }
                }
                Sl::BGradAngle => {
                    if let Background::LinearGradient { angle_deg, .. } = &mut b.background {
                        *angle_deg = v.round();
                    }
                }
                Sl::BWmOpacity => {
                    if let Some(w) = &mut b.watermark {
                        w.opacity = v;
                    }
                }
                Sl::BWmSize => {
                    if let Some(w) = &mut b.watermark
                        && let WatermarkContent::Text { style, .. } = &mut w.content
                    {
                        style.size = v.round();
                    }
                }
                _ => {}
            }
            self.model.set_beautify(b, Some(&format!("{sl:?}")));
        } else {
            let e = match sl {
                Sl::StrokeWidth => StyleEdit::StrokeWidth(v),
                Sl::Opacity => StyleEdit::Opacity(v),
                Sl::CornerRadius => StyleEdit::CornerRadius(v.round()),
                Sl::ShadowBlur => StyleEdit::ShadowBlur(v.round()),
                Sl::HighlighterWidth => StyleEdit::HighlighterWidth(v.round()),
                Sl::StepRadius => StyleEdit::StepRadius(v.round()),
                Sl::SpotDim => StyleEdit::SpotlightDim(v),
                Sl::MagZoom => StyleEdit::MagnifierZoom(v),
                Sl::BlurStrength => StyleEdit::BlurStrength(v.round()),
                Sl::PixelBlock => StyleEdit::PixelBlock(v.round() as u32),
                Sl::CalloutPadding => StyleEdit::CalloutPadding(v.round()),
                Sl::FontSize => StyleEdit::FontSize(v.round()),
                Sl::OutlineWidth => StyleEdit::TextOutlineWidth(v),
                _ => return,
            };
            self.model.apply_style(&e);
        }
        self.changed(cx);
    }

    /// Apply a color and record it as a recent color (swatch clicks,
    /// eyedropper).
    pub(crate) fn set_color(&mut self, col: Col, c: Color, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_color(col, c, window, cx);
        self.push_recent_color(c, cx);
    }

    /// Record `c` as a recent color once the picker has settled (no further
    /// change for 600 ms), so dragging in the picker doesn't flood Recent.
    pub(crate) fn schedule_recent_color(&mut self, c: Color, window: &mut Window, cx: &mut Context<Self>) {
        self.panel.recent_gen = self.panel.recent_gen.wrapping_add(1);
        let generation = self.panel.recent_gen;
        cx.spawn_in(window, async move |this: WeakEntity<EditorView>, cx: &mut AsyncWindowContext| {
            cx.background_executor().timer(std::time::Duration::from_millis(600)).await;
            let _ = this.update(cx, |this, cx| {
                if this.panel.recent_gen == generation {
                    this.push_recent_color(c, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Apply a color to its target without touching the recent colors.
    pub(crate) fn apply_color(&mut self, col: Col, c: Color, window: &mut Window, cx: &mut Context<Self>) {
        let mut b = self.model.doc.beautify.clone();
        match col {
            Col::Primary => self.model.apply_style(&StyleEdit::Color(c)),
            Col::Fill => self.model.apply_style(&StyleEdit::Fill(c)),
            Col::StepText => self.model.apply_style(&StyleEdit::StepTextColor(c)),
            Col::TextBg => self.model.apply_style(&StyleEdit::TextBackgroundColor(c)),
            Col::BgSolid => {
                b.background = Background::Solid { color: c };
                self.model.set_beautify(b, Some("bg_solid"));
            }
            Col::GradA | Col::GradB => {
                match &mut b.background {
                    Background::LinearGradient { stops, .. } | Background::RadialGradient { stops, .. } => {
                        let ix = if col == Col::GradA { 0 } else { stops.len().saturating_sub(1) };
                        if let Some(s) = stops.get_mut(ix) {
                            s.color = c;
                        }
                    }
                    _ => {}
                }
                self.model.set_beautify(b, Some("grad"));
            }
            Col::Inset => {
                b.inset.color = InsetColor::Custom(c);
                self.model.set_beautify(b, Some("inset_color"));
            }
            Col::Watermark => {
                if let Some(w) = &mut b.watermark
                    && let WatermarkContent::Text { style, .. } = &mut w.content
                {
                    style.color = c;
                }
                self.model.set_beautify(b, Some("wm_color"));
            }
        }
        self.panel.set_picker(col, c, window, cx);
        self.changed(cx);
    }

    pub(crate) fn panel_color_picked(&mut self, c: Color, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.panel.eyedropper_target.take().unwrap_or(Col::Primary);
        self.model.break_coalescing();
        self.set_color(target, c, window, cx);
    }

    /// Dominant edge color of the content (cached per image transform).
    fn auto_inset_color(&self) -> Color {
        let t = self.model.doc.image_transform;
        if let Some((ct, c)) = self.panel.inset_cache.get()
            && ct == t
        {
            return c;
        }
        let c = rs_render::content_image(&self.model.doc).map(|i| rs_render::dominant_edge_color(&i)).unwrap_or(Color::WHITE);
        self.panel.inset_cache.set(Some((t, c)));
        c
    }

    fn edit(&mut self, e: StyleEdit, cx: &mut Context<Self>) {
        self.model.break_coalescing();
        self.model.apply_style(&e);
        self.model.break_coalescing();
        self.changed(cx);
    }

    fn beautify(&mut self, f: impl FnOnce(&mut BeautifySettings), cx: &mut Context<Self>) {
        let mut b = self.model.doc.beautify.clone();
        f(&mut b);
        self.model.break_coalescing();
        self.model.set_beautify(b, None);
        self.changed(cx);
    }

    fn save_preset(&mut self, cx: &mut Context<Self>) {
        let (tool, st, _) = self.model.panel_target();
        let name = self.panel.preset_name.read(cx).value().to_string();
        let prefs_count = self.prefs(cx).style_presets.len();
        let name = if name.trim().is_empty() { format!("{} {}", tool.name(), prefs_count + 1) } else { name };
        if cx.try_global::<PreferencesGlobal>().is_some() {
            cx.global_mut::<PreferencesGlobal>().0.style_presets.push(StylePreset {
                id: uuid::Uuid::new_v4(),
                name,
                tool: Some(tool),
                style: st,
            });
        }
        cx.notify();
    }

    fn remove_preset(&mut self, id: uuid::Uuid, cx: &mut Context<Self>) {
        if cx.try_global::<PreferencesGlobal>().is_some() {
            cx.global_mut::<PreferencesGlobal>().0.style_presets.retain(|p| p.id != id);
        }
        cx.notify();
    }

    fn add_custom_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(add) = self.options.services.add_background.clone() else { return };
        cx.activate(true);
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add Background".into()),
        });
        cx.spawn_in(window, async move |this: WeakEntity<EditorView>, cx: &mut AsyncWindowContext| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| match add(&path) {
                Ok(item) => {
                    let key = format!("custom:{}", item.path.display());
                    let p = item.path.clone();
                    this.panel.custom_bgs.push(item);
                    load_thumbs(vec![(key, p.clone())], window, cx);
                    this.beautify(|b| b.background = Background::CustomImage { path: p }, cx);
                }
                Err(e) => this.last_error = Some(format!("Add background failed: {e}")),
            });
        })
        .detach();
    }

    fn remove_custom_background(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(remove) = self.options.services.remove_background.clone() else { return };
        match remove(&id) {
            Ok(()) => self.panel.custom_bgs.retain(|c| c.id != id),
            Err(e) => self.last_error = Some(format!("Remove failed: {e}")),
        }
        cx.notify();
    }
}

// ----------------------------------------------------------------------
// Spec builders (single source of truth for every control's behavior)
// ----------------------------------------------------------------------

fn slider(label: &str, sl: Sl, value: String) -> Row {
    Row::new(label.to_string(), Ctl::Slider { sl, value: value.into() })
}

fn seg(label: &str, id: &str, opts: Vec<Opt>, sel: Option<usize>, on: SegFn) -> Row {
    Row::new(label.to_string(), Ctl::Seg(Seg { id: id.to_string().into(), opts, sel, on }))
}

fn toggle(label: &str, id: &str, on_: bool, on: ToggleFn) -> Row {
    Row::new(label.to_string(), Ctl::Toggle(Tog { id: id.to_string().into(), on_, on }))
}

fn tog(id: &str, on_: bool, on: ToggleFn) -> Tog {
    Tog { id: id.to_string().into(), on_, on }
}

fn color(label: &str, col: Col, cur: Color, eyedropper: bool) -> Row {
    Row::new(label.to_string(), Ctl::Color { col, cur, eyedropper })
}

impl EditorView {
    pub(crate) fn render_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let spec = if self.panel.tab == 0 { self.style_spec(cx) } else { self.beautify_spec(cx) };
        self.render_panel_layout(spec, cx)
    }

    pub(crate) fn set_panel_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.panel.tab = tab.min(1);
        cx.notify();
    }

    pub(crate) fn style_spec(&mut self, cx: &mut Context<Self>) -> Spec {
        let (tool, st, sec) = self.model.panel_target();
        let sel_count = self.model.selection.len();
        let editing = self.model.editing.is_some();
        let mut spec = Spec::default();
        if tool == ToolKind::Select && sel_count == 0 {
            spec.empty = Some((
                "Nothing selected".into(),
                "Select an annotation to edit its style, or pick a tool from the toolbar — every tool has a single-letter shortcut.".into(),
            ));
            return spec;
        }
        let subtitle = if editing {
            "Editing text".to_string()
        } else if sel_count > 1 {
            format!("{sel_count} selected")
        } else if sel_count == 1 {
            "Selected".to_string()
        } else {
            "New shapes".to_string()
        };
        spec.head = Some(Head { title: tool.name().to_string().into(), subtitle: subtitle.into() });
        if tool == ToolKind::Crop {
            spec.secs.push(
                Sec::new("crop", "Crop & rotate", Ico::Crop)
                    .row(Row::new("", Ctl::Note("Drag on the image to set the crop. Enter applies, Esc cancels.".into())))
                    .row(Row::new(
                        "",
                        Ctl::Buttons(vec![
                            Btn::new(
                                "crop-apply",
                                act(|this, _, cx| {
                                    this.model.apply_crop();
                                    this.changed(cx)
                                }),
                            )
                            .label("Apply crop")
                            .primary(),
                            Btn::new(
                                "crop-cancel",
                                act(|this, _, cx| {
                                    this.model.cancel_crop();
                                    this.changed(cx)
                                }),
                            )
                            .label("Cancel"),
                        ]),
                    )),
            );
            return spec;
        }

        // --- Color & stroke -------------------------------------------------
        let mut main = Sec::new("main", "Color & stroke", PIco::Palette);
        if sec.color {
            let label = match tool {
                ToolKind::Text => "Text",
                ToolKind::Step => "Fill",
                ToolKind::Magnifier => "Border",
                _ => "Color",
            };
            main = main.row(color(label, Col::Primary, style_map::primary_color(tool, &st), true));
        }
        if sec.fill {
            let modes = [FillMode::Outline, FillMode::Filled, FillMode::OutlineAndFill];
            main = main.row(seg(
                "Fill",
                "fill-mode",
                vec![Opt::icon("Outline", Ico::Square), Opt::icon("Filled", PIco::SquareFilled), Opt::icon("Outline + fill", PIco::SquareBoth)],
                modes.iter().position(|m| *m == st.fill_mode),
                seg_fn(move |this, i, _, cx| this.edit(StyleEdit::FillMode(modes[i]), cx)),
            ));
            if st.fill_mode.has_fill() {
                main = main.row(color("Fill color", Col::Fill, st.fill, true));
            }
        }
        if sec.stroke_width {
            main = main.row(slider("Width", Sl::StrokeWidth, format!("{:.1}", st.stroke.width)));
        }
        if sec.highlighter {
            main = main.row(slider("Width", Sl::HighlighterWidth, format!("{:.0}", st.highlighter_width)));
        }
        if sec.dash {
            let d = [DashStyle::Solid, DashStyle::Dashed, DashStyle::Dotted];
            main = main.row(
                seg(
                    "Dash",
                    "dash",
                    vec![Opt::icon("Solid", PIco::DashSolid), Opt::icon("Dashed", PIco::DashDashed), Opt::icon("Dotted", PIco::DashDotted)],
                    d.iter().position(|x| *x == st.stroke.dash),
                    seg_fn(move |this, i, _, cx| this.edit(StyleEdit::Dash(d[i]), cx)),
                )
                .adv(),
            );
        }
        if sec.caps {
            let c = [LineCap::Round, LineCap::Square, LineCap::Butt];
            main = main.row(
                seg(
                    "Caps",
                    "caps",
                    vec![Opt::icon("Round", PIco::CapRound), Opt::icon("Square", PIco::CapSquare), Opt::icon("Flat", PIco::CapButt)],
                    c.iter().position(|x| *x == st.stroke.cap),
                    seg_fn(move |this, i, _, cx| this.edit(StyleEdit::Cap(c[i]), cx)),
                )
                .adv(),
            );
        }
        if sec.corner_radius {
            main = main.row(slider("Corners", Sl::CornerRadius, format!("{:.0}", st.corner_radius)).adv());
        }
        main = main.row(slider("Opacity", Sl::Opacity, format!("{:.0}%", st.opacity * 100.0)));
        if !(sec.color || sec.stroke_width || sec.highlighter) {
            main.title = "Appearance".into();
            main.icon = PIco::Sliders.into();
        }

        // --- Tool-specific --------------------------------------------------
        let mut tool_secs: Vec<Sec> = Vec::new();
        if sec.arrow {
            let heads = [ArrowHead::FilledTriangle, ArrowHead::Open, ArrowHead::Round, ArrowHead::None];
            tool_secs.push(
                Sec::new("arrow", "Arrow", Ico::MoveUpRight)
                    .row(seg(
                        "Head",
                        "head",
                        vec![
                            Opt::icon("Filled", PIco::HeadFilled),
                            Opt::icon("Open", PIco::HeadOpen),
                            Opt::icon("Round", PIco::HeadRound),
                            Opt::icon("None", PIco::HeadNone),
                        ],
                        heads.iter().position(|h| *h == st.arrow_head),
                        seg_fn(move |this, i, _, cx| this.edit(StyleEdit::ArrowHead(heads[i]), cx)),
                    ))
                    .row(toggle("Curved", "arrow-curved", st.arrow_curved, tog_fn(|this, v, _, cx| this.edit(StyleEdit::ArrowCurved(v), cx))))
                    .row(
                        toggle("Double-headed", "arrow-double", st.arrow_double, tog_fn(|this, v, _, cx| this.edit(StyleEdit::ArrowDouble(v), cx)))
                            .adv(),
                    )
                    .row(
                        toggle("Tapered", "arrow-tapered", st.arrow_tapered, tog_fn(|this, v, _, cx| this.edit(StyleEdit::ArrowTapered(v), cx)))
                            .adv(),
                    ),
            );
        }
        if sec.step {
            let labels = [StepLabel::Number, StepLabel::UpperLetter, StepLabel::LowerLetter];
            tool_secs.push(
                Sec::new("step", "Step counter", Ico::CircleDot)
                    .row(seg(
                        "Label",
                        "step-label",
                        vec![Opt::text("1 2 3"), Opt::text("A B C"), Opt::text("a b c")],
                        labels.iter().position(|l| *l == st.step_label),
                        seg_fn(move |this, i, _, cx| this.edit(StyleEdit::StepLabel(labels[i]), cx)),
                    ))
                    .row(slider("Size", Sl::StepRadius, format!("{:.0}", st.step_radius)))
                    .row(color("Number", Col::StepText, st.step_text_color, false).adv())
                    .row(
                        Row::new(
                            "",
                            Ctl::Buttons(vec![Btn::new(
                                "renumber",
                                act(|this, _, cx| {
                                    this.model.renumber_steps();
                                    this.changed(cx);
                                }),
                            )
                            .label("Renumber all steps")]),
                        )
                        .adv(),
                    ),
            );
        }
        if sec.spotlight {
            let shapes = [SpotlightShape::Rect, SpotlightShape::Ellipse];
            tool_secs.push(
                Sec::new("spotlight", "Spotlight", Ico::Spotlight)
                    .row(seg(
                        "Shape",
                        "spot-shape",
                        vec![Opt::icon("Rectangle", Ico::Square), Opt::icon("Ellipse", Ico::Circle)],
                        shapes.iter().position(|s| *s == st.spotlight_shape),
                        seg_fn(move |this, i, _, cx| this.edit(StyleEdit::SpotlightShape(shapes[i]), cx)),
                    ))
                    .row(slider("Dim", Sl::SpotDim, format!("{:.0}%", st.spotlight_dim.alpha_f32() * 100.0))),
            );
        }
        if sec.magnifier {
            tool_secs.push(Sec::new("magnifier", "Magnifier", Ico::ZoomIn).row(slider("Zoom", Sl::MagZoom, format!("{:.1}×", st.magnifier_zoom))));
        }
        if sec.blur {
            tool_secs.push(Sec::new("blur", "Blur", Ico::Droplets).row(slider("Strength", Sl::BlurStrength, format!("{:.0}", st.blur_strength))));
        }
        if sec.pixelate {
            tool_secs.push(Sec::new("pixelate", "Pixelate", Ico::Grid3x3).row(slider("Block", Sl::PixelBlock, format!("{}", st.pixelate_block))));
        }
        if sec.text {
            tool_secs.push(self.text_sec(&st));
        }
        if sec.callout {
            tool_secs.push(
                Sec::new("callout", "Bubble", Ico::MessageSquareText).row(slider("Padding", Sl::CalloutPadding, format!("{:.0}", st.callout_padding))),
            );
        }
        // Text is primarily about type: show its section before color.
        if sec.text {
            spec.secs.extend(tool_secs);
            spec.secs.push(main);
        } else {
            spec.secs.push(main);
            spec.secs.extend(tool_secs);
        }

        if sec.shadow {
            let sh = if tool == ToolKind::Text { st.text.shadow } else { st.shadow };
            let mut s = Sec::new("shadow", "Shadow", PIco::Shadow)
                .toggle(tog("shadow", sh.is_some(), tog_fn(|this, v, _, cx| this.edit(StyleEdit::Shadow(v), cx))))
                .adv();
            if let Some(sh) = sh {
                s = s.row(slider("Blur", Sl::ShadowBlur, format!("{:.0}", sh.blur)));
            }
            spec.secs.push(s);
        }
        spec.secs.push(self.presets_sec(tool, cx));
        if sel_count > 0 && !editing {
            spec.footer = Some(self.arrange_sec());
        }
        spec
    }

    fn text_sec(&self, st: &rs_core::ToolStyle) -> Sec {
        let t = &st.text;
        let weights = [FontWeight::LIGHT, FontWeight::REGULAR, FontWeight::MEDIUM, FontWeight::SEMIBOLD, FontWeight::BOLD, FontWeight::BLACK];
        let wnames = ["Light", "Regular", "Medium", "Semibold", "Bold", "Black"];
        let aligns = [TextAlign::Left, TextAlign::Center, TextAlign::Right];
        let bg_kind = style_map::text_bg_kind(&t.background) as usize;
        let italic = t.italic;
        let mut s = Sec::new("text", "Text", Ico::Type)
            .row(
                seg(
                    "Preset",
                    "text-preset",
                    TextPreset::ALL.iter().map(|p| Opt::text(p.name())).collect(),
                    None,
                    seg_fn(|this, i, _, cx| this.edit(StyleEdit::TextPreset(TextPreset::ALL[i]), cx)),
                )
                .adv(),
            )
            .row(Row::new("Font", Ctl::Font))
            .row(slider("Size", Sl::FontSize, format!("{:.0}", t.size)))
            .row(
                Row::new(
                    "Weight",
                    Ctl::SegPlus(
                        Seg {
                            id: "weight".into(),
                            opts: weights.iter().zip(wnames).map(|(w, n)| Opt::vis(n, Vis::Weight(w.0))).collect(),
                            sel: weights.iter().position(|w| *w == t.weight),
                            on: seg_fn(move |this, i, _, cx| this.edit(StyleEdit::FontWeight(weights[i]), cx)),
                        },
                        Btn::new("italic", act(move |this, _, cx| this.edit(StyleEdit::Italic(!italic), cx)))
                            .icon(Ico::Italic)
                            .tip("Italic")
                            .selected(italic),
                    ),
                )
                .adv(),
            )
            .row(seg(
                "Align",
                "align",
                vec![
                    Opt::icon("Align left", Ico::TextAlignStart),
                    Opt::icon("Align center", Ico::TextAlignCenter),
                    Opt::icon("Align right", Ico::TextAlignEnd),
                ],
                aligns.iter().position(|a| *a == t.align),
                seg_fn(move |this, i, _, cx| this.edit(StyleEdit::Align(aligns[i]), cx)),
            ))
            .row(
                seg(
                    "Backdrop",
                    "text-bg",
                    vec![Opt::text("None"), Opt::text("Pill"), Opt::text("Soft"), Opt::text("Outline")],
                    Some(bg_kind),
                    seg_fn(|this, i, _, cx| this.edit(StyleEdit::TextBackgroundKind(i as u8), cx)),
                )
                .adv(),
            );
        if bg_kind != 0 {
            s = s.row(color(if bg_kind == 3 { "Outline" } else { "Backdrop" }, Col::TextBg, style_map::text_bg_color(&t.background), true).adv());
        }
        if bg_kind == 3 {
            let w = if let rs_core::TextBackground::Outline { width, .. } = t.background { width } else { 0.0 };
            s = s.row(slider("Thickness", Sl::OutlineWidth, format!("{w:.1}")).adv());
        }
        s
    }

    fn presets_sec(&self, tool: ToolKind, cx: &mut Context<Self>) -> Sec {
        let presets: Vec<StylePreset> =
            self.prefs(cx).style_presets.into_iter().filter(|p| p.tool.is_none() || p.tool == Some(tool)).collect();
        let chips: Vec<Chip> = presets
            .into_iter()
            .map(|p| {
                let id = p.id;
                let style = p.style.clone();
                Chip {
                    id: format!("preset-{id}").into(),
                    label: p.name.clone().into(),
                    on: act(move |this, _, cx| this.edit(StyleEdit::Preset(Box::new(style.clone())), cx)),
                    on_remove: act(move |this, _, cx| this.remove_preset(id, cx)),
                }
            })
            .collect();
        let mut s = Sec::new("presets", "My presets", PIco::Bookmark).adv();
        if !chips.is_empty() {
            s = s.row(Row::new("", Ctl::Chips(chips)));
        }
        s.row(Row::new(
            "",
            Ctl::InputAction(
                self.panel.preset_name.clone(),
                Btn::new("save-preset", act(|this, _, cx| this.save_preset(cx))).icon(Ico::Plus).label("Save").tip("Save the current style as a preset"),
            ),
        ))
    }

    fn arrange_sec(&self) -> Sec {
        use crate::model::ZOrder;
        let z = |id: &str, ico: crate::panel_icons::AnyIco, tip: &str, o: ZOrder| {
            Btn::new(
                id.to_string(),
                act(move |this, _, cx| {
                    this.model.zorder(o);
                    this.changed(cx)
                }),
            )
            .icon(ico)
            .tip(tip.to_string())
        };
        Sec::new("arrange", "Arrange", PIco::Layers).row(Row::new(
            "",
            Ctl::Buttons(vec![
                z("z-front", Ico::BringToFront.into(), "Bring to front (⌥⌘])", ZOrder::Front),
                z("z-fwd", PIco::ArrowUp.into(), "Bring forward (⌘])", ZOrder::Forward),
                z("z-back1", PIco::ArrowDown.into(), "Send backward (⌘[)", ZOrder::Backward),
                z("z-back", Ico::SendToBack.into(), "Send to back (⌥⌘[)", ZOrder::Back),
                Btn::new(
                    "dup",
                    act(|this, _, cx| {
                        this.model.duplicate_selection();
                        this.changed(cx)
                    }),
                )
                .icon(Ico::Copy)
                .tip("Duplicate (⌘D)"),
                Btn::new(
                    "del",
                    act(|this, _, cx| {
                        this.model.delete_selection();
                        this.changed(cx)
                    }),
                )
                .icon(Ico::Trash)
                .tip("Delete (⌫)"),
            ]),
        ))
    }

    pub(crate) fn beautify_spec(&mut self, cx: &mut Context<Self>) -> Spec {
        let b = self.model.doc.beautify.clone();
        let mut spec = Spec {
            master: Some(tog("beautify-enabled", b.enabled, tog_fn(|this, v, _, cx| this.beautify(|b| b.enabled = v, cx)))),
            ..Default::default()
        };

        // --- Background -----------------------------------------------------
        let kind = match &b.background {
            Background::None => 0,
            Background::Solid { .. } => 1,
            Background::LinearGradient { .. } | Background::RadialGradient { .. } => 2,
            Background::Wallpaper { .. } => 3,
            Background::CustomImage { .. } => 4,
            Background::DesktopWallpaper { .. } => 5,
        };
        let can_add = self.options.services.add_background.is_some();
        let first_custom = self.panel.custom_bgs.first().map(|c| c.path.clone());
        let mut bg = Sec::new("background", "Background", Ico::Image).row(seg(
            "",
            "bg-kind",
            vec![
                Opt::icon("None", PIco::Ban),
                Opt::icon("Solid", PIco::SquareFilled),
                Opt::icon("Gradient", PIco::Gradient),
                Opt::icon("Wallpaper", PIco::Images),
                Opt::icon("Image", Ico::Image),
                Opt::icon("Desktop", PIco::Monitor),
            ],
            Some(kind),
            seg_fn(move |this, i, window, cx| {
                if i == 4 && first_custom.is_none() {
                    // No custom image yet: go straight to the file picker.
                    this.add_custom_background(window, cx);
                    return;
                }
                let fc = first_custom.clone();
                this.beautify(
                    |b| {
                        b.enabled = true;
                        b.background = match i {
                            0 => Background::None,
                            1 => Background::Solid { color: Color::WHITE },
                            2 => rs_core::gradient_presets().remove(0).1,
                            3 => Background::Wallpaper { name: rs_core::assets::BUNDLED_WALLPAPERS[0].into() },
                            4 => match fc {
                                Some(path) => Background::CustomImage { path },
                                None => b.background.clone(),
                            },
                            _ => Background::DesktopWallpaper { blur: 0.0, brightness: 1.0 },
                        }
                    },
                    cx,
                )
            }),
        ));
        match &b.background {
            Background::Solid { color: c } => bg = bg.row(color("Color", Col::BgSolid, *c, true)),
            Background::LinearGradient { .. } | Background::RadialGradient { .. } => {
                let tiles: Vec<Tile> = rs_core::gradient_presets()
                    .into_iter()
                    .enumerate()
                    .map(|(i, (name, g))| {
                        let (a, c, angle) = gradient_stops(&g).unwrap_or((Color::WHITE, Color::BLACK, 0.0));
                        let selected = std::mem::discriminant(&g) == std::mem::discriminant(&b.background)
                            && gradient_stops(&b.background).map(|x| (x.0, x.1)) == Some((a, c));
                        Tile {
                            id: format!("grad-{i}").into(),
                            tip: name.into(),
                            vis: TileVis::Gradient(a, c, angle),
                            selected,
                            on: act(move |this, _, cx| {
                                let g = g.clone();
                                this.beautify(|b| b.background = g, cx)
                            }),
                            on_remove: None,
                        }
                    })
                    .collect();
                let (a, c, angle) = gradient_stops(&b.background).unwrap_or((Color::WHITE, Color::BLACK, 0.0));
                bg = bg
                    .row(Row::new("", Ctl::Tiles(tiles)))
                    .row(color("From", Col::GradA, a, true).adv())
                    .row(color("To", Col::GradB, c, true).adv());
                if matches!(b.background, Background::LinearGradient { .. }) {
                    bg = bg.row(slider("Angle", Sl::BGradAngle, format!("{angle:.0}°")).adv());
                }
            }
            Background::Wallpaper { name: cur } => {
                let tiles = rs_core::assets::BUNDLED_WALLPAPERS
                    .iter()
                    .map(|name| Tile {
                        id: format!("wp-{name}").into(),
                        tip: name.replace("wallpaper-", "Wallpaper ").into(),
                        vis: TileVis::Image(self.panel.thumbs.get(&format!("wp:{name}")).cloned()),
                        selected: cur == name,
                        on: act(move |this, _, cx| this.beautify(|b| b.background = Background::Wallpaper { name: name.to_string() }, cx)),
                        on_remove: None,
                    })
                    .collect();
                bg = bg.row(Row::new("", Ctl::Tiles(tiles)));
            }
            Background::CustomImage { path: cur } => {
                let can_remove = self.options.services.remove_background.is_some();
                let mut tiles: Vec<Tile> = self
                    .panel
                    .custom_bgs
                    .clone()
                    .into_iter()
                    .map(|item| {
                        let path = item.path.clone();
                        let id = item.id.clone();
                        Tile {
                            id: format!("cbg-{}", item.id).into(),
                            tip: item.name.clone().into(),
                            vis: TileVis::Image(self.panel.thumbs.get(&format!("custom:{}", item.path.display())).cloned()),
                            selected: *cur == item.path,
                            on: act(move |this, _, cx| {
                                let p = path.clone();
                                this.beautify(|b| b.background = Background::CustomImage { path: p }, cx)
                            }),
                            on_remove: can_remove.then(|| act(move |this, _, cx| this.remove_custom_background(id.clone(), cx))),
                        }
                    })
                    .collect();
                if can_add {
                    tiles.push(Tile {
                        id: "add-bg".into(),
                        tip: "Add image…".into(),
                        vis: TileVis::Add,
                        selected: false,
                        on: act(|this, window, cx| this.add_custom_background(window, cx)),
                        on_remove: None,
                    });
                }
                bg = bg.row(Row::new("", Ctl::Tiles(tiles)));
            }
            Background::DesktopWallpaper { blur, brightness } => {
                bg = bg
                    .row(slider("Blur", Sl::BDeskBlur, format!("{blur:.0}")))
                    .row(slider("Brightness", Sl::BDeskBright, format!("{:.0}%", brightness * 100.0)));
            }
            Background::None => {}
        }
        spec.secs.push(bg);

        // --- Padding --------------------------------------------------------
        let per_side = self.panel.per_side;
        let mut pad = Sec::new("padding", "Padding & corners", PIco::Padding).row(
            toggle("Auto-balance", "auto-balance", b.auto_balance, tog_fn(|this, v, _, cx| this.beautify(|b| b.auto_balance = v, cx))).adv(),
        );
        if !b.auto_balance {
            pad = pad.row(
                toggle(
                    "Per side",
                    "per-side",
                    per_side,
                    tog_fn(|this, v, _, cx| {
                        this.panel.per_side = v;
                        if !v {
                            let t = this.model.doc.beautify.padding.top;
                            this.beautify(|b| b.padding = Padding::uniform(t), cx);
                        }
                        cx.notify();
                    }),
                )
                .adv(),
            );
            if per_side {
                pad = pad.rows([
                    slider("Top", Sl::BPadTop, format!("{:.0}", b.padding.top)),
                    slider("Right", Sl::BPadRight, format!("{:.0}", b.padding.right)),
                    slider("Bottom", Sl::BPadBottom, format!("{:.0}", b.padding.bottom)),
                    slider("Left", Sl::BPadLeft, format!("{:.0}", b.padding.left)),
                ]);
            } else {
                pad = pad.row(slider("Padding", Sl::BPadding, format!("{:.0}", b.padding.top)));
            }
        }
        pad = pad.row(slider("Corners", Sl::BCorner, format!("{:.0}", b.corner_radius)));
        spec.secs.push(pad);

        // --- Shadow ---------------------------------------------------------
        let mut sh = Sec::new("bshadow", "Shadow", PIco::Shadow)
            .toggle(tog("shadow-on", b.shadow.enabled, tog_fn(|this, v, _, cx| this.beautify(|b| b.shadow.enabled = v, cx))));
        if b.shadow.enabled {
            sh = sh.rows([
                slider("Size", Sl::BShadowSize, format!("{:.0}", b.shadow.size)),
                slider("Opacity", Sl::BShadowOpacity, format!("{:.0}%", b.shadow.opacity * 100.0)),
                slider("Offset X", Sl::BShadowX, format!("{:.0}", b.shadow.offset_x)).adv(),
                slider("Offset Y", Sl::BShadowY, format!("{:.0}", b.shadow.offset_y)).adv(),
            ]);
        }
        spec.secs.push(sh);

        // --- Aspect ratio ---------------------------------------------------
        spec.secs.push(Sec::new("aspect", "Aspect ratio", PIco::Ratio).row(seg(
            "",
            "aspect",
            AspectRatio::ALL
                .iter()
                .map(|a| {
                    // Text-only chips; the tooltip carries the exact ratio.
                    let (label, tip) = match a {
                        AspectRatio::Free => ("Free", "Free · no fixed ratio".to_string()),
                        AspectRatio::Twitter => ("X", "X (Twitter) · 16:9".to_string()),
                        AspectRatio::LinkedIn => ("LinkedIn", "LinkedIn · 1.91:1".to_string()),
                        AspectRatio::YouTube => ("YouTube", "YouTube · 16:9".to_string()),
                        AspectRatio::Instagram => ("Instagram", "Instagram portrait · 4:5".to_string()),
                        a => (a.label(), a.label().to_string()),
                    };
                    Opt::text(label).tip(tip)
                })
                .collect(),
            AspectRatio::ALL.iter().position(|a| *a == b.aspect_ratio),
            seg_fn(|this, i, _, cx| this.beautify(|b| b.aspect_ratio = AspectRatio::ALL[i], cx)),
        )));

        // --- Edge inset -----------------------------------------------------
        let auto_inset = matches!(b.inset.color, InsetColor::Auto);
        let inset_color = match b.inset.color {
            InsetColor::Custom(c) => c,
            InsetColor::Auto => self.auto_inset_color(),
        };
        let mut inset = Sec::new("inset", "Edge-color inset", PIco::Frame)
            .toggle(tog("inset", b.inset.enabled, tog_fn(|this, v, _, cx| this.beautify(|b| b.inset.enabled = v, cx))))
            .adv();
        if b.inset.enabled {
            inset = inset.row(slider("Inset", Sl::BInset, format!("{:.0}", b.inset.size))).row(toggle(
                "Auto edge color",
                "inset-auto",
                auto_inset,
                tog_fn(move |this, v, _, cx| this.beautify(|b| b.inset.color = if v { InsetColor::Auto } else { InsetColor::Custom(inset_color) }, cx)),
            ));
            if !auto_inset {
                inset = inset.row(color("Color", Col::Inset, inset_color, true));
            }
        }
        spec.secs.push(inset);

        // --- Window frame ---------------------------------------------------
        let (ck, dark) = match &b.chrome {
            WindowChrome::None => (0, false),
            WindowChrome::MacOs { dark, .. } => (1, *dark),
            WindowChrome::Browser { dark, .. } => (2, *dark),
        };
        let mut chrome = Sec::new("chrome", "Window frame", PIco::AppWindow)
            .row(seg(
                "",
                "chrome",
                vec![Opt::icon("None", PIco::Ban), Opt::icon("macOS", PIco::AppWindow), Opt::icon("Browser", PIco::Globe)],
                Some(ck),
                seg_fn(move |this, i, _, cx| {
                    this.beautify(
                        |b| {
                            b.chrome = match i {
                                1 => WindowChrome::MacOs { dark, title: None },
                                2 => WindowChrome::Browser { dark, url: "https://".into() },
                                _ => WindowChrome::None,
                            }
                        },
                        cx,
                    )
                }),
            ))
            .adv();
        if ck != 0 {
            chrome = chrome
                .row(toggle(
                    "Dark",
                    "chrome-dark",
                    dark,
                    tog_fn(|this, v, _, cx| {
                        this.beautify(
                            |b| match &mut b.chrome {
                                WindowChrome::MacOs { dark, .. } | WindowChrome::Browser { dark, .. } => *dark = v,
                                WindowChrome::None => {}
                            },
                            cx,
                        )
                    }),
                ))
                .row(Row::new(if ck == 1 { "Title" } else { "URL" }, Ctl::Input(self.panel.chrome_text.clone())));
        }
        spec.secs.push(chrome);

        // --- Watermark ------------------------------------------------------
        let wm = b.watermark.clone();
        let mut wsec = Sec::new("watermark", "Watermark", PIco::Stamp)
            .toggle(tog(
                "wm-on",
                wm.is_some(),
                tog_fn(|this, v, window, cx| {
                    let text = this.panel.watermark_text.read(cx).value().to_string();
                    let text = if text.is_empty() { "@rapidsnap".to_string() } else { text };
                    if v && this.panel.watermark_text.read(cx).value().is_empty() {
                        let t = text.clone();
                        this.panel.watermark_text.update(cx, |s, cx| s.set_value(t, window, cx));
                    }
                    this.beautify(
                        |b| {
                            b.watermark = v.then(|| Watermark {
                                content: WatermarkContent::Text {
                                    text,
                                    style: rs_core::TextStyle { size: 14.0, color: Color::WHITE, weight: FontWeight::SEMIBOLD, ..Default::default() },
                                },
                                position: Corner::BottomRight,
                                opacity: 0.8,
                                margin: 16.0,
                            })
                        },
                        cx,
                    )
                }),
            ))
            .adv();
        if let Some(w) = &wm {
            let corners = [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];
            let is_image = matches!(w.content, WatermarkContent::Image { .. });
            wsec = wsec.row(seg(
                "Type",
                "wm-kind",
                vec![Opt::text("Text"), Opt::text("Image")],
                Some(is_image as usize),
                seg_fn(|this, i, window, cx| {
                    if i == 1 {
                        this.choose_watermark_image(window, cx);
                    } else {
                        let text = this.panel.watermark_text.read(cx).value().to_string();
                        let text = if text.is_empty() { "@rapidsnap".to_string() } else { text };
                        this.beautify(
                            |b| {
                                if let Some(w) = &mut b.watermark
                                    && matches!(w.content, WatermarkContent::Image { .. })
                                {
                                    w.content = WatermarkContent::Text {
                                        text,
                                        style: rs_core::TextStyle { size: 14.0, color: Color::WHITE, weight: FontWeight::SEMIBOLD, ..Default::default() },
                                    };
                                }
                            },
                            cx,
                        )
                    }
                }),
            ));
            match &w.content {
                WatermarkContent::Text { .. } => {
                    wsec = wsec.row(Row::new("Text", Ctl::Input(self.panel.watermark_text.clone())));
                }
                WatermarkContent::Image { path, .. } => {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    wsec = wsec.row(Row::new(
                        "Image",
                        Ctl::Buttons(vec![
                            Btn::new("wm-image", act(|this, window, cx| this.choose_watermark_image(window, cx)))
                                .icon(Ico::Image)
                                .label(if name.chars().count() > 16 { format!("{}…", name.chars().take(15).collect::<String>()) } else { name })
                                .tip("Choose another image"),
                        ]),
                    ));
                }
            }
            wsec = wsec
                .row(seg(
                    "Corner",
                    "wm-corner",
                    vec![
                        Opt::icon("Top left", PIco::CornerTL),
                        Opt::icon("Top right", PIco::CornerTR),
                        Opt::icon("Bottom left", PIco::CornerBL),
                        Opt::icon("Bottom right", PIco::CornerBR),
                    ],
                    corners.iter().position(|c| *c == w.position),
                    seg_fn(move |this, i, _, cx| {
                        this.beautify(
                            |b| {
                                if let Some(w) = &mut b.watermark {
                                    w.position = corners[i];
                                }
                            },
                            cx,
                        )
                    }),
                ))
                .row(slider("Opacity", Sl::BWmOpacity, format!("{:.0}%", w.opacity * 100.0)));
            match &w.content {
                WatermarkContent::Text { style, .. } => {
                    wsec = wsec
                        .row(slider("Size", Sl::BWmSize, format!("{:.0}", style.size)))
                        .row(color("Color", Col::Watermark, style.color, false));
                }
                WatermarkContent::Image { width, .. } => {
                    wsec = wsec.row(slider("Width", Sl::BWmWidth, format!("{width:.0}")));
                }
            }
        }
        spec.secs.push(wsec);

        // --- Crop & rotate --------------------------------------------------
        let cropped = self.model.doc.image_transform.crop.is_some();
        let mut btns = vec![
            Btn::new("crop", act(|this, window, cx| this.set_tool(ToolKind::Crop, window, cx))).icon(Ico::Crop).label("Crop").tip("Crop (K)"),
            Btn::new(
                "rot-ccw",
                act(|this, _, cx| {
                    this.model.rotate(false);
                    this.changed(cx)
                }),
            )
            .icon(Ico::RotateCcw)
            .tip("Rotate left"),
            Btn::new(
                "rot-cw",
                act(|this, _, cx| {
                    this.model.rotate(true);
                    this.changed(cx)
                }),
            )
            .icon(Ico::RotateCw)
            .tip("Rotate right"),
        ];
        if cropped {
            btns.push(
                Btn::new(
                    "crop-reset",
                    act(|this, _, cx| {
                        this.model.reset_crop();
                        this.changed(cx)
                    }),
                )
                .label("Reset")
                .tip("Reset crop"),
            );
        }
        spec.secs.push(Sec::new("crop", "Crop & rotate", Ico::Crop).row(Row::new("", Ctl::Buttons(btns))).adv());

        // --- Export defaults ------------------------------------------------
        let prefs = self.prefs(cx);
        let cur_fmt = match prefs.export.format {
            rs_core::ExportFormat::Png => 0,
            rs_core::ExportFormat::Jpeg { .. } => 1,
            rs_core::ExportFormat::WebP => 2,
        };
        let scales = [rs_core::ExportScale::X1, rs_core::ExportScale::X2];
        let mut export = Sec::new("export", "Export", PIco::Share)
                .row(seg(
                    "Format",
                    "fmt",
                    vec![Opt::text("PNG"), Opt::text("JPEG"), Opt::text("WebP")],
                    Some(cur_fmt),
                    seg_fn(move |this, i, _, cx| {
                        let format = match i {
                            1 => rs_core::ExportFormat::Jpeg { quality: this.panel.jpeg_quality },
                            2 => rs_core::ExportFormat::WebP,
                            _ => rs_core::ExportFormat::Png,
                        };
                        if cx.try_global::<PreferencesGlobal>().is_some() {
                            cx.global_mut::<PreferencesGlobal>().0.export.format = format;
                        }
                        cx.notify();
                    }),
                ));
        if let rs_core::ExportFormat::Jpeg { quality } = prefs.export.format {
            export = export.row(slider("Quality", Sl::JpegQuality, format!("{quality}")));
        }
        spec.secs.push(
            export
                .row(seg(
                    "Scale",
                    "scale",
                    vec![Opt::text("1×"), Opt::text("2×")],
                    scales.iter().position(|s| *s == prefs.export.scale),
                    seg_fn(move |_, i, _, cx| {
                        if cx.try_global::<PreferencesGlobal>().is_some() {
                            cx.global_mut::<PreferencesGlobal>().0.export.scale = scales[i];
                        }
                        cx.notify();
                    }),
                ))
                .adv(),
        );
        spec
    }

    /// Pick an image file for the watermark (keeps corner / opacity).
    fn choose_watermark_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.activate(true);
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose Watermark".into()),
        });
        cx.spawn_in(window, async move |this: WeakEntity<EditorView>, cx: &mut AsyncWindowContext| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.beautify(
                    |b| {
                        let content = WatermarkContent::Image { path, width: 120.0 };
                        match &mut b.watermark {
                            Some(w) => w.content = content,
                            None => {
                                b.watermark =
                                    Some(Watermark { content, position: Corner::BottomRight, opacity: 0.8, margin: 16.0 })
                            }
                        }
                    },
                    cx,
                )
            });
        })
        .detach();
    }
}

