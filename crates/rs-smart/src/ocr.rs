//! OCR via macOS Vision (`VNRecognizeTextRequest`, objc2-vision).
//!
//! Input: the **content image** (`rs_render::content_image(doc)`), so all
//! returned rects are in content-space px (top-left origin, y down — converted
//! from Vision's normalized bottom-left coords).
//!
//! Threading: every function here is **blocking** (≈50–500 ms depending on
//! image size and `accurate`). Call from `cx.background_executor().spawn(..)`,
//! never from the GPUI main thread. Vision is safe to use from any thread; each
//! call creates its own request/handler and autorelease pool.

use rs_core::{Rect, RgbaImage};

use crate::Result;

/// OCR options.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrOptions {
    /// BCP-47 language codes, e.g. `["en-US"]`. Empty = Vision default.
    pub languages: Vec<String>,
    /// `VNRequestTextRecognitionLevelAccurate` (true) vs `Fast`.
    pub accurate: bool,
    /// Language correction. Keep **off** for auto-redact (it "fixes" keys,
    /// hex strings and codes into dictionary words).
    pub language_correction: bool,
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self { languages: Vec::new(), accurate: true, language_correction: false }
    }
}

impl OcrOptions {
    /// Options tuned for "Copy text from image": accurate + language
    /// correction (nicer prose).
    pub fn for_copy_text() -> Self {
        Self { languages: Vec::new(), accurate: true, language_correction: true }
    }

    /// Options tuned for auto-redact: accurate, no language correction.
    pub fn for_redaction() -> Self {
        Self::default()
    }
}

/// A recognized word/token with its box.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    /// Byte range of this word inside `OcrLine::text`.
    pub range: std::ops::Range<usize>,
    /// Content-space px.
    pub bounds: Rect,
}

/// A recognized text line.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    /// Content-space px.
    pub bounds: Rect,
    /// 0..=1.
    pub confidence: f32,
    /// Whitespace-separated tokens with boxes (from
    /// `VNRecognizedText boundingBoxForRange:`).
    pub words: Vec<OcrWord>,
}

impl OcrLine {
    /// Tight box (content px) for a byte range of `self.text`.
    ///
    /// Uses the word boxes that the range overlaps; a partially covered word is
    /// trimmed proportionally to the covered characters (Vision only reports
    /// per-token boxes reliably). Falls back to proportional slicing of the line
    /// box when no word boxes are available. `None` for an empty/invalid range.
    pub fn range_bounds(&self, range: std::ops::Range<usize>) -> Option<Rect> {
        let text = &self.text;
        if range.start >= range.end || range.end > text.len() {
            return None;
        }
        if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            return None;
        }
        let mut out: Option<Rect> = None;
        let mut covered_any = false;
        for w in &self.words {
            let s = range.start.max(w.range.start);
            let e = range.end.min(w.range.end);
            if s >= e {
                continue;
            }
            covered_any = true;
            let r = slice_rect(&w.bounds, text, w.range.clone(), s..e);
            out = Some(match out {
                Some(o) => o.union(&r),
                None => r,
            });
        }
        if covered_any {
            return out;
        }
        // Range only covers whitespace, or there are no words: slice the line.
        let trimmed = trim_range(text, range);
        if trimmed.start >= trimmed.end {
            return None;
        }
        Some(slice_rect(&self.bounds, text, 0..text.len(), trimmed))
    }
}

/// Shrink a byte range so it doesn't start/end on whitespace.
fn trim_range(text: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let sub = &text[range.clone()];
    let lead = sub.len() - sub.trim_start().len();
    let trail = sub.len() - sub.trim_end().len();
    let start = range.start + lead;
    let end = (range.end - trail).max(start);
    start..end
}

/// Horizontal proportional slice of `rect` (which spans `full` of `text`)
/// covering `part` (by character count).
fn slice_rect(rect: &Rect, text: &str, full: std::ops::Range<usize>, part: std::ops::Range<usize>) -> Rect {
    let n = text[full.clone()].chars().count().max(1) as f32;
    let before = text[full.start..part.start].chars().count() as f32;
    let len = text[part.start..part.end].chars().count() as f32;
    let x0 = rect.x + rect.width * (before / n);
    let w = rect.width * (len / n);
    Rect::new(x0, rect.y, w, rect.height)
}

