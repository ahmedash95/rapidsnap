//! Pure (platform-independent) region / multi-display / pixel math.
//!
//! Everything here works on global screen points (CG global, y down) and
//! physical pixels, and is unit-tested without any screen access.

use rs_core::{CaptureSource, Color, IRect, Point, Rect, RgbaImage};

use crate::{CaptureError, Captured, DisplayInfo, Result, WindowInfo};

/// Tolerance used when snapping point coordinates to the pixel grid, so that
/// values like `99.99998 * 2` don't round to an extra pixel.
const SNAP_EPS: f32 = 1e-3;

/// One display's contribution to a (possibly multi-display) region capture.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionPart {
    /// Display the pixels come from.
    pub display_id: u32,
    /// Source area in **display-local points** (origin = display top-left).
    pub source_rect: Rect,
    /// That display's px/pt.
    pub source_scale: f32,
    /// Destination area in the output image, in output pixels.
    pub dest: IRect,
}

/// How to assemble a region capture from one or more displays.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionPlan {
    /// The region actually captured: the request clipped (when a display was
    /// forced) and snapped outward to the output pixel grid (global points).
    pub rect: Rect,
    /// Output px per pt = the highest scale factor among the covered
    /// displays (lower-DPI parts are upscaled, so nothing loses detail).
    pub scale_factor: f32,
    /// Output size in pixels.
    pub width: u32,
    pub height: u32,
    /// One entry per covered display (areas covered by no display stay
    /// transparent).
    pub parts: Vec<RegionPart>,
}

fn snap_down(v: f32, s: f32) -> f32 {
    ((v * s) + SNAP_EPS).floor() / s
}

fn snap_up(v: f32, s: f32) -> f32 {
    ((v * s) - SNAP_EPS).ceil() / s
}

/// Plan a region capture of `rect` (global points).
///
/// * `display_id = Some(id)`: only that display; the rect is clipped to it.
/// * `None`: every display the rect intersects (mixed DPI → the output uses
///   the highest scale factor).
///
/// Errors: `Unsupported` for an empty rect, `NotFound` if the rect touches
/// no (or not the given) display.
pub fn plan_region(rect: Rect, displays: &[DisplayInfo], display_id: Option<u32>) -> Result<RegionPlan> {
    let rect = rect.normalized();
    if rect.is_empty() || !rect.x.is_finite() || !rect.y.is_finite() {
        return Err(CaptureError::Unsupported("empty capture region".into()));
    }
    let (rect, candidates): (Rect, Vec<&DisplayInfo>) = match display_id {
        Some(id) => {
            let d = displays.iter().find(|d| d.id == id).ok_or(CaptureError::NotFound)?;
            let clipped = rect.intersection(&d.frame).ok_or(CaptureError::NotFound)?;
            (clipped, vec![d])
        }
        None => (rect, displays.iter().filter(|d| d.frame.intersects(&rect)).collect()),
    };
    if candidates.is_empty() {
        return Err(CaptureError::NotFound);
    }
    let s = candidates.iter().map(|d| d.scale_factor.max(0.1)).fold(0.0f32, f32::max);

    let x0 = snap_down(rect.x, s);
    let y0 = snap_down(rect.y, s);
    let x1 = snap_up(rect.right(), s).max(x0 + 1.0 / s);
    let y1 = snap_up(rect.bottom(), s).max(y0 + 1.0 / s);
    let snapped = Rect::new(x0, y0, x1 - x0, y1 - y0);
    let width = ((x1 - x0) * s).round().max(1.0) as u32;
    let height = ((y1 - y0) * s).round().max(1.0) as u32;

    let to_px = |v: f32, origin: f32, max: u32| (((v - origin) * s).round().max(0.0) as u32).min(max);
    let mut parts = Vec::new();
    for d in candidates {
        let Some(inter) = snapped.intersection(&d.frame) else { continue };
        let dx0 = to_px(inter.x, x0, width);
        let dy0 = to_px(inter.y, y0, height);
        let dx1 = to_px(inter.right(), x0, width);
        let dy1 = to_px(inter.bottom(), y0, height);
        if dx1 <= dx0 || dy1 <= dy0 {
            continue;
        }
        parts.push(RegionPart {
            display_id: d.id,
            source_rect: inter.translate(-d.frame.x, -d.frame.y),
            source_scale: d.scale_factor,
            dest: IRect::new(dx0, dy0, dx1 - dx0, dy1 - dy0),
        });
    }
    if parts.is_empty() {
        return Err(CaptureError::NotFound);
    }
    Ok(RegionPlan { rect: snapped, scale_factor: s, width, height, parts })
}

