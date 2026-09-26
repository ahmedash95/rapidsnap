//! Mapping between the flat [`ToolStyle`] (pt, per-tool memory) and
//! annotation styles (content px). The style panel emits [`StyleEdit`]s which
//! are applied to the remembered tool style and/or to selected annotations.

use rs_core::{
    Annotation, AnnotationKind, ArrowHead, Color, DashStyle, FillMode, FontFamily, FontWeight, LineCap, Shadow,
    SpotlightShape, StepLabel, TextAlign, TextBackground, TextPreset, TextStyle, ToolKind, ToolStyle,
};

/// One style change from the panel. Lengths in **pt**.
#[derive(Debug, Clone, PartialEq)]
pub enum StyleEdit {
    /// The primary color of the object (stroke / text / step fill / marker…).
    Color(Color),
    Fill(Color),
    FillMode(FillMode),
    StrokeWidth(f32),
    Dash(DashStyle),
    Cap(LineCap),
    Opacity(f32),
    CornerRadius(f32),
    Shadow(bool),
    ShadowBlur(f32),
    ArrowHead(ArrowHead),
    ArrowDouble(bool),
    ArrowTapered(bool),
    ArrowCurved(bool),
    HighlighterWidth(f32),
    StepLabel(StepLabel),
    StepRadius(f32),
    StepTextColor(Color),
    SpotlightShape(SpotlightShape),
    SpotlightDim(f32),
    MagnifierZoom(f32),
    BlurStrength(f32),
    PixelBlock(u32),
    CalloutPadding(f32),
    FontFamily(FontFamily),
    FontSize(f32),
    FontWeight(FontWeight),
    Italic(bool),
    Align(TextAlign),
    /// Background kind: 0 none, 1 pill, 2 semi-transparent, 3 outline.
    TextBackgroundKind(u8),
    TextBackgroundColor(Color),
    TextOutlineWidth(f32),
    TextShadow(bool),
    TextPreset(TextPreset),
    /// Replace the whole style (apply a user preset).
    Preset(Box<ToolStyle>),
}

impl StyleEdit {
    /// Coalescing key: consecutive edits of the same kind merge into one
    /// undo step.
    pub fn key(&self) -> &'static str {
        match self {
            StyleEdit::Color(_) => "color",
            StyleEdit::Fill(_) => "fill",
            StyleEdit::FillMode(_) => "fill_mode",
            StyleEdit::StrokeWidth(_) => "stroke_width",
            StyleEdit::Dash(_) => "dash",
            StyleEdit::Cap(_) => "cap",
            StyleEdit::Opacity(_) => "opacity",
            StyleEdit::CornerRadius(_) => "corner_radius",
            StyleEdit::Shadow(_) => "shadow",
            StyleEdit::ShadowBlur(_) => "shadow_blur",
            StyleEdit::ArrowHead(_) => "arrow_head",
            StyleEdit::ArrowDouble(_) => "arrow_double",
            StyleEdit::ArrowTapered(_) => "arrow_tapered",
            StyleEdit::ArrowCurved(_) => "arrow_curved",
            StyleEdit::HighlighterWidth(_) => "hl_width",
            StyleEdit::StepLabel(_) => "step_label",
            StyleEdit::StepRadius(_) => "step_radius",
            StyleEdit::StepTextColor(_) => "step_text",
            StyleEdit::SpotlightShape(_) => "spot_shape",
            StyleEdit::SpotlightDim(_) => "spot_dim",
            StyleEdit::MagnifierZoom(_) => "mag_zoom",
            StyleEdit::BlurStrength(_) => "blur",
            StyleEdit::PixelBlock(_) => "pixel",
            StyleEdit::CalloutPadding(_) => "callout_padding",
            StyleEdit::FontFamily(_) => "font_family",
            StyleEdit::FontSize(_) => "font_size",
            StyleEdit::FontWeight(_) => "font_weight",
            StyleEdit::Italic(_) => "italic",
            StyleEdit::Align(_) => "align",
            StyleEdit::TextBackgroundKind(_) => "text_bg",
            StyleEdit::TextBackgroundColor(_) => "text_bg_color",
            StyleEdit::TextOutlineWidth(_) => "text_outline",
            StyleEdit::TextShadow(_) => "text_shadow",
            StyleEdit::TextPreset(_) => "text_preset",
            StyleEdit::Preset(_) => "preset",
        }
    }

    /// The color carried by this edit (for "recent colors").
    pub fn color(&self) -> Option<Color> {
        match self {
            StyleEdit::Color(c) | StyleEdit::Fill(c) | StyleEdit::StepTextColor(c) | StyleEdit::TextBackgroundColor(c) => {
                Some(*c)
            }
            _ => None,
        }
    }
}

