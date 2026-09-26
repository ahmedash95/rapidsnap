//! Beautify settings (§7 of PRODUCT.md).
//!
//! **Units:** every length here (padding, inset, corner radius, shadow,
//! chrome, watermark margin) is in **logical points**. The renderer converts
//! to output px with `pt * DocumentMetadata::scale_factor * render_scale`
//! (see `rs_render::compute_layout`). This keeps a 64pt padding looking the
//! same on Retina and non-Retina captures.
//!
//! Crop & rotation of the base image are NOT here — they live in
//! [`crate::ImageTransform`] on the Document, because they change the content
//! coordinate space and must not be remembered as "last beautify settings".

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Color, TextStyle};

/// Top-level beautify configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeautifySettings {
    /// Master switch. `false` → output is exactly the (cropped/rotated)
    /// content + annotations; all other fields are ignored.
    pub enabled: bool,
    pub background: Background,
    /// Ignored when `auto_balance` is true.
    pub padding: Padding,
    pub inset: Inset,
    /// Corner radius of the frame (chrome + inset + content), pt. v1 range 1–100.
    pub corner_radius: f32,
    pub shadow: FrameShadow,
    pub aspect_ratio: AspectRatio,
    /// Auto-balance: uniform padding = `clamp(0.08 * max(content_w_pt, content_h_pt), 16, 160)`
    /// (content size in pt, i.e. content px / scale_factor), content centered.
    pub auto_balance: bool,
    pub chrome: WindowChrome,
    pub watermark: Option<Watermark>,
}

impl Default for BeautifySettings {
    /// v1 defaults: padding 50, radius 20, shadow 20, background on (wallpaper 1).
    fn default() -> Self {
        Self {
            enabled: true,
            background: Background::Wallpaper { name: crate::assets::BUNDLED_WALLPAPERS[0].to_string() },
            padding: Padding::uniform(50.0),
            inset: Inset::default(),
            corner_radius: 20.0,
            shadow: FrameShadow::default(),
            aspect_ratio: AspectRatio::Free,
            auto_balance: false,
            chrome: WindowChrome::None,
            watermark: None,
        }
    }
}

impl BeautifySettings {
    /// Settings that produce the bare content (beautify off).
    pub fn disabled() -> Self {
        Self { enabled: false, ..Self::default() }
    }
}

/// Canvas background.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Background {
    /// Transparent canvas (PNG/WebP keep alpha; JPEG flattens onto white).
    None,
    Solid { color: Color },
    /// Linear gradient. `angle_deg`: 0 = left→right, 90 = top→bottom
    /// (clockwise, CSS-like but 0° = to right). Stops' `offset` in 0..=1.
    LinearGradient { stops: Vec<GradientStop>, angle_deg: f32 },
    /// Radial gradient. `center` in normalized canvas coords (0..=1, 0..=1);
    /// `radius` as a fraction of the canvas diagonal/2 (1.0 reaches corners
    /// when centered).
    RadialGradient { stops: Vec<GradientStop>, center: [f32; 2], radius: f32 },
    /// Bundled wallpaper by name (see `rs_core::assets::BUNDLED_WALLPAPERS`),
    /// scaled to *cover* the canvas, centered.
    Wallpaper { name: String },
    /// Image file on disk (from the custom background library), cover-fit.
    CustomImage { path: PathBuf },
    /// Current desktop wallpaper (resolved at render time by the app and
    /// passed to the renderer), cover-fit, blurred (`blur` = sigma in pt)
    /// and brightness-adjusted (`brightness` multiplier, 1.0 = unchanged).
    DesktopWallpaper { blur: f32, brightness: f32 },
}

/// A gradient color stop.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

impl GradientStop {
    pub const fn new(offset: f32, color: Color) -> Self {
        Self { offset, color }
    }
}

/// Built-in gradient presets offered by the UI: `(name, background)`.
pub fn gradient_presets() -> Vec<(&'static str, Background)> {
    let lin = |a: Color, b: Color, angle: f32| Background::LinearGradient {
        stops: vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)],
        angle_deg: angle,
    };
    vec![
        ("Sunset", lin(Color::rgb(0xFF, 0x7E, 0x5F), Color::rgb(0xFE, 0xB4, 0x7B), 45.0)),
        ("Ocean", lin(Color::rgb(0x2E, 0x3C, 0xD8), Color::rgb(0x1C, 0xB5, 0xE0), 45.0)),
        ("Grape", lin(Color::rgb(0x7F, 0x00, 0xFF), Color::rgb(0xE1, 0x00, 0xFF), 45.0)),
        ("Mint", lin(Color::rgb(0x11, 0x99, 0x8E), Color::rgb(0x38, 0xEF, 0x7D), 45.0)),
        ("Peach", lin(Color::rgb(0xFF, 0xD2, 0x94), Color::rgb(0xFF, 0x8A, 0x8A), 90.0)),
        ("Night", lin(Color::rgb(0x14, 0x1E, 0x30), Color::rgb(0x24, 0x3B, 0x55), 90.0)),
        (
            "Aurora",
            Background::RadialGradient {
                stops: vec![
                    GradientStop::new(0.0, Color::rgb(0x8E, 0xC5, 0xFC)),
                    GradientStop::new(1.0, Color::rgb(0xE0, 0xC3, 0xFC)),
                ],
                center: [0.5, 0.5],
                radius: 1.0,
            },
        ),
        (
            "Spotlight",
            Background::RadialGradient {
                stops: vec![
                    GradientStop::new(0.0, Color::rgb(0x43, 0x43, 0x43)),
                    GradientStop::new(1.0, Color::rgb(0x00, 0x00, 0x00)),
                ],
                center: [0.5, 0.3],
                radius: 1.0,
            },
        ),
    ]
}

