//! Services the app injects into the editor (rs-editor must not depend on
//! rs-smart / rs-capture — see CONTRACTS.md §1). Every field is optional;
//! the corresponding UI is hidden or disabled when a service is missing.
//!
//! Background-thread services are `Arc<dyn Fn + Send + Sync>` and are called
//! from `cx.background_executor()`; main-thread services are `Rc<dyn Fn>`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use rs_core::{Document, Rect, RgbaImage};

/// One recognized text line (content px).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrTextLine {
    pub text: String,
    pub bounds: Rect,
}

/// A detected sensitive item for the auto-redact review list (content px).
#[derive(Debug, Clone, PartialEq)]
pub struct SensitiveMatch {
    /// Human label, e.g. "Email", "API key".
    pub kind: String,
    pub text: String,
    pub bounds: Rect,
}

/// A custom background image in the user's library.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomBackground {
    /// Opaque id (the app uses the library's UUID string).
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

/// Background (off-main-thread) OCR: content image → text lines.
pub type OcrFn = Arc<dyn Fn(&RgbaImage) -> Result<Vec<OcrTextLine>, String> + Send + Sync>;
/// Background OCR + sensitive-data detection.
pub type DetectFn = Arc<dyn Fn(&RgbaImage) -> Result<Vec<SensitiveMatch>, String> + Send + Sync>;
/// Background `.rsnap` save: (document, path, thumbnail).
pub type SaveProjectFn = Arc<dyn Fn(&Document, &Path, Option<&RgbaImage>) -> Result<(), String> + Send + Sync>;
/// Background `.rsnap` load.
pub type LoadProjectFn = Arc<dyn Fn(&Path) -> Result<Document, String> + Send + Sync>;
/// Background: add a capture to the history (document + rendered image).
pub type AddHistoryFn = Arc<dyn Fn(&Document, &RgbaImage) -> Result<(), String> + Send + Sync>;
/// Background: put the exported image on the system clipboard.
/// `(image, png_bytes, px_per_pt)` — `px_per_pt` is the output pixels per
/// point (2.0 for an X2 export) so Retina images paste at point size. When
/// absent the editor falls back to GPUI's (PNG-only) clipboard.
pub type CopyImageFn = Arc<dyn Fn(&RgbaImage, &[u8], f32) -> Result<(), String> + Send + Sync>;
/// Main thread: list the custom background library.
pub type ListBackgroundsFn = Rc<dyn Fn() -> Vec<CustomBackground>>;
/// Main thread: import an image file into the library.
pub type AddBackgroundFn = Rc<dyn Fn(&Path) -> Result<CustomBackground, String>>;
/// Main thread: remove a library item by id.
pub type RemoveBackgroundFn = Rc<dyn Fn(&str) -> Result<(), String>>;

/// All injectable services. `Default` = none (editor still fully works for
/// annotation, beautify with bundled wallpapers, copy and save).
#[derive(Clone, Default)]
pub struct EditorServices {
    pub recognize_text: Option<OcrFn>,
    pub detect_sensitive: Option<DetectFn>,
    pub save_project: Option<SaveProjectFn>,
    pub load_project: Option<LoadProjectFn>,
    pub add_to_history: Option<AddHistoryFn>,
    /// Native clipboard copy (the app uses rs-capture's NSPasteboard PNG+TIFF).
    pub copy_image: Option<CopyImageFn>,
    pub list_backgrounds: Option<ListBackgroundsFn>,
    pub add_background: Option<AddBackgroundFn>,
    pub remove_background: Option<RemoveBackgroundFn>,
    /// Current desktop wallpaper file (for `Background::DesktopWallpaper`).
    pub desktop_wallpaper: Option<PathBuf>,
}
