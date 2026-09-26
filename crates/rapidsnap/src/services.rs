//! `rs_editor::EditorServices` implemented with rs-smart / rs-capture (the
//! editor must not depend on them — CONTRACTS §1). Also owns the history
//! bookkeeping shared by capture-time and editor-copy history adds.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui_kit::{App, Global};
use rs_core::{Document, DocumentId, RgbaImage};
use rs_editor::{CustomBackground, EditorServices, OcrTextLine, SensitiveMatch};
use rs_smart::backgrounds::{BackgroundItem, BackgroundLibrary};
use rs_smart::history::HistoryStore;

/// Document id → history entry id, so re-adding the same document (capture,
/// then copy from the editor with annotations) replaces its entry instead
/// of duplicating it.
#[derive(Clone, Default)]
pub struct HistoryIndex(Arc<Mutex<HashMap<DocumentId, uuid::Uuid>>>);

impl HistoryIndex {
    /// Blocking: add or replace `doc` in the history. Returns the new list.
    pub fn add(&self, doc: &Document, image: &RgbaImage, limit: u32) -> Result<Vec<rs_smart::history::HistoryEntry>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let store = HistoryStore::open_default();
        let previous = self.0.lock().ok().and_then(|m| m.get(&doc.id).copied());
        if let Some(old) = previous {
            let _ = store.remove(old);
        }
        let entry = store.add(doc, image, limit).map_err(|e| e.to_string())?;
        if let Ok(mut m) = self.0.lock() {
            m.insert(doc.id, entry.id);
        }
        store.list().map_err(|e| e.to_string())
    }
}

/// App-wide shared state for services.
#[derive(Clone, Default)]
pub struct ServicesState {
    pub history: HistoryIndex,
    /// Cached desktop wallpaper (resolved in the background).
    pub wallpaper: Arc<Mutex<Option<PathBuf>>>,
}
impl Global for ServicesState {}

pub fn init(cx: &mut App) {
    // The desktop wallpaper is resolved lazily (`prefetch_wallpaper`, on the
    // first capture / open): resolving can decode a multi-megapixel HEIC,
    // which would bloat the idle tray-only process.
    cx.set_global(ServicesState::default());
}

/// Resolve the desktop wallpaper in the background if not known yet.
pub fn prefetch_wallpaper(cx: &mut App) {
    let state = cx.default_global::<ServicesState>().clone();
    if state.wallpaper.lock().ok().is_some_and(|w| w.is_none()) {
        refresh_wallpaper(&state, cx);
    }
}

fn refresh_wallpaper(state: &ServicesState, cx: &App) {
    let slot = state.wallpaper.clone();
    cx.background_executor()
        .spawn(async move {
            match rs_capture::desktop_wallpaper_path(None) {
                Ok(p) => {
                    if let Ok(mut s) = slot.lock() {
                        *s = Some(p);
                    }
                }
                Err(e) => log::debug!("desktop wallpaper: {e}"),
            }
        })
        .detach();
}

fn to_custom(b: BackgroundItem) -> CustomBackground {
    CustomBackground { id: b.id.to_string(), name: b.name, path: b.path }
}

/// Services for a new editor window.
pub fn editor_services(cx: &mut App) -> EditorServices {
    let state = cx.default_global::<ServicesState>().clone();
    let limit = cx.try_global::<rs_editor::PreferencesGlobal>().map(|p| p.0.history_limit).unwrap_or(50);
    let wallpaper = state.wallpaper.lock().ok().and_then(|w| w.clone());
    refresh_wallpaper(&state, cx);
    let history = state.history.clone();
    EditorServices {
        recognize_text: Some(Arc::new(|img: &RgbaImage| {
            rs_smart::ocr::recognize_text(img, &rs_smart::ocr::OcrOptions::for_copy_text())
                .map(|lines| lines.into_iter().map(|l| OcrTextLine { text: l.text, bounds: l.bounds }).collect())
                .map_err(|e| e.to_string())
        })),
        detect_sensitive: Some(Arc::new(|img: &RgbaImage| {
            let lines = rs_smart::ocr::recognize_text(img, &rs_smart::ocr::OcrOptions::for_redaction())
                .map_err(|e| e.to_string())?;
            Ok(rs_smart::redact::detect_sensitive(&lines, &rs_smart::redact::SensitiveKind::ALL)
                .into_iter()
                .map(|d| SensitiveMatch { kind: d.kind.label().to_string(), text: d.text, bounds: d.bounds })
                .collect())
        })),
        save_project: Some(Arc::new(|doc: &Document, path: &std::path::Path, thumb: Option<&RgbaImage>| {
            rs_smart::project::save_project(doc, path, thumb).map_err(|e| e.to_string())
        })),
        load_project: Some(Arc::new(|path: &std::path::Path| {
            rs_smart::project::load_project(path).map_err(|e| e.to_string())
        })),
        add_to_history: Some(Arc::new(move |doc: &Document, img: &RgbaImage| history.add(doc, img, limit).map(|_| ()))),
        // NSPasteboard PNG + TIFF with pHYs = px/pt, instead of GPUI's PNG-only clipboard.
        copy_image: Some(Arc::new(|img: &RgbaImage, _png: &[u8], px_per_pt: f32| {
            rs_capture::copy_image_to_clipboard_with_scale(img, px_per_pt).map_err(|e| e.to_string())
        })),
        list_backgrounds: Some(Rc::new(|| {
            BackgroundLibrary::open_default().list().map(|v| v.into_iter().map(to_custom).collect()).unwrap_or_else(|e| {
                log::warn!("background library: {e}");
                Vec::new()
            })
        })),
        add_background: Some(Rc::new(|path: &std::path::Path| {
            BackgroundLibrary::open_default().add_from_file(path).map(to_custom).map_err(|e| e.to_string())
        })),
        remove_background: Some(Rc::new(|id: &str| {
            let id = uuid::Uuid::parse_str(id).map_err(|e| e.to_string())?;
            BackgroundLibrary::open_default().remove(id).map_err(|e| e.to_string())
        })),
        desktop_wallpaper: wallpaper,
    }
}
