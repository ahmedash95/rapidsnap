//! CGImage → straight-alpha sRGB RGBA8 conversion (vImage accelerated, so
//! it stays fast even though this crate is built at -O0 in dev).

use std::ffi::c_void;

use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGImageComponentInfo, kCGColorSpaceSRGB,
};
use rs_core::RgbaImage;

use crate::{CaptureError, Result};

#[repr(C)]
struct VImageBuffer {
    data: *mut c_void,
    height: usize,
    width: usize,
    row_bytes: usize,
}

#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn vImagePermuteChannels_ARGB8888(
        src: *const VImageBuffer,
        dest: *const VImageBuffer,
        permute_map: *const u8,
        flags: u32,
    ) -> isize;
    fn vImageOverwriteChannelsWithScalar_ARGB8888(
        scalar: u8,
        src: *const VImageBuffer,
        dest: *const VImageBuffer,
        copy_mask: u8,
        flags: u32,
    ) -> isize;
    fn vImageUnpremultiplyData_RGBA8888(src: *const VImageBuffer, dest: *const VImageBuffer, flags: u32) -> isize;
}

const KV_IMAGE_NO_FLAGS: u32 = 0;

fn is_srgb(space: Option<&CGColorSpace>) -> bool {
    let Some(space) = space else { return false };
    let Some(name) = CGColorSpace::name(Some(space)) else { return false };
    let srgb: &CFString = unsafe { kCGColorSpaceSRGB };
    *name == *srgb
}

/// In-place unpremultiply of a tight RGBA buffer.
fn unpremultiply(buf: &mut [u8], w: usize, h: usize) -> Result<()> {
    let b = VImageBuffer { data: buf.as_mut_ptr().cast(), height: h, width: w, row_bytes: w * 4 };
    let err = unsafe { vImageUnpremultiplyData_RGBA8888(&b, &b, KV_IMAGE_NO_FLAGS) };
    if err != 0 {
        return Err(CaptureError::Platform(format!("vImageUnpremultiply failed: {err}")));
    }
    Ok(())
}

/// Convert any CGImage into straight-alpha **sRGB** RGBA8 (tight rows).
///
/// Fast path (the ScreenCaptureKit case: 8-bit BGRA / RGBA already tagged
/// sRGB): one vImage permute from the backing store. Anything else is
/// drawn into an sRGB bitmap context (colour-matched by CoreGraphics).
pub(crate) fn cgimage_to_rgba(image: &CGImage) -> Result<RgbaImage> {
    let w = CGImage::width(Some(image));
    let h = CGImage::height(Some(image));
    if w == 0 || h == 0 {
        return Err(CaptureError::Platform("empty CGImage".into()));
    }
    if w > 32_768 || h > 32_768 {
        return Err(CaptureError::Unsupported(format!("image too large: {w}x{h}")));
    }
    let alpha = CGImage::alpha_info(Some(image));
    let order = CGImage::byte_order_info(Some(image));
    let bpc = CGImage::bits_per_component(Some(image));
    let bpp = CGImage::bits_per_pixel(Some(image));
    let bpr = CGImage::bytes_per_row(Some(image));
    let space = CGImage::color_space(Some(image));
    let float = CGImage::bitmap_info(Some(image)).0 & CGImageComponentInfo::Float.0 != 0;

    // (permute map to RGBA, premultiplied?, opaque?)
    let layout: Option<([u8; 4], bool, bool)> = if bpc == 8 && bpp == 32 && !float && is_srgb(space.as_deref()) {
        let little = order == CGImageByteOrderInfo::Order32Little;
        let big = order == CGImageByteOrderInfo::Order32Big || order == CGImageByteOrderInfo::OrderDefault;
        match alpha {
            // Memory B,G,R,A
            a if little && a == CGImageAlphaInfo::PremultipliedFirst => Some(([2, 1, 0, 3], true, false)),
            a if little && a == CGImageAlphaInfo::First => Some(([2, 1, 0, 3], false, false)),
            a if little && a == CGImageAlphaInfo::NoneSkipFirst => Some(([2, 1, 0, 3], false, true)),
            // Memory R,G,B,A
            a if big && a == CGImageAlphaInfo::PremultipliedLast => Some(([0, 1, 2, 3], true, false)),
            a if big && a == CGImageAlphaInfo::Last => Some(([0, 1, 2, 3], false, false)),
            a if big && a == CGImageAlphaInfo::NoneSkipLast => Some(([0, 1, 2, 3], false, true)),
            // Memory A,R,G,B
            a if big && a == CGImageAlphaInfo::PremultipliedFirst => Some(([1, 2, 3, 0], true, false)),
            a if big && a == CGImageAlphaInfo::NoneSkipFirst => Some(([1, 2, 3, 0], false, true)),
            _ => None,
        }
    } else {
        None
    };

    let mut out = vec![0u8; w * h * 4];
    if let Some((map, premul, opaque)) = layout
        && let Some(provider) = CGImage::data_provider(Some(image))
        && let Some(data) = CGDataProvider::data(Some(&provider))
        && data.length() as usize >= bpr * (h - 1) + w * 4
    {
        let bytes = unsafe { data.as_bytes_unchecked() };
        let src = VImageBuffer { data: bytes.as_ptr() as *mut c_void, height: h, width: w, row_bytes: bpr };
        let dst = VImageBuffer { data: out.as_mut_ptr().cast(), height: h, width: w, row_bytes: w * 4 };
        let err = unsafe { vImagePermuteChannels_ARGB8888(&src, &dst, map.as_ptr(), KV_IMAGE_NO_FLAGS) };
        if err != 0 {
            return Err(CaptureError::Platform(format!("vImagePermuteChannels failed: {err}")));
        }
        if opaque {
            // Force alpha (the 4th byte, mask bit 0x1) to 255.
            let err = unsafe { vImageOverwriteChannelsWithScalar_ARGB8888(255, &dst, &dst, 0x1, KV_IMAGE_NO_FLAGS) };
            if err != 0 {
                return Err(CaptureError::Platform(format!("vImageOverwriteChannels failed: {err}")));
            }
        } else if premul {
            unpremultiply(&mut out, w, h)?;
        }
    } else {
        draw_into_rgba(image, &mut out, w, h)?;
        unpremultiply(&mut out, w, h)?;
    }
    RgbaImage::new(w as u32, h as u32, out).ok_or(CaptureError::Platform("image buffer size".into()))
}

