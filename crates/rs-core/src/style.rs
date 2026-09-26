//! Shared stroke / fill / shadow styles used by annotations.
//!
//! All lengths in styles stored **inside a Document** are in content-space
//! pixels. Styles stored in **Preferences** ([`crate::ToolStyle`]) are in
//! logical points and are multiplied by `DocumentMetadata::scale_factor` when a
//! new annotation is instantiated (see [`crate::ToolStyle::to_content_px`]).

use serde::{Deserialize, Serialize};

use crate::Color;

/// Line cap for open paths (lines, arrows' tails, pen strokes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineCap {
    Butt,
    #[default]
    Round,
    Square,
}

/// Dash style. Dash lengths are multiples of the stroke width:
/// `Dashed` = dash `3w`, gap `2w`; `Dotted` = dash `0` (+ round cap), gap `2w`.
/// Renderers must enforce a minimum dash/gap of 1px.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

impl DashStyle {
    /// Dash array (on, off, ...) in pixels for a given stroke width, or `None`
    /// for solid strokes.
    pub fn dash_array(self, width: f32) -> Option<[f32; 2]> {
        let w = width.max(1.0);
        match self {
            DashStyle::Solid => None,
            DashStyle::Dashed => Some([3.0 * w, 2.0 * w]),
            DashStyle::Dotted => Some([0.01, 2.0 * w]),
        }
    }
}

/// Stroke description.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub color: Color,
    /// Stroke width in px (content space). v1 range 1–20.
    pub width: f32,
    pub dash: DashStyle,
    pub cap: LineCap,
}

impl Default for Stroke {
    fn default() -> Self {
        Self { color: Color::RED, width: 4.0, dash: DashStyle::Solid, cap: LineCap::Round }
    }
}

impl Stroke {
    pub fn new(color: Color, width: f32) -> Self {
        Self { color, width, ..Default::default() }
    }
}

/// How a closed shape is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillMode {
    /// Stroke only.
    #[default]
    Outline,
    /// Fill only (no stroke).
    Filled,
    /// Fill, then stroke on top.
    OutlineAndFill,
}

impl FillMode {
    pub fn has_stroke(self) -> bool {
        matches!(self, FillMode::Outline | FillMode::OutlineAndFill)
    }
    pub fn has_fill(self) -> bool {
        matches!(self, FillMode::Filled | FillMode::OutlineAndFill)
    }
}

/// Style for closed shapes (rect, ellipse, callout bubble).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeStyle {
    pub mode: FillMode,
    pub stroke: Stroke,
    /// Fill color (used when `mode.has_fill()`).
    pub fill: Color,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self { mode: FillMode::Outline, stroke: Stroke::default(), fill: Color::RED.with_alpha(0.25) }
    }
}

/// Drop shadow. Used by every annotation (`Annotation::shadow`) and by text.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    /// Shadow color; its alpha is the shadow opacity.
    pub color: Color,
    /// Gaussian blur radius (≈ 2σ) in px.
    pub blur: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Self { color: Color::BLACK.with_alpha(0.35), blur: 8.0, offset_x: 0.0, offset_y: 2.0 }
    }
}

impl Shadow {
    /// Same shadow with all lengths multiplied by `s`.
    pub fn scaled(self, s: f32) -> Self {
        Self { blur: self.blur * s, offset_x: self.offset_x * s, offset_y: self.offset_y * s, ..self }
    }
}
