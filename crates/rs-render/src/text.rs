//! Text engine: cosmic-text shaping/layout + glyph outlines → tiny-skia paths.
//!
//! * One process-wide [`FontSystem`] (system fonts are loaded once, lazily, or
//!   eagerly on a background thread via [`warm_up_fonts`]).
//! * Layouts are cached by (content, layout-relevant style, width).
//! * Glyphs are drawn as **vector outlines** (so the outline background is a
//!   true stroke of the glyph path, and preview == export at any scale).
//!   Glyphs without outlines (color emoji bitmaps) are rasterized by swash.
//!
//! All layout coordinates are **box-local content px** (origin = top-left of
//! the text box = `TextAnnotation::position`).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Command, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    SwashContent, Weight, Wrap,
};
use rs_core::{
    CalloutAnnotation, Color, FontFamily, FontWeight, Point, Rect, Size, TextAlign, TextAnnotation, TextBackground,
    TextStyle,
};
use tiny_skia as sk;

use crate::geom::{rounded_rect_path, sk_color};

const LAYOUT_CACHE_MAX: usize = 512;

struct Engine {
    fs: FontSystem,
    swash: SwashCache,
    layouts: HashMap<LayoutKey, Arc<TextLayout>>,
    /// Fonts whose glyphs are color bitmaps (sbix / CBDT), e.g. Apple Color Emoji.
    bitmap_fonts: HashMap<fontdb::ID, bool>,
    families: Vec<String>,
    /// Lower-cased family names that have an italic/oblique face.
    italic_families: std::collections::HashSet<String>,
    sans: String,
    mono: String,
    serif: String,
}

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();

fn engine() -> MutexGuard<'static, Engine> {
    ENGINE.get_or_init(|| Mutex::new(Engine::new())).lock().unwrap_or_else(|e| e.into_inner())
}

/// Load system fonts on a background thread so the first text render does
/// not stall the UI (loading takes ~100–500 ms). Safe to call many times.
pub fn warm_up_fonts() {
    if ENGINE.get().is_none() {
        std::thread::spawn(|| drop(engine()));
    }
}

/// Sorted, de-duplicated list of installed font family names (for a font
/// picker; use with `FontFamily::Named`). Hidden system families (leading
/// '.') are omitted.
pub fn font_families() -> Vec<String> {
    engine().families.clone()
}

impl Engine {
    fn new() -> Self {
        let t0 = std::time::Instant::now();
        let mut fs = FontSystem::new();
        let mut names: Vec<String> = Vec::new();
        let mut italic = std::collections::HashSet::new();
        for f in fs.db().faces() {
            for (n, _) in &f.families {
                names.push(n.clone());
                if f.style != Style::Normal {
                    italic.insert(n.to_lowercase());
                }
            }
        }
        names.sort();
        names.dedup();
        let pick = |cands: &[&str], fallback: &str| -> String {
            cands
                .iter()
                .find(|c| names.iter().any(|n| n.eq_ignore_ascii_case(c)))
                .map(|s| s.to_string())
                .unwrap_or_else(|| fallback.to_string())
        };
        let sans = pick(&[".SF NS", "SF Pro", "SF Pro Text", "Helvetica Neue", "Helvetica", "Arial", "DejaVu Sans"], "Arial");
        let mono = pick(&["SF Mono", "Menlo", "Monaco", "Courier New", "DejaVu Sans Mono"], "Courier New");
        let serif = pick(&["New York", "Times New Roman", "Times", "Georgia", "DejaVu Serif"], "Times New Roman");
        {
            let db = fs.db_mut();
            db.set_sans_serif_family(sans.clone());
            db.set_monospace_family(mono.clone());
            db.set_serif_family(serif.clone());
        }
        let families = names.iter().filter(|n| !n.starts_with('.')).cloned().collect();
        log::debug!("rs-render: fonts loaded in {:?} (sans={sans}, mono={mono}, serif={serif})", t0.elapsed());
        Engine {
            fs,
            swash: SwashCache::new(),
            layouts: HashMap::new(),
            bitmap_fonts: HashMap::new(),
            families,
            italic_families: italic,
            sans,
            mono,
            serif,
        }
    }