/// Primary color of a tool style for `tool`.
pub fn primary_color(tool: ToolKind, s: &ToolStyle) -> Color {
    match tool {
        ToolKind::Text => s.text.color,
        ToolKind::Highlighter => s.highlighter_color,
        ToolKind::Step => s.fill,
        ToolKind::Redact => s.redact_color,
        ToolKind::Spotlight => s.spotlight_dim.with_alpha(1.0),
        _ => s.stroke.color,
    }
}

/// Default "background" for a text background kind (pt).
fn text_bg_for_kind(kind: u8, color: Color, width: f32) -> TextBackground {
    match kind {
        1 => TextBackground::Pill { color: color.with_alpha(1.0), corner_radius: 999.0, padding_x: 10.0, padding_y: 4.0 },
        2 => TextBackground::SemiTransparent {
            color: color.with_alpha(0.6),
            corner_radius: 8.0,
            padding_x: 10.0,
            padding_y: 8.0,
        },
        3 => TextBackground::Outline { width: width.max(1.0), color },
        _ => TextBackground::None,
    }
}

/// Kind index of a text background (see [`StyleEdit::TextBackgroundKind`]).
pub fn text_bg_kind(b: &TextBackground) -> u8 {
    match b {
        TextBackground::None => 0,
        TextBackground::Pill { .. } => 1,
        TextBackground::SemiTransparent { .. } => 2,
        TextBackground::Outline { .. } => 3,
    }
}

/// Color of a text background (black if none).
pub fn text_bg_color(b: &TextBackground) -> Color {
    match b {
        TextBackground::None => Color::BLACK,
        TextBackground::Pill { color, .. } | TextBackground::SemiTransparent { color, .. } => *color,
        TextBackground::Outline { color, .. } => *color,
    }
}

fn edit_text_style(t: &mut TextStyle, e: &StyleEdit, scale: f32) {
    match e {
        StyleEdit::Color(c) => t.color = *c,
        StyleEdit::FontFamily(f) => t.family = f.clone(),
        StyleEdit::FontSize(v) => t.size = v * scale,
        StyleEdit::FontWeight(w) => t.weight = *w,
        StyleEdit::Italic(i) => t.italic = *i,
        StyleEdit::Align(a) => t.align = *a,
        StyleEdit::TextBackgroundKind(k) => {
            let bg = text_bg_for_kind(*k, text_bg_color(&t.background), 3.0);
            t.background = TextStyle { background: bg, ..TextStyle::default() }.scaled(scale).background;
        }
        StyleEdit::TextBackgroundColor(c) => match &mut t.background {
            TextBackground::None => {}
            TextBackground::Pill { color, .. } => *color = *c,
            TextBackground::SemiTransparent { color, .. } => *color = c.with_alpha(color.alpha_f32()),
            TextBackground::Outline { color, .. } => *color = *c,
        },
        StyleEdit::TextOutlineWidth(w) => {
            if let TextBackground::Outline { width, .. } = &mut t.background {
                *width = w * scale;
            }
        }
        StyleEdit::TextShadow(on) => t.shadow = on.then(|| Shadow::default().scaled(scale)),
        StyleEdit::TextPreset(p) => *t = p.style().scaled(scale),
        _ => {}
    }
}

