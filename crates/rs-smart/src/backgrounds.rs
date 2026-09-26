//! Custom background image library (persisted, v1 parity: add / remove).
//!
//! Images are **copied** into `paths::backgrounds_dir()` as `<uuid>.<ext>`;
//! `index.json` (a JSON array of [`BackgroundItem`]) keeps order and display
//! names. Use the item's `path` in `rs_core::Background::CustomImage { path }`.
//!
//! Writes are atomic and serialized in-process. Blocking file I/O (`add_*`
//! decodes the image header to validate it) — background executor preferred.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fsutil::{atomic_write, remove_file_if_exists};
use crate::{paths, Result, SmartError};

/// Extensions accepted by the library.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif"];

/// One library entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundItem {
    pub id: Uuid,
    /// Display name (source file stem).
    pub name: String,
    /// Absolute path of the copied image inside the library.
    pub path: PathBuf,
}

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// The library (a directory + index).
#[derive(Debug, Clone)]
pub struct BackgroundLibrary {
    dir: PathBuf,
}

impl BackgroundLibrary {
    /// Library at the default location.
    pub fn open_default() -> Self {
        Self::open(paths::backgrounds_dir())
    }

    /// Library at `dir` (created lazily on first add).
    pub fn open(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    /// Entries in insertion order. Missing files are skipped; a corrupt index
    /// is treated as empty (logged).
    pub fn list(&self) -> Result<Vec<BackgroundItem>> {
        let _g = lock();
        self.read_locked()
    }

    /// One entry by id.
    pub fn get(&self, id: Uuid) -> Result<BackgroundItem> {
        self.list()?.into_iter().find(|i| i.id == id).ok_or(SmartError::NotFound)
    }

    /// True if `path` points at an image inside this library (e.g. to decide
    /// whether a `Background::CustomImage` is still available).
    pub fn contains_path(&self, path: &Path) -> bool {
        self.list().map(|l| l.iter().any(|i| i.path == path)).unwrap_or(false)
    }

    fn read_locked(&self) -> Result<Vec<BackgroundItem>> {
        let items = match std::fs::read(self.index_path()) {
            Ok(b) => match serde_json::from_slice::<Vec<BackgroundItem>>(&b) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("background index corrupt ({e}); starting empty");
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(items
            .into_iter()
            .map(|mut i| {
                // Relocate if the data dir moved.
                if !i.path.is_file()
                    && let Some(name) = i.path.file_name()
                {
                    i.path = self.dir.join(name);
                }
                i
            })
            .filter(|i| i.path.is_file())
            .collect())
    }

    fn write_locked(&self, items: &[BackgroundItem]) -> Result<()> {
        atomic_write(&self.index_path(), &serde_json::to_vec_pretty(items)?)
    }

    /// Copy `source` into the library (validates it decodes as an image).
    pub fn add_from_file(&self, source: &Path) -> Result<BackgroundItem> {
        let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let (w, h) = image::image_dimensions(source)?;
        if w == 0 || h == 0 {
            return Err(SmartError::Platform(format!("{source:?}: empty image")));
        }
        let ext = if SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
            ext
        } else {
            // Unknown/missing extension but decodable: name by detected format.
            image::ImageReader::open(source)?
                .with_guessed_format()?
                .format()
                .and_then(|f| f.extensions_str().first().copied())
                .unwrap_or("png")
                .to_string()
        };
        let name = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = std::fs::read(source)?;
        self.insert(&bytes, &ext, name)
    }

    /// Add encoded image bytes (e.g. from a drop or the clipboard).
    pub fn add_from_bytes(&self, bytes: &[u8], name: &str) -> Result<BackgroundItem> {
        let fmt = image::guess_format(bytes)?;
        let img = image::ImageReader::with_format(std::io::Cursor::new(bytes), fmt).into_dimensions()?;
        if img.0 == 0 || img.1 == 0 {
            return Err(SmartError::Platform("empty image".into()));
        }
        let ext = fmt.extensions_str().first().copied().unwrap_or("png");
        self.insert(bytes, ext, name.to_string())
    }

    fn insert(&self, bytes: &[u8], ext: &str, name: String) -> Result<BackgroundItem> {
        let id = Uuid::new_v4();
        let path = self.dir.join(format!("{id}.{ext}"));
        atomic_write(&path, bytes)?;
        let item = BackgroundItem { id, name, path };
        let _g = lock();
        let mut items = self.read_locked()?;
        items.push(item.clone());
        if let Err(e) = self.write_locked(&items) {
            let _ = std::fs::remove_file(&item.path);
            return Err(e);
        }
        Ok(item)
    }

    /// Change an entry's display name.
    pub fn rename(&self, id: Uuid, name: &str) -> Result<()> {
        let _g = lock();
        let mut items = self.read_locked()?;
        let item = items.iter_mut().find(|i| i.id == id).ok_or(SmartError::NotFound)?;
        item.name = name.to_string();
        self.write_locked(&items)
    }

    /// Remove an entry and delete its file.
    pub fn remove(&self, id: Uuid) -> Result<()> {
        let _g = lock();
        let mut items = self.read_locked()?;
        let pos = items.iter().position(|i| i.id == id).ok_or(SmartError::NotFound)?;
        let item = items.remove(pos);
        self.write_locked(&items)?;
        remove_file_if_exists(&item.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_remove() {
        let dir = std::env::temp_dir().join(format!("rs-smart-bg-{}", Uuid::new_v4()));
        let lib = BackgroundLibrary::open(dir.clone());
        assert!(lib.list().unwrap().is_empty());
        let src = dir.with_extension("png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255])).save(&src).unwrap();
        let item = lib.add_from_file(&src).unwrap();
        assert_eq!(item.name, src.file_stem().unwrap().to_string_lossy());
        assert_eq!(lib.list().unwrap(), vec![item.clone()]);
        assert!(lib.contains_path(&item.path));
        lib.rename(item.id, "Sunset").unwrap();
        assert_eq!(lib.get(item.id).unwrap().name, "Sunset");

        let bytes = std::fs::read(&src).unwrap();
        let b2 = lib.add_from_bytes(&bytes, "pasted").unwrap();
        assert_eq!(b2.path.extension().unwrap(), "png");
        assert_eq!(lib.list().unwrap().len(), 2);

        lib.remove(item.id).unwrap();
        assert!(!item.path.exists());
        assert!(matches!(lib.remove(item.id), Err(SmartError::NotFound)));
        assert_eq!(lib.list().unwrap(), vec![b2.clone()]);

        // Rejects non-images.
        let bad = dir.with_extension("txt");
        std::fs::write(&bad, b"hello").unwrap();
        assert!(lib.add_from_file(&bad).is_err());
        assert!(lib.add_from_bytes(b"hello", "x").is_err());

        // Corrupt index → empty list, not an error.
        std::fs::write(dir.join("index.json"), b"[{").unwrap();
        assert!(lib.list().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_file(bad);
    }
}