/// Copy `src_rect` (in `src` pixels, fractional allowed) of `src` into
/// `dest_rect` of `dst` (tight RGBA, `dst_w` wide). Exact 1:1 copies are
/// memcpy'd; otherwise nearest-neighbour resampling (sharp pixels, like the
/// system does when upscaling a 1× display into a 2× composite).
pub(crate) fn blit(src: &RgbaImage, src_rect: Rect, dst: &mut [u8], dst_w: u32, dest_rect: IRect) {
    if !src.is_loaded() || dest_rect.width == 0 || dest_rect.height == 0 || src.width == 0 || src.height == 0 {
        return;
    }
    let dst_h = (dst.len() / 4) as u32 / dst_w.max(1);
    let (sw, sh) = (src.width as i64, src.height as i64);
    let same_size = (src_rect.width - dest_rect.width as f32).abs() < 0.01
        && (src_rect.height - dest_rect.height as f32).abs() < 0.01;
    let sx0 = src_rect.x.round() as i64;
    let sy0 = src_rect.y.round() as i64;
    if same_size && (src_rect.x - sx0 as f32).abs() < 0.01 && (src_rect.y - sy0 as f32).abs() < 0.01 {
        for row in 0..dest_rect.height as i64 {
            let sy = sy0 + row;
            let dy = dest_rect.y as i64 + row;
            if sy < 0 || sy >= sh || dy >= dst_h as i64 {
                continue;
            }
            // Horizontal clip.
            let mut sx = sx0;
            let mut dx = dest_rect.x as i64;
            let mut n = dest_rect.width as i64;
            if sx < 0 {
                dx -= sx;
                n += sx;
                sx = 0;
            }
            n = n.min(sw - sx).min(dst_w as i64 - dx);
            if n <= 0 {
                continue;
            }
            let s_off = ((sy * sw + sx) * 4) as usize;
            let d_off = ((dy * dst_w as i64 + dx) * 4) as usize;
            dst[d_off..d_off + n as usize * 4].copy_from_slice(&src.pixels[s_off..s_off + n as usize * 4]);
        }
        return;
    }
    let kx = src_rect.width / dest_rect.width as f32;
    let ky = src_rect.height / dest_rect.height as f32;
    for row in 0..dest_rect.height {
        let dy = dest_rect.y + row;
        if dy >= dst_h {
            break;
        }
        let sy = (src_rect.y + (row as f32 + 0.5) * ky).floor() as i64;
        if sy < 0 || sy >= sh {
            continue;
        }
        for col in 0..dest_rect.width {
            let dx = dest_rect.x + col;
            if dx >= dst_w {
                break;
            }
            let sx = (src_rect.x + (col as f32 + 0.5) * kx).floor() as i64;
            if sx < 0 || sx >= sw {
                continue;
            }
            let s_off = ((sy * sw + sx) * 4) as usize;
            let d_off = ((dy * dst_w + dx) * 4) as usize;
            dst[d_off..d_off + 4].copy_from_slice(&src.pixels[s_off..s_off + 4]);
        }
    }
}

