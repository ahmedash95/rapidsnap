//! Beautify layers: background, frame shadow, window chrome, watermark.

use std::path::Path;
use std::sync::Arc;

use rs_core::{Background, Color, Corner, FontWeight, Rect, TextAlign, TextStyle, Watermark, WatermarkContent, WindowChrome};
use tiny_skia as sk;

use crate::effects::{gaussian_blur, shadow_from_alpha, shadow_from_path, PxRect};
use crate::geom::{circle_path, paint, rounded_rect_path, sk_color, sk_stops};
use crate::layout::CanvasLayout;
use crate::text::{draw_text_layout, layout_text};
use crate::Renderer;

impl Renderer {
    /// Background pixmap for a canvas size (cached; image decodes cached).
    pub(crate) fn background_layer(&mut self, bg: &Background, layout: &CanvasLayout) -> Option<Arc<sk::Pixmap>> {
        let (w, h) = (layout.canvas_width, layout.canvas_height);
        if matches!(bg, Background::None) {
            return None;
        }
        let key = format!("{bg:?}|{w}x{h}|{:?}|{}", self.desktop_wallpaper, layout.px_per_pt.to_bits());
        if let Some((_, pm)) = self.bg_cache.iter().find(|(k, _)| *k == key) {
            return Some(pm.clone());
        }
        let mut pm = sk::Pixmap::new(w, h)?;
        self.draw_background(&mut pm, bg, layout);
        let pm = Arc::new(pm);
        self.bg_cache.insert(0, (key, pm.clone()));
        self.bg_cache.truncate(2);
        Some(pm)
    }

    fn draw_background(&mut self, canvas: &mut sk::Pixmap, bg: &Background, layout: &CanvasLayout) {
        let (w, h) = (canvas.width() as f32, canvas.height() as f32);
        let Some(full) = sk::Rect::from_xywh(0.0, 0.0, w, h) else { return };
        let mut p = sk::Paint::default();
        match bg {
            Background::None => {}
            Background::Solid { color } => {
                p.set_color(sk_color(*color));
                canvas.fill_rect(full, &p, sk::Transform::identity(), None);
            }
            Background::LinearGradient { stops, angle_deg } => {
                let a = angle_deg.to_radians();
                let (dx, dy) = (a.cos(), a.sin());
                let half = (w * dx.abs() + h * dy.abs()) / 2.0;
                let (cx, cy) = (w / 2.0, h / 2.0);
                let start = sk::Point::from_xy(cx - dx * half, cy - dy * half);
                let end = sk::Point::from_xy(cx + dx * half, cy + dy * half);
                if let Some(shader) = gradient_linear(start, end, stops) {
                    p.shader = shader;
                    canvas.fill_rect(full, &p, sk::Transform::identity(), None);
                }
            }
            Background::RadialGradient { stops, center, radius } => {
                let c = sk::Point::from_xy(center[0] * w, center[1] * h);
                let r = radius.max(0.001) * (w * w + h * h).sqrt() / 2.0;
                let shader = match stops.len() {
                    0 => None,
                    1 => Some(sk::Shader::SolidColor(sk_color(stops[0].color))),
                    _ => sk::RadialGradient::new(c, 0.0, c, r, sk_stops(stops), sk::SpreadMode::Pad, sk::Transform::identity()),
                };
                if let Some(shader) = shader {
                    p.shader = shader;
                    canvas.fill_rect(full, &p, sk::Transform::identity(), None);
                }
            }
            Background::Wallpaper { name } => {
                if let Some(path) = rs_core::assets::wallpaper_path(name) {
                    self.draw_cover_image(canvas, &path);
                }
            }
            Background::CustomImage { path } => {
                let path = path.clone();
                self.draw_cover_image(canvas, &path);
            }
            Background::DesktopWallpaper { blur, brightness } => {
                let Some(path) = self.desktop_wallpaper.clone() else { return };
                let sigma = crate::effects::safe_sigma(blur * layout.px_per_pt);
                let bright = if brightness.is_finite() { brightness.max(0.0) } else { 1.0 };
                if sigma < 0.5 && (bright - 1.0).abs() < 1e-3 {
                    self.draw_cover_image(canvas, &path);
                    return;
                }
                // Work at reduced resolution: blurred content is smooth.
                let d = if sigma > 4.0 { (sigma / 2.0).floor().clamp(1.0, 16.0) } else { 1.0 };
                let sw = (w / d).ceil().max(1.0) as u32;
                let sh = (h / d).ceil().max(1.0) as u32;
                let Some(mut small) = sk::Pixmap::new(sw, sh) else { return };
                self.draw_cover_image(&mut small, &path);
                gaussian_blur::<4>(small.data_mut(), sw as usize, sh as usize, sigma / d);
                if (bright - 1.0).abs() > 1e-3 {
                    for px in small.data_mut().as_chunks_mut::<4>().0.iter_mut() {
                        let a = px[3] as f32;
                        for c in &mut px[..3] {
                            *c = (*c as f32 * bright).min(a).round() as u8;
                        }
                    }
                }
                let paint = sk::PixmapPaint { quality: sk::FilterQuality::Bilinear, ..Default::default() };
                canvas.draw_pixmap(0, 0, small.as_ref(), &paint, sk::Transform::from_scale(d, d), None);
            }
        }
    }

