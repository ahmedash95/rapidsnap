//! NSPasteboard image I/O. `NSPasteboard` is usable from any thread.

use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageRep, NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use crate::{CaptureError, Result};

/// Image payload found on the pasteboard.
pub(crate) enum PasteboardImage {
    /// Encoded image data (PNG / TIFF / JPEG / HEIC …).
    Data(Vec<u8>),
    /// A copied image file (Finder ⌘C).
    File(PathBuf),
}

fn data_for(pb: &NSPasteboard, ty: &NSPasteboardType) -> Option<Vec<u8>> {
    let d = pb.dataForType(ty)?;
    (d.length() > 0).then(|| d.to_vec())
}

pub(crate) fn read_image() -> Option<PasteboardImage> {
    let pb = NSPasteboard::generalPasteboard();
    unsafe {
        if let Some(d) = data_for(&pb, NSPasteboardTypePNG) {
            return Some(PasteboardImage::Data(d));
        }
        if let Some(d) = data_for(&pb, NSPasteboardTypeTIFF) {
            return Some(PasteboardImage::Data(d));
        }
    }
    for ty in ["public.jpeg", "public.heic", "public.heif", "org.webmproject.webp", "com.compuserve.gif"] {
        if let Some(d) = data_for(&pb, &NSString::from_str(ty)) {
            return Some(PasteboardImage::Data(d));
        }
    }
    let url = pb.stringForType(unsafe { NSPasteboardTypeFileURL })?;
    let url = NSURL::URLWithString(&url)?;
    let path = PathBuf::from(url.path()?.to_string());
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    crate::SUPPORTED_IMPORT_EXTENSIONS.contains(&ext.as_str()).then_some(PasteboardImage::File(path))
}

/// Replace the pasteboard contents with a PNG plus a TIFF representation
/// (derived by AppKit from the same PNG, so DPI metadata carries over).
/// PNG is what browsers/Slack/Figma read; TIFF is the legacy AppKit type
/// some apps (Keynote, Mail, older Office) prefer.
pub(crate) fn write_png(png: &[u8]) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    let data = NSData::with_bytes(png);
    let tiff: Option<Retained<NSData>> = NSBitmapImageRep::imageRepWithData(&data).and_then(|r| r.TIFFRepresentation());
    pb.clearContents();
    let ok = unsafe { pb.setData_forType(Some(&data), NSPasteboardTypePNG) };
    if let Some(t) = tiff {
        unsafe { pb.setData_forType(Some(&t), NSPasteboardTypeTIFF) };
    }
    if !ok {
        return Err(CaptureError::Clipboard("NSPasteboard rejected PNG data".into()));
    }
    Ok(())
}

/// Put a file reference on the pasteboard (paste into Finder / Mail /
/// Slack attaches the file).
pub(crate) fn write_file_url(path: &Path) -> Result<()> {
    let path = path.canonicalize()?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let obj: Retained<ProtocolObject<dyn NSPasteboardWriting>> = ProtocolObject::from_retained(url);
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    if !pb.writeObjects(&NSArray::from_retained_slice(&[obj])) {
        return Err(CaptureError::Clipboard("NSPasteboard rejected file URL".into()));
    }
    Ok(())
}

/// Replace the pasteboard contents with plain text.
pub(crate) fn write_text(text: &str) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let ok = unsafe { pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
    if !ok {
        return Err(CaptureError::Clipboard("NSPasteboard rejected text".into()));
    }
    Ok(())
}

/// Every item on the general pasteboard with every (type, data) pair.
/// Lazily promised data is resolved while reading.
pub(crate) fn snapshot() -> Vec<Vec<(String, Vec<u8>)>> {
    let pb = NSPasteboard::generalPasteboard();
    let Some(items) = pb.pasteboardItems() else { return Vec::new() };
    items
        .iter()
        .map(|item| {
            item.types()
                .iter()
                .filter_map(|ty| item.dataForType(&ty).map(|d| (ty.to_string(), d.to_vec())))
                .collect()
        })
        .collect()
}

/// Replace the general pasteboard with a [`snapshot`] (empty → cleared).
pub(crate) fn restore(items: &[Vec<(String, Vec<u8>)>]) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    if items.is_empty() {
        return Ok(());
    }
    if let [only] = items {
        // Single item (the common case): write each type straight to the
        // pasteboard server. `NSPasteboardItem` data can stay in-process
        // until someone asks for it, and is lost if we exit abruptly.
        for (ty, data) in only {
            if !pb.setData_forType(Some(&NSData::with_bytes(data)), &NSString::from_str(ty)) {
                return Err(CaptureError::Clipboard(format!("NSPasteboard rejected restored type {ty}")));
            }
        }
        return Ok(());
    }
    let objs: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = items
        .iter()
        .map(|entries| {
            let item = NSPasteboardItem::new();
            for (ty, data) in entries {
                item.setData_forType(&NSData::with_bytes(data), &NSString::from_str(ty));
            }
            ProtocolObject::from_retained(item)
        })
        .collect();
    if !pb.writeObjects(&NSArray::from_retained_slice(&objs)) {
        return Err(CaptureError::Clipboard("NSPasteboard rejected restored items".into()));
    }
    Ok(())
}
