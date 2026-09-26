//! Background rendering with latest-wins coalescing.
//!
//! One dedicated thread per editor window owns a `rs_render::Renderer`
//! (so the renderer never needs to be `Send`). Preview requests replace any
//! pending preview (latest wins); export requests are queued FIFO and always
//! run. Results come back through `smol` channels.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use gpui_kit::RenderImage;
use rs_core::{AnnotationId, Document, ExportSettings, RgbaImage};
use rs_render::{CanvasLayout, RenderOptions, Renderer};

/// A finished preview frame.
pub struct PreviewFrame {
    pub generation: u64,
    pub scale: f32,
    pub image: Arc<RenderImage>,
    /// The same pixels as straight RGBA (shares the render's buffer): the
    /// composed canvas the eyedropper samples (works over the padding).
    pub rgba: RgbaImage,
    pub layout: CanvasLayout,
    pub width: u32,
    pub height: u32,
    pub millis: f32,
}

/// Result of a preview render.
pub type PreviewResult = Result<PreviewFrame, String>;

/// Result of an export: encoded bytes + the rendered RGBA image.
pub type ExportResult = Result<(Vec<u8>, RgbaImage), String>;

struct PreviewJob {
    doc: Document,
    exclude: Vec<AnnotationId>,
    scale: f32,
    generation: u64,
}

struct ExportJob {
    doc: Document,
    settings: ExportSettings,
    reply: smol::channel::Sender<ExportResult>,
}

#[derive(Default)]
struct Queue {
    preview: Option<PreviewJob>,
    exports: VecDeque<ExportJob>,
    desktop_wallpaper: Option<Option<std::path::PathBuf>>,
    shutdown: bool,
}

/// Handle to the render thread. Dropping it stops the thread.
pub struct RenderWorker {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl RenderWorker {
    /// Spawn the worker. Preview results are sent to `results`.
    pub fn spawn(results: smol::channel::Sender<PreviewResult>) -> Self {
        let shared: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let s2 = shared.clone();
        let spawned = std::thread::Builder::new().name("rs-editor-render".into()).spawn(move || {
            let mut renderer = Renderer::new();
            loop {
                let (preview, export, wallpaper) = {
                    let (lock, cv) = &*s2;
                    let Ok(mut q) = lock.lock() else { return };
                    while !q.shutdown && q.preview.is_none() && q.exports.is_empty() && q.desktop_wallpaper.is_none() {
                        q = match cv.wait(q) {
                            Ok(q) => q,
                            Err(_) => return,
                        };
                    }
                    if q.shutdown {
                        return;
                    }
                    // Exports first (user is waiting on them).
                    let export = q.exports.pop_front();
                    let preview = if export.is_none() { q.preview.take() } else { None };
                    (preview, export, q.desktop_wallpaper.take())
                };
                if let Some(w) = wallpaper {
                    renderer.set_desktop_wallpaper(w);
                }
                if let Some(job) = export {
                    let r = rs_render::export(&mut renderer, &job.doc, &job.settings).map_err(|e| e.to_string());
                    let _ = job.reply.send_blocking(r);
                }
                if let Some(job) = preview {
                    let t0 = std::time::Instant::now();
                    let opts = RenderOptions { exclude: job.exclude, ..RenderOptions::preview(job.scale) };
                    let r = renderer.render(&job.doc, &opts).map(|out| PreviewFrame {
                        generation: job.generation,
                        scale: job.scale,
                        width: out.image.width,
                        height: out.image.height,
                        image: crate::rgba_to_render_image(&out.image),
                        rgba: out.image,
                        layout: out.layout,
                        millis: t0.elapsed().as_secs_f32() * 1000.0,
                    });
                    if results.send_blocking(r.map_err(|e| e.to_string())).is_err() {
                        return;
                    }
                }
            }
        });
        if let Err(e) = spawned {
            log::error!("failed to spawn render thread: {e}");
        }
        Self { shared }
    }

    fn with_queue(&self, f: impl FnOnce(&mut Queue)) {
        let (lock, cv) = &*self.shared;
        if let Ok(mut q) = lock.lock() {
            f(&mut q);
            cv.notify_one();
        }
    }

    /// Request a preview (replaces any pending preview request).
    pub fn request_preview(&self, doc: Document, exclude: Vec<AnnotationId>, scale: f32, generation: u64) {
        self.with_queue(|q| q.preview = Some(PreviewJob { doc, exclude, scale, generation }));
    }

    /// Queue an export; await the returned receiver.
    pub fn export(&self, doc: Document, settings: ExportSettings) -> smol::channel::Receiver<ExportResult> {
        let (tx, rx) = smol::channel::bounded(1);
        self.with_queue(|q| q.exports.push_back(ExportJob { doc, settings, reply: tx }));
        rx
    }

    pub fn set_desktop_wallpaper(&self, path: Option<std::path::PathBuf>) {
        self.with_queue(|q| q.desktop_wallpaper = Some(path));
    }
}

impl Drop for RenderWorker {
    fn drop(&mut self) {
        self.with_queue(|q| q.shutdown = true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::{BeautifySettings, DocumentMetadata};

    #[test]
    fn preview_and_export_roundtrip() {
        let (tx, rx) = smol::channel::unbounded();
        let w = RenderWorker::spawn(tx);
        let doc = Document::new(rs_core::RgbaImage::filled(40, 30, [1, 2, 3, 255]), DocumentMetadata::default())
            .with_beautify(BeautifySettings::disabled());
        // Many requests coalesce; the last generation is always delivered.
        for g in 1..=5 {
            w.request_preview(doc.clone(), vec![], 1.0, g);
        }
        let mut last = 0;
        while last != 5 {
            let f = smol::block_on(rx.recv()).unwrap().unwrap();
            assert_eq!((f.width, f.height), (40, 30));
            last = f.generation;
        }
        let r = smol::block_on(w.export(doc, ExportSettings::default()).recv()).unwrap().unwrap();
        assert!(r.0.starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
