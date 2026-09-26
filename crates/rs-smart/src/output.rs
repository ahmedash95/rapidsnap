//! Save-location helpers (PRODUCT §8): default folder, filename template
//! expansion with the local clock, collision-free paths.
//!
//! Template expansion itself is `rs_core::render_filename` (pure, usable from
//! rs-editor); this module adds the parts that need the OS (clock, folders,
//! existing files). Blocking only in `unique_path` (stat calls).

use std::path::{Path, PathBuf};

use chrono::{Datelike, Timelike};
use rs_core::{render_filename, CaptureSource, ExportSettings, FilenameContext};

/// `settings.default_folder` if it is an existing directory, else `~/Desktop`,
/// else the home directory, else the temp dir.
pub fn default_save_folder(settings: &ExportSettings) -> PathBuf {
    if let Some(d) = settings.default_folder.as_ref().filter(|d| d.is_dir()) {
        return d.clone();
    }
    dirs::desktop_dir()
        .filter(|d| d.is_dir())
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
}

/// `FilenameContext` filled with the current **local** time.
pub fn filename_context_now(counter: u32, width: u32, height: u32, app_name: Option<String>) -> FilenameContext {
    let now = chrono::Local::now();
    FilenameContext {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        hour: now.hour(),
        minute: now.minute(),
        second: now.second(),
        counter,
        width,
        height,
        app_name,
    }
}

/// Source app name for `{app}` (window captures only).
pub fn app_name_for_source(source: &CaptureSource) -> Option<String> {
    match source {
        CaptureSource::Window { app_name, .. } => app_name.clone().filter(|s| !s.trim().is_empty()),
        _ => None,
    }
}

/// Expanded template + extension of `settings.format`, e.g.
/// `"RapidSnap 2026-09-25 at 10.11.12.png"`. Leading dots / empty results are
/// replaced so the file is never hidden or nameless.
pub fn suggested_file_name(settings: &ExportSettings, ctx: &FilenameContext) -> String {
    let stem = render_filename(&settings.filename_template, ctx);
    let stem = stem.trim().trim_start_matches('.').trim();
    let stem = if stem.is_empty() { "RapidSnap" } else { stem };
    format!("{stem}.{}", settings.format.extension())
}

/// `dir/file_name`, or `dir/<stem> 2.<ext>`, `… 3` … if it already exists.
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(file_name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for i in 2..10_000 {
        let cand = dir.join(format!("{stem} {i}{ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    dir.join(format!("{stem} {}{ext}", uuid::Uuid::new_v4().simple()))
}

/// Full default path for a new export: folder + expanded name, collision-free.
pub fn suggested_save_path(settings: &ExportSettings, ctx: &FilenameContext) -> PathBuf {
    unique_path(&default_save_folder(settings), &suggested_file_name(settings, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::ExportFormat;

    #[test]
    fn names_and_paths() {
        let mut s = ExportSettings::default();
        let ctx = FilenameContext { year: 2026, month: 9, day: 25, hour: 1, minute: 2, second: 3, ..Default::default() };
        assert_eq!(suggested_file_name(&s, &ctx), "RapidSnap 2026-09-25 at 01.02.03.png");
        s.format = ExportFormat::Jpeg { quality: 90 };
        s.filename_template = " .{app} {n} ".into();
        assert_eq!(suggested_file_name(&s, &FilenameContext { counter: 4, ..ctx.clone() }), "RapidSnap 4.jpg");
        s.filename_template = "".into();
        assert_eq!(suggested_file_name(&s, &ctx), "RapidSnap.jpg");

        let dir = std::env::temp_dir().join(format!("rs-smart-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        s.default_folder = Some(dir.clone());
        assert_eq!(default_save_folder(&s), dir);
        let p1 = suggested_save_path(&s, &ctx);
        assert_eq!(p1, dir.join("RapidSnap.jpg"));
        std::fs::write(&p1, b"x").unwrap();
        assert_eq!(suggested_save_path(&s, &ctx), dir.join("RapidSnap 2.jpg"));
        s.default_folder = Some(dir.join("missing"));
        assert_ne!(default_save_folder(&s), dir.join("missing"));
        let _ = std::fs::remove_dir_all(dir);

        let now = filename_context_now(7, 10, 20, Some("Safari".into()));
        assert!(now.year >= 2024 && now.counter == 7);
        let src = CaptureSource::Window { app_name: Some("Xcode".into()), title: None };
        assert_eq!(app_name_for_source(&src).as_deref(), Some("Xcode"));
    }
}