    /// Cover-fit `path` into `canvas`, centered.
    fn draw_cover_image(&mut self, canvas: &mut sk::Pixmap, path: &Path) {
        let Some(pm) = self.load_pixmap(path) else { return };
        let (w, h) = (canvas.width() as f32, canvas.height() as f32);
        let s = (w / pm.width() as f32).max(h / pm.height() as f32);
        let tx = (w - pm.width() as f32 * s) / 2.0;
        let ty = (h - pm.height() as f32 * s) / 2.0;
        // Pre-shrink by an integer factor when strongly downscaling (avoids aliasing).
        let f = (1.0 / s).floor() as u32;
        let (src, s) = if f >= 2 {
            match self.downsampled_image(path, &pm, f) {
                Some(d) => (d, s * f as f32),
                None => (pm, s),
            }
        } else {
            (pm, s)
        };
        let paint = sk::PixmapPaint { quality: sk::FilterQuality::Bilinear, ..Default::default() };
        canvas.draw_pixmap(0, 0, src.as_ref().as_ref(), &paint, sk::Transform::from_row(s, 0.0, 0.0, s, tx, ty), None);
    }

    fn downsampled_image(&mut self, path: &Path, pm: &Arc<sk::Pixmap>, f: u32) -> Option<Arc<sk::Pixmap>> {
        let key = (path.to_path_buf(), f);
        if let Some(p) = self.image_small_cache.get(&key) {
            return Some(p.clone());
        }
        let d = Arc::new(crate::effects::downsample(pm, f)?);
        if self.image_small_cache.len() > 8 {
            self.image_small_cache.clear();
        }
        self.image_small_cache.insert(key, d.clone());
        Some(d)
    }

    /// Frame drop shadow (under the rounded frame). `covered` = the frame
    /// interior will be painted opaquely, so only the ring outside it is drawn.
    pub(crate) fn draw_frame_shadow(
        &self,
        canvas: &mut sk::Pixmap,
        layout: &CanvasLayout,
        s: &rs_core::FrameShadow,
        covered: bool,
    ) {
        if !s.enabled || s.opacity <= 0.0 {
            return;
        }
        let k = layout.px_per_pt;
        let fr = layout.frame_rect;
        let Some(path) = rounded_rect_path(fr, layout.corner_radius) else { return };
        let sigma = (s.size.max(0.0) * k) / 2.0;
        let color = s.color.with_alpha(s.color.alpha_f32() * s.opacity.clamp(0.0, 1.0));
        let Some(img) = shadow_from_path(&path, sigma, color) else { return };
        let (dx, dy) = (s.offset_x * k, s.offset_y * k);
        if covered {
            let inner = fr.outset(-(layout.corner_radius + 2.0));
            img.draw_outside(canvas, dx, dy, inner);
        } else {
            img.draw(canvas, dx, dy, 1.0, None);
        }
    }

