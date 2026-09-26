//! Text styling shared by text annotations, callouts, step counters and
//! watermarks. Layout/shaping is done by rs-render (cosmic-text); rs-core only
//! describes the style.

use serde::{Deserialize, Serialize};

use crate::{Color, Shadow};

/// Font family selection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum FontFamily {
    /// The platform UI font (SF Pro on macOS). cosmic-text: `Family::SansSerif`
    /// resolved to the system font.
    #[default]
    System,
    /// The platform monospace font (SF Mono / Menlo). Used by the *Code* preset.
    Monospace,
    /// Serif fallback family.
    Serif,
    /// A specific installed family by name, e.g. `"Helvetica Neue"`. Renderers
    /// fall back to `System` if not installed.
    Named(String),
}

/// CSS-style numeric font weight, 100..=900. 400 = regular, 700 = bold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const THIN: FontWeight = FontWeight(100);
    pub const LIGHT: FontWeight = FontWeight(300);
    pub const REGULAR: FontWeight = FontWeight(400);
    pub const MEDIUM: FontWeight = FontWeight(500);
    pub const SEMIBOLD: FontWeight = FontWeight(600);
    pub const BOLD: FontWeight = FontWeight(700);
    pub const HEAVY: FontWeight = FontWeight(800);
    pub const BLACK: FontWeight = FontWeight(900);
}

impl Default for FontWeight {
    fn default() -> Self {
        FontWeight::REGULAR
    }
}

/// Horizontal alignment of lines inside the text box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Background treatment behind/around text glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextBackground {
    /// Glyphs only.
    #[default]
    None,
    /// Opaque rounded rectangle behind the text block.
    /// `corner_radius` in px; `f32::INFINITY` is not allowed — use a large
    /// value (renderer clamps radius to half the box height = full pill).
    Pill { color: Color, corner_radius: f32, padding_x: f32, padding_y: f32 },
    /// Like `Pill` but the color is expected to be translucent (e.g. black
    /// @ 50%). Kept distinct so the UI can offer it as its own preset.
    SemiTransparent { color: Color, corner_radius: f32, padding_x: f32, padding_y: f32 },
    /// True glyph outline (stroke drawn *behind* the fill so the fill keeps its
    /// full width). `width` in px.
    Outline { width: f32, color: Color },
}

/// Named text presets (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPreset {
    Title,
    Label,
    Note,
    Code,
}

impl TextPreset {
    pub const ALL: [TextPreset; 4] =
        [TextPreset::Title, TextPreset::Label, TextPreset::Note, TextPreset::Code];

    pub fn name(self) -> &'static str {
        match self {
            TextPreset::Title => "Title",
            TextPreset::Label => "Label",
            TextPreset::Note => "Note",
            TextPreset::Code => "Code",
        }
    }

    /// The preset's style in **logical points** (multiply sizes by the
    /// document scale factor via [`TextStyle::scaled`] before storing in an
    /// annotation).
    pub fn style(self) -> TextStyle {
        match self {
            TextPreset::Title => TextStyle {
                size: 36.0,
                weight: FontWeight::BOLD,
                color: Color::WHITE,
                background: TextBackground::Outline { width: 3.0, color: Color::BLACK },
                ..TextStyle::default()
            },
            TextPreset::Label => TextStyle {
                size: 16.0,
                weight: FontWeight::SEMIBOLD,
                color: Color::WHITE,
                background: TextBackground::Pill {
                    color: Color::RED,
                    corner_radius: 999.0,
                    padding_x: 10.0,
                    padding_y: 4.0,
                },
                ..TextStyle::default()
            },
            TextPreset::Note => TextStyle {
                size: 14.0,
                color: Color::WHITE,
                background: TextBackground::SemiTransparent {
                    color: Color::BLACK.with_alpha(0.6),
                    corner_radius: 8.0,
                    padding_x: 10.0,
                    padding_y: 8.0,
                },
                ..TextStyle::default()
            },
            TextPreset::Code => TextStyle {
                family: FontFamily::Monospace,
                size: 14.0,
                color: Color::rgb(0xE6, 0xE6, 0xE6),
                background: TextBackground::Pill {
                    color: Color::rgb(0x1E, 0x1E, 0x1E),
                    corner_radius: 6.0,
                    padding_x: 8.0,
                    padding_y: 6.0,
                },
                ..TextStyle::default()
            },
        }
    }
}

/// Complete text style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub family: FontFamily,
    /// Font size in px (content space). UI range 8–200 (v1: 12–72).
    pub size: f32,
    pub weight: FontWeight,
    pub italic: bool,
    pub color: Color,
    pub align: TextAlign,
    /// Line height as a multiple of `size` (1.2 default).
    pub line_height: f32,
    pub background: TextBackground,
    /// Text-specific drop shadow (applied to glyphs + background). Independent
    /// of `Annotation::shadow`; renderers apply `Annotation::shadow` if this is
    /// `None`.
    pub shadow: Option<Shadow>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: FontFamily::System,
            size: 24.0,
            weight: FontWeight::REGULAR,
            italic: false,
            color: Color::RED,
            align: TextAlign::Left,
            line_height: 1.2,
            background: TextBackground::None,
            shadow: None,
        }
    }
}

impl TextStyle {
    /// Multiply every length (size, paddings, radii, outline width, shadow) by `s`.
    pub fn scaled(&self, s: f32) -> TextStyle {
        let background = match self.background {
            TextBackground::None => TextBackground::None,
            TextBackground::Pill { color, corner_radius, padding_x, padding_y } => TextBackground::Pill {
                color,
                corner_radius: corner_radius * s,
                padding_x: padding_x * s,
                padding_y: padding_y * s,
            },
            TextBackground::SemiTransparent { color, corner_radius, padding_x, padding_y } => {
                TextBackground::SemiTransparent {
                    color,
                    corner_radius: corner_radius * s,
                    padding_x: padding_x * s,
                    padding_y: padding_y * s,
                }
            }
            TextBackground::Outline { width, color } => TextBackground::Outline { width: width * s, color },
        };
        TextStyle {
            size: self.size * s,
            background,
            shadow: self.shadow.map(|sh| sh.scaled(s)),
            ..self.clone()
        }
    }
}