/// Apply an edit to a remembered tool style (pt).
pub fn apply_to_tool_style(tool: ToolKind, s: &mut ToolStyle, e: &StyleEdit) {
    match e {
        StyleEdit::Preset(p) => *s = (**p).clone(),
        StyleEdit::Color(c) => match tool {
            ToolKind::Text => s.text.color = *c,
            ToolKind::Highlighter => s.highlighter_color = c.with_alpha(s.highlighter_color.alpha_f32()),
            ToolKind::Step => s.fill = *c,
            ToolKind::Redact => s.redact_color = *c,
            ToolKind::Spotlight => s.spotlight_dim = c.with_alpha(s.spotlight_dim.alpha_f32()),
            _ => s.stroke.color = *c,
        },
        StyleEdit::Fill(c) => s.fill = *c,
        StyleEdit::FillMode(m) => s.fill_mode = *m,
        StyleEdit::StrokeWidth(w) => s.stroke.width = *w,
        StyleEdit::Dash(d) => s.stroke.dash = *d,
        StyleEdit::Cap(c) => s.stroke.cap = *c,
        StyleEdit::Opacity(o) => s.opacity = o.clamp(0.0, 1.0),
        StyleEdit::CornerRadius(r) => s.corner_radius = *r,
        StyleEdit::Shadow(on) => s.shadow = on.then(Shadow::default),
        StyleEdit::ShadowBlur(b) => {
            if let Some(sh) = &mut s.shadow {
                sh.blur = *b;
                sh.offset_y = (b / 4.0).max(1.0);
            }
        }
        StyleEdit::ArrowHead(h) => s.arrow_head = *h,
        StyleEdit::ArrowDouble(b) => s.arrow_double = *b,
        StyleEdit::ArrowTapered(b) => s.arrow_tapered = *b,
        StyleEdit::ArrowCurved(b) => s.arrow_curved = *b,
        StyleEdit::HighlighterWidth(w) => s.highlighter_width = *w,
        StyleEdit::StepLabel(l) => s.step_label = *l,
        StyleEdit::StepRadius(r) => s.step_radius = *r,
        StyleEdit::StepTextColor(c) => s.step_text_color = *c,
        StyleEdit::SpotlightShape(sh) => s.spotlight_shape = *sh,
        StyleEdit::SpotlightDim(a) => s.spotlight_dim = s.spotlight_dim.with_alpha(*a),
        StyleEdit::MagnifierZoom(z) => s.magnifier_zoom = z.clamp(2.0, 4.0),
        StyleEdit::BlurStrength(b) => s.blur_strength = *b,
        StyleEdit::PixelBlock(b) => s.pixelate_block = (*b).max(1),
        StyleEdit::CalloutPadding(p) => s.callout_padding = *p,
        _ => edit_text_style(&mut s.text, e, 1.0),
    }
}