    fn resolve_family<'a>(&self, f: &'a FontFamily) -> (Family<'a>, String) {
        match f {
            FontFamily::System => (Family::SansSerif, self.sans.to_lowercase()),
            FontFamily::Monospace => (Family::Monospace, self.mono.to_lowercase()),
            FontFamily::Serif => (Family::Serif, self.serif.to_lowercase()),
            FontFamily::Named(n) => {
                if self.families.iter().any(|x| x.eq_ignore_ascii_case(n)) || n.starts_with('.') {
                    (Family::Name(n.as_str()), n.to_lowercase())
                } else {
                    (Family::SansSerif, self.sans.to_lowercase())
                }
            }
        }
    }

    fn is_bitmap_font(&mut self, id: fontdb::ID, weight: Weight) -> bool {
        if let Some(b) = self.bitmap_fonts.get(&id) {
            return *b;
        }
        let b = self.fs.get_font(id, weight).is_some_and(|f| {
            let s = f.as_swash();
            s.table(u32::from_be_bytes(*b"sbix")).is_some() || s.table(u32::from_be_bytes(*b"CBDT")).is_some()
        });
        self.bitmap_fonts.insert(id, b);
        b
    }

    fn layout(&mut self, key: &LayoutKey) -> Arc<TextLayout> {
        if let Some(l) = self.layouts.get(key) {
            return l.clone();
        }
        let l = Arc::new(self.build_layout(key));
        if self.layouts.len() >= LAYOUT_CACHE_MAX {
            self.layouts.clear();
        }
        self.layouts.insert(key.clone(), l.clone());
        l
    }

    fn build_layout(&mut self, key: &LayoutKey) -> TextLayout {
        let size = f32::from_bits(key.size).clamp(0.5, 4000.0);
        let lh = (f32::from_bits(key.line_height).max(0.1)) * size;
        let width = key.width.map(f32::from_bits).map(|w| w.max(1.0));
        let (family, fam_lc) = self.resolve_family(&key.family);
        let mut flags = CacheKeyFlags::DISABLE_HINTING;
        let style = if key.italic {
            if !self.italic_families.contains(&fam_lc) {
                flags |= CacheKeyFlags::FAKE_ITALIC;
            }
            Style::Italic
        } else {
            Style::Normal
        };
        let attrs = Attrs::new().family(family).weight(Weight(key.weight.clamp(1, 1000))).style(style).cache_key_flags(flags);

        let mut buffer = Buffer::new(&mut self.fs, Metrics::new(size, lh));
        buffer.set_wrap(if width.is_some() { Wrap::WordOrGlyph } else { Wrap::None });
        buffer.set_size(width, None);
        buffer.set_text(&key.content, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);

        // Byte range of every hard line, split exactly like cosmic-text does
        // (`\n`, `\r\n`, lone `\r`, `\n\r`), plus the trailing empty line
        // cosmic-text appends after a final line ending.
        let content = key.content.as_str();
        let mut hard_lines: Vec<Range<usize>> = Vec::new();
        let mut last_ending = cosmic_text::LineEnding::Lf;
        for (range, ending) in cosmic_text::LineIter::new(content) {
            hard_lines.push(range);
            last_ending = ending;
        }
        if hard_lines.is_empty() || last_ending != cosmic_text::LineEnding::None {
            hard_lines.push(content.len()..content.len());
        }
        let line_offsets: Vec<usize> = hard_lines.iter().map(|r| r.start).collect();
        let hard_line_len = |i: usize| -> usize { hard_lines.get(i).map(|r| r.len()).unwrap_or(0) };
        // Every index we hand out must be a char boundary of the content.
        let snap = |i: usize| -> usize { content.floor_char_boundary(i.min(content.len())) };

        struct RawRun {
            line_i: usize,
            top: f32,
            baseline: f32,
            height: f32,
            w: f32,
            glyphs: Vec<cosmic_text::LayoutGlyph>,
        }
        let runs: Vec<RawRun> = buffer
            .layout_runs()
            .map(|r| RawRun {
                line_i: r.line_i,
                top: r.line_top,
                baseline: r.line_y,
                height: r.line_height,
                w: r.line_w,
                glyphs: r.glyphs.to_vec(),
            })
            .collect();
        let max_w = runs.iter().map(|r| r.w).fold(0.0f32, f32::max);
        let box_w = width.unwrap_or(max_w.ceil());
        let align_f = match key.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => 0.5,
            TextAlign::Right => 1.0,
        };

        let mut pb = sk::PathBuilder::new();
        let mut bitmaps = Vec::new();
        let mut lines = Vec::new();
        // Track whether each hard line got at least one visual run.
        let mut seen_hard = vec![false; line_offsets.len()];
        for run in &runs {
            let off = line_offsets.get(run.line_i).copied().unwrap_or(0);
            if let Some(s) = seen_hard.get_mut(run.line_i) {
                *s = true;
            }
            let dx = ((box_w - run.w) * align_f).max(0.0);
            let mut carets: Vec<(usize, f32)> = Vec::with_capacity(run.glyphs.len() * 2 + 1);
            let (mut start, mut end) = (usize::MAX, 0usize);
            for g in &run.glyphs {
                start = start.min(g.start);
                end = end.max(g.end);
                let rtl = g.level.is_rtl();
                let (xs, xe) = if rtl { (g.x + g.w, g.x) } else { (g.x, g.x + g.w) };
                carets.push((snap(off + g.start), dx + xs));
                carets.push((snap(off + g.end), dx + xe));

                let ox = dx + g.x + g.font_size * g.x_offset;
                let oy = run.baseline + g.y - g.font_size * g.y_offset;
                let (ck, _, _) =
                    CacheKey::new(g.font_id, g.glyph_id, g.font_size, (0.0, 0.0), g.font_weight, g.cache_key_flags | CacheKeyFlags::DISABLE_HINTING);
                let outline = if self.is_bitmap_font(g.font_id, g.font_weight) {
                    None
                } else {
                    self.swash.get_outline_commands(&mut self.fs, ck)
                };
                match outline {
                    Some(cmds) if !cmds.is_empty() => append_outline(&mut pb, cmds, ox, oy),
                    _ => bitmaps.push(BitmapGlyph { glyph: g.clone(), x: ox, y: oy }),
                }
            }
            if run.glyphs.is_empty() {
                start = 0;
                end = hard_line_len(run.line_i);
                carets.push((snap(off + end), dx));
            }
            carets.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            carets.dedup_by(|a, b| a.0 == b.0);
            lines.push(TextLine {
                top: run.top,
                height: run.height,
                baseline: run.baseline,
                x: dx,
                width: run.w,
                range: snap(off + start)..snap(off + end),
                hard_line: run.line_i,
                carets,
            });
        }
        // Hard lines that produced no run (e.g. trailing empty line).
        for (i, seen) in seen_hard.iter().enumerate() {
            if !seen {
                let top = lines.last().map(|l: &TextLine| l.top + l.height).unwrap_or(0.0);
                let off = line_offsets[i];
                let dx = (box_w * align_f).max(0.0);
                lines.push(TextLine {
                    top,
                    height: lh,
                    baseline: top + lh * 0.8,
                    x: dx,
                    width: 0.0,
                    range: off..off,
                    hard_line: i,
                    carets: vec![(off, dx)],
                });
            }
        }
        lines.sort_by(|a, b| a.top.total_cmp(&b.top));
        let height = lines.last().map(|l| l.top + l.height).unwrap_or(lh).max(lh);
        let path = pb.finish();
        let ink = path.as_ref().map(|p| {
            let b = p.bounds();
            Rect::new(b.x(), b.y(), b.width(), b.height())
        });
        TextLayout {
            size: Size::new(box_w.max(0.0), height),
            font_size: size,
            line_height: lh,
            lines,
            ink_bounds: ink,
            path,
            bitmaps,
        }
    }
}

