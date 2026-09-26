//! Recent captures history (local only).
//!
//! Layout in `paths::history_dir()`:
//! ```text
//! index.json            {"version":1,"entries":[HistoryEntry…]}   newest first
//! <uuid>.rsnap          full project (base image + annotations + thumbnail)
//! <uuid>.thumb.png      ≤ 512 px thumbnail (fast list rendering)
//! ```
//! All writes are atomic (temp + rename). Index mutations are serialized
//! within the process by a mutex. A missing or corrupt index is rebuilt by
//! scanning the `.rsnap` files, so the store self-heals.
//!
//! Threading: all methods are blocking file I/O (`add` encodes PNGs: ~10–200 ms
//! for large captures) — call from the background executor. `HistoryStore` is
//! `Clone + Send + Sync`.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rs_core::{CaptureSource, Document, RgbaImage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fsutil::{atomic_write, remove_file_if_exists};
use crate::{paths, project, Result, SmartError};

/// Longest side of stored thumbnails (px).
pub const THUMBNAIL_MAX_SIDE: u32 = 512;
/// Default number of entries kept (mirrors `Preferences::history_limit`).
pub const DEFAULT_HISTORY_LIMIT: u32 = 50;
const INDEX_VERSION: u32 = 1;

/// One history entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: Uuid,
    /// Unix epoch ms.
    pub created_at_ms: i64,
    pub project_path: PathBuf,
    /// ≤ 512 px PNG.
    pub thumbnail_path: PathBuf,
    /// Base image size px.
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub source: CaptureSource,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<HistoryEntry>,
}

static INDEX_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    INDEX_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// The history store.
#[derive(Debug, Clone)]
pub struct HistoryStore {
    dir: PathBuf,
}

impl HistoryStore {
    pub fn open_default() -> Self {
        Self::open(paths::history_dir())
    }

