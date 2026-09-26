//! Pixel effects: fast gaussian blur (3-pass box approximation, multi-threaded),
//! pixelate, shadows from alpha, fingerprints for caching.
//!
//! All RGBA buffers here are **premultiplied** RGBA8 (tiny-skia layout).

use rs_core::Rect;
use tiny_skia as sk;

/// Largest gaussian sigma (output px) any effect uses. Hand-edited documents /
/// preferences can carry absurd blur / shadow sizes (`1e12`, `inf`); beyond
/// this a blur is visually uniform anyway, and the cap bounds margins,
/// allocations and loop counts.
pub(crate) const MAX_SIGMA: f32 = 1024.0;

/// `sigma` clamped to `0..=MAX_SIGMA` (NaN → 0, +inf → MAX_SIGMA).
pub(crate) fn safe_sigma(sigma: f32) -> f32 {
    if sigma.is_nan() { 0.0 } else { sigma.clamp(0.0, MAX_SIGMA) }
}

/// Pixel count above which blur passes are split across threads.
const PARALLEL_MIN_PX: usize = 64 * 1024;

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 12)
}

/// Box radii whose 3 successive box blurs approximate a gaussian of `sigma`
/// (W. Jarosz / "boxes for gauss").
pub(crate) fn box_radii(sigma: f32) -> [usize; 3] {
    let sigma = safe_sigma(sigma);
    if sigma <= 0.3 {
        return [0, 0, 0];
    }
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let (wlf, s2) = (wl as f32, sigma * sigma);
    let m = ((12.0 * s2 - n * wlf * wlf - 4.0 * n * wlf - 3.0 * n) / (-4.0 * wlf - 4.0)).round() as i32;
    let mut out = [0usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let w = if (i as i32) < m { wl } else { wu };
        *o = ((w - 1) / 2).max(0) as usize;
    }
    out
}

/// One horizontal box blur pass over a single row with clamp-to-edge.
#[inline]
fn box_row<const C: usize>(src: &[u8], dst: &mut [u8], w: usize, r: usize) {
    if r == 0 || w == 0 {
        dst[..w * C].copy_from_slice(&src[..w * C]);
        return;
    }
    let div = (2 * r + 1) as u64;
    let inv: u64 = (1u64 << 32).div_ceil(div);
    let mut acc = [0u64; C];
    let last = w - 1;
    // Initial window centered on x = 0.
    for i in 0..=(2 * r) {
        let x = (i as isize - r as isize).clamp(0, last as isize) as usize;
        for c in 0..C {
            acc[c] += src[x * C + c] as u64;
        }
    }
    for x in 0..w {
        for c in 0..C {
            dst[x * C + c] = ((acc[c] * inv + (1u64 << 31)) >> 32).min(255) as u8;
        }
        let add = (x + r + 1).min(last);
        let sub = x.saturating_sub(r);
        for c in 0..C {
            acc[c] = acc[c] + src[add * C + c] as u64 - src[sub * C + c] as u64;
        }
    }
}

/// Three box passes on every row of `buf` (w×h, C channels), in place.
fn blur_rows<const C: usize>(buf: &mut [u8], w: usize, h: usize, radii: [usize; 3]) {
    let row = w * C;
    let work = |chunk: &mut [u8]| {
        let mut a = vec![0u8; row];
        let mut b = vec![0u8; row];
        for line in chunk.chunks_exact_mut(row) {
            box_row::<C>(line, &mut a, w, radii[0]);
            box_row::<C>(&a, &mut b, w, radii[1]);
            box_row::<C>(&b, line, w, radii[2]);
        }
    };
    if w * h < PARALLEL_MIN_PX || h < 8 {
        work(buf);
        return;
    }
    let n = threads();
    let rows_per = h.div_ceil(n).max(1);
    std::thread::scope(|s| {
        for chunk in buf.chunks_mut(rows_per * row) {
            s.spawn(move || work(chunk));
        }
    });
}

