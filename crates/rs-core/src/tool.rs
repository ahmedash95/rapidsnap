//! Editor tools, per-tool style memory and user style presets.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    annotation::*, Color, FillMode, Point, Rect, Shadow, ShapeStyle, Stroke, TextStyle,
};

/// Every editor tool. `ToolKind` is also the key for per-tool style memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    #[default]
    Select,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Highlighter,
    Step,
    Callout,
    Spotlight,
    Magnifier,
    Blur,
    Pixelate,
    Redact,
    Text,
    /// Crop the base image (edits `Document::image_transform`, not an annotation).
    Crop,
}

impl ToolKind {
    pub const ALL: [ToolKind; 16] = [
        ToolKind::Select,
        ToolKind::Rect,
        ToolKind::Ellipse,
        ToolKind::Line,
        ToolKind::Arrow,
        ToolKind::Pen,
        ToolKind::Highlighter,
        ToolKind::Step,
        ToolKind::Callout,
        ToolKind::Spotlight,
        ToolKind::Magnifier,
        ToolKind::Blur,
        ToolKind::Pixelate,
        ToolKind::Redact,
        ToolKind::Text,
        ToolKind::Crop,
    ];

    /// Single-letter shortcut (lowercase key, no modifiers), PRODUCT.md §5.
    pub fn shortcut(self) -> char {
        match self {
            ToolKind::Select => 'v',
            ToolKind::Rect => 'r',
            ToolKind::Ellipse => 'o',
            ToolKind::Line => 'l',
            ToolKind::Arrow => 'a',
            ToolKind::Pen => 'p',
            ToolKind::Highlighter => 'h',
            ToolKind::Step => 'n',
            ToolKind::Callout => 'c',
            ToolKind::Spotlight => 's',
            ToolKind::Magnifier => 'm',
            ToolKind::Blur => 'b',
            ToolKind::Pixelate => 'x',
            ToolKind::Redact => 'd',
            ToolKind::Text => 't',
            ToolKind::Crop => 'k',
        }
    }

    pub fn from_shortcut(c: char) -> Option<ToolKind> {
        let c = c.to_ascii_lowercase();
        ToolKind::ALL.into_iter().find(|t| t.shortcut() == c)
    }

    pub fn name(self) -> &'static str {
        match self {
            ToolKind::Select => "Select",
            ToolKind::Rect => "Rectangle",
            ToolKind::Ellipse => "Ellipse",
            ToolKind::Line => "Line",
            ToolKind::Arrow => "Arrow",
            ToolKind::Pen => "Pen",
            ToolKind::Highlighter => "Highlighter",
            ToolKind::Step => "Step Counter",
            ToolKind::Callout => "Callout",
            ToolKind::Spotlight => "Spotlight",
            ToolKind::Magnifier => "Magnifier",
            ToolKind::Blur => "Blur",
            ToolKind::Pixelate => "Pixelate",
            ToolKind::Redact => "Redact",
            ToolKind::Text => "Text",
            ToolKind::Crop => "Crop",
        }
    }

    /// Whether the tool creates annotations (false for Select/Crop).
    pub fn creates_annotation(self) -> bool {
        !matches!(self, ToolKind::Select | ToolKind::Crop)
    }
}

/// A flat bag of every style knob any tool uses. Each tool reads the subset it
/// needs. Stored in Preferences (per-tool memory and presets).
///
/// **Units: logical points** (not content px). Use [`ToolStyle::to_content_px`]
/// with `DocumentMetadata::scale_factor` before building an annotation, and
/// [`ToolStyle::create`] to build a default annotation from a drag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolStyle {
    pub stroke: Stroke,
    pub fill_mode: FillMode,
    pub fill: Color,
    pub corner_radius: f32,
    pub opacity: f32,
    pub shadow: Option<Shadow>,
    pub text: TextStyle,
    // arrow
    pub arrow_head: ArrowHead,
    pub arrow_double: bool,
    pub arrow_tapered: bool,
    pub arrow_curved: bool,
    // highlighter
    pub highlighter_color: Color,
    pub highlighter_width: f32,
    // step
    pub step_label: StepLabel,
    pub step_radius: f32,
    pub step_text_color: Color,
    // spotlight
    pub spotlight_shape: SpotlightShape,
    pub spotlight_dim: Color,
    // magnifier
    pub magnifier_zoom: f32,
    // obfuscation
    pub blur_strength: f32,
    pub pixelate_block: u32,
    pub redact_color: Color,
    // callout
    pub callout_padding: f32,
}