/// Crop a region (global points, may span displays) out of whole-display
/// captures — e.g. the freeze-frame from `capture_all_displays`, so the
/// result is pixel-identical to what the overlay showed. Mixed DPI: output
/// at the highest scale among covered displays, lower-DPI parts upscaled
/// (nearest). Areas outside every display are transparent.
pub fn crop_region_from_displays(captures: &[(DisplayInfo, Captured)], rect: Rect) -> Result<Captured> {
    let displays: Vec<DisplayInfo> = captures.iter().map(|(d, _)| d.clone()).collect();
    let plan = plan_region(rect, &displays, None)?;
    let mut out = vec![0u8; plan.width as usize * plan.height as usize * 4];
    for part in &plan.parts {
        let Some((_, cap)) = captures.iter().find(|(d, _)| d.id == part.display_id) else { continue };
        let s = cap.scale_factor;
        blit(&cap.image, part.source_rect.scale(s), &mut out, plan.width, part.dest);
    }
    Ok(Captured {
        image: RgbaImage::new(plan.width, plan.height, out).ok_or(CaptureError::Platform("region size".into()))?,
        scale_factor: plan.scale_factor,
        source: CaptureSource::Region,
        frame: Some(plan.rect),
    })
}

/// The display containing `pt` (global points), if any.
pub fn display_at_point(displays: &[DisplayInfo], pt: Point) -> Option<&DisplayInfo> {
    displays.iter().find(|d| d.frame.contains(pt))
}

/// Front-most window under `pt` (global points) from a front-to-back list
/// as returned by `list_windows` — for the overlay's window-hover highlight.
pub fn window_at_point(windows: &[WindowInfo], pt: Point) -> Option<&WindowInfo> {
    windows.iter().find(|w| w.frame.contains(pt))
}

/// Physical pixel coordinate of global point `pt` inside a capture of
/// `display` (None if outside).
pub fn point_to_pixel(capture: &Captured, display: &DisplayInfo, pt: Point) -> Option<(u32, u32)> {
    let s = capture.scale_factor;
    let x = ((pt.x - display.frame.x) * s).floor();
    let y = ((pt.y - display.frame.y) * s).floor();
    if x < 0.0 || y < 0.0 || x >= capture.image.width as f32 || y >= capture.image.height as f32 {
        return None;
    }
    Some((x as u32, y as u32))
}

/// Color of the pixel under `pt` (global points) in the freeze-frame
/// captures (loupe readout). Straight RGBA.
pub fn sample_pixel(captures: &[(DisplayInfo, Captured)], pt: Point) -> Option<Color> {
    let (d, cap) = captures.iter().find(|(d, _)| d.frame.contains(pt))?;
    let (x, y) = point_to_pixel(cap, d, pt)?;
    let [r, g, b, a] = cap.image.pixel(x, y)?;
    Some(Color::rgba(r, g, b, a))
}

/// A `(2·radius+1)²` pixel grid centred on the pixel under `pt` (global
/// points) — the loupe's magnified content. Pixels outside the capture are
/// transparent. Display it with nearest-neighbour scaling.
pub fn sample_grid(capture: &Captured, display: &DisplayInfo, pt: Point, radius: u32) -> RgbaImage {
    let side = radius * 2 + 1;
    let mut out = vec![0u8; side as usize * side as usize * 4];
    let s = capture.scale_factor;
    let cx = ((pt.x - display.frame.x) * s).floor() as i64;
    let cy = ((pt.y - display.frame.y) * s).floor() as i64;
    let (w, h) = (capture.image.width as i64, capture.image.height as i64);
    if capture.image.is_loaded() {
        for gy in 0..side as i64 {
            let sy = cy - radius as i64 + gy;
            if sy < 0 || sy >= h {
                continue;
            }
            for gx in 0..side as i64 {
                let sx = cx - radius as i64 + gx;
                if sx < 0 || sx >= w {
                    continue;
                }
                let s_off = ((sy * w + sx) * 4) as usize;
                let d_off = ((gy * side as i64 + gx) * 4) as usize;
                out[d_off..d_off + 4].copy_from_slice(&capture.image.pixels[s_off..s_off + 4]);
            }
        }
    }
    RgbaImage::new(side, side, out).unwrap_or_else(|| RgbaImage::filled(side, side, [0, 0, 0, 0]))
}