fn append_outline(pb: &mut sk::PathBuilder, cmds: &[Command], ox: f32, oy: f32) {
    let p = |x: f32, y: f32| (ox + x, oy - y);
    let mut open = false;
    for c in cmds {
        match *c {
            Command::MoveTo(a) => {
                if open {
                    pb.close();
                }
                let (x, y) = p(a.x, a.y);
                pb.move_to(x, y);
                open = true;
            }
            Command::LineTo(a) => {
                let (x, y) = p(a.x, a.y);
                pb.line_to(x, y);
            }
            Command::QuadTo(a, b) => {
                let (x1, y1) = p(a.x, a.y);
                let (x, y) = p(b.x, b.y);
                pb.quad_to(x1, y1, x, y);
            }
            Command::CurveTo(a, b, e) => {
                let (x1, y1) = p(a.x, a.y);
                let (x2, y2) = p(b.x, b.y);
                let (x, y) = p(e.x, e.y);
                pb.cubic_to(x1, y1, x2, y2, x, y);
            }
            Command::Close => {
                pb.close();
                open = false;
            }
        }
    }
    if open {
        pb.close();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    content: String,
    family: FontFamily,
    size: u32,
    weight: u16,
    italic: bool,
    line_height: u32,
    align: TextAlign,
    width: Option<u32>,
}

impl LayoutKey {
    fn new(text: &str, style: &TextStyle, width: Option<f32>) -> Self {
        LayoutKey {
            content: text.to_string(),
            family: style.family.clone(),
            size: style.size.to_bits(),
            weight: style.weight.0,
            italic: style.italic,
            line_height: style.line_height.to_bits(),
            align: style.align,
            width: width.filter(|w| w.is_finite() && *w > 0.0).map(f32::to_bits),
        }
    }
}

#[derive(Debug, Clone)]
struct BitmapGlyph {
    glyph: cosmic_text::LayoutGlyph,
    x: f32,
    y: f32,
}

/// One visual (possibly wrapped) line of a [`TextLayout`]. Box-local px.
#[derive(Debug, Clone)]
pub struct TextLine {
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
    /// Left edge of the line after alignment.
    pub x: f32,
    pub width: f32,
    /// Byte range of the text on this line (global byte offsets into the content).
    pub range: Range<usize>,
    /// Index of the hard ('\n'-separated) line.
    pub hard_line: usize,
    /// Caret stops: (byte index, x) sorted by x.
    carets: Vec<(usize, f32)>,
}

/// A shaped, laid-out text block (box-local content px). Cheap to clone (Arc).
#[derive(Debug)]
pub struct TextLayout {
    /// Text box size (width = fixed width, or the widest line for auto width).
    pub size: Size,
    pub font_size: f32,
    /// Line height in px.
    pub line_height: f32,
    pub lines: Vec<TextLine>,
    /// Tight bounds of the glyph outlines (None if nothing visible).
    pub ink_bounds: Option<Rect>,
    path: Option<sk::Path>,
    bitmaps: Vec<BitmapGlyph>,
}

impl TextLayout {
    fn line_for_y(&self, y: f32) -> Option<&TextLine> {
        let first = self.lines.first()?;
        if y < first.top {
            return Some(first);
        }
        self.lines.iter().find(|l| y < l.top + l.height).or(self.lines.last())
    }

    /// Byte index (into the content) of the caret position nearest to
    /// `local` (box-local px). Always on a char boundary.
    pub fn hit_index(&self, local: Point) -> usize {
        let Some(line) = self.line_for_y(local.y) else { return 0 };
        line.carets
            .iter()
            .min_by(|a, b| (a.1 - local.x).abs().total_cmp(&(b.1 - local.x).abs()))
            .map(|c| c.0)
            .unwrap_or(line.range.start)
    }

    /// Visual line containing caret `index` (a caret at a soft-wrap boundary
    /// belongs to the following line).
    fn line_for_index(&self, index: usize) -> Option<&TextLine> {
        let mut best: Option<&TextLine> = None;
        for l in &self.lines {
            if index >= l.range.start && index <= l.range.end {
                match best {
                    // Prefer the next line when the caret sits at a wrap boundary.
                    Some(b) if b.hard_line == l.hard_line && index == b.range.end && index == l.range.start => {
                        best = Some(l)
                    }
                    Some(_) => {}
                    None => best = Some(l),
                }
            }
        }
        best.or_else(|| {
            // Index past the end → last line.
            self.lines.iter().rfind(|l| l.range.start <= index).or(self.lines.first())
        })
    }

    fn caret_x(line: &TextLine, index: usize) -> f32 {
        if let Some(c) = line.carets.iter().find(|c| c.0 == index) {
            return c.1;
        }
        // Inside a cluster/ligature: nearest stop by byte distance.
        line.carets.iter().min_by_key(|c| c.0.abs_diff(index)).map(|c| c.1).unwrap_or(line.x)
    }

    /// Caret rectangle (box-local) for byte `index`; width ≈ 1/16 of the font
    /// size (min 1 px).
    pub fn caret_rect(&self, index: usize) -> Rect {
        let w = (self.font_size / 16.0).max(1.0);
        match self.line_for_index(index) {
            Some(l) => Rect::new(Self::caret_x(l, index) - w / 2.0, l.top, w, l.height),
            None => Rect::new(0.0, 0.0, w, self.line_height),
        }
    }

    /// Selection highlight rectangles (box-local) for a byte range.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Rect> {
        let (s, e) = (range.start.min(range.end), range.start.max(range.end));
        let mut out = Vec::new();
        if s == e {
            return out;
        }
        for l in &self.lines {
            let (ls, le) = (l.range.start.max(s), l.range.end.min(e));
            if ls > le || (ls == le && !(l.range.start == l.range.end && s <= ls && ls < e)) {
                continue;
            }
            let x0 = Self::caret_x(l, ls);
            let mut x1 = Self::caret_x(l, le);
            if le == l.range.end && e > l.range.end {
                // Selection continues past this line: show a small tail.
                x1 = x1.max(x0) + self.font_size * 0.25;
            }
            out.push(Rect::new(x0.min(x1), l.top, (x1 - x0).abs().max(1.0), l.height));
        }
        out
    }

    /// Draw glyphs (fill) with `paint`, at `t` (box-local → canvas).
    fn fill_glyphs(&self, pm: &mut sk::Pixmap, color: Color, t: sk::Transform, clip: Option<&sk::Mask>) {
        if let Some(path) = &self.path {
            let mut paint = sk::Paint::default();
            paint.set_color(sk_color(color));
            paint.anti_alias = true;
            pm.fill_path(path, &paint, sk::FillRule::Winding, t, clip);
        }
        if !self.bitmaps.is_empty() {
            self.draw_bitmaps(pm, color, t, clip);
        }
    }

    fn draw_bitmaps(&self, pm: &mut sk::Pixmap, color: Color, t: sk::Transform, clip: Option<&sk::Mask>) {
        let scale = (t.sx * t.sx + t.ky * t.ky).sqrt().max(0.01);
        let mut eng = engine();
        let Engine { fs, swash, .. } = &mut *eng;
        for g in &self.bitmaps {
            let mut o = sk::Point::from_xy(g.x, g.y);
            t.map_point(&mut o);
            let lg = &g.glyph;
            let (ck, ix, iy) =
                CacheKey::new(lg.font_id, lg.glyph_id, lg.font_size * scale, (o.x, o.y), lg.font_weight, lg.cache_key_flags);
            let Some(img) = swash.get_image(fs, ck).as_ref() else { continue };
            let (w, h) = (img.placement.width, img.placement.height);
            let Some(mut gp) = sk::Pixmap::new(w.max(1), h.max(1)) else { continue };
            match img.content {
                SwashContent::Color => {
                    for (d, s) in gp.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(img.data.as_chunks::<4>().0.iter()) {
                        let a = s[3] as u16;
                        d[0] = ((s[0] as u16 * a + 127) / 255) as u8;
                        d[1] = ((s[1] as u16 * a + 127) / 255) as u8;
                        d[2] = ((s[2] as u16 * a + 127) / 255) as u8;
                        d[3] = s[3];
                    }
                }
                SwashContent::Mask | SwashContent::SubpixelMask => {
                    let step = if img.content == SwashContent::Mask { 1 } else { 4 };
                    for (d, s) in gp.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(img.data.chunks(step)) {
                        let a = (s[0] as u32 * color.a as u32 / 255) as u8;
                        d[0] = ((color.r as u32 * a as u32 + 127) / 255) as u8;
                        d[1] = ((color.g as u32 * a as u32 + 127) / 255) as u8;
                        d[2] = ((color.b as u32 * a as u32 + 127) / 255) as u8;
                        d[3] = a;
                    }
                }
            }
            let x = ix + img.placement.left;
            let y = iy - img.placement.top;
            pm.draw_pixmap(x, y, gp.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), clip);
        }
    }

    /// Box-local bounds of everything drawn for `style` (box ∪ ink ∪
    /// background padding ∪ outline).
    pub fn visual_bounds(&self, style: &TextStyle) -> Rect {
        let mut r = Rect::new(0.0, 0.0, self.size.width, self.size.height);
        if let Some(ink) = self.ink_bounds {
            r = r.union(&ink);
        }
        match style.background {
            TextBackground::Pill { padding_x, padding_y, .. }
            | TextBackground::SemiTransparent { padding_x, padding_y, .. } => {
                r = r.union(&background_rect(self, padding_x, padding_y));
            }
            TextBackground::Outline { width, .. } => {
                if let Some(ink) = self.ink_bounds {
                    r = r.union(&ink.outset(width.max(0.0)));
                }
            }
            TextBackground::None => {}
        }
        r
    }
}

