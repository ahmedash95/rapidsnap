//! ImageIO decoding — fallback for formats the `image` crate can't read
//! (HEIC/HEIF, JPEG 2000, exotic TIFF, …). Thread-safe.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFData, CFDictionary};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use objc2_image_io::{CGImageSource, kCGImagePropertyDPIWidth, kCGImagePropertyOrientation};

use super::cgimage::cgimage_to_rgba;
use crate::{CaptureError, Result};

pub(crate) struct Decoded {
    pub image: rs_core::RgbaImage,
    /// EXIF orientation (1–8), 1 = none.
    pub orientation: u8,
    pub dpi: Option<f64>,
}

fn prop_number(props: &NSDictionary<NSString, AnyObject>, k: &objc2_core_foundation::CFString) -> Option<Retained<NSNumber>> {
    let k: &NSString = unsafe { &*(k as *const objc2_core_foundation::CFString as *const NSString) };
    props.objectForKey(k)?.downcast::<NSNumber>().ok()
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Decoded> {
    let data = CFData::from_bytes(bytes);
    let src = unsafe { CGImageSource::with_data(&data, None) }
        .ok_or_else(|| CaptureError::Unsupported("unrecognised image data".into()))?;
    if unsafe { src.count() } == 0 {
        return Err(CaptureError::Unsupported("image contains no frames".into()));
    }
    let idx = unsafe { src.primary_image_index() };
    let cg = unsafe { src.image_at_index(idx, None) }
        .ok_or_else(|| CaptureError::Unsupported("ImageIO could not decode image".into()))?;
    let image = cgimage_to_rgba(&cg)?;
    let (mut orientation, mut dpi) = (1u8, None);
    if let Some(props) = unsafe { src.properties_at_index(idx, None) } {
        // SAFETY: CFDictionary<CFString, CFType> is toll-free bridged.
        let props: &NSDictionary<NSString, AnyObject> =
            unsafe { &*(&*props as *const CFDictionary as *const NSDictionary<NSString, AnyObject>) };
        if let Some(o) = prop_number(props, unsafe { kCGImagePropertyOrientation }) {
            orientation = o.as_u8().clamp(1, 8);
        }
        dpi = prop_number(props, unsafe { kCGImagePropertyDPIWidth }).map(|n| n.as_f64());
    }
    Ok(Decoded { image, orientation, dpi })
}