impl Default for ToolStyle {
    fn default() -> Self {
        Self {
            stroke: Stroke::default(),
            fill_mode: FillMode::Outline,
            fill: Color::RED.with_alpha(0.25),
            corner_radius: 4.0,
            opacity: 1.0,
            shadow: None,
            text: TextStyle::default(),
            arrow_head: ArrowHead::FilledTriangle,
            arrow_double: false,
            arrow_tapered: false,
            arrow_curved: false,
            highlighter_color: Color::YELLOW.with_alpha(0.5),
            highlighter_width: 20.0,
            step_label: StepLabel::Number,
            step_radius: 14.0,
            step_text_color: Color::WHITE,
            spotlight_shape: SpotlightShape::Rect,
            spotlight_dim: Color::BLACK.with_alpha(0.55),
            magnifier_zoom: 2.0,
            blur_strength: 12.0,
            pixelate_block: 12,
            redact_color: Color::BLACK,
            callout_padding: 10.0,
        }
    }
}

impl ToolStyle {
    /// Sensible per-tool defaults (in points).
    pub fn default_for(tool: ToolKind) -> Self {
        let mut s = Self::default();
        match tool {
            ToolKind::Callout => {
                s.fill_mode = FillMode::OutlineAndFill;
                s.fill = Color::WHITE;
                s.stroke = Stroke::new(Color::BLACK, 2.0);
                s.corner_radius = 10.0;
                s.text.color = Color::BLACK;
                s.text.size = 16.0;
            }
            ToolKind::Step => {
                s.fill_mode = FillMode::Filled;
                s.fill = Color::RED;
            }
            ToolKind::Magnifier => {
                s.stroke = Stroke::new(Color::WHITE, 3.0);
                s.shadow = Some(Shadow::default());
            }
            _ => {}
        }
        s
    }

    /// Convert all point lengths to content px (multiply by `scale`).
    /// Zoom, opacity, colors and counts are unit-less and unchanged, except
    /// `pixelate_block` which is also scaled (rounded, min 1).
    pub fn to_content_px(&self, scale: f32) -> ToolStyle {
        let mut s = self.clone();
        s.stroke.width *= scale;
        s.corner_radius *= scale;
        s.shadow = s.shadow.map(|sh| sh.scaled(scale));
        s.text = s.text.scaled(scale);
        s.highlighter_width *= scale;
        s.step_radius *= scale;
        s.blur_strength *= scale;
        s.pixelate_block = ((s.pixelate_block as f32 * scale).round() as u32).max(1);
        s.callout_padding *= scale;
        s
    }

    fn shape_style(&self) -> ShapeStyle {
        ShapeStyle { mode: self.fill_mode, stroke: self.stroke, fill: self.fill }
    }