fn background_rect(l: &TextLayout, px: f32, py: f32) -> Rect {
    Rect::new(-px, -py, l.size.width + 2.0 * px, l.size.height + 2.0 * py)
}

/// Draw a laid-out text block with its full style (background, outline,
/// fill). `t` maps box-local px → canvas px. Shadows are handled by the
/// caller (group shadow).
pub(crate) fn draw_text_layout(
    pm: &mut sk::Pixmap,
    layout: &TextLayout,
    style: &TextStyle,
    t: sk::Transform,
    clip: Option<&sk::Mask>,
) {
    match style.background {
        TextBackground::Pill { color, corner_radius, padding_x, padding_y }
        | TextBackground::SemiTransparent { color, corner_radius, padding_x, padding_y } => {
            let r = background_rect(layout, padding_x, padding_y);
            if let Some(path) = rounded_rect_path(r, corner_radius) {
                let mut paint = sk::Paint::default();
                paint.set_color(sk_color(color));
                pm.fill_path(&path, &paint, sk::FillRule::Winding, t, clip);
            }
        }
        TextBackground::Outline { width, color } => {
            if let Some(path) = &layout.path
                && width > 0.0 {
                    let mut paint = sk::Paint::default();
                    paint.set_color(sk_color(color));
                    let stroke = sk::Stroke {
                        width: width * 2.0,
                        line_join: sk::LineJoin::Round,
                        line_cap: sk::LineCap::Round,
                        ..Default::default()
                    };
                    pm.stroke_path(path, &paint, &stroke, t, clip);
                }
        }
        TextBackground::None => {}
    }
    layout.fill_glyphs(pm, style.color, t, clip);
}