/// Apply an edit to an annotation (content px; `sf` = doc scale factor).
/// Returns `true` if the annotation changed.
pub fn apply_to_annotation(a: &mut Annotation, e: &StyleEdit, sf: f32) -> bool {
    let before = a.clone();
    if let StyleEdit::Preset(p) = e {
        // Rebuild the annotation's style from the preset while keeping geometry.
        let tool = a.tool();
        let px = p.to_content_px(sf);
        let edits = [
            StyleEdit::Color(primary_color(tool, p)),
            StyleEdit::Fill(p.fill),
            StyleEdit::FillMode(p.fill_mode),
            StyleEdit::StrokeWidth(p.stroke.width),
            StyleEdit::Dash(p.stroke.dash),
            StyleEdit::Cap(p.stroke.cap),
            StyleEdit::Opacity(p.opacity),
            StyleEdit::CornerRadius(p.corner_radius),
            StyleEdit::ArrowHead(p.arrow_head),
            StyleEdit::ArrowDouble(p.arrow_double),
            StyleEdit::ArrowTapered(p.arrow_tapered),
            StyleEdit::HighlighterWidth(p.highlighter_width),
            StyleEdit::StepLabel(p.step_label),
            StyleEdit::StepTextColor(p.step_text_color),
            StyleEdit::BlurStrength(p.blur_strength),
            StyleEdit::MagnifierZoom(p.magnifier_zoom),
        ];
        for e in &edits {
            apply_to_annotation(a, e, sf);
        }
        a.shadow = px.shadow;
        match &mut a.kind {
            AnnotationKind::Text(t) => t.style = px.text.clone(),
            AnnotationKind::Callout(c) => c.text_style = px.text.clone(),
            AnnotationKind::Pixelate(x) => x.block_size = px.pixelate_block,
            _ => {}
        }
        return *a != before;
    }
    match e {
        StyleEdit::Opacity(o) => a.opacity = o.clamp(0.0, 1.0),
        StyleEdit::Shadow(on) => {
            if let AnnotationKind::Text(t) = &mut a.kind {
                t.style.shadow = on.then(|| Shadow::default().scaled(sf));
            } else {
                a.shadow = on.then(|| Shadow::default().scaled(sf));
            }
        }
        StyleEdit::ShadowBlur(b) => {
            let target = match &mut a.kind {
                AnnotationKind::Text(t) => &mut t.style.shadow,
                _ => &mut a.shadow,
            };
            if let Some(sh) = target {
                sh.blur = b * sf;
                sh.offset_y = (b / 4.0).max(1.0) * sf;
            }
        }
        _ => {}
    }
    match &mut a.kind {
        AnnotationKind::Rect(r) => match e {
            StyleEdit::Color(c) => r.style.stroke.color = *c,
            StyleEdit::Fill(c) => r.style.fill = *c,
            StyleEdit::FillMode(m) => r.style.mode = *m,
            StyleEdit::StrokeWidth(w) => r.style.stroke.width = w * sf,
            StyleEdit::Dash(d) => r.style.stroke.dash = *d,
            StyleEdit::CornerRadius(v) => r.corner_radius = v * sf,
            _ => {}
        },
        AnnotationKind::Ellipse(r) => match e {
            StyleEdit::Color(c) => r.style.stroke.color = *c,
            StyleEdit::Fill(c) => r.style.fill = *c,
            StyleEdit::FillMode(m) => r.style.mode = *m,
            StyleEdit::StrokeWidth(w) => r.style.stroke.width = w * sf,
            StyleEdit::Dash(d) => r.style.stroke.dash = *d,
            _ => {}
        },
        AnnotationKind::Line(l) => match e {
            StyleEdit::Color(c) => l.stroke.color = *c,
            StyleEdit::StrokeWidth(w) => l.stroke.width = w * sf,
            StyleEdit::Dash(d) => l.stroke.dash = *d,
            StyleEdit::Cap(c) => l.stroke.cap = *c,
            _ => {}
        },
        AnnotationKind::Arrow(ar) => match e {
            StyleEdit::Color(c) => ar.stroke.color = *c,
            StyleEdit::StrokeWidth(w) => ar.stroke.width = w * sf,
            StyleEdit::Dash(d) => ar.stroke.dash = *d,
            StyleEdit::Cap(c) => ar.stroke.cap = *c,
            StyleEdit::ArrowHead(h) => ar.head = *h,
            StyleEdit::ArrowDouble(b) => ar.double_headed = *b,
            StyleEdit::ArrowTapered(b) => ar.tapered = *b,
            StyleEdit::ArrowCurved(b) => {
                if *b && ar.control.is_none() {
                    let mid = ar.start.lerp(ar.end, 0.5);
                    let d = ar.end - ar.start;
                    ar.control = Some(mid + rs_core::Point::new(-d.y, d.x) * 0.2);
                } else if !*b {
                    ar.control = None;
                }
            }
            _ => {}
        },
        AnnotationKind::Pen(p) => match e {
            StyleEdit::Color(c) => p.stroke.color = *c,
            StyleEdit::StrokeWidth(w) => p.stroke.width = w * sf,
            StyleEdit::Dash(d) => p.stroke.dash = *d,
            StyleEdit::Cap(c) => p.stroke.cap = *c,
            _ => {}
        },
        AnnotationKind::Highlighter(h) => match e {
            StyleEdit::Color(c) => h.color = c.with_alpha(h.color.alpha_f32()),
            StyleEdit::HighlighterWidth(w) | StyleEdit::StrokeWidth(w) => h.width = w * sf,
            _ => {}
        },
        AnnotationKind::Step(s) => match e {
            StyleEdit::Color(c) => s.fill = *c,
            StyleEdit::StepLabel(l) => s.label = *l,
            StyleEdit::StepRadius(r) => s.radius = r * sf,
            StyleEdit::StepTextColor(c) => s.text_color = *c,
            StyleEdit::FontFamily(f) => s.font_family = f.clone(),
            StyleEdit::FontWeight(w) => s.font_weight = *w,
            _ => {}
        },
        AnnotationKind::Callout(c) => match e {
            StyleEdit::Color(col) => c.style.stroke.color = *col,
            StyleEdit::Fill(col) => c.style.fill = *col,
            StyleEdit::FillMode(m) => c.style.mode = *m,
            StyleEdit::StrokeWidth(w) => c.style.stroke.width = w * sf,
            StyleEdit::Dash(d) => c.style.stroke.dash = *d,
            StyleEdit::CornerRadius(v) => c.corner_radius = v * sf,
            StyleEdit::CalloutPadding(p) => c.padding = p * sf,
            StyleEdit::FontFamily(_)
            | StyleEdit::FontSize(_)
            | StyleEdit::FontWeight(_)
            | StyleEdit::Italic(_)
            | StyleEdit::Align(_)
            | StyleEdit::TextPreset(_) => edit_text_style(&mut c.text_style, e, sf),
            StyleEdit::StepTextColor(col) => c.text_style.color = *col,
            _ => {}
        },
        AnnotationKind::Spotlight(s) => match e {
            StyleEdit::Color(c) => s.dim = c.with_alpha(s.dim.alpha_f32()),
            StyleEdit::SpotlightShape(sh) => s.shape = *sh,
            StyleEdit::SpotlightDim(v) => s.dim = s.dim.with_alpha(*v),
            StyleEdit::CornerRadius(v) => s.corner_radius = v * sf,
            _ => {}
        },
        AnnotationKind::Magnifier(m) => match e {
            StyleEdit::Color(c) => m.border.color = *c,
            StyleEdit::StrokeWidth(w) => m.border.width = w * sf,
            StyleEdit::MagnifierZoom(z) => {
                // Keep the drawn loupe size; change the magnified area.
                let target = m.target_radius();
                m.zoom = z.clamp(2.0, 4.0);
                m.source_radius = target / m.zoom;
            }
            _ => {}
        },
        AnnotationKind::Blur(b) => match e {
            StyleEdit::BlurStrength(v) => b.strength = v * sf,
            StyleEdit::CornerRadius(v) => b.corner_radius = v * sf,
            _ => {}
        },
        AnnotationKind::Pixelate(p) => {
            if let StyleEdit::PixelBlock(v) = e {
                p.block_size = ((*v as f32 * sf).round() as u32).max(1);
            }
        }
        AnnotationKind::Redact(r) => match e {
            StyleEdit::Color(c) => r.color = c.with_alpha(1.0),
            StyleEdit::CornerRadius(v) => r.corner_radius = v * sf,
            _ => {}
        },
        AnnotationKind::Text(t) => {
            if let StyleEdit::TextPreset(p) = e {
                t.preset = Some(*p);
            }
            edit_text_style(&mut t.style, e, sf);
        }
    }
    *a != before
}