fn draw_into_rgba(image: &CGImage, out: &mut [u8], w: usize, h: usize) -> Result<()> {
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or(CaptureError::Platform("no sRGB colour space".into()))?;
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    let ctx: CFRetained<CGContext> =
        unsafe { CGBitmapContextCreate(out.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), info) }
            .ok_or(CaptureError::Platform("CGBitmapContextCreate failed".into()))?;
    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&ctx), rect, Some(image));
    drop(ctx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_graphics::CGBitmapContextCreateImage;

    fn make_image(bytes: &mut [u8], w: usize, h: usize, space: &CGColorSpace, info: u32) -> CFRetained<CGImage> {
        let ctx = unsafe { CGBitmapContextCreate(bytes.as_mut_ptr().cast(), w, h, 8, w * 4, Some(space), info) }
            .expect("context");
        CGBitmapContextCreateImage(Some(&ctx)).expect("image")
    }

    #[test]
    fn bgra_premultiplied_fast_path() {
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
        // Memory B,G,R,A premultiplied: straight (200,100,50,128), opaque (10,20,30).
        let mut px = [25u8, 50, 100, 128, 30, 20, 10, 255];
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
        let img = make_image(&mut px, 2, 1, &srgb, info);
        let out = cgimage_to_rgba(&img).unwrap();
        let p0 = out.pixel(0, 0).unwrap();
        assert!((p0[0] as i32 - 199).abs() <= 1 && (p0[1] as i32 - 100).abs() <= 1 && (p0[2] as i32 - 50).abs() <= 1);
        assert_eq!(p0[3], 128);
        assert_eq!(out.pixel(1, 0), Some([10, 20, 30, 255]));
    }

    #[test]
    fn rgbx_opaque_fast_path() {
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
        let mut px = [1u8, 2, 3, 0, 4, 5, 6, 0];
        let info = CGImageAlphaInfo::NoneSkipLast.0 | CGImageByteOrderInfo::Order32Big.0;
        let img = make_image(&mut px, 2, 1, &srgb, info);
        let out = cgimage_to_rgba(&img).unwrap();
        assert_eq!(out.pixel(0, 0), Some([1, 2, 3, 255]));
        assert_eq!(out.pixel(1, 0), Some([4, 5, 6, 255]));
    }

    #[test]
    fn other_colour_space_is_converted() {
        // Display P3 pure red must map to (clipped) sRGB red, via the draw path.
        let p3 = CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceDisplayP3 })).unwrap();
        let mut px = [255u8, 0, 0, 255];
        let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
        let img = make_image(&mut px, 1, 1, &p3, info);
        let out = cgimage_to_rgba(&img).unwrap();
        let p = out.pixel(0, 0).unwrap();
        assert!(p[0] >= 250 && p[1] <= 10 && p[2] <= 10 && p[3] == 255, "{p:?}");
        // sRGB red from sRGB is exact.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
        let mut px = [255u8, 0, 0, 255];
        let img = make_image(&mut px, 1, 1, &srgb, info);
        assert_eq!(cgimage_to_rgba(&img).unwrap().pixel(0, 0), Some([255, 0, 0, 255]));
    }

    /// `cargo test -p rs-capture -- --ignored --nocapture convert_speed`
    #[test]
    #[ignore]
    fn convert_speed() {
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
        let (w, h) = (6720usize, 3780usize);
        let mut px = vec![200u8; w * h * 4];
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
        let img = make_image(&mut px, w, h, &srgb, info);
        let t = std::time::Instant::now();
        let out = cgimage_to_rgba(&img).unwrap();
        println!("convert {w}x{h}: {:?}", t.elapsed());
        assert_eq!(out.width as usize, w);
    }
}