    /// Watermark in a canvas corner.
    pub(crate) fn draw_watermark(&mut self, canvas: &mut sk::Pixmap, layout: &CanvasLayout, wm: &Watermark) {
        let k = layout.px_per_pt;
        let (cw, ch) = (canvas.width() as f32, canvas.height() as f32);
        let m = wm.margin.max(0.0) * k;
        let opacity = wm.opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        let place = |w: f32, h: f32| -> (f32, f32) {
            match wm.position {
                Corner::TopLeft => (m, m),
                Corner::TopRight => (cw - m - w, m),
                Corner::BottomLeft => (m, ch - m - h),
                Corner::BottomRight => (cw - m - w, ch - m - h),
            }
        };
        match &wm.content {
            WatermarkContent::Text { text, style } => {
                if text.is_empty() {
                    return;
                }
                let st = style.scaled(k);
                let l = layout_text(text, &st, None);
                let vb = l.visual_bounds(&st);
                let (x, y) = place(vb.width, vb.height);
                let (ox, oy) = (x - vb.x, y - vb.y);
                let px = PxRect::cover(x, y, vb.width, vb.height).outset(2).intersect(&PxRect {
                    x: 0,
                    y: 0,
                    w: canvas.width() as i32,
                    h: canvas.height() as i32,
                });
                if px.is_empty() {
                    return;
                }
                let Some(mut layer) = sk::Pixmap::new(px.w as u32, px.h as u32) else { return };
                draw_text_layout(
                    &mut layer,
                    &l,
                    &st,
                    sk::Transform::from_translate(ox - px.x as f32, oy - px.y as f32),
                    None,
                );
                if let Some(sh) = st.shadow {
                    let alpha: Vec<u8> = layer.pixels().iter().map(|p| p.alpha()).collect();
                    if let Some(img) = shadow_from_alpha(
                        &alpha,
                        px.w as usize,
                        px.h as usize,
                        px.x as f32,
                        px.y as f32,
                        sh.blur / 2.0,
                        sh.color,
                    ) {
                        img.draw(canvas, sh.offset_x, sh.offset_y, opacity, None);
                    }
                }
                let paint = sk::PixmapPaint { opacity, ..Default::default() };
                canvas.draw_pixmap(px.x, px.y, layer.as_ref(), &paint, sk::Transform::identity(), None);
            }
            WatermarkContent::Image { path, width } => {
                let Some(pm) = self.load_pixmap(path) else { return };
                let w = width.max(1.0) * k;
                let s = w / pm.width().max(1) as f32;
                let h = pm.height() as f32 * s;
                let (x, y) = place(w, h);
                let paint =
                    sk::PixmapPaint { opacity, quality: sk::FilterQuality::Bicubic, ..Default::default() };
                canvas.draw_pixmap(0, 0, pm.as_ref().as_ref(), &paint, sk::Transform::from_row(s, 0.0, 0.0, s, x, y), None);
            }
        }
    }
}

fn gradient_linear(start: sk::Point, end: sk::Point, stops: &[rs_core::GradientStop]) -> Option<sk::Shader<'static>> {
    match stops.len() {
        0 => None,
        1 => Some(sk::Shader::SolidColor(sk_color(stops[0].color))),
        _ => sk::LinearGradient::new(start, end, sk_stops(stops), sk::SpreadMode::Pad, sk::Transform::identity()),
    }
}

