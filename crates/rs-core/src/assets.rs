//! Locating bundled assets (wallpapers, icons) at runtime.
//!
//! Layout (repo `assets/` == bundle `Contents/Resources/assets/`):
//! ```text
//! assets/wallpapers/<name>.jpg      bundled wallpapers (BUNDLED_WALLPAPERS)
//! assets/icons/AppIcon.icns         app icon (bundle)
//! assets/icons/app-icon-<N>.png     app icon PNGs (16..1024)
//! assets/icons/tray-template.png    22×22 monochrome tray template
//! assets/icons/tray-template@2x.png 44×44
//! ```

use std::path::{Path, PathBuf};

/// Names of the bundled wallpapers (ported from v1's 11 wallpapers, in v1
/// order). File = `assets/wallpapers/<name>.jpg`.
pub const BUNDLED_WALLPAPERS: [&str; 11] = [
    "wallpaper-01",
    "wallpaper-02",
    "wallpaper-03",
    "wallpaper-04",
    "wallpaper-05",
    "wallpaper-06",
    "wallpaper-07",
    "wallpaper-08",
    "wallpaper-09",
    "wallpaper-10",
    "wallpaper-11",
];

/// Environment variable that overrides the assets directory.
pub const ASSETS_DIR_ENV: &str = "RAPIDSNAP_ASSETS_DIR";

/// The assets directory. Resolution order:
/// 1. `$RAPIDSNAP_ASSETS_DIR` if set and exists;
/// 2. `<exe>/../Resources/assets` (inside `RapidSnap.app/Contents/MacOS/`);
/// 3. `<repo>/assets` via this crate's compile-time `CARGO_MANIFEST_DIR`
///    (dev builds / `cargo run` / tests).
///
/// Always returns a path (3. as the final fallback even if missing).
pub fn assets_dir() -> PathBuf {
    if let Some(p) = std::env::var_os(ASSETS_DIR_ENV).map(PathBuf::from)
        && p.is_dir() {
            return p;
        }
    if let Some(p) = bundle_resources_dir().map(|r| r.join("assets"))
        && p.is_dir() {
            return p;
        }
    dev_assets_dir()
}

/// `Contents/Resources` if the running executable is inside a `.app` bundle.
pub fn bundle_resources_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let res = macos.parent()?.join("Resources");
    res.is_dir().then_some(res)
}

/// `<repo>/assets`, computed at compile time.
pub fn dev_assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("assets")
}

/// Path of a file inside the assets dir, e.g. `asset_path("icons/tray-template.png")`.
pub fn asset_path(relative: impl AsRef<Path>) -> PathBuf {
    assets_dir().join(relative)
}

/// Path of a bundled wallpaper by name (`None` if the name is unknown).
pub fn wallpaper_path(name: &str) -> Option<PathBuf> {
    BUNDLED_WALLPAPERS
        .contains(&name)
        .then(|| assets_dir().join("wallpapers").join(format!("{name}.jpg")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_assets_exist() {
        let dir = assets_dir();
        assert!(dir.is_dir(), "{dir:?}");
        for name in BUNDLED_WALLPAPERS {
            let p = wallpaper_path(name).unwrap();
            assert!(p.is_file(), "{p:?}");
        }
        assert!(asset_path("icons/tray-template.png").is_file());
        assert!(wallpaper_path("nope").is_none());
    }
}