    pub fn open(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn project_path(&self, id: Uuid) -> PathBuf {
        self.dir.join(format!("{id}.{}", project::PROJECT_EXTENSION))
    }

    fn thumb_path(&self, id: Uuid) -> PathBuf {
        self.dir.join(format!("{id}.thumb.png"))
    }

    /// Newest first. Entries whose project file vanished are dropped.
    pub fn list(&self) -> Result<Vec<HistoryEntry>> {
        let _g = lock();
        self.read_index_locked()
    }

    /// Store a document (+ its rendered thumbnail, any size — downscaled to
    /// ≤ [`THUMBNAIL_MAX_SIDE`]). Then prunes to `limit` entries
    /// (0 = keep nothing, i.e. no-op returning `NotFound`).
    pub fn add(&self, doc: &Document, thumbnail: &RgbaImage, limit: u32) -> Result<HistoryEntry> {
        if limit == 0 {
            return Err(SmartError::NotFound);
        }
        if !doc.base_image.is_loaded() {
            return Err(SmartError::InvalidProject("base image not loaded".into()));
        }
        std::fs::create_dir_all(&self.dir)?;
        let id = Uuid::new_v4();
        let thumb = if thumbnail.is_loaded() && thumbnail.width > 0 && thumbnail.height > 0 {
            make_thumbnail(thumbnail, THUMBNAIL_MAX_SIDE)
        } else {
            make_thumbnail(&doc.base_image, THUMBNAIL_MAX_SIDE)
        };
        let project_path = self.project_path(id);
        let thumbnail_path = self.thumb_path(id);
        atomic_write(&thumbnail_path, &crate::project::encode_png(&thumb)?)?;
        if let Err(e) = project::save_project(doc, &project_path, Some(&thumb)) {
            let _ = std::fs::remove_file(&thumbnail_path);
            return Err(e);
        }
        let created_at_ms = if doc.metadata.created_at_ms > 0 { doc.metadata.created_at_ms } else { now_ms() };
        let entry = HistoryEntry {
            id,
            created_at_ms,
            project_path,
            thumbnail_path,
            width: doc.base_image.width,
            height: doc.base_image.height,
            source: doc.metadata.source.clone(),
        };
        let _g = lock();
        let mut entries = self.read_index_locked()?;
        entries.retain(|e| e.id != id);
        entries.insert(0, entry.clone());
        sort_newest_first(&mut entries);
        self.prune_vec(&mut entries, limit as usize);
        self.write_index_locked(&entries)?;
        Ok(entry)
    }

    /// Keep only the newest `limit` entries (deletes the rest's files).
    pub fn prune(&self, limit: u32) -> Result<()> {
        let _g = lock();
        let mut entries = self.read_index_locked()?;
        let before = entries.len();
        self.prune_vec(&mut entries, limit as usize);
        if entries.len() != before {
            self.write_index_locked(&entries)?;
        }
        Ok(())
    }

    /// Look up one entry.
    pub fn get(&self, id: Uuid) -> Result<HistoryEntry> {
        self.list()?.into_iter().find(|e| e.id == id).ok_or(SmartError::NotFound)
    }

    /// Load an entry's document (fully decoded, editable). `metadata.project_path`
    /// is cleared so "Save project" asks for a real location instead of
    /// overwriting the history copy.
    pub fn load(&self, id: Uuid) -> Result<Document> {
        let e = self.get(id)?;
        // `load_project` → `project_from_bytes` already runs
        // `Document::sanitize` (untrusted file on disk).
        let mut doc = project::load_project(&e.project_path)?;
        doc.metadata.project_path = None;
        Ok(doc)
    }

    /// Load an entry's thumbnail (falls back to the one embedded in the project).
    pub fn load_thumbnail(&self, id: Uuid) -> Result<RgbaImage> {
        let e = self.get(id)?;
        match image::open(&e.thumbnail_path) {
            Ok(img) => {
                let img = img.to_rgba8();
                let (w, h) = img.dimensions();
                RgbaImage::new(w, h, img.into_raw()).ok_or(SmartError::NotFound)
            }
            Err(_) => project::load_project_thumbnail(&e.project_path),
        }
    }

    pub fn remove(&self, id: Uuid) -> Result<()> {
        let _g = lock();
        let mut entries = self.read_index_locked()?;
        let pos = entries.iter().position(|e| e.id == id).ok_or(SmartError::NotFound)?;
        let e = entries.remove(pos);
        self.write_index_locked(&entries)?;
        delete_entry_files(&e);
        Ok(())
    }

    /// Delete every entry (and stray history files).
    pub fn clear(&self) -> Result<()> {
        let _g = lock();
        let entries = self.read_index_locked().unwrap_or_default();
        for e in &entries {
            delete_entry_files(e);
        }
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for f in rd.flatten() {
                let p = f.path();
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if name.ends_with(".rsnap") || name.ends_with(".thumb.png") || name.ends_with(".tmp") {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        remove_file_if_exists(&self.index_path())?;
        Ok(())
    }

    // ---- internals (call with the lock held) ----

    fn prune_vec(&self, entries: &mut Vec<HistoryEntry>, limit: usize) {
        if entries.len() > limit {
            for e in entries.drain(limit..) {
                delete_entry_files(&e);
            }
        }
    }

    fn read_index_locked(&self) -> Result<Vec<HistoryEntry>> {
        let parsed = match std::fs::read(self.index_path()) {
            Ok(bytes) => match serde_json::from_slice::<IndexFile>(&bytes) {
                Ok(idx) => Some(idx.entries),
                Err(e) => {
                    log::warn!("history index corrupt ({e}); rebuilding");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let mut entries = match parsed {
            Some(v) => v,
            None => {
                let rebuilt = self.rebuild()?;
                if !rebuilt.is_empty() {
                    let _ = self.write_index_locked(&rebuilt);
                }
                rebuilt
            }
        };
        // Relocate paths if the data dir moved; drop entries whose project is gone.
        for e in entries.iter_mut() {
            if !e.project_path.is_file() {
                e.project_path = self.project_path(e.id);
            }
            if !e.thumbnail_path.is_file() {
                e.thumbnail_path = self.thumb_path(e.id);
            }
        }
        entries.retain(|e| e.project_path.is_file());
        sort_newest_first(&mut entries);
        Ok(entries)
    }

    fn write_index_locked(&self, entries: &[HistoryEntry]) -> Result<()> {
        let idx = IndexFile { version: INDEX_VERSION, entries: entries.to_vec() };
        atomic_write(&self.index_path(), &serde_json::to_vec_pretty(&idx)?)
    }

    /// Scan `<uuid>.rsnap` files (reads only `document.json`, no PNG decode).
    fn rebuild(&self) -> Result<Vec<HistoryEntry>> {
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for f in rd.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some(project::PROJECT_EXTENSION) {
                continue;
            }
            let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| Uuid::parse_str(s).ok()) else {
                continue;
            };
            match peek_document(&p) {
                Ok(doc) => out.push(HistoryEntry {
                    id,
                    created_at_ms: if doc.metadata.created_at_ms > 0 {
                        doc.metadata.created_at_ms
                    } else {
                        file_mtime_ms(&p)
                    },
                    project_path: p.clone(),
                    thumbnail_path: self.thumb_path(id),
                    width: doc.base_image.width,
                    height: doc.base_image.height,
                    source: doc.metadata.source,
                }),
                Err(e) => log::warn!("history: skipping unreadable {p:?}: {e}"),
            }
        }
        sort_newest_first(&mut out);
        Ok(out)
    }
}

fn delete_entry_files(e: &HistoryEntry) {
    let _ = std::fs::remove_file(&e.project_path);
    let _ = std::fs::remove_file(&e.thumbnail_path);
}

fn sort_newest_first(v: &mut [HistoryEntry]) {
    v.sort_by_key(|e| std::cmp::Reverse(e.created_at_ms));
}

/// Read only `document.json` of a project (pixels not decoded).
fn peek_document(path: &Path) -> Result<Document> {
    let bytes = std::fs::read(path)?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut f = zip.by_name("document.json").map_err(|_| SmartError::InvalidProject("missing document.json".into()))?;
    let mut s = Vec::new();
    f.read_to_end(&mut s)?;
    Ok(serde_json::from_slice(&s)?)
}

fn file_mtime_ms(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Downscale (aspect preserved, triangle filter) so the longest side is
/// ≤ `max_side`. Returns a clone when already small enough.
pub fn make_thumbnail(img: &RgbaImage, max_side: u32) -> RgbaImage {
    let (w, h) = (img.width, img.height);
    let max_side = max_side.max(1);
    if !img.is_loaded() || (w <= max_side && h <= max_side) {
        return img.clone();
    }
    let s = max_side as f64 / w.max(h) as f64;
    let nw = ((w as f64 * s).round() as u32).max(1);
    let nh = ((h as f64 * s).round() as u32).max(1);
    let Some(src) = image::RgbaImage::from_raw(w, h, img.pixels.as_ref().clone()) else {
        return img.clone();
    };
    let out = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle);
    RgbaImage::new(nw, nh, out.into_raw()).unwrap_or_else(|| img.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(ms: i64) -> Document {
        let mut d = rs_core::test_support::sample_document();
        d.base_image = RgbaImage::filled(900, 300, [10, 20, 30, 255]);
        d.metadata.created_at_ms = ms;
        d
    }

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("rs-smart-hist-{}", Uuid::new_v4()))
    }

    #[test]
    fn add_list_load_remove_clear() {
        let dir = tmp();
        let store = HistoryStore::open(dir.clone());
        assert!(store.list().unwrap().is_empty());
        let da = doc(1_000);
        let a = store.add(&da, &RgbaImage::filled(1024, 512, [1, 1, 1, 255]), 50).unwrap();
        let b = store.add(&doc(2_000), &RgbaImage::new(0, 0, Vec::new()).unwrap(), 50).unwrap();
        let l = store.list().unwrap();
        assert_eq!(l.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b.id, a.id], "newest first");
        assert_eq!((a.width, a.height), (900, 300));
        let t = store.load_thumbnail(a.id).unwrap();
        assert_eq!((t.width, t.height), (512, 256));
        let t = store.load_thumbnail(b.id).unwrap();
        assert_eq!((t.width, t.height), (512, 171));
        let d = store.load(a.id).unwrap();
        assert_eq!(d.annotations, da.annotations);
        assert_eq!(d.base_image, da.base_image);
        assert!(d.metadata.project_path.is_none());
        store.remove(a.id).unwrap();
        assert!(matches!(store.remove(a.id), Err(SmartError::NotFound)));
        assert!(!a.project_path.exists() && !a.thumbnail_path.exists());
        assert_eq!(store.list().unwrap().len(), 1);
        store.clear().unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(!b.project_path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prunes_to_limit() {
        let dir = tmp();
        let store = HistoryStore::open(dir.clone());
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(store.add(&doc(1_000 + i), &RgbaImage::filled(4, 4, [0; 4]), 3).unwrap());
        }
        let l = store.list().unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].id, ids[4].id);
        assert!(!ids[0].project_path.exists(), "pruned files deleted");
        store.prune(1).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(matches!(store.add(&doc(1), &RgbaImage::new(0, 0, Vec::new()).unwrap(), 0), Err(SmartError::NotFound)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rebuilds_corrupt_index() {
        let dir = tmp();
        let store = HistoryStore::open(dir.clone());
        let a = store.add(&doc(5_000), &RgbaImage::filled(4, 4, [0; 4]), 10).unwrap();
        let b = store.add(&doc(6_000), &RgbaImage::filled(4, 4, [0; 4]), 10).unwrap();
        std::fs::write(dir.join("index.json"), b"{garbage").unwrap();
        let l = store.list().unwrap();
        assert_eq!(l.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b.id, a.id]);
        assert_eq!(l[0].width, 900);
        std::fs::remove_file(dir.join("index.json")).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_adds() {
        let dir = tmp();
        let store = HistoryStore::open(dir.clone());
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let s = store.clone();
                std::thread::spawn(move || s.add(&doc(100 + i), &RgbaImage::filled(4, 4, [0; 4]), 50).map(|_| ()))
            })
            .collect();
        for h in hs {
            h.join().unwrap().unwrap();
        }
        assert_eq!(store.list().unwrap().len(), 8);
        let _ = std::fs::remove_dir_all(dir);
    }
}