// ---------------------------------------------------------------------------
// Public text API
// ---------------------------------------------------------------------------

/// Lay out `text` with `style` (content px); `width = Some(w)` wraps at `w`.
/// Cached; cheap to call repeatedly.
pub fn layout_text(text: &str, style: &TextStyle, width: Option<f32>) -> Arc<TextLayout> {
    engine().layout(&LayoutKey::new(text, style, width))
}

/// Layout of a text annotation.
pub fn text_layout(a: &TextAnnotation) -> Arc<TextLayout> {
    layout_text(&a.content, &a.style, a.width)
}

/// The text box in content space (`position` + laid-out size). This is the
/// box used for alignment/wrapping; background padding extends outside it.
pub fn text_box(a: &TextAnnotation) -> Rect {
    let l = text_layout(a);
    Rect::new(a.position.x, a.position.y, l.size.width, l.size.height)
}

/// Everything the text paints (box, glyph ink, pill padding, outline), in
/// content space. Use for selection boxes / hit-testing. Excludes shadow.
pub fn measure_text_annotation(a: &TextAnnotation) -> Rect {
    let l = text_layout(a);
    l.visual_bounds(&a.style).translate(a.position.x, a.position.y)
}

/// Caret byte index nearest to content-space point `p` (for click-to-place
/// caret / drag selection during inline editing).
pub fn text_hit_index(a: &TextAnnotation, p: Point) -> usize {
    text_layout(a).hit_index(Point::new(p.x - a.position.x, p.y - a.position.y))
}