/// Blocked transpose of a w×h image with C channels.
fn transpose<const C: usize>(src: &[u8], dst: &mut [u8], w: usize, h: usize) {
    const B: usize = 32;
    for by in (0..h).step_by(B) {
        for bx in (0..w).step_by(B) {
            for y in by..(by + B).min(h) {
                for x in bx..(bx + B).min(w) {
                    let s = (y * w + x) * C;
                    let d = (x * h + y) * C;
                    dst[d..d + C].copy_from_slice(&src[s..s + C]);
                }
            }
        }
    }
}

/// Gaussian blur (σ px) of a C-channel buffer in place.
pub(crate) fn gaussian_blur<const C: usize>(buf: &mut [u8], w: usize, h: usize, sigma: f32) {
    let radii = box_radii(sigma);
    if radii == [0, 0, 0] || w == 0 || h == 0 {
        return;
    }
    blur_rows::<C>(buf, w, h, radii);
    let mut t = vec![0u8; buf.len()];
    transpose::<C>(buf, &mut t, w, h);
    blur_rows::<C>(&mut t, h, w, radii);
    transpose::<C>(&t, buf, h, w);
    if C == 4 {
        // Keep premultiplied invariant (color <= alpha) after rounding.
        for p in buf.as_chunks_mut::<4>().0.iter_mut() {
            let a = p[3];
            p[0] = p[0].min(a);
            p[1] = p[1].min(a);
            p[2] = p[2].min(a);
        }
    }
}

/// Integer pixel rectangle (may be empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PxRect {
    /// Smallest integer rect covering the float rect (snap outward).
    pub fn cover(x: f32, y: f32, w: f32, h: f32) -> PxRect {
        let x0 = (x + 1e-3).floor() as i32;
        let y0 = (y + 1e-3).floor() as i32;
        let x1 = (x + w - 1e-3).ceil() as i32;
        let y1 = (y + h - 1e-3).ceil() as i32;
        PxRect { x: x0, y: y0, w: x1.saturating_sub(x0).max(0), h: y1.saturating_sub(y0).max(0) }
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn intersect(&self, o: &PxRect) -> PxRect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.x.saturating_add(self.w).min(o.x.saturating_add(o.w));
        let y1 = self.y.saturating_add(self.h).min(o.y.saturating_add(o.h));
        PxRect { x: x0, y: y0, w: x1.saturating_sub(x0).max(0), h: y1.saturating_sub(y0).max(0) }
    }
    pub fn outset(&self, d: i32) -> PxRect {
        let d2 = d.saturating_mul(2);
        PxRect {
            x: self.x.saturating_sub(d),
            y: self.y.saturating_sub(d),
            w: self.w.saturating_add(d2),
            h: self.h.saturating_add(d2),
        }
    }
}

/// Copy a region of a pixmap into a tight buffer.
pub(crate) fn extract(pm: &sk::Pixmap, r: PxRect) -> Vec<u8> {
    let stride = pm.width() as usize * 4;
    let mut out = Vec::with_capacity(r.w as usize * r.h as usize * 4);
    let data = pm.data();
    for y in r.y..r.y + r.h {
        let s = y as usize * stride + r.x as usize * 4;
        out.extend_from_slice(&data[s..s + r.w as usize * 4]);
    }
    out
}