/// Window chrome (title bar) inside the frame. `mask` = rounded frame clip.
pub(crate) fn draw_chrome(canvas: &mut sk::Pixmap, chrome: &WindowChrome, r: Rect, k: f32, mask: Option<&sk::Mask>) {
    let (dark, title, url) = match chrome {
        WindowChrome::None => return,
        WindowChrome::MacOs { dark, title } => (*dark, title.clone(), None),
        WindowChrome::Browser { dark, url } => (*dark, None, Some(url.clone())),
    };
    let Some(rect) = sk::Rect::from_xywh(r.x, r.y, r.width.max(1.0), r.height.max(1.0)) else { return };
    // Title bar background: subtle vertical gradient.
    let (top, bottom, sep) = if dark {
        (Color::rgb(0x3A, 0x3A, 0x3C), Color::rgb(0x2E, 0x2E, 0x30), Color::rgb(0x1C, 0x1C, 0x1E))
    } else {
        (Color::rgb(0xF4, 0xF4, 0xF4), Color::rgb(0xE6, 0xE6, 0xE6), Color::rgb(0xCF, 0xCF, 0xCF))
    };
    let mut p = sk::Paint::default();
    match sk::LinearGradient::new(
        sk::Point::from_xy(r.x, r.y),
        sk::Point::from_xy(r.x, r.bottom()),
        vec![sk::GradientStop::new(0.0, sk_color(top)), sk::GradientStop::new(1.0, sk_color(bottom))],
        sk::SpreadMode::Pad,
        sk::Transform::identity(),
    ) {
        Some(s) => p.shader = s,
        None => p.set_color(sk_color(bottom)),
    }
    canvas.fill_rect(rect, &p, sk::Transform::identity(), mask);
    // Bottom separator.
    let sep_h = (0.5 * k).max(1.0);
    if let Some(sr) = sk::Rect::from_xywh(r.x, r.bottom() - sep_h, r.width.max(1.0), sep_h) {
        canvas.fill_rect(sr, &paint(sep), sk::Transform::identity(), mask);
    }

    // Traffic lights: 12pt dots, 8pt spacing, 12pt from the left edge.
    let cy = r.y + r.height / 2.0;
    let dot_r = 6.0 * k;
    let colors = [
        (Color::rgb(0xFF, 0x5F, 0x57), Color::rgb(0xE0, 0x44, 0x3E)),
        (Color::rgb(0xFE, 0xBC, 0x2E), Color::rgb(0xDE, 0xA1, 0x23)),
        (Color::rgb(0x28, 0xC8, 0x40), Color::rgb(0x1A, 0xAB, 0x29)),
    ];
    for (i, (fill, ring)) in colors.iter().enumerate() {
        let cx = r.x + 12.0 * k + dot_r + i as f32 * (2.0 * dot_r + 8.0 * k);
        if let Some(c) = circle_path(rs_core::Point::new(cx, cy), dot_r) {
            canvas.fill_path(&c, &paint(*fill), sk::FillRule::Winding, sk::Transform::identity(), mask);
            let st = sk::Stroke { width: (0.5 * k).max(0.5), ..Default::default() };
            canvas.stroke_path(&c, &paint(*ring), &st, sk::Transform::identity(), mask);
        }
    }
    let lights_right = r.x + 12.0 * k + 3.0 * 2.0 * dot_r + 2.0 * 8.0 * k;
    let text_color = if dark { Color::rgb(0xD8, 0xD8, 0xDA) } else { Color::rgb(0x4D, 0x4D, 0x4D) };

    if let Some(title) = title.filter(|t| !t.is_empty()) {
        let st = TextStyle {
            size: 13.0 * k,
            weight: FontWeight::SEMIBOLD,
            color: text_color,
            align: TextAlign::Left,
            line_height: 1.2,
            ..TextStyle::default()
        };
        let l = layout_text(&title, &st, None);
        let x = (r.x + (r.width - l.size.width) / 2.0).max(lights_right + 8.0 * k);
        let y = cy - l.size.height / 2.0;
        draw_text_layout(canvas, &l, &st, sk::Transform::from_translate(x, y), mask);
    }

    if let Some(url) = url {
        // Back / forward chevrons.
        let chev = if dark { Color::rgb(0x8E, 0x8E, 0x93) } else { Color::rgb(0x9A, 0x9A, 0x9A) };
        let st = sk::Stroke {
            width: 1.6 * k,
            line_cap: sk::LineCap::Round,
            line_join: sk::LineJoin::Round,
            ..Default::default()
        };
        for (i, dir) in [-1.0f32, 1.0].iter().enumerate() {
            let cx = lights_right + (18.0 + i as f32 * 22.0) * k;
            let (hw, hh) = (3.5 * k, 6.0 * k);
            let mut pb = sk::PathBuilder::new();
            // Apex points left for "back" (dir = -1), right for "forward".
            pb.move_to(cx - dir * hw, cy - hh);
            pb.line_to(cx + dir * hw, cy);
            pb.line_to(cx - dir * hw, cy + hh);
            if let Some(path) = pb.finish() {
                canvas.stroke_path(&path, &paint(chev), &st, sk::Transform::identity(), mask);
            }
        }
        // URL field.
        let fh = 26.0 * k;
        let min_x = lights_right + 64.0 * k;
        let fw = (r.width * 0.56).min(640.0 * k).min(r.right() - 16.0 * k - min_x).max(40.0 * k);
        let fx = (r.x + (r.width - fw) / 2.0).max(min_x);
        let field = Rect::new(fx, cy - fh / 2.0, fw, fh);
        let (fbg, fborder) = if dark {
            (Color::rgb(0x1E, 0x1E, 0x20), Color::rgb(0x48, 0x48, 0x4A))
        } else {
            (Color::rgb(0xFF, 0xFF, 0xFF), Color::rgb(0xD6, 0xD6, 0xD6))
        };
        if let Some(path) = rounded_rect_path(field, 7.0 * k) {
            canvas.fill_path(&path, &paint(fbg), sk::FillRule::Winding, sk::Transform::identity(), mask);
            let st = sk::Stroke { width: (0.75 * k).max(0.5), ..Default::default() };
            canvas.stroke_path(&path, &paint(fborder), &st, sk::Transform::identity(), mask);
        }
        let st = TextStyle { size: 12.0 * k, color: text_color, line_height: 1.2, ..TextStyle::default() };
        let l = layout_text(&url, &st, None);
        let lock_w = 12.0 * k;
        let total = l.size.width + lock_w;
        let x0 = (field.x + (field.width - total) / 2.0).max(field.x + 10.0 * k);
        // Lock icon: shackle + body.
        let lx = x0;
        let body = Rect::new(lx, cy - 1.0 * k, 7.0 * k, 5.5 * k);
        if let Some(bp) = rounded_rect_path(body, 1.2 * k) {
            canvas.fill_path(&bp, &paint(text_color), sk::FillRule::Winding, sk::Transform::identity(), mask);
        }
        let mut pb = sk::PathBuilder::new();
        let (sx0, sx1, sy) = (lx + 1.6 * k, lx + 5.4 * k, cy - 1.0 * k);
        pb.move_to(sx0, sy);
        pb.line_to(sx0, sy - 1.8 * k);
        pb.cubic_to(sx0, sy - 4.6 * k, sx1, sy - 4.6 * k, sx1, sy - 1.8 * k);
        pb.line_to(sx1, sy);
        if let Some(path) = pb.finish() {
            let st = sk::Stroke { width: 1.2 * k, ..Default::default() };
            canvas.stroke_path(&path, &paint(text_color), &st, sk::Transform::identity(), mask);
        }
        let tx = x0 + lock_w;
        let ty = cy - l.size.height / 2.0;
        draw_text_layout(canvas, &l, &st, sk::Transform::from_translate(tx, ty), mask);
    }
}