    /// Build a new annotation for `tool` spanning the drag `from → to`
    /// (content space). `self` must already be in content px
    /// (see [`Self::to_content_px`]). Returns `None` for Select/Crop.
    ///
    /// Drag semantics: rect-like tools use the normalized rect; line/arrow use
    /// start/end; pen/highlighter start with `[from, to]`; step/text use
    /// `from` as center/top-left; magnifier uses `from` as source center with
    /// radius = distance and places the loupe at `to`. Callers set step
    /// `value` / text `content` afterwards.
    pub fn create(&self, tool: ToolKind, from: Point, to: Point) -> Option<Annotation> {
        let rect = Rect::from_points(from, to);
        let kind = match tool {
            ToolKind::Select | ToolKind::Crop => return None,
            ToolKind::Rect => AnnotationKind::Rect(RectAnnotation {
                rect,
                style: self.shape_style(),
                corner_radius: self.corner_radius,
            }),
            ToolKind::Ellipse => AnnotationKind::Ellipse(EllipseAnnotation { rect, style: self.shape_style() }),
            ToolKind::Line => AnnotationKind::Line(LineAnnotation { start: from, end: to, stroke: self.stroke }),
            ToolKind::Arrow => AnnotationKind::Arrow(ArrowAnnotation {
                start: from,
                end: to,
                control: self.arrow_curved.then(|| {
                    // Default control point: perpendicular offset of 20% length.
                    let mid = from.lerp(to, 0.5);
                    let d = to - from;
                    mid + Point::new(-d.y, d.x) * 0.2
                }),
                stroke: self.stroke,
                head: self.arrow_head,
                double_headed: self.arrow_double,
                tapered: self.arrow_tapered,
                head_scale: 1.0,
            }),
            ToolKind::Pen => AnnotationKind::Pen(PenAnnotation { points: vec![from, to], stroke: self.stroke }),
            ToolKind::Highlighter => AnnotationKind::Highlighter(HighlighterAnnotation {
                points: vec![from, to],
                color: self.highlighter_color,
                width: self.highlighter_width,
            }),
            ToolKind::Step => AnnotationKind::Step(StepAnnotation {
                center: from,
                radius: self.step_radius,
                value: 1,
                label: self.step_label,
                fill: self.fill.with_alpha(1.0),
                text_color: self.step_text_color,
                border: None,
                font_family: self.text.family.clone(),
                font_weight: crate::FontWeight::BOLD,
            }),
            ToolKind::Callout => AnnotationKind::Callout(CalloutAnnotation {
                rect,
                tail: Point::new(rect.x + rect.width * 0.25, rect.bottom() + rect.height.max(20.0) * 0.6),
                corner_radius: self.corner_radius,
                style: self.shape_style(),
                text: String::new(),
                text_style: self.text.clone(),
                padding: self.callout_padding,
            }),
            ToolKind::Spotlight => AnnotationKind::Spotlight(SpotlightAnnotation {
                rect,
                shape: self.spotlight_shape,
                corner_radius: self.corner_radius,
                dim: self.spotlight_dim,
            }),
            ToolKind::Magnifier => AnnotationKind::Magnifier(MagnifierAnnotation {
                source_center: from,
                source_radius: from.distance(to).max(8.0) / self.magnifier_zoom.max(1.0),
                target_center: to,
                zoom: self.magnifier_zoom.clamp(2.0, 4.0),
                border: self.stroke,
            }),
            ToolKind::Blur => AnnotationKind::Blur(BlurAnnotation {
                rect,
                strength: self.blur_strength,
                corner_radius: 0.0,
            }),
            ToolKind::Pixelate => {
                AnnotationKind::Pixelate(PixelateAnnotation { rect, block_size: self.pixelate_block.max(1) })
            }
            ToolKind::Redact => {
                AnnotationKind::Redact(RedactAnnotation { rect, color: self.redact_color, corner_radius: 0.0 })
            }
            ToolKind::Text => AnnotationKind::Text(TextAnnotation {
                position: from,
                width: (rect.width > 4.0).then_some(rect.width),
                content: String::new(),
                style: self.text.clone(),
                preset: None,
            }),
        };
        let shadow = if matches!(tool, ToolKind::Text) { None } else { self.shadow };
        Some(Annotation::new(kind).with_opacity(self.opacity).with_shadow(shadow))
    }
}

/// A user-defined named style preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StylePreset {
    pub id: Uuid,
    pub name: String,
    /// `None` = applicable to any tool.
    #[serde(default)]
    pub tool: Option<ToolKind>,
    /// In logical points.
    pub style: ToolStyle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in ToolKind::ALL {
            assert!(seen.insert(t.shortcut()), "dup shortcut {:?}", t);
            assert_eq!(ToolKind::from_shortcut(t.shortcut()), Some(t));
        }
    }

    #[test]
    fn create_every_tool() {
        let st = ToolStyle::default().to_content_px(2.0);
        assert_eq!(st.stroke.width, 8.0);
        for t in ToolKind::ALL {
            let a = st.create(t, Point::new(10.0, 10.0), Point::new(50.0, 40.0));
            assert_eq!(a.is_some(), t.creates_annotation());
            if let Some(a) = a {
                assert_eq!(a.tool(), t);
            }
        }
    }
}
