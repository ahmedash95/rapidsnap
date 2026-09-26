//! # rs-smart — smart features & persistence
//!
//! Modules:
//! * [`paths`] — app data locations (`~/Library/Application Support/io.github.ahmedash95.rapidsnap`, one-time migration from the pre-release `com.rapidsnap.v2`).
//! * [`prefs`] — load/save `Preferences` (JSON, atomic, tolerant).
//! * [`project`] — `.rsnap` project files (zip, versioned, forward compatible).
//! * [`backgrounds`] — custom background image library.
//! * [`history`] — recent captures store (thumbnails + projects, pruned).
//! * [`ocr`] — text recognition via macOS Vision (objc2), "copy text".
//! * [`redact`] — sensitive-data detection over OCR results (auto-redact).
//! * [`login_item`] — launch at login (SMAppService, bundled .app only).
//! * [`output`] — default save folder, filename template with local time.
//! * [`fsutil`] — atomic writes.
//!
//! All functions are blocking; call file/OCR work from the background executor
//! (`cx.background_executor().spawn(..)`). Everything is `Send`; stores are
//! `Clone` and serialize their index writes internally.

use thiserror::Error;

pub mod backgrounds;
pub mod fsutil;
pub mod history;
pub mod login_item;
pub mod ocr;
pub mod output;
pub mod paths;
pub mod prefs;
pub mod project;
pub mod redact;

/// Errors from rs-smart.
#[derive(Debug, Error)]
pub enum SmartError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("invalid project file: {0}")]
    InvalidProject(String),
    #[error("unsupported project version {0}")]
    UnsupportedVersion(u32),
    #[error("not found")]
    NotFound,
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
    #[error("platform error: {0}")]
    Platform(String),
}

pub type Result<T, E = SmartError> = std::result::Result<T, E>;