/// Caret rectangle in content space for byte `index` of `a.content`.
pub fn caret_rect(a: &TextAnnotation, index: usize) -> Rect {
    text_layout(a).caret_rect(index).translate(a.position.x, a.position.y)
}

/// Selection rectangles in content space for a byte range of `a.content`.
pub fn selection_rects(a: &TextAnnotation, range: Range<usize>) -> Vec<Rect> {
    text_layout(a).selection_rects(range).into_iter().map(|r| r.translate(a.position.x, a.position.y)).collect()
}

/// The bubble rect the renderer actually draws for a callout: `c.rect`,
/// grown vertically when the wrapped text (plus `padding` on both sides) is
/// taller than the rect. The bubble grows away from the tail (upwards when
/// the tail points below the bubble's center, else downwards), so the tail
/// stays attached. The width never changes: text wraps at
/// `rect.width − 2·padding`. Use it for handles / selection of callouts.
pub fn callout_bubble_rect(c: &CalloutAnnotation) -> Rect {
    let r = c.rect.normalized();
    if c.text.is_empty() {
        return r;
    }
    let pad = c.padding.max(0.0);
    let w = (r.width - 2.0 * pad).max(1.0);
    let l = layout_text(&c.text, &c.text_style, Some(w));
    let need = l.size.height + 2.0 * pad;
    if need <= r.height || !need.is_finite() {
        return r;
    }
    let grow = need - r.height;
    let y = if c.tail.y > r.center().y { r.y - grow } else { r.y };
    Rect::new(r.x, y, r.width, need)
}