/// Pixelate a tight premultiplied buffer (w×h) with blocks of `block` px
/// aligned to (`ox`, `oy`) (block grid origin, relative to the buffer).
pub(crate) fn pixelate(buf: &mut [u8], w: usize, h: usize, block: f32, ox: f32, oy: f32) {
    let b = block.max(1.0);
    let edges = |origin: f32, len: usize| -> Vec<usize> {
        // Grid lines at origin + k*b, clipped to [0, len].
        let mut v = vec![0usize];
        let k0 = ((0.0 - origin) / b).floor() as i64 + 1;
        let mut k = k0;
        loop {
            let e = (origin + k as f32 * b).round();
            if e >= len as f32 {
                break;
            }
            if e > 0.0 && e as usize > *v.last().unwrap_or(&0) {
                v.push(e as usize);
            }
            k += 1;
        }
        v.push(len);
        v
    };
    let xs = edges(ox, w);
    let ys = edges(oy, h);
    for yw in ys.windows(2) {
        for xw in xs.windows(2) {
            let (x0, x1, y0, y1) = (xw[0], xw[1], yw[0], yw[1]);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                let row = &buf[(y * w + x0) * 4..(y * w + x1) * 4];
                for p in row.as_chunks::<4>().0.iter() {
                    for c in 0..4 {
                        sum[c] += p[c] as u64;
                    }
                }
            }
            let n = ((x1 - x0) * (y1 - y0)).max(1) as u64;
            let avg = [
                ((sum[0] + n / 2) / n) as u8,
                ((sum[1] + n / 2) / n) as u8,
                ((sum[2] + n / 2) / n) as u8,
                ((sum[3] + n / 2) / n) as u8,
            ];
            let avg = [avg[0].min(avg[3]), avg[1].min(avg[3]), avg[2].min(avg[3]), avg[3]];
            for y in y0..y1 {
                for p in buf[(y * w + x0) * 4..(y * w + x1) * 4].as_chunks_mut::<4>().0.iter_mut() {
                    p.copy_from_slice(&avg);
                }
            }
        }
    }
}

/// Write `src` (tight, region `r`) back into `pm`, per pixel:
/// `dst = lerp(dst, src, coverage)` where coverage = clip × shape (both
/// optional; `shape` is region-sized). Coverage 255 = exact replacement
/// (destructive).
pub(crate) fn write_back(
    pm: &mut sk::Pixmap,
    r: PxRect,
    src: &[u8],
    clip: Option<&sk::Mask>,
    shape: Option<&sk::Mask>,
) {
    let cw = pm.width() as usize;
    let stride = cw * 4;
    let rw = r.w as usize;
    let data = pm.data_mut();
    for yy in 0..r.h as usize {
        let cy = r.y as usize + yy;
        let drow = &mut data[cy * stride + r.x as usize * 4..cy * stride + (r.x as usize + rw) * 4];
        let srow = &src[yy * rw * 4..(yy + 1) * rw * 4];
        let crow = clip.map(|m| &m.data()[cy * cw + r.x as usize..cy * cw + r.x as usize + rw]);
        let shrow = shape.map(|m| &m.data()[yy * rw..(yy + 1) * rw]);
        if crow.is_none() && shrow.is_none() {
            drow.copy_from_slice(srow);
            continue;
        }
        let mut xx = 0;
        while xx < rw {
            let cov = |x: usize| -> u32 {
                let mut c = crow.map_or(255, |m| m[x] as u32);
                if let Some(s) = shrow {
                    c = c * s[x] as u32 / 255;
                }
                c
            };
            let c = cov(xx);
            if c >= 255 {
                // Copy the whole fully-covered run at once.
                let start = xx;
                while xx < rw && cov(xx) >= 255 {
                    xx += 1;
                }
                drow[start * 4..xx * 4].copy_from_slice(&srow[start * 4..xx * 4]);
                continue;
            }
            if c > 0 {
                for ch in 0..4 {
                    let s = srow[xx * 4 + ch] as u32;
                    let d = drow[xx * 4 + ch] as u32;
                    drow[xx * 4 + ch] = ((s * c + d * (255 - c) + 127) / 255) as u8;
                }
            }
            xx += 1;
        }
    }
}

