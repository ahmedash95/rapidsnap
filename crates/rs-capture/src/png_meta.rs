//! Minimal PNG metadata helpers: read / write the `pHYs` (pixel density)
//! chunk so Retina captures round-trip their scale factor (macOS writes
//! 144 dpi for 2× screenshots; apps use it to paste at the right size).

const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const INCH_PER_METER: f64 = 0.0254;

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Iterate PNG chunks: (type, data, start offset of the chunk).
fn chunks(png: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8], usize)> {
    let mut off = if png.len() >= 8 && &png[..8] == PNG_SIG { 8 } else { png.len() };
    std::iter::from_fn(move || {
        if off + 12 > png.len() {
            return None;
        }
        let len = u32::from_be_bytes(png[off..off + 4].try_into().ok()?) as usize;
        let ty: [u8; 4] = png[off + 4..off + 8].try_into().ok()?;
        let end = off.checked_add(12)?.checked_add(len)?;
        if end > png.len() {
            return None;
        }
        let start = off;
        let data = &png[off + 8..off + 8 + len];
        off = end;
        Some((ty, data, start))
    })
}

/// Horizontal DPI from the `pHYs` chunk (only when the unit is metres).
pub fn png_dpi(png: &[u8]) -> Option<f64> {
    for (ty, data, _) in chunks(png) {
        if &ty == b"pHYs" && data.len() == 9 {
            let ppm_x = u32::from_be_bytes(data[0..4].try_into().ok()?);
            if data[8] == 1 && ppm_x > 0 {
                return Some(ppm_x as f64 * INCH_PER_METER);
            }
            return None;
        }
        if &ty == b"IDAT" {
            break; // pHYs must precede IDAT
        }
    }
    None
}

/// Scale factor implied by a DPI value: 144 dpi → 2.0, 216 → 3.0; values
/// that aren't a clean multiple of 72 (e.g. 300-dpi scans) → 1.0.
pub fn dpi_to_scale(dpi: f64) -> f32 {
    let s = dpi / 72.0;
    let r = s.round();
    if (2.0..=4.0).contains(&r) && (s - r).abs() < 0.05 { r as f32 } else { 1.0 }
}

/// Return `png` with a `pHYs` chunk for `scale × 72` dpi (replacing any
/// existing one). No-op for scale ≤ 1 or malformed input.
pub fn with_scale(png: Vec<u8>, scale: f32) -> Vec<u8> {
    if scale <= 1.0 || !scale.is_finite() || png.len() < 8 || &png[..8] != PNG_SIG {
        return png;
    }
    let ppm = (scale as f64 * 72.0 / INCH_PER_METER).round() as u32;
    let mut chunk = Vec::with_capacity(21);
    chunk.extend_from_slice(&9u32.to_be_bytes());
    let mut body = Vec::with_capacity(13);
    body.extend_from_slice(b"pHYs");
    body.extend_from_slice(&ppm.to_be_bytes());
    body.extend_from_slice(&ppm.to_be_bytes());
    body.push(1);
    chunk.extend_from_slice(&body);
    chunk.extend_from_slice(&crc32(&body).to_be_bytes());

    let mut out = Vec::with_capacity(png.len() + chunk.len());
    out.extend_from_slice(PNG_SIG);
    let mut inserted = false;
    for (ty, data, start) in chunks(&png) {
        let raw = &png[start..start + 12 + data.len()];
        if &ty == b"pHYs" {
            continue;
        }
        out.extend_from_slice(raw);
        if &ty == b"IHDR" && !inserted {
            out.extend_from_slice(&chunk);
            inserted = true;
        }
    }
    if inserted { out } else { png }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn roundtrip_scale() {
        let png = tiny_png();
        assert_eq!(png_dpi(&png), None);
        let p2 = with_scale(png.clone(), 2.0);
        let dpi = png_dpi(&p2).unwrap();
        assert!((dpi - 144.0).abs() < 0.1, "{dpi}");
        assert_eq!(dpi_to_scale(dpi), 2.0);
        // Still decodes.
        let img = image::load_from_memory(&p2).unwrap();
        assert_eq!(img.width(), 2);
        // Replacing keeps one chunk.
        let p3 = with_scale(p2, 3.0);
        assert_eq!(chunks(&p3).filter(|(t, _, _)| t == b"pHYs").count(), 1);
        assert_eq!(dpi_to_scale(png_dpi(&p3).unwrap()), 3.0);
        assert_eq!(with_scale(png.clone(), 1.0), png);
    }

    #[test]
    fn dpi_mapping() {
        assert_eq!(dpi_to_scale(72.0), 1.0);
        assert_eq!(dpi_to_scale(300.0), 1.0);
        assert_eq!(dpi_to_scale(143.9), 2.0);
    }
}
