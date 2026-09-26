//! Preferences persistence (JSON, pretty-printed, atomic write).
//!
//! Forward/backward compatible: `rs_core::Preferences` is `#[serde(default)]`
//! throughout, so missing fields take defaults and unknown fields (from a newer
//! build) are ignored. A corrupt file is preserved as `preferences.json.bak`
//! and defaults are used. Blocking but fast (< 1 ms); fine to call on the main
//! thread at startup, debounce saves.

use std::path::Path;

use rs_core::Preferences;

use crate::fsutil::atomic_write;
use crate::{paths, Result};

/// Load preferences from the default path. Never fails: a missing or corrupt
/// file yields `Preferences::default()` (corrupt files are logged and kept as
/// `preferences.json.bak`).
pub fn load_preferences() -> Preferences {
    load_preferences_from(&paths::preferences_path())
}

/// Load from an explicit path (same fallback rules).
pub fn load_preferences_from(path: &Path) -> Preferences {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes)
            .and_then(|v| Ok((serde_json::from_value::<Preferences>(v.clone())?, v)))
        {
            Ok((p, raw)) => sanitize(migrate(p, &raw)),
            Err(e) => {
                log::warn!("preferences {path:?} unreadable ({e}); using defaults");
                let _ = std::fs::copy(path, path.with_extension("json.bak"));
                Preferences::default()
            }
        },
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("preferences {path:?}: {e}; using defaults");
            }
            Preferences::default()
        }
    }
}

/// Fill fields that older files lack from their predecessors.
fn migrate(mut p: Preferences, raw: &serde_json::Value) -> Preferences {
    // Before `window_capture_transparent` existed, `window_capture_shadow`
    // drove both the shadow and the transparent background.
    if raw.get("window_capture_transparent").is_none() {
        p.window_capture_transparent = p.window_capture_shadow;
    }
    p
}

/// Clamp values a hand-edited or older file could have out of range
/// (`rs_core::Preferences::sanitize`: hotkeys, padding, styles, …).
fn sanitize(mut p: Preferences) -> Preferences {
    p.sanitize();
    p
}

/// Save to the default path (creates the directory).
pub fn save_preferences(prefs: &Preferences) -> Result<()> {
    save_preferences_to(prefs, &paths::preferences_path())
}

/// Save to an explicit path: unique temp file + fsync + rename (atomic).
pub fn save_preferences_to(prefs: &Preferences, path: &Path) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(prefs)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("rs-smart-prefs-{}", uuid::Uuid::new_v4()));
        let path = dir.join("preferences.json");
        assert_eq!(load_preferences_from(&path), Preferences::default());
        let p = Preferences { instant_copy: true, history_limit: 7, ..Default::default() };
        save_preferences_to(&p, &path).unwrap();
        assert_eq!(load_preferences_from(&path), p);
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load_preferences_from(&path), Preferences::default());
        assert_eq!(std::fs::read(path.with_extension("json.bak")).unwrap(), b"{not json");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_transparent_and_sanitizes() {
        let dir = std::env::temp_dir().join(format!("rs-smart-prefs-{}", uuid::Uuid::new_v4()));
        let path = dir.join("preferences.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, br#"{"window_capture_shadow": false, "history_limit": 999999,
            "last_beautify": {"padding": {"top": -5, "right": 1e30, "bottom": 1, "left": 1}},
            "hotkeys": {"region": {"modifiers": {}, "key": "KeyA"}}}"#).unwrap();
        let p = load_preferences_from(&path);
        assert!(!p.window_capture_shadow && !p.window_capture_transparent);
        assert_eq!(p.history_limit, rs_core::sanitize::limits::MAX_HISTORY_LIMIT);
        assert_eq!(p.last_beautify.padding.top, 0.0);
        assert_eq!(p.last_beautify.padding.right, rs_core::sanitize::limits::MAX_PADDING);
        assert_eq!(p.hotkeys.region, rs_core::Hotkeys::default().region);
        std::fs::write(&path, br#"{"window_capture_shadow": false, "window_capture_transparent": true}"#).unwrap();
        let p = load_preferences_from(&path);
        assert!(!p.window_capture_shadow && p.window_capture_transparent);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tolerant_of_partial_and_unknown_fields() {
        let dir = std::env::temp_dir().join(format!("rs-smart-prefs-{}", uuid::Uuid::new_v4()));
        let path = dir.join("preferences.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, br#"{"instant_copy": true, "from_the_future": {"a": 1}}"#).unwrap();
        let p = load_preferences_from(&path);
        assert!(p.instant_copy);
        assert_eq!(p.hotkeys, Preferences::default().hotkeys);
        let _ = std::fs::remove_dir_all(dir);
    }
}