/// Source-over a solid color onto region `r` of `pm` with coverage
/// `clip × (255 − holes)` (`holes` is region-sized). Used for the spotlight
/// dim layer.
pub(crate) fn blend_solid(pm: &mut sk::Pixmap, r: PxRect, color: rs_core::Color, clip: Option<&sk::Mask>, holes: &sk::Mask) {
    let ca = color.a as u32;
    if ca == 0 || r.is_empty() {
        return;
    }
    // Premultiplied source.
    let sp = [color.r as u32 * ca / 255, color.g as u32 * ca / 255, color.b as u32 * ca / 255, ca];
    let cw = pm.width() as usize;
    let stride = cw * 4;
    let rw = r.w as usize;
    let (rx, ry) = (r.x as usize, r.y as usize);
    let hdata = holes.data();
    let cdata = clip.map(|m| m.data());
    let rows = &mut pm.data_mut()[ry * stride..(ry + r.h as usize) * stride];
    let work = |y0: usize, chunk: &mut [u8]| {
        for (k, line) in chunk.chunks_exact_mut(stride).enumerate() {
            let yy = y0 + k;
            let cy = ry + yy;
            let drow = &mut line[rx * 4..(rx + rw) * 4];
            let hrow = &hdata[yy * rw..(yy + 1) * rw];
            let crow = cdata.map(|m| &m[cy * cw + rx..cy * cw + rx + rw]);
            for x in 0..rw {
                let mut c = 255 - hrow[x] as u32;
                if let Some(cr) = crow {
                    c = c * cr[x] as u32 / 255;
                }
                if c == 0 {
                    continue;
                }
                let a = sp[3] * c / 255;
                let inv = 255 - a;
                let d = &mut drow[x * 4..x * 4 + 4];
                for ch in 0..4 {
                    let s = sp[ch] * c / 255;
                    d[ch] = (s + (d[ch] as u32 * inv + 127) / 255).min(255) as u8;
                }
            }
        }
    };
    let h = r.h as usize;
    if rw * h < PARALLEL_MIN_PX {
        work(0, rows);
        return;
    }
    let per = h.div_ceil(threads()).max(1);
    std::thread::scope(|s| {
        for (i, chunk) in rows.chunks_mut(per * stride).enumerate() {
            let work = &work;
            s.spawn(move || work(i * per, chunk));
        }
    });
}

/// A blurred, tinted shadow image ready to be composited.
pub(crate) struct ShadowImage {
    pub pm: sk::Pixmap,
    /// Canvas position of the pixmap's top-left.
    pub x: f32,
    pub y: f32,
    /// Downsample factor (pixmap is drawn scaled up by `d`).
    pub d: f32,
}

/// Build a gaussian shadow from an alpha buffer `alpha` (w×h) located at
/// canvas (`x0`, `y0`). `sigma` in canvas px. Large sigmas are computed at
/// reduced resolution (the result is smooth anyway).
pub(crate) fn shadow_from_alpha(
    alpha: &[u8],
    w: usize,
    h: usize,
    x0: f32,
    y0: f32,
    sigma: f32,
    color: rs_core::Color,
) -> Option<ShadowImage> {
    let sigma = safe_sigma(sigma);
    let d = if sigma > 6.0 { (sigma / 3.0).floor().clamp(1.0, 16.0) as usize } else { 1 };
    let pad = (sigma * 3.0).ceil() as usize + d;
    let pw = w + 2 * pad;
    let ph = h + 2 * pad;
    let dw = pw.div_ceil(d);
    let dh = ph.div_ceil(d);
    let mut acc = vec![0u32; dw * dh];
    for y in 0..h {
        let oy = (y + pad) / d;
        let row = &alpha[y * w..y * w + w];
        let arow = &mut acc[oy * dw..oy * dw + dw];
        for (x, &a) in row.iter().enumerate() {
            if a != 0 {
                arow[(x + pad) / d] += a as u32;
            }
        }
    }
    let dd = (d * d) as u32;
    let mut buf: Vec<u8> = acc.iter().map(|&v| ((v + dd / 2) / dd).min(255) as u8).collect();
    gaussian_blur::<1>(&mut buf, dw, dh, sigma / d as f32);
    let mut pm = sk::Pixmap::new(dw as u32, dh as u32)?;
    colorize(&mut pm, &buf, color);
    Some(ShadowImage { pm, x: x0 - pad as f32, y: y0 - pad as f32, d: d as f32 })
}