/// Padding around the frame, in pt.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Padding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    /// UI hint: sides are edited together (uniform). Renderer ignores it.
    pub linked: bool,
}

impl Padding {
    pub const fn uniform(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v, linked: true }
    }
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

impl Default for Padding {
    fn default() -> Self {
        Self::uniform(50.0)
    }
}

/// Edge-color inset (v1 "Implicit"): an extra band around the content filled
/// with the content's edge color so the screenshot seems to extend.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Inset {
    pub enabled: bool,
    /// Band thickness on every side, pt.
    pub size: f32,
    pub color: InsetColor,
}

impl Default for Inset {
    fn default() -> Self {
        Self { enabled: false, size: 24.0, color: InsetColor::Auto }
    }
}

/// Inset band color.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "color", rename_all = "snake_case")]
pub enum InsetColor {
    /// Dominant color of the content's 1-px border ring (all four edges,
    /// quantized to 5 bits/channel, most frequent bucket, averaged).
    /// Implemented by `rs_render::dominant_edge_color`.
    #[default]
    Auto,
    Custom(Color),
}

/// Drop shadow of the frame, pt.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameShadow {
    pub enabled: bool,
    /// Blur radius (≈ 2σ), pt. v1 range 1–50.
    pub size: f32,
    /// 0..=1.
    pub opacity: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub color: Color,
}

impl Default for FrameShadow {
    fn default() -> Self {
        Self { enabled: true, size: 20.0, opacity: 0.45, offset_x: 0.0, offset_y: 8.0, color: Color::BLACK }
    }
}

/// Output aspect-ratio presets. The canvas is **grown** (never shrunk) in one
/// dimension to hit the ratio; the extra space is split equally so the frame
/// stays centered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatio {
    #[default]
    Free,
    /// 1:1
    Square,
    /// 4:3
    FourThree,
    /// 16:9
    SixteenNine,
    /// X/Twitter in-feed image: 16:9 (1600×900)
    Twitter,
    /// LinkedIn shared image: 1.91:1 (1200×627)
    LinkedIn,
    /// YouTube thumbnail: 16:9 (1280×720)
    YouTube,
    /// Instagram portrait: 4:5 (1080×1350)
    Instagram,
}

impl AspectRatio {
    pub const ALL: [AspectRatio; 8] = [
        AspectRatio::Free,
        AspectRatio::Square,
        AspectRatio::FourThree,
        AspectRatio::SixteenNine,
        AspectRatio::Twitter,
        AspectRatio::LinkedIn,
        AspectRatio::YouTube,
        AspectRatio::Instagram,
    ];

    /// width / height, or `None` for Free.
    pub fn ratio(self) -> Option<f32> {
        match self {
            AspectRatio::Free => None,
            AspectRatio::Square => Some(1.0),
            AspectRatio::FourThree => Some(4.0 / 3.0),
            AspectRatio::SixteenNine | AspectRatio::Twitter | AspectRatio::YouTube => Some(16.0 / 9.0),
            AspectRatio::LinkedIn => Some(1.91),
            AspectRatio::Instagram => Some(4.0 / 5.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AspectRatio::Free => "Free",
            AspectRatio::Square => "1:1",
            AspectRatio::FourThree => "4:3",
            AspectRatio::SixteenNine => "16:9",
            AspectRatio::Twitter => "X / Twitter",
            AspectRatio::LinkedIn => "LinkedIn",
            AspectRatio::YouTube => "YouTube",
            AspectRatio::Instagram => "Instagram",
        }
    }
}

/// Optional window frame drawn as a title bar on top of the content, inside
/// the rounded frame.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WindowChrome {
    #[default]
    None,
    /// macOS title bar: 28pt tall, traffic lights (12pt dots, 8pt spacing,
    /// 12pt from left), optional centered title.
    MacOs { dark: bool, title: Option<String> },
    /// Browser bar: 40pt tall, traffic lights + rounded URL field.
    Browser { dark: bool, url: String },
}

impl WindowChrome {
    /// Title bar height in pt (0 for None).
    pub fn height_pt(&self) -> f32 {
        match self {
            WindowChrome::None => 0.0,
            WindowChrome::MacOs { .. } => 28.0,
            WindowChrome::Browser { .. } => 40.0,
        }
    }
}

/// Watermark / signature, drawn on the canvas (over the background, and over
/// the frame if it overlaps), positioned at a corner with `margin` pt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watermark {
    pub content: WatermarkContent,
    pub position: Corner,
    /// 0..=1.
    pub opacity: f32,
    /// Distance from the canvas edges, pt.
    pub margin: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WatermarkContent {
    /// Text; `style` lengths in pt.
    Text { text: String, style: TextStyle },
    /// Image file; drawn `width` pt wide keeping aspect ratio.
    Image { path: PathBuf, width: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}