/// Read an annotation's style back as a [`ToolStyle`] in pt, starting from
/// `base` (the remembered tool style) for fields the kind doesn't have.
pub fn style_of(a: &Annotation, base: &ToolStyle, sf: f32) -> ToolStyle {
    let inv = 1.0 / sf.max(0.01);
    let mut s = base.clone();
    s.opacity = a.opacity;
    s.shadow = a.shadow.map(|sh| sh.scaled(inv));
    match &a.kind {
        AnnotationKind::Rect(r) => {
            s.stroke = r.style.stroke;
            s.stroke.width *= inv;
            s.fill = r.style.fill;
            s.fill_mode = r.style.mode;
            s.corner_radius = r.corner_radius * inv;
        }
        AnnotationKind::Ellipse(r) => {
            s.stroke = r.style.stroke;
            s.stroke.width *= inv;
            s.fill = r.style.fill;
            s.fill_mode = r.style.mode;
        }
        AnnotationKind::Line(l) => {
            s.stroke = l.stroke;
            s.stroke.width *= inv;
        }
        AnnotationKind::Arrow(ar) => {
            s.stroke = ar.stroke;
            s.stroke.width *= inv;
            s.arrow_head = ar.head;
            s.arrow_double = ar.double_headed;
            s.arrow_tapered = ar.tapered;
            s.arrow_curved = ar.control.is_some();
        }
        AnnotationKind::Pen(p) => {
            s.stroke = p.stroke;
            s.stroke.width *= inv;
        }
        AnnotationKind::Highlighter(h) => {
            s.highlighter_color = h.color;
            s.highlighter_width = h.width * inv;
        }
        AnnotationKind::Step(st) => {
            s.fill = st.fill;
            s.step_label = st.label;
            s.step_radius = st.radius * inv;
            s.step_text_color = st.text_color;
        }
        AnnotationKind::Callout(c) => {
            s.stroke = c.style.stroke;
            s.stroke.width *= inv;
            s.fill = c.style.fill;
            s.fill_mode = c.style.mode;
            s.corner_radius = c.corner_radius * inv;
            s.text = c.text_style.scaled(inv);
            s.callout_padding = c.padding * inv;
        }
        AnnotationKind::Spotlight(sp) => {
            s.spotlight_shape = sp.shape;
            s.spotlight_dim = sp.dim;
            s.corner_radius = sp.corner_radius * inv;
        }
        AnnotationKind::Magnifier(m) => {
            s.stroke = m.border;
            s.stroke.width *= inv;
            s.magnifier_zoom = m.zoom;
        }
        AnnotationKind::Blur(b) => {
            s.blur_strength = b.strength * inv;
            s.corner_radius = b.corner_radius * inv;
        }
        AnnotationKind::Pixelate(p) => s.pixelate_block = ((p.block_size as f32 * inv).round() as u32).max(1),
        AnnotationKind::Redact(r) => {
            s.redact_color = r.color;
            s.corner_radius = r.corner_radius * inv;
        }
        AnnotationKind::Text(t) => {
            s.text = t.style.scaled(inv);
            s.shadow = t.style.shadow.map(|sh| sh.scaled(inv));
        }
    }
    s
}

