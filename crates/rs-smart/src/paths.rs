//! App data locations.
//!
//! Base: `$RAPIDSNAP_DATA_DIR` if set (tests, portable), else
//! `~/Library/Application Support/io.github.ahmedash95.rapidsnap`
//! (the bundle id RapidSnap v1 shipped with; v2 is its successor).
//!
//! ```text
//! <base>/preferences.json
//! <base>/backgrounds/<uuid>.<ext>      custom background library
//! <base>/backgrounds/index.json
//! <base>/history/<uuid>.rsnap         recent captures
//! <base>/history/<uuid>.thumb.png
//! <base>/history/index.json
//! ```
//!
//! Pre-release v2 builds used `com.rapidsnap.v2`; [`migrate_legacy_data_dir`]
//! moves that folder's contents over once at startup.

use std::path::{Path, PathBuf};

/// Bundle identifier / data folder name (same as RapidSnap v1).
pub const APP_ID: &str = "io.github.ahmedash95.rapidsnap";
/// Data folder name used by pre-release v2 builds (bundle id `com.rapidsnap.v2`).
pub const LEGACY_APP_ID: &str = "com.rapidsnap.v2";
/// Env var overriding the data dir.
pub const DATA_DIR_ENV: &str = "RAPIDSNAP_DATA_DIR";
/// Lock file of the single-instance guard (transient; never migrated).
pub const LOCK_FILE_NAME: &str = "rapidsnap.lock";

/// Base data directory (not created).
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os(DATA_DIR_ENV) {
        return PathBuf::from(p);
    }
    default_base().join(APP_ID)
}

fn default_base() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir)
}

pub fn preferences_path() -> PathBuf {
    data_dir().join("preferences.json")
}

pub fn backgrounds_dir() -> PathBuf {
    data_dir().join("backgrounds")
}

pub fn history_dir() -> PathBuf {
    data_dir().join("history")
}

/// What [`migrate_legacy_data_dir_from`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Migration {
    /// Entry names moved from the legacy dir into the new one.
    pub moved: Vec<String>,
    /// Entry names left in the legacy dir (name already taken in the new
    /// dir, the lock file, or a failed move — see the log).
    pub skipped: Vec<String>,
}

/// One-time startup migration of the pre-release v2 data folder
/// (`~/Library/Application Support/com.rapidsnap.v2`) into [`data_dir`].
/// Does nothing when `$RAPIDSNAP_DATA_DIR` is set. Call before anything
/// creates files in the data dir. Returns `None` when nothing was to do.
pub fn migrate_legacy_data_dir() -> Option<Migration> {
    if std::env::var_os(DATA_DIR_ENV).is_some() {
        return None;
    }
    let base = default_base();
    migrate_legacy_data_dir_from(&base.join(LEGACY_APP_ID), &base.join(APP_ID))
}

/// Move the contents of `legacy` into `new` when `new` has no
/// `preferences.json` yet and `legacy` is a directory. Entries whose name
/// already exists in `new` stay where they are; the lock file is never moved;
/// nothing is deleted (the emptied legacy folder itself is kept). Logs what
/// it does. Returns `None` when the migration doesn't apply.
pub fn migrate_legacy_data_dir_from(legacy: &Path, new: &Path) -> Option<Migration> {
    if new.join("preferences.json").exists() || !legacy.is_dir() {
        return None;
    }
    let entries = match std::fs::read_dir(legacy) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("data migration: can't read {legacy:?}: {e}");
            return None;
        }
    };
    if let Err(e) = std::fs::create_dir_all(new) {
        log::warn!("data migration: can't create {new:?}: {e}");
        return None;
    }
    let mut out = Migration::default();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy().into_owned();
        let dest = new.join(&name);
        if name_s == LOCK_FILE_NAME || dest.symlink_metadata().is_ok() {
            out.skipped.push(name_s);
            continue;
        }
        match std::fs::rename(entry.path(), &dest) {
            Ok(()) => out.moved.push(name_s),
            Err(e) => {
                log::warn!("data migration: can't move {:?} → {dest:?}: {e}", entry.path());
                out.skipped.push(name_s);
            }
        }
    }
    out.moved.sort();
    out.skipped.sort();
    log::info!(
        "data migration: moved {:?} from {legacy:?} to {new:?} (left in place: {:?})",
        out.moved,
        out.skipped
    );
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("rs-smart-paths-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn moves_legacy_contents_into_new_dir() {
        let t = TempDir::new();
        let legacy = t.0.join(LEGACY_APP_ID);
        let new = t.0.join(APP_ID);
        write(&legacy.join("preferences.json"), "{\"old\":1}");
        write(&legacy.join("history/index.json"), "[]");
        write(&legacy.join("backgrounds/a.png"), "png");
        write(&legacy.join(LOCK_FILE_NAME), "");

        let m = migrate_legacy_data_dir_from(&legacy, &new).expect("migrated");
        assert_eq!(m.moved, vec!["backgrounds", "history", "preferences.json"]);
        assert_eq!(m.skipped, vec![LOCK_FILE_NAME]);
        assert_eq!(std::fs::read_to_string(new.join("preferences.json")).unwrap(), "{\"old\":1}");
        assert!(new.join("history/index.json").is_file());
        assert!(new.join("backgrounds/a.png").is_file());
        // Nothing deleted: legacy folder and its lock file remain.
        assert!(legacy.join(LOCK_FILE_NAME).is_file());
        assert!(!legacy.join("preferences.json").exists());

        // Second run: new dir has preferences → no-op.
        assert_eq!(migrate_legacy_data_dir_from(&legacy, &new), None);
    }

    #[test]
    fn skipped_when_new_dir_has_preferences() {
        let t = TempDir::new();
        let legacy = t.0.join("legacy");
        let new = t.0.join("new");
        write(&legacy.join("preferences.json"), "old");
        write(&new.join("preferences.json"), "new");
        assert_eq!(migrate_legacy_data_dir_from(&legacy, &new), None);
        assert_eq!(std::fs::read_to_string(new.join("preferences.json")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(legacy.join("preferences.json")).unwrap(), "old");
    }

    #[test]
    fn skipped_when_no_legacy_dir() {
        let t = TempDir::new();
        let new = t.0.join("new");
        assert_eq!(migrate_legacy_data_dir_from(&t.0.join("missing"), &new), None);
        assert!(!new.exists(), "nothing created");
    }

    #[test]
    fn existing_entries_in_new_dir_are_kept() {
        let t = TempDir::new();
        let legacy = t.0.join("legacy");
        let new = t.0.join("new");
        write(&legacy.join("preferences.json"), "old");
        write(&legacy.join("history/index.json"), "old-history");
        write(&new.join("history/index.json"), "new-history");
        write(&new.join(LOCK_FILE_NAME), "");

        let m = migrate_legacy_data_dir_from(&legacy, &new).expect("migrated");
        assert_eq!(m.moved, vec!["preferences.json"]);
        assert_eq!(m.skipped, vec!["history"]);
        assert_eq!(std::fs::read_to_string(new.join("history/index.json")).unwrap(), "new-history");
        assert_eq!(std::fs::read_to_string(legacy.join("history/index.json")).unwrap(), "old-history");
    }
}