/// Gaussian shadow of a filled `path` (canvas px), rasterized directly at the
/// reduced working resolution (cheap even for 4K frames).
pub(crate) fn shadow_from_path(path: &sk::Path, sigma: f32, color: rs_core::Color) -> Option<ShadowImage> {
    let sigma = safe_sigma(sigma);
    let d = if sigma > 6.0 { (sigma / 3.0).floor().clamp(1.0, 16.0) } else { 1.0 };
    let b = path.bounds();
    let pad = (sigma * 3.0).ceil() + d;
    let x0 = (b.x() - pad).floor();
    let y0 = (b.y() - pad).floor();
    let dw = ((b.width() + 2.0 * pad) / d).ceil().max(1.0) as u32 + 1;
    let dh = ((b.height() + 2.0 * pad) / d).ceil().max(1.0) as u32 + 1;
    let mut m = sk::Mask::new(dw, dh)?;
    let t = sk::Transform::from_row(1.0 / d, 0.0, 0.0, 1.0 / d, -x0 / d, -y0 / d);
    m.fill_path(path, sk::FillRule::Winding, true, t);
    let mut buf = m.take();
    gaussian_blur::<1>(&mut buf, dw as usize, dh as usize, sigma / d);
    let mut pm = sk::Pixmap::new(dw, dh)?;
    colorize(&mut pm, &buf, color);
    Some(ShadowImage { pm, x: x0, y: y0, d })
}

fn colorize(pm: &mut sk::Pixmap, alpha: &[u8], color: rs_core::Color) {
    let ca = color.a as u32;
    for (p, &a) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(alpha) {
        let a = a as u32 * ca / 255;
        p[0] = ((color.r as u32 * a + 127) / 255) as u8;
        p[1] = ((color.g as u32 * a + 127) / 255) as u8;
        p[2] = ((color.b as u32 * a + 127) / 255) as u8;
        p[3] = a as u8;
    }
}

impl ShadowImage {
    /// Composite only outside `hole` (canvas px) — for shadows under opaque
    /// objects, whose interior would be painted over anyway.
    pub fn draw_outside(&self, canvas: &mut sk::Pixmap, dx: f32, dy: f32, hole: Rect) {
        let t = sk::Transform::from_row(self.d, 0.0, 0.0, self.d, self.x + dx, self.y + dy);
        let q = if self.d > 1.0 { sk::FilterQuality::Bilinear } else { sk::FilterQuality::Nearest };
        let p = sk::Paint {
            shader: sk::Pattern::new(self.pm.as_ref(), sk::SpreadMode::Pad, q, 1.0, t),
            ..Default::default()
        };
        let ox0 = (self.x + dx).floor();
        let oy0 = (self.y + dy).floor();
        let ox1 = (self.x + dx + self.pm.width() as f32 * self.d).ceil();
        let oy1 = (self.y + dy + self.pm.height() as f32 * self.d).ceil();
        let (hx0, hy0) = (hole.x.ceil().max(ox0), hole.y.ceil().max(oy0));
        let (hx1, hy1) = (hole.right().floor().min(ox1), hole.bottom().floor().min(oy1));
        let mut strips = Vec::new();
        if hx1 <= hx0 || hy1 <= hy0 {
            strips.push((ox0, oy0, ox1, oy1));
        } else {
            strips.push((ox0, oy0, ox1, hy0));
            strips.push((ox0, hy1, ox1, oy1));
            strips.push((ox0, hy0, hx0, hy1));
            strips.push((hx1, hy0, ox1, hy1));
        }
        for (x0, y0, x1, y1) in strips {
            if let Some(r) = sk::Rect::from_ltrb(x0, y0, x1, y1) {
                canvas.fill_rect(r, &p, sk::Transform::identity(), None);
            }
        }
    }