/// Which panel sections apply to a tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sections {
    pub color: bool,
    pub fill: bool,
    pub stroke_width: bool,
    pub dash: bool,
    pub caps: bool,
    pub corner_radius: bool,
    pub shadow: bool,
    pub arrow: bool,
    pub highlighter: bool,
    pub step: bool,
    pub spotlight: bool,
    pub magnifier: bool,
    pub blur: bool,
    pub pixelate: bool,
    pub text: bool,
    pub callout: bool,
}

impl Sections {
    pub fn for_tool(t: ToolKind) -> Self {
        let mut s = Sections { shadow: true, ..Default::default() };
        match t {
            ToolKind::Select | ToolKind::Crop => return Sections::default(),
            ToolKind::Rect => {
                s.color = true;
                s.fill = true;
                s.stroke_width = true;
                s.dash = true;
                s.corner_radius = true;
            }
            ToolKind::Ellipse => {
                s.color = true;
                s.fill = true;
                s.stroke_width = true;
                s.dash = true;
            }
            ToolKind::Line | ToolKind::Pen => {
                s.color = true;
                s.stroke_width = true;
                s.dash = true;
                s.caps = true;
            }
            ToolKind::Arrow => {
                s.color = true;
                s.stroke_width = true;
                s.dash = true;
                s.arrow = true;
            }
            ToolKind::Highlighter => {
                s.color = true;
                s.highlighter = true;
                s.shadow = false;
            }
            ToolKind::Step => {
                s.color = true;
                s.step = true;
            }
            ToolKind::Callout => {
                s.color = true;
                s.fill = true;
                s.stroke_width = true;
                s.dash = true;
                s.corner_radius = true;
                s.text = true;
                s.callout = true;
            }
            ToolKind::Spotlight => {
                s.spotlight = true;
                s.corner_radius = true;
                s.shadow = false;
            }
            ToolKind::Magnifier => {
                s.color = true;
                s.stroke_width = true;
                s.magnifier = true;
            }
            ToolKind::Blur => {
                s.blur = true;
                s.corner_radius = true;
                s.shadow = false;
            }
            ToolKind::Pixelate => {
                s.pixelate = true;
                s.shadow = false;
            }
            ToolKind::Redact => {
                s.color = true;
                s.corner_radius = true;
                s.shadow = false;
            }
            ToolKind::Text => {
                s.color = true;
                s.text = true;
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::{Point, ToolKind};

    #[test]
    fn edit_roundtrip_through_annotation() {
        let sf = 2.0;
        let base = ToolStyle::default_for(ToolKind::Rect);
        let mut a = base.to_content_px(sf).create(ToolKind::Rect, Point::ZERO, Point::new(50.0, 50.0)).unwrap();
        assert!(apply_to_annotation(&mut a, &StyleEdit::StrokeWidth(7.0), sf));
        assert!(apply_to_annotation(&mut a, &StyleEdit::Color(Color::BLUE), sf));
        assert!(!apply_to_annotation(&mut a, &StyleEdit::Color(Color::BLUE), sf));
        let back = style_of(&a, &base, sf);
        assert_eq!(back.stroke.width, 7.0);
        assert_eq!(back.stroke.color, Color::BLUE);
        // Irrelevant edit does nothing.
        assert!(!apply_to_annotation(&mut a, &StyleEdit::BlurStrength(3.0), sf));
    }

    #[test]
    fn every_tool_every_edit_is_safe() {
        let edits = vec![
            StyleEdit::Color(Color::GREEN),
            StyleEdit::Fill(Color::YELLOW),
            StyleEdit::FillMode(FillMode::Filled),
            StyleEdit::StrokeWidth(3.0),
            StyleEdit::Opacity(0.5),
            StyleEdit::Shadow(true),
            StyleEdit::ShadowBlur(10.0),
            StyleEdit::TextBackgroundKind(1),
            StyleEdit::TextBackgroundColor(Color::BLUE),
            StyleEdit::TextBackgroundKind(3),
            StyleEdit::TextOutlineWidth(4.0),
            StyleEdit::TextPreset(TextPreset::Code),
            StyleEdit::MagnifierZoom(9.0),
            StyleEdit::PixelBlock(0),
            StyleEdit::Preset(Box::default()),
        ];
        for t in ToolKind::ALL {
            let mut ts = ToolStyle::default_for(t);
            let Some(mut a) = ts.to_content_px(2.0).create(t, Point::ZERO, Point::new(40.0, 40.0)) else { continue };
            for e in &edits {
                apply_to_tool_style(t, &mut ts, e);
                apply_to_annotation(&mut a, e, 2.0);
            }
            assert!(a.opacity <= 1.0);
            if let AnnotationKind::Magnifier(m) = &a.kind {
                assert!(m.zoom <= 4.0);
            }
            if let AnnotationKind::Pixelate(p) = &a.kind {
                assert!(p.block_size >= 1);
            }
        }
    }

    #[test]
    fn text_background_kinds() {
        let mut t = TextStyle::default();
        edit_text_style(&mut t, &StyleEdit::TextBackgroundKind(3), 2.0);
        assert!(matches!(t.background, TextBackground::Outline { width, .. } if width == 6.0));
        edit_text_style(&mut t, &StyleEdit::TextOutlineWidth(2.0), 2.0);
        assert!(matches!(t.background, TextBackground::Outline { width, .. } if width == 4.0));
        assert_eq!(text_bg_kind(&t.background), 3);
    }

    #[test]
    fn highlighter_keeps_alpha() {
        let mut s = ToolStyle::default_for(ToolKind::Highlighter);
        let a0 = s.highlighter_color.a;
        apply_to_tool_style(ToolKind::Highlighter, &mut s, &StyleEdit::Color(Color::GREEN));
        assert_eq!(s.highlighter_color.a, a0);
        assert_eq!(s.highlighter_color.g, Color::GREEN.g);
    }
}
