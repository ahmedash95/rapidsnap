//! `.rsnap` project files.
//!
//! Format: a **zip archive** (deflate) containing:
//!
//! | entry           | content |
//! |-----------------|---------|
//! | `manifest.json` | `{"format":"rsnap","version":1,"app_version":"x.y.z"}` |
//! | `document.json` | `rs_core::Document` as JSON (pixels are skipped by serde) |
//! | `base.png`      | the original base image (lossless) |
//! | `thumbnail.png` | optional preview (≤ 512 px), written by callers that pass one |
//!
//! Loading validates `manifest.version <= PROJECT_VERSION`, decodes `base.png`
//! into `Document::base_image.pixels`, and sets `metadata.project_path`.
//! Custom background / watermark images are referenced by absolute path (not
//! embedded) in v1 of the format.
//!
//! Forward compatibility: unknown JSON fields are ignored, missing fields take
//! serde defaults, and annotations of an unknown kind (written by a newer
//! build) are skipped with a warning instead of failing the whole load.
//! Writes are atomic (temp file + fsync + rename).
//!
//! Threading: blocking (PNG encode/decode); use the background executor.

use std::io::{Cursor, Read, Write};
use std::path::Path;

use rs_core::{Annotation, Document, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::fsutil::atomic_write;
use crate::{Result, SmartError};

/// Current project format version.
pub const PROJECT_VERSION: u32 = 1;
/// File extension (without dot).
pub const PROJECT_EXTENSION: &str = "rsnap";
/// Largest entry we are willing to inflate (guards against zip bombs).
const MAX_ENTRY_BYTES: u64 = 1 << 30;

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    #[serde(default)]
    app_version: String,
}

/// Serialize a project to bytes. `thumbnail` is optional.
pub fn project_to_bytes(doc: &Document, thumbnail: Option<&RgbaImage>) -> Result<Vec<u8>> {
    if !doc.base_image.is_loaded() || doc.base_image.width == 0 || doc.base_image.height == 0 {
        return Err(SmartError::InvalidProject("base image not loaded".into()));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let manifest = Manifest {
        format: "rsnap".into(),
        version: PROJECT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
    };
    zip.start_file("manifest.json", opts)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;

    let mut d = doc.clone();
    d.metadata.project_path = None;
    zip.start_file("document.json", opts)?;
    zip.write_all(&serde_json::to_vec_pretty(&d)?)?;

    // PNG is already compressed: store.
    zip.start_file("base.png", stored)?;
    zip.write_all(&encode_png(&doc.base_image)?)?;

    if let Some(t) = thumbnail.filter(|t| t.is_loaded() && t.width > 0 && t.height > 0) {
        zip.start_file("thumbnail.png", stored)?;
        zip.write_all(&encode_png(t)?)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// Parse a project from bytes.
pub fn project_from_bytes(bytes: &[u8]) -> Result<Document> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let manifest: Manifest = serde_json::from_slice(&read_entry(&mut zip, "manifest.json")?)
        .map_err(|e| SmartError::InvalidProject(format!("manifest.json: {e}")))?;
    if manifest.format != "rsnap" {
        return Err(SmartError::InvalidProject(format!("format {:?}", manifest.format)));
    }
    if manifest.version > PROJECT_VERSION {
        return Err(SmartError::UnsupportedVersion(manifest.version));
    }
    let mut doc = parse_document_lenient(&read_entry(&mut zip, "document.json")?)?;
    let png = read_entry(&mut zip, "base.png")?;
    doc.base_image = decode_png(&png)?;
    // Untrusted input: clamp NaN/inf/negative/huge values, fresh ids for
    // duplicates (rs_core::Document::sanitize).
    doc.sanitize();
    Ok(doc)
}

/// Deserialize `document.json`, skipping annotations that fail to parse.
fn parse_document_lenient(json: &[u8]) -> Result<Document> {
    let mut v: serde_json::Value = serde_json::from_slice(json)?;
    let raw_anns = match v.as_object_mut() {
        Some(o) => o.remove("annotations"),
        None => return Err(SmartError::InvalidProject("document.json is not an object".into())),
    };
    let mut doc: Document = serde_json::from_value(v)?;
    if let Some(serde_json::Value::Array(items)) = raw_anns {
        let mut anns = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            match serde_json::from_value::<Annotation>(item) {
                Ok(a) => anns.push(a),
                Err(e) => log::warn!("rsnap: skipping annotation #{i} (unsupported: {e})"),
            }
        }
        doc.annotations = anns;
    }
    Ok(doc)
}

/// Save to `path` (atomic: temp file + rename). Does not modify `doc`;
/// callers should set `doc.metadata.project_path` and `History::mark_saved`.
pub fn save_project(doc: &Document, path: &Path, thumbnail: Option<&RgbaImage>) -> Result<()> {
    let bytes = project_to_bytes(doc, thumbnail)?;
    atomic_write(path, &bytes)
}

/// Load from `path`; sets `metadata.project_path = Some(path)`.
pub fn load_project(path: &Path) -> Result<Document> {
    let mut doc = project_from_bytes(&std::fs::read(path)?)?;
    doc.metadata.project_path = Some(path.to_path_buf());
    Ok(doc)
}

/// Read only the embedded thumbnail (fast, for history/recents UIs).
pub fn load_project_thumbnail(path: &Path) -> Result<RgbaImage> {
    let bytes = std::fs::read(path)?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let png = match read_entry(&mut zip, "thumbnail.png") {
        Ok(p) => p,
        Err(SmartError::InvalidProject(_)) => return Err(SmartError::NotFound),
        Err(e) => return Err(e),
    };
    decode_png(&png)
}

/// True if `path` has the `.rsnap` extension (case-insensitive).
pub fn is_project_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXTENSION))
}

fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let f = zip
        .by_name(name)
        .map_err(|_| SmartError::InvalidProject(format!("missing {name}")))?;
    if f.size() > MAX_ENTRY_BYTES {
        return Err(SmartError::InvalidProject(format!("{name} too large")));
    }
    let mut out = Vec::with_capacity(f.size() as usize);
    f.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut out)?;
    if out.len() as u64 > MAX_ENTRY_BYTES {
        return Err(SmartError::InvalidProject(format!("{name} too large")));
    }
    Ok(out)
}

fn decode_png(png: &[u8]) -> Result<RgbaImage> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgba8();
    let (w, h) = img.dimensions();
    RgbaImage::new(w, h, img.into_raw()).ok_or_else(|| SmartError::InvalidProject("png size".into()))
}

pub(crate) fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(&img.pixels, img.width, img.height, image::ExtendedColorType::Rgba8)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_core::{Rotation, ImageTransform, IRect};

    #[test]
    fn roundtrip_bytes_and_file() {
        let mut doc = rs_core::test_support::sample_document();
        doc.base_image = RgbaImage::filled(7, 5, [9, 8, 7, 255]);
        let bytes = project_to_bytes(&doc, Some(&RgbaImage::filled(2, 2, [0, 0, 0, 255]))).unwrap();
        let back = project_from_bytes(&bytes).unwrap();
        assert_eq!(back.annotations, doc.annotations);
        assert_eq!(back.base_image, doc.base_image);
        assert_eq!(back.beautify, doc.beautify);

        let path = std::env::temp_dir().join(format!("{}.rsnap", uuid::Uuid::new_v4()));
        save_project(&doc, &path, Some(&RgbaImage::filled(2, 2, [0, 0, 0, 255]))).unwrap();
        let loaded = load_project(&path).unwrap();
        assert_eq!(loaded.metadata.project_path.as_deref(), Some(path.as_path()));
        assert_eq!(load_project_thumbnail(&path).unwrap().width, 2);
        assert!(is_project_path(&path));
        // Without thumbnail → NotFound.
        save_project(&doc, &path, None).unwrap();
        assert!(matches!(load_project_thumbnail(&path), Err(SmartError::NotFound)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn every_annotation_kind_roundtrips() {
        let kinds = rs_core::test_support::all_kinds();
        assert!(kinds.len() >= 15);
        let mut doc = rs_core::test_support::sample_document();
        // Non-uniform pixels to check exact image round-trip (incl. alpha).
        let px: Vec<u8> = (0..(13 * 9 * 4)).map(|i| (i * 37 % 256) as u8).collect();
        doc.base_image = RgbaImage::new(13, 9, px).unwrap();
        doc.image_transform = ImageTransform { crop: Some(IRect::new(1, 1, 10, 6)), rotation: Rotation::Deg90 };
        doc.metadata.title = Some("t".into());
        doc.annotations = kinds.into_iter().map(|k| Annotation::new(k).with_opacity(0.5)).collect();
        let back = project_from_bytes(&project_to_bytes(&doc, None).unwrap()).unwrap();
        for (a, b) in doc.annotations.iter().zip(&back.annotations) {
            assert_eq!(a, b, "kind {:?}", a.tool());
        }
        assert_eq!(back.annotations.len(), doc.annotations.len());
        assert_eq!(back.image_transform, doc.image_transform);
        assert_eq!(back.metadata, doc.metadata);
        assert_eq!(back.id, doc.id);
        assert_eq!(back.base_image, doc.base_image);
    }

    fn rezip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (n, b) in entries {
            zip.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn forward_compatible() {
        let mut doc = rs_core::test_support::sample_document();
        doc.base_image = RgbaImage::filled(3, 3, [1, 2, 3, 255]);
        let n = doc.annotations.len();
        let mut dj: serde_json::Value = serde_json::to_value(&doc).unwrap();
        dj["some_future_field"] = serde_json::json!({"x": 1});
        dj["annotations"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id": uuid::Uuid::new_v4(), "kind": {"type": "hologram", "power": 9000}}));
        let manifest = serde_json::json!({"format": "rsnap", "version": 1, "app_version": "9.9", "extra": true});
        let bytes = rezip(&[
            ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
            ("document.json", serde_json::to_vec(&dj).unwrap()),
            ("base.png", encode_png(&doc.base_image).unwrap()),
            ("future.bin", vec![1, 2, 3]),
        ]);
        let back = project_from_bytes(&bytes).unwrap();
        assert_eq!(back.annotations.len(), n, "unknown annotation skipped");
        assert_eq!(back.annotations, doc.annotations);
    }

    #[test]
    fn hostile_values_are_sanitized_on_load() {
        let mut doc = rs_core::test_support::sample_document();
        doc.base_image = RgbaImage::filled(3, 3, [1, 2, 3, 255]);
        let dup = doc.annotations[0].id;
        let mut dj: serde_json::Value = serde_json::to_value(&doc).unwrap();
        dj["metadata"]["scale_factor"] = serde_json::json!(-7.0);
        dj["beautify"]["padding"]["top"] = serde_json::json!(-1e30);
        dj["beautify"]["corner_radius"] = serde_json::json!(1e30);
        for a in dj["annotations"].as_array_mut().unwrap() {
            a["id"] = serde_json::json!(dup);
            a["opacity"] = serde_json::json!(-4.0);
        }
        let bytes = rezip(&[
            ("manifest.json", br#"{"format":"rsnap","version":1}"#.to_vec()),
            ("document.json", serde_json::to_vec(&dj).unwrap()),
            ("base.png", encode_png(&doc.base_image).unwrap()),
        ]);
        let back = project_from_bytes(&bytes).unwrap();
        assert_eq!(back.metadata.scale_factor, 1.0);
        assert_eq!(back.beautify.padding.top, 0.0);
        assert_eq!(back.beautify.corner_radius, rs_core::sanitize::limits::MAX_FRAME_RADIUS);
        let ids: std::collections::HashSet<_> = back.annotations.iter().map(|a| a.id).collect();
        assert_eq!(ids.len(), back.annotations.len());
        assert!(back.annotations.iter().all(|a| a.opacity == 0.0));
    }

    #[test]
    fn rejects_newer_and_invalid() {
        let doc = {
            let mut d = rs_core::test_support::sample_document();
            d.base_image = RgbaImage::filled(3, 3, [1, 2, 3, 255]);
            d
        };
        let dj = serde_json::to_vec(&doc).unwrap();
        let png = encode_png(&doc.base_image).unwrap();
        let newer = rezip(&[
            ("manifest.json", br#"{"format":"rsnap","version":99}"#.to_vec()),
            ("document.json", dj.clone()),
            ("base.png", png.clone()),
        ]);
        assert!(matches!(project_from_bytes(&newer), Err(SmartError::UnsupportedVersion(99))));
        let wrong = rezip(&[("manifest.json", br#"{"format":"zip","version":1}"#.to_vec())]);
        assert!(matches!(project_from_bytes(&wrong), Err(SmartError::InvalidProject(_))));
        let missing = rezip(&[("manifest.json", br#"{"format":"rsnap","version":1}"#.to_vec()), ("document.json", dj)]);
        assert!(matches!(project_from_bytes(&missing), Err(SmartError::InvalidProject(_))));
        assert!(project_from_bytes(b"not a zip").is_err());
        let mut empty = doc.clone();
        empty.base_image = RgbaImage::filled(0, 0, [0; 4]);
        assert!(project_to_bytes(&empty, None).is_err());
    }
}