/// Split `text` into whitespace-separated tokens (byte ranges).
pub(crate) fn token_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push(s..i);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(s..text.len());
    }
    out
}

/// Recognize text. Blocking (tens–hundreds of ms); run on a background thread.
/// Lines are returned in reading order (top-to-bottom rows, left-to-right).
pub fn recognize_text(image: &RgbaImage, options: &OcrOptions) -> Result<Vec<OcrLine>> {
    if !image.is_loaded() || image.width == 0 || image.height == 0 {
        return Err(crate::SmartError::Platform("OCR: image has no pixels".into()));
    }
    let lines = imp::recognize(image, options)?;
    Ok(reading_order(lines))
}

/// All recognized text joined with '\n' in reading order (for "Copy text").
pub fn recognize_plain_text(image: &RgbaImage, options: &OcrOptions) -> Result<String> {
    Ok(recognize_text(image, options)?.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n"))
}

/// "Copy all text from image": OCR with [`OcrOptions::for_copy_text`], lines in
/// reading order; lines on the same visual row are joined with a tab-like gap
/// (`"  "`), rows with `'\n'`. Blocking.
pub fn copy_text_from_image(image: &RgbaImage) -> Result<String> {
    let lines = recognize_text(image, &OcrOptions::for_copy_text())?;
    Ok(join_rows(&lines))
}

/// Join lines (already in reading order) into text, keeping rows together.
pub fn join_rows(lines: &[OcrLine]) -> String {
    let mut out = String::new();
    let mut prev: Option<&OcrLine> = None;
    for l in lines {
        if let Some(p) = prev {
            if same_row(&p.bounds, &l.bounds) {
                out.push_str("  ");
            } else {
                out.push('\n');
            }
        }
        out.push_str(&l.text);
        prev = Some(l);
    }
    out
}

fn same_row(a: &Rect, b: &Rect) -> bool {
    let top = a.y.max(b.y);
    let bottom = (a.y + a.height).min(b.y + b.height);
    let overlap = bottom - top;
    overlap > 0.5 * a.height.min(b.height)
}

/// Group lines into visual rows (by vertical overlap), rows top→bottom,
/// lines within a row left→right. Uses only total orders (no panics).
fn reading_order(mut lines: Vec<OcrLine>) -> Vec<OcrLine> {
    lines.sort_by(|a, b| a.bounds.center().y.total_cmp(&b.bounds.center().y));
    let mut rows: Vec<Vec<OcrLine>> = Vec::new();
    for l in lines {
        match rows.last_mut() {
            Some(row) if row.iter().any(|r| same_row(&r.bounds, &l.bounds)) => row.push(l),
            _ => rows.push(vec![l]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.bounds.x.total_cmp(&b.bounds.x));
            row
        })
        .collect()
}

/// Convert a Vision normalized rect (origin bottom-left, 0..1) into image px
/// (origin top-left, y down).
pub(crate) fn vision_rect_to_px(x: f64, y: f64, w: f64, h: f64, img_w: u32, img_h: u32) -> Rect {
    let iw = img_w as f64;
    let ih = img_h as f64;
    let px = (x * iw).clamp(0.0, iw);
    let pw = (w * iw).clamp(0.0, iw - px);
    let top = ((1.0 - y - h) * ih).clamp(0.0, ih);
    let ph = (h * ih).clamp(0.0, ih - top);
    Rect::new(px as f32, top as f32, pw as f32, ph as f32)
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, AnyThread};
    use objc2_core_foundation::{CFData, CGRect};
    use objc2_core_graphics::{
        CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    };
    use objc2_foundation::{NSArray, NSDictionary, NSError, NSRange, NSString};
    use objc2_vision::{
        VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRectangleObservation, VNRequest,
        VNRequestTextRecognitionLevel,
    };
    use rs_core::{Rect, RgbaImage};

    use super::{token_ranges, vision_rect_to_px, OcrLine, OcrOptions, OcrWord};
    use crate::{Result, SmartError};

    /// RGBA (straight alpha) → CGImage (RGBX, alpha flattened onto white so
    /// transparent window captures still OCR as dark-on-light).
    fn make_cgimage(image: &RgbaImage) -> Result<objc2_core_foundation::CFRetained<CGImage>> {
        let w = image.width as usize;
        let h = image.height as usize;
        let mut buf = Vec::with_capacity(w * h * 4);
        for p in image.pixels.as_chunks::<4>().0 {
            let a = p[3] as u32;
            if a == 255 {
                buf.extend_from_slice(&[p[0], p[1], p[2], 255]);
            } else {
                let f = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
                buf.extend_from_slice(&[f(p[0]), f(p[1]), f(p[2]), 255]);
            }
        }
        let data = CFData::from_bytes(&buf);
        let provider = CGDataProvider::with_cf_data(Some(&data))
            .ok_or_else(|| SmartError::Platform("CGDataProviderCreateWithCFData failed".into()))?;
        let space = CGColorSpace::new_device_rgb()
            .ok_or_else(|| SmartError::Platform("CGColorSpaceCreateDeviceRGB failed".into()))?;
        let info = CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0);
        // SAFETY: buffer is w*h*4 bytes, 8 bpc / 32 bpp, tightly packed rows,
        // and the provider retains the CFData.
        let img = unsafe {
            CGImage::new(
                w,
                h,
                8,
                32,
                w * 4,
                Some(&space),
                info,
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        };
        img.ok_or_else(|| SmartError::Platform("CGImageCreate failed".into()))
    }

    fn utf16_len(s: &str) -> usize {
        s.encode_utf16().count()
    }

    fn ns_range(text: &str, r: &std::ops::Range<usize>) -> NSRange {
        let loc = utf16_len(&text[..r.start]);
        let len = utf16_len(&text[r.start..r.end]);
        NSRange::new(loc, len)
    }

    fn cg_to_px(r: CGRect, iw: u32, ih: u32) -> Rect {
        vision_rect_to_px(r.origin.x, r.origin.y, r.size.width, r.size.height, iw, ih)
    }

    pub(super) fn recognize(image: &RgbaImage, options: &OcrOptions) -> Result<Vec<OcrLine>> {
        let cg = make_cgimage(image)?;
        let (iw, ih) = (image.width, image.height);
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(if options.accurate {
                VNRequestTextRecognitionLevel::Accurate
            } else {
                VNRequestTextRecognitionLevel::Fast
            });
            request.setUsesLanguageCorrection(options.language_correction);
            if !options.languages.is_empty() {
                let langs: Vec<Retained<NSString>> = options.languages.iter().map(|l| NSString::from_str(l)).collect();
                let arr = NSArray::from_retained_slice(&langs);
                request.setRecognitionLanguages(&arr);
            }

            let opts: Retained<NSDictionary<VNImageOption, AnyObject>> = NSDictionary::new();
            // SAFETY: valid CGImage and an empty options dictionary.
            let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &cg, &opts) };
            let req: &VNRequest = &request;
            let reqs = NSArray::from_slice(&[req]);
            handler
                .performRequests_error(&reqs)
                .map_err(|e| SmartError::Platform(format!("Vision: {}", e.localizedDescription())))?;

            let Some(results) = request.results() else {
                return Ok(Vec::new());
            };
            let mut lines = Vec::with_capacity(results.len());
            for obs in results.iter() {
                let cands = obs.topCandidates(1);
                let Some(cand) = cands.firstObject() else { continue };
                let text = cand.string().to_string();
                if text.trim().is_empty() {
                    continue;
                }
                // SAFETY: plain property getter.
                let line_box = cg_to_px(unsafe { obs.boundingBox() }, iw, ih);
                let mut words = Vec::new();
                for r in token_ranges(&text) {
                    let range = ns_range(&text, &r);
                    // SAFETY: `boundingBoxForRange:error:` takes an NSRange in
                    // UTF-16 units of `string` and returns a nullable
                    // VNRectangleObservation (error on failure).
                    let res: std::result::Result<Retained<VNRectangleObservation>, Retained<NSError>> =
                        unsafe { msg_send![&*cand, boundingBoxForRange: range, error: _] };
                    let bounds = match res {
                        // SAFETY: plain property getter.
                        Ok(o) => cg_to_px(unsafe { o.boundingBox() }, iw, ih),
                        Err(_) => continue,
                    };
                    words.push(OcrWord { text: text[r.clone()].to_string(), range: r, bounds });
                }
                fix_degenerate_words(&text, &line_box, &mut words);
                lines.push(OcrLine { text, bounds: line_box, confidence: cand.confidence(), words });
            }
            Ok(lines)
        })
    }

    /// Some Vision revisions return the whole line box for every sub-range.
    /// Detect that and fall back to proportional slicing of the line box.
    fn fix_degenerate_words(text: &str, line: &Rect, words: &mut [OcrWord]) {
        if words.len() < 2 || line.width <= 0.0 {
            return;
        }
        let degenerate = words.iter().all(|w| w.bounds.width >= line.width * 0.95);
        if degenerate {
            for w in words.iter_mut() {
                w.bounds = super::slice_rect(line, text, 0..text.len(), w.range.clone());
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{OcrLine, OcrOptions};
    use crate::{Result, SmartError};
    use rs_core::RgbaImage;

    pub(super) fn recognize(_image: &RgbaImage, _options: &OcrOptions) -> Result<Vec<OcrLine>> {
        Err(SmartError::Platform("OCR is only available on macOS".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, bounds: Rect) -> OcrLine {
        // Words with proportional boxes (like a monospace font).
        let words = token_ranges(text)
            .into_iter()
            .map(|r| OcrWord {
                text: text[r.clone()].to_string(),
                bounds: slice_rect(&bounds, text, 0..text.len(), r.clone()),
                range: r,
            })
            .collect();
        OcrLine { text: text.to_string(), bounds, confidence: 1.0, words }
    }

    #[test]
    fn vision_conversion_flips_y() {
        // Normalized box at bottom-left quarter → bottom-left quarter in px.
        let r = vision_rect_to_px(0.0, 0.0, 0.5, 0.25, 200, 100);
        assert_eq!(r, Rect::new(0.0, 75.0, 100.0, 25.0));
        let r = vision_rect_to_px(0.5, 0.75, 0.5, 0.25, 200, 100);
        assert_eq!(r, Rect::new(100.0, 0.0, 100.0, 25.0));
    }

    #[test]
    fn tokens() {
        assert_eq!(token_ranges("  ab c\td  "), vec![2..4, 5..6, 7..8]);
        assert!(token_ranges("   ").is_empty());
    }

    #[test]
    fn range_bounds_trims_partial_words() {
        // 20 chars over 200 px → 10 px per char.
        let l = line("key: abcdef ghij xyz", Rect::new(0.0, 10.0, 200.0, 20.0));
        // "abcdef" = bytes 5..11 = whole word.
        let r = l.range_bounds(5..11).unwrap();
        assert!((r.x - 50.0).abs() < 0.01 && (r.width - 60.0).abs() < 0.01 && r.y == 10.0, "{r:?}");
        // "cdef gh" → partial words.
        let r = l.range_bounds(7..14).unwrap();
        assert!((r.x - 70.0).abs() < 0.01 && (r.right() - 140.0).abs() < 0.01, "{r:?}");
        assert_eq!(l.range_bounds(4..5), None); // whitespace only
        assert_eq!(l.range_bounds(3..3), None);
        assert_eq!(l.range_bounds(0..999), None);
    }

    #[test]
    fn reading_order_rows() {
        let a = line("right", Rect::new(100.0, 0.0, 50.0, 10.0));
        let b = line("left", Rect::new(0.0, 1.0, 50.0, 10.0));
        let c = line("below", Rect::new(0.0, 30.0, 50.0, 10.0));
        let out = reading_order(vec![c.clone(), a.clone(), b.clone()]);
        let texts: Vec<_> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["left", "right", "below"]);
        assert_eq!(join_rows(&out), "left  right\nbelow");
    }
}