/// The text annotation the renderer draws inside a callout (same content,
/// style, wrap width `rect.width - 2*padding`, vertically centered in the
/// padded [`callout_bubble_rect`]). Use it with [`caret_rect`] /
/// [`text_hit_index`] to edit a callout's text inline.
pub fn callout_text(c: &CalloutAnnotation) -> TextAnnotation {
    let inner = callout_bubble_rect(c).outset(-c.padding.max(0.0));
    let w = inner.width.max(1.0);
    let l = layout_text(&c.text, &c.text_style, Some(w));
    let y = inner.y + ((inner.height - l.size.height) / 2.0).max(0.0);
    TextAnnotation {
        position: Point::new(inner.x, y),
        width: Some(w),
        content: c.text.clone(),
        style: c.text_style.clone(),
        preset: None,
    }
}

/// Step-counter label style & layout: font size fitted to the circle.
pub(crate) fn step_label_layout(
    label: &str,
    radius: f32,
    family: &FontFamily,
    weight: FontWeight,
) -> (Arc<TextLayout>, TextStyle) {
    let mut style = TextStyle {
        family: family.clone(),
        size: (radius * 1.15).max(1.0),
        weight,
        line_height: 1.0,
        ..TextStyle::default()
    };
    let mut l = layout_text(label, &style, None);
    if let Some(ink) = l.ink_bounds {
        let max_w = radius * 1.35;
        let max_h = radius * 1.0;
        let f = (max_w / ink.width.max(0.01)).min(max_h / ink.height.max(0.01)).min(1.0);
        if f < 0.999 {
            style.size = (style.size * f).max(1.0);
            l = layout_text(label, &style, None);
        }
    }
    (l, style)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(content: &str, width: Option<f32>) -> TextAnnotation {
        TextAnnotation {
            position: Point::new(10.0, 20.0),
            width,
            content: content.into(),
            style: TextStyle { size: 20.0, ..TextStyle::default() },
            preset: None,
        }
    }

    #[test]
    fn measure_multiline_and_wrap() {
        let one = text_box(&ann("Hello", None));
        let two = text_box(&ann("Hello\nWorld", None));
        assert!(one.width > 20.0 && one.width < 120.0, "{one:?}");
        assert!((two.height - 2.0 * one.height).abs() < 1.0, "{one:?} {two:?}");
        let wrapped = text_box(&ann("one two three four five six", Some(60.0)));
        assert_eq!(wrapped.width, 60.0);
        assert!(wrapped.height > 2.5 * one.height, "{wrapped:?}");
        let empty = text_box(&ann("", None));
        assert!(empty.height > 0.0);
    }

    #[test]
    fn caret_and_hit_roundtrip() {
        let a = ann("abc\ndef", None);
        for i in [0usize, 1, 2, 3, 4, 5, 7] {
            let r = caret_rect(&a, i);
            let hit = text_hit_index(&a, r.center());
            assert_eq!(hit, i, "caret {i} {r:?}");
        }
        let c0 = caret_rect(&a, 0);
        let c4 = caret_rect(&a, 4);
        assert!(c4.y > c0.y);
        assert!((c0.x - 10.0).abs() < 2.0);
        let sel = selection_rects(&a, 1..6);
        assert_eq!(sel.len(), 2);
    }

    #[test]
    fn alignment_shifts_lines() {
        let mut a = ann("a\nlonger line", None);
        a.style.align = TextAlign::Right;
        let l = text_layout(&a);
        assert!(l.lines[0].x > l.lines[1].x);
    }

    /// Every index the caret API returns is a char boundary, for every kind
    /// of text (lone CR, CRLF, LFCR, emoji ZWJ/modifiers, combining marks,
    /// CJK, RTL, mixed bidi), wrapped and unwrapped.
    #[test]
    fn caret_indices_are_char_boundaries() {
        let samples = [
            "a\rb",
            "é\rx",
            "\r",
            "\r\r\n\n",
            "x\n\ry",
            "ü\r\nß\n",
            "👍🏽 👨‍👩‍👧 ok",
            "e\u{301}e\u{301} cafe\u{301}",
            "漢字かなカナ 한국어",
            "שלום עולם",
            "abc مرحبا بالعالم def",
            "",
            "\n",
            "tail\n",
        ];
        for text in samples {
            for width in [None, Some(30.0f32)] {
                let a = ann(text, width);
                let l = text_layout(&a);
                for line in &l.lines {
                    assert!(text.is_char_boundary(line.range.start) && text.is_char_boundary(line.range.end), "{text:?} {line:?}");
                    for (i, _) in &line.carets {
                        assert!(text.is_char_boundary(*i), "caret stop {i} in {text:?}");
                    }
                }
                let b = text_box(&a);
                let mut y = b.y - 5.0;
                while y < b.bottom() + 5.0 {
                    let mut x = b.x - 10.0;
                    while x < b.right() + 10.0 {
                        let i = text_hit_index(&a, Point::new(x, y));
                        assert!(i <= text.len() && text.is_char_boundary(i), "hit {i} in {text:?}");
                        x += 1.5;
                    }
                    y += 3.0;
                }
                let bounds: Vec<usize> = (0..=text.len()).filter(|i| text.is_char_boundary(*i)).collect();
                for &i in &bounds {
                    let r = caret_rect(&a, i);
                    assert!(r.x.is_finite() && r.y.is_finite() && r.height > 0.0, "{text:?} {i} {r:?}");
                    // Clicking the caret lands on a boundary too.
                    let h = text_hit_index(&a, r.center());
                    assert!(text.is_char_boundary(h));
                    for &j in &bounds {
                        for s in selection_rects(&a, i..j) {
                            assert!(s.width.is_finite() && s.height > 0.0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn lone_cr_is_a_line_break_with_correct_offsets() {
        let a = ann("é\rxy", None);
        let l = text_layout(&a);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[1].range, 3..5);
        // Caret after 'x' (byte 4) is on the second line.
        assert!(caret_rect(&a, 4).y > caret_rect(&a, 0).y);
    }
}