/// Crop fully transparent borders (alpha == 0). Returns the image unchanged
/// if it is entirely transparent. Used for shadowed window captures.
pub(crate) fn trim_transparent(img: &RgbaImage) -> (RgbaImage, IRect) {
    let (w, h) = (img.width as usize, img.height as usize);
    let full = IRect::new(0, 0, img.width, img.height);
    if !img.is_loaded() || w == 0 || h == 0 {
        return (img.clone(), full);
    }
    let a = |x: usize, y: usize| img.pixels[(y * w + x) * 4 + 3];
    let row_empty = |y: usize| (0..w).all(|x| a(x, y) == 0);
    let col_empty = |x: usize, y0: usize, y1: usize| (y0..y1).all(|y| a(x, y) == 0);
    let mut y0 = 0;
    while y0 < h && row_empty(y0) {
        y0 += 1;
    }
    if y0 == h {
        return (img.clone(), full);
    }
    let mut y1 = h;
    while y1 > y0 && row_empty(y1 - 1) {
        y1 -= 1;
    }
    let mut x0 = 0;
    while x0 < w && col_empty(x0, y0, y1) {
        x0 += 1;
    }
    let mut x1 = w;
    while x1 > x0 && col_empty(x1 - 1, y0, y1) {
        x1 -= 1;
    }
    if (x0, y0, x1, y1) == (0, 0, w, h) {
        return (img.clone(), full);
    }
    let r = IRect::new(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
    let mut out = vec![0u8; r.width as usize * r.height as usize * 4];
    blit(img, r.to_rect(), &mut out, r.width, IRect::new(0, 0, r.width, r.height));
    (RgbaImage::new(r.width, r.height, out).unwrap_or_else(|| img.clone()), r)
}

/// Flatten a straight-alpha `fg` (e.g. a shadowed single-window capture)
/// onto `bg` (the on-screen pixels behind it), source-over. The output has
/// `fg`'s size; `bg` is aligned **centered** under `fg` (they're captured
/// from the same frame, so sizes match up to rounding) and pixels outside
/// `bg` fall back to opaque white. The result is fully opaque when `bg` is.
pub(crate) fn composite_over(fg: &RgbaImage, bg: &RgbaImage) -> RgbaImage {
    let (w, h) = (fg.width as usize, fg.height as usize);
    if !fg.is_loaded() {
        return fg.clone();
    }
    let (bw, bh) = (bg.width as i64, bg.height as i64);
    let ox = (bw - w as i64) / 2;
    let oy = (bh - h as i64) / 2;
    let bg_ok = bg.is_loaded();
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let f = &fg.pixels[i..i + 4];
            let (bx, by) = (x as i64 + ox, y as i64 + oy);
            let b: [u8; 4] = if bg_ok && bx >= 0 && by >= 0 && bx < bw && by < bh {
                let j = ((by * bw + bx) * 4) as usize;
                [bg.pixels[j], bg.pixels[j + 1], bg.pixels[j + 2], bg.pixels[j + 3]]
            } else {
                [255, 255, 255, 255]
            };
            let fa = f[3] as u32;
            let ba = b[3] as u32;
            // out_a = fa + ba·(1−fa), straight-alpha colour blend.
            let oa = fa * 255 + ba * (255 - fa); // ×255
            if oa == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            for c in 0..3 {
                let v = (f[c] as u32 * fa * 255 + b[c] as u32 * ba * (255 - fa) + oa / 2) / oa;
                out.push(v.min(255) as u8);
            }
            out.push(((oa + 127) / 255).min(255) as u8);
        }
    }
    RgbaImage::new(fg.width, fg.height, out).unwrap_or_else(|| fg.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disp(id: u32, x: f32, y: f32, w: f32, h: f32, s: f32) -> DisplayInfo {
        DisplayInfo { id, name: format!("d{id}"), frame: Rect::new(x, y, w, h), scale_factor: s, is_main: id == 1 }
    }

    fn cap_filled(d: &DisplayInfo, rgba: [u8; 4]) -> Captured {
        let w = (d.frame.width * d.scale_factor) as u32;
        let h = (d.frame.height * d.scale_factor) as u32;
        Captured {
            image: RgbaImage::filled(w, h, rgba),
            scale_factor: d.scale_factor,
            source: CaptureSource::Fullscreen { display_id: Some(d.id) },
            frame: Some(d.frame),
        }
    }

    /// Image whose pixel (x,y) encodes its coordinates.
    fn cap_coords(d: &DisplayInfo) -> Captured {
        let w = (d.frame.width * d.scale_factor) as u32;
        let h = (d.frame.height * d.scale_factor) as u32;
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, d.id as u8, 255]);
            }
        }
        Captured {
            image: RgbaImage::new(w, h, px).unwrap(),
            scale_factor: d.scale_factor,
            source: CaptureSource::Fullscreen { display_id: Some(d.id) },
            frame: Some(d.frame),
        }
    }

    #[test]
    fn single_display_retina_snaps_to_pixel_grid() {
        let ds = [disp(1, 0.0, 0.0, 1512.0, 982.0, 2.0)];
        let p = plan_region(Rect::new(10.3, 20.6, 100.2, 50.0), &ds, None).unwrap();
        // 10.3*2=20.6 → floor 20 → 10.0pt; 110.5*2=221 → 110.5pt.
        assert_eq!(p.rect, Rect::new(10.0, 20.5, 100.5, 50.5));
        assert_eq!((p.width, p.height), (201, 101));
        assert_eq!(p.scale_factor, 2.0);
        assert_eq!(p.parts.len(), 1);
        assert_eq!(p.parts[0].dest, IRect::new(0, 0, 201, 101));
        assert_eq!(p.parts[0].source_rect, Rect::new(10.0, 20.5, 100.5, 50.5));
    }

    #[test]
    fn integer_rect_is_exact() {
        let ds = [disp(1, 0.0, 0.0, 800.0, 600.0, 2.0)];
        let p = plan_region(Rect::new(100.0, 100.0, 200.0, 100.0), &ds, None).unwrap();
        assert_eq!((p.width, p.height), (400, 200));
        assert_eq!(p.rect, Rect::new(100.0, 100.0, 200.0, 100.0));
    }

    #[test]
    fn spanning_mixed_dpi_displays() {
        // Main retina 1000×800pt @2, secondary 1× to the right, offset down.
        let ds = [disp(1, 0.0, 0.0, 1000.0, 800.0, 2.0), disp(2, 1000.0, 100.0, 1920.0, 1080.0, 1.0)];
        let p = plan_region(Rect::new(900.0, 50.0, 200.0, 100.0), &ds, None).unwrap();
        assert_eq!(p.scale_factor, 2.0);
        assert_eq!((p.width, p.height), (400, 200));
        assert_eq!(p.parts.len(), 2);
        let a = &p.parts[0];
        assert_eq!(a.display_id, 1);
        assert_eq!(a.dest, IRect::new(0, 0, 200, 200));
        assert_eq!(a.source_rect, Rect::new(900.0, 50.0, 100.0, 100.0));
        let b = &p.parts[1];
        assert_eq!(b.display_id, 2);
        // Display 2 starts at y=100 → dest y = (100-50)*2 = 100.
        assert_eq!(b.dest, IRect::new(200, 100, 200, 100));
        assert_eq!(b.source_rect, Rect::new(0.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn negative_origin_display_left_of_main() {
        let ds = [disp(1, 0.0, 0.0, 1000.0, 800.0, 1.0), disp(2, -1280.0, -200.0, 1280.0, 1024.0, 2.0)];
        let p = plan_region(Rect::new(-10.0, 10.0, 20.0, 10.0), &ds, None).unwrap();
        assert_eq!((p.width, p.height), (40, 20));
        let left = p.parts.iter().find(|x| x.display_id == 2).unwrap();
        assert_eq!(left.dest, IRect::new(0, 0, 20, 20));
        assert_eq!(left.source_rect, Rect::new(1270.0, 210.0, 10.0, 10.0));
        let right = p.parts.iter().find(|x| x.display_id == 1).unwrap();
        assert_eq!(right.dest, IRect::new(20, 0, 20, 20));
    }

    #[test]
    fn forced_display_clips() {
        let ds = [disp(1, 0.0, 0.0, 1000.0, 800.0, 2.0), disp(2, 1000.0, 0.0, 1000.0, 800.0, 1.0)];
        let p = plan_region(Rect::new(900.0, 0.0, 200.0, 100.0), &ds, Some(2)).unwrap();
        assert_eq!(p.scale_factor, 1.0);
        assert_eq!(p.rect, Rect::new(1000.0, 0.0, 100.0, 100.0));
        assert_eq!((p.width, p.height), (100, 100));
        assert!(plan_region(Rect::new(0.0, 0.0, 10.0, 10.0), &ds, Some(2)).is_err());
        assert!(plan_region(Rect::new(0.0, 0.0, 10.0, 10.0), &ds, Some(9)).is_err());
    }

    #[test]
    fn empty_or_offscreen_rect_errors() {
        let ds = [disp(1, 0.0, 0.0, 100.0, 100.0, 2.0)];
        assert!(matches!(plan_region(Rect::new(5.0, 5.0, 0.0, 10.0), &ds, None), Err(CaptureError::Unsupported(_))));
        assert!(matches!(plan_region(Rect::new(500.0, 5.0, 10.0, 10.0), &ds, None), Err(CaptureError::NotFound)));
    }

    #[test]
    fn crop_from_displays_mixed_dpi() {
        let d1 = disp(1, 0.0, 0.0, 100.0, 50.0, 2.0);
        let d2 = disp(2, 100.0, 0.0, 100.0, 50.0, 1.0);
        let caps = vec![(d1.clone(), cap_coords(&d1)), (d2.clone(), cap_filled(&d2, [9, 8, 7, 255]))];
        let c = crop_region_from_displays(&caps, Rect::new(90.0, 10.0, 20.0, 5.0)).unwrap();
        assert_eq!((c.image.width, c.image.height), (40, 10));
        assert_eq!(c.scale_factor, 2.0);
        // Left half from display 1 at native px: dest (0,0) = src (180,20).
        assert_eq!(c.image.pixel(0, 0), Some([180, 20, 1, 255]));
        assert_eq!(c.image.pixel(19, 9), Some([199, 29, 1, 255]));
        // Right half from the 1× display (upscaled).
        assert_eq!(c.image.pixel(20, 0), Some([9, 8, 7, 255]));
        assert_eq!(c.image.pixel(39, 9), Some([9, 8, 7, 255]));
    }

    #[test]
    fn crop_from_displays_gap_is_transparent() {
        // Two displays with a vertical offset: region covers an uncovered area.
        let d1 = disp(1, 0.0, 0.0, 100.0, 50.0, 1.0);
        let d2 = disp(2, 100.0, 30.0, 100.0, 50.0, 1.0);
        let caps = vec![(d1.clone(), cap_filled(&d1, [1, 1, 1, 255])), (d2.clone(), cap_filled(&d2, [2, 2, 2, 255]))];
        let c = crop_region_from_displays(&caps, Rect::new(90.0, 0.0, 20.0, 40.0)).unwrap();
        assert_eq!(c.image.pixel(15, 5), Some([0, 0, 0, 0]));
        assert_eq!(c.image.pixel(15, 35), Some([2, 2, 2, 255]));
        assert_eq!(c.image.pixel(5, 5), Some([1, 1, 1, 255]));
    }

    #[test]
    fn crop_matches_crop_to_region_single_display() {
        let d = disp(1, 100.0, 0.0, 100.0, 50.0, 2.0);
        let cap = cap_coords(&d);
        let r = Rect::new(110.0, 10.0, 20.0, 5.0);
        let a = crate::crop_to_region(&cap, &d, r).unwrap();
        let b = crop_region_from_displays(&[(d.clone(), cap)], r).unwrap();
        assert_eq!(a.image.pixels, b.image.pixels);
    }

    #[test]
    fn sampling() {
        let d1 = disp(1, 0.0, 0.0, 100.0, 50.0, 2.0);
        let d2 = disp(2, -50.0, 0.0, 50.0, 50.0, 1.0);
        let caps = vec![(d1.clone(), cap_coords(&d1)), (d2.clone(), cap_filled(&d2, [3, 4, 5, 255]))];
        assert_eq!(sample_pixel(&caps, Point::new(10.25, 5.75)), Some(Color::rgba(20, 11, 1, 255)));
        assert_eq!(sample_pixel(&caps, Point::new(-1.0, 1.0)), Some(Color::rgba(3, 4, 5, 255)));
        assert_eq!(sample_pixel(&caps, Point::new(500.0, 1.0)), None);
        let g = sample_grid(&caps[0].1, &d1, Point::new(0.0, 0.0), 2);
        assert_eq!((g.width, g.height), (5, 5));
        assert_eq!(g.pixel(2, 2), Some([0, 0, 1, 255]));
        assert_eq!(g.pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(g.pixel(4, 3), Some([2, 1, 1, 255]));
    }

    #[test]
    fn hit_testing() {
        let ds = [disp(1, 0.0, 0.0, 100.0, 50.0, 2.0)];
        assert_eq!(display_at_point(&ds, Point::new(5.0, 5.0)).map(|d| d.id), Some(1));
        assert!(display_at_point(&ds, Point::new(100.0, 5.0)).is_none());
        let w = |id, x| WindowInfo {
            id,
            owner_pid: 1,
            app_name: "a".into(),
            title: String::new(),
            frame: Rect::new(x, 0.0, 50.0, 50.0),
            layer: 0,
            display_id: Some(1),
        };
        let ws = [w(7, 10.0), w(8, 0.0)];
        assert_eq!(window_at_point(&ws, Point::new(20.0, 5.0)).map(|w| w.id), Some(7));
        assert_eq!(window_at_point(&ws, Point::new(5.0, 5.0)).map(|w| w.id), Some(8));
    }

    #[test]
    fn trims_transparent_border() {
        let mut px = vec![0u8; 6 * 4 * 4];
        for (x, y) in [(2, 1), (3, 2)] {
            let o = (y * 6 + x) * 4;
            px[o..o + 4].copy_from_slice(&[1, 2, 3, 200]);
        }
        let img = RgbaImage::new(6, 4, px).unwrap();
        let (t, r) = trim_transparent(&img);
        assert_eq!(r, IRect::new(2, 1, 2, 2));
        assert_eq!(t.pixel(0, 0), Some([1, 2, 3, 200]));
        assert_eq!(t.pixel(1, 1), Some([1, 2, 3, 200]));
        let (t2, _) = trim_transparent(&RgbaImage::filled(3, 3, [0, 0, 0, 0]));
        assert_eq!(t2.width, 3);
    }

    #[test]
    fn composite_over_flattens_alpha() {
        let fg = RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 0]).unwrap();
        let bg = RgbaImage::filled(2, 1, [0, 255, 0, 255]);
        let out = super::composite_over(&fg, &bg);
        assert_eq!(&out.pixels[..], &[255, 0, 0, 255, 0, 255, 0, 255]);
        // Half-transparent black over white → mid grey, opaque.
        let fg = RgbaImage::new(1, 1, vec![0, 0, 0, 128]).unwrap();
        let out = super::composite_over(&fg, &RgbaImage::filled(3, 3, [255, 255, 255, 255]));
        assert_eq!(out.pixels[3], 255);
        assert!((out.pixels[0] as i32 - 127).abs() <= 1);
        // Size mismatch: bg missing → white fallback.
        let out = super::composite_over(&RgbaImage::filled(4, 4, [0, 0, 0, 0]), &RgbaImage::filled(1, 1, [0, 0, 0, 255]));
        assert!(out.pixels.chunks(4).all(|p| p[3] == 255));
    }
}
