//! Export settings (§8).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Output file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum ExportFormat {
    #[default]
    Png,
    /// `quality` 1..=100 (default 90). Alpha is flattened onto white.
    Jpeg { quality: u8 },
    /// Lossless WebP (the `image` crate encoder is lossless-only).
    WebP,
}


impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg { .. } => "jpg",
            ExportFormat::WebP => "webp",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ExportFormat::Png => "image/png",
            ExportFormat::Jpeg { .. } => "image/jpeg",
            ExportFormat::WebP => "image/webp",
        }
    }
}

/// Export resolution relative to **logical points**.
///
/// * `X1` → 1 output px per pt (a Retina 2× capture is downscaled by half).
/// * `X2` → 2 output px per pt (a Retina capture is exported at native res;
///   a 1× capture is upscaled).
///
/// Render scale (output px per content px) =
/// `export_scale.factor() / DocumentMetadata::scale_factor`
/// (see `rs_render::RenderOptions::for_export`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportScale {
    X1,
    #[default]
    X2,
}

impl ExportScale {
    pub fn factor(self) -> f32 {
        match self {
            ExportScale::X1 => 1.0,
            ExportScale::X2 => 2.0,
        }
    }
}

/// User export preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub format: ExportFormat,
    pub scale: ExportScale,
    /// Default save folder; `None` → `~/Desktop` (resolved by the app).
    pub default_folder: Option<PathBuf>,
    /// File name template **without extension**. Tokens:
    /// `{date}` → `YYYY-MM-DD`, `{time}` → `HH.MM.SS`, `{n}` → counter,
    /// `{w}`/`{h}` → output size in px, `{app}` → source app name or `RapidSnap`.
    /// Unknown tokens are left verbatim. See [`render_filename`].
    pub filename_template: String,
    /// Next value for `{n}` (incremented by the app after each save).
    pub counter: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: ExportFormat::Png,
            scale: ExportScale::X2,
            default_folder: None,
            filename_template: "RapidSnap {date} at {time}".to_string(),
            counter: 1,
        }
    }
}

/// Values available to [`render_filename`]. (rs-core has no clock dependency;
/// the caller fills in the local time.)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilenameContext {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub counter: u32,
    pub width: u32,
    pub height: u32,
    pub app_name: Option<String>,
}

/// Expand a filename template (no extension added). Characters invalid in
/// macOS file names (`/` and `:`) are replaced by `-`.
pub fn render_filename(template: &str, ctx: &FilenameContext) -> String {
    let out = template
        .replace("{date}", &format!("{:04}-{:02}-{:02}", ctx.year, ctx.month, ctx.day))
        .replace("{time}", &format!("{:02}.{:02}.{:02}", ctx.hour, ctx.minute, ctx.second))
        .replace("{n}", &ctx.counter.to_string())
        .replace("{w}", &ctx.width.to_string())
        .replace("{h}", &ctx.height.to_string())
        .replace("{app}", ctx.app_name.as_deref().unwrap_or("RapidSnap"));
    out.replace(['/', ':'], "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_template() {
        let ctx = FilenameContext {
            year: 2026,
            month: 9,
            day: 5,
            hour: 7,
            minute: 3,
            second: 9,
            counter: 42,
            width: 100,
            height: 50,
            app_name: None,
        };
        assert_eq!(
            render_filename("RapidSnap {date} at {time} #{n} {w}x{h} {x}", &ctx),
            "RapidSnap 2026-09-05 at 07.03.09 #42 100x50 {x}"
        );
        assert_eq!(render_filename("a/b:c", &ctx), "a-b-c");
    }
}