    /// Composite onto `canvas` shifted by (`dx`, `dy`).
    pub fn draw(&self, canvas: &mut sk::Pixmap, dx: f32, dy: f32, opacity: f32, clip: Option<&sk::Mask>) {
        let paint = sk::PixmapPaint {
            opacity: opacity.clamp(0.0, 1.0),
            quality: if self.d > 1.0 { sk::FilterQuality::Bilinear } else { sk::FilterQuality::Nearest },
            ..Default::default()
        };
        let t = sk::Transform::from_row(self.d, 0.0, 0.0, self.d, self.x + dx, self.y + dy);
        canvas.draw_pixmap(0, 0, self.pm.as_ref(), &paint, t, clip);
    }
}

/// Integer box downsample of a premultiplied pixmap by `f` (≥ 2).
pub(crate) fn downsample(src: &sk::Pixmap, f: u32) -> Option<sk::Pixmap> {
    let f = f.max(1);
    if f == 1 {
        return Some(src.clone());
    }
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let dw = sw.div_ceil(f as usize).max(1);
    let dh = sh.div_ceil(f as usize).max(1);
    let mut out = sk::Pixmap::new(dw as u32, dh as u32)?;
    let sdata = src.data();
    let f = f as usize;
    let work = |(oy, orow): (usize, &mut [u8])| {
        let y0 = oy * f;
        let y1 = (y0 + f).min(sh);
        let mut acc = vec![0u32; dw * 4];
        let mut cnt = vec![0u32; dw];
        for y in y0..y1 {
            let row = &sdata[y * sw * 4..(y + 1) * sw * 4];
            for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
                let ox = x / f;
                let a = &mut acc[ox * 4..ox * 4 + 4];
                a[0] += p[0] as u32;
                a[1] += p[1] as u32;
                a[2] += p[2] as u32;
                a[3] += p[3] as u32;
                cnt[ox] += 1;
            }
        }
        for ox in 0..dw {
            let n = cnt[ox].max(1);
            for c in 0..4 {
                orow[ox * 4 + c] = ((acc[ox * 4 + c] + n / 2) / n) as u8;
            }
        }
    };
    let rows: Vec<(usize, &mut [u8])> = out.data_mut().chunks_exact_mut(dw * 4).enumerate().collect();
    if sw * sh < PARALLEL_MIN_PX {
        rows.into_iter().for_each(work);
    } else {
        let n = threads();
        let per = dh.div_ceil(n).max(1);
        let mut groups: Vec<Vec<(usize, &mut [u8])>> = Vec::new();
        for (i, r) in rows.into_iter().enumerate() {
            if i % per == 0 {
                groups.push(Vec::new());
            }
            if let Some(g) = groups.last_mut() {
                g.push(r);
            }
        }
        std::thread::scope(|s| {
            for g in groups {
                s.spawn(move || g.into_iter().for_each(work));
            }
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radii_grow_with_sigma() {
        assert_eq!(box_radii(0.0), [0, 0, 0]);
        let a = box_radii(2.0);
        let b = box_radii(10.0);
        assert!(b.iter().sum::<usize>() > a.iter().sum::<usize>());
    }

    #[test]
    fn blur_preserves_flat_and_smooths_edges() {
        let (w, h) = (40, 20);
        let mut buf = vec![0u8; w * h];
        for y in 0..h {
            for x in 20..w {
                buf[y * w + x] = 255;
            }
        }
        gaussian_blur::<1>(&mut buf, w, h, 3.0);
        assert_eq!(buf[10 * w], 0);
        assert_eq!(buf[10 * w + 39], 255);
        let mid = buf[10 * w + 20] as i32;
        assert!((mid - 128).abs() < 40, "{mid}");
    }

    #[test]
    fn pixelate_blocks_uniform() {
        let (w, h) = (8, 8);
        let mut buf: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        pixelate(&mut buf, w, h, 4.0, 0.0, 0.0);
        assert_eq!(buf[0], buf[3 * 4]);
        assert_eq!(buf[0], buf[(3 * w + 3) * 4]);
        assert_ne!(buf[0], buf[4 * 4]);
    }
}
