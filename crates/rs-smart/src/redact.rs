//! Auto-redact: detect sensitive data in OCR results and turn detections into
//! obfuscation annotations (after the user reviews the list).
//!
//! Flow (all content-space px):
//! ```text
//! let img  = rs_render::content_image(&doc)?;                   // main or bg thread
//! let dets = redact::detect_in_image(&img, &SensitiveKind::ALL)?; // BACKGROUND thread (OCR)
//! // show review list (Detection::masked_text / kind.label()), toggle `selected`
//! let anns = redact::detections_to_annotations(&dets, RedactStyle::Solid(Color::BLACK));
//! history.apply(&mut doc, Command::Add(anns));                   // one undo step
//! ```
//!
//! Detection is pure text matching over OCR lines ([`find_in_text`]) followed by
//! mapping byte ranges to boxes ([`OcrLine::range_bounds`]).

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use rs_core::{Annotation, AnnotationKind, BlurAnnotation, Color, PixelateAnnotation, Rect, RedactAnnotation, RgbaImage};
use uuid::Uuid;

use crate::ocr::{OcrLine, OcrOptions};

/// Kinds of sensitive data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensitiveKind {
    Email,
    /// API keys / tokens (AWS `AKIA…`, `sk-…`, `ghp_…`, JWTs, long hex/base64 secrets).
    ApiKey,
    /// Luhn-valid 13–19 digit numbers.
    CreditCard,
    /// IPv4 and IPv6.
    IpAddress,
    Phone,
}

impl SensitiveKind {
    pub const ALL: [SensitiveKind; 5] = [
        SensitiveKind::Email,
        SensitiveKind::ApiKey,
        SensitiveKind::CreditCard,
        SensitiveKind::IpAddress,
        SensitiveKind::Phone,
    ];

    /// Human-readable label for the review list.
    pub fn label(self) -> &'static str {
        match self {
            SensitiveKind::Email => "Email",
            SensitiveKind::ApiKey => "API key / secret",
            SensitiveKind::CreditCard => "Credit card",
            SensitiveKind::IpAddress => "IP address",
            SensitiveKind::Phone => "Phone number",
        }
    }

    /// Overlap priority (lower wins).
    fn priority(self) -> u8 {
        match self {
            SensitiveKind::CreditCard => 0,
            SensitiveKind::ApiKey => 1,
            SensitiveKind::Email => 2,
            SensitiveKind::IpAddress => 3,
            SensitiveKind::Phone => 4,
        }
    }
}

/// One detection for the review list.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub id: Uuid,
    pub kind: SensitiveKind,
    /// The matched text (show masked in UI).
    pub text: String,
    /// Content-space px (union of the matched words' boxes, slightly padded).
    pub bounds: Rect,
    /// Whether the user keeps it selected in the review list (default true).
    pub selected: bool,
}

impl Detection {
    /// Text safe to show in a review list: keeps the first/last 2 characters
    /// (4 for long strings ≥ 16 chars keep 3), masks the rest with `•`.
    pub fn masked_text(&self) -> String {
        mask(&self.text)
    }
}

fn mask(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let keep = if n >= 16 { 3 } else if n >= 6 { 2 } else { 0 };
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| if i < keep || i + keep >= n || c == ' ' || c == '@' || c == '.' && n < 16 { c } else { '•' })
        .collect()
}

/// A raw text match (byte range in the searched string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMatch {
    pub kind: SensitiveKind,
    pub range: Range<usize>,
}

/// How detections are obfuscated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RedactStyle {
    Solid(Color),
    /// Gaussian sigma px.
    Blur(f32),
    /// Block size px.
    Pixelate(u32),
}

/// Find sensitive data in OCR lines. Pure function.
pub fn detect_sensitive(lines: &[OcrLine], kinds: &[SensitiveKind]) -> Vec<Detection> {
    let mut out = Vec::new();
    for line in lines {
        for m in find_in_text(&line.text, kinds) {
            let Some(b) = line.range_bounds(m.range.clone()) else { continue };
            let pad = (b.height * 0.12).max(1.5);
            out.push(Detection {
                id: Uuid::new_v4(),
                kind: m.kind,
                text: line.text[m.range].to_string(),
                bounds: b.outset(pad),
                selected: true,
            });
        }
    }
    out
}

/// OCR (with [`OcrOptions::for_redaction`]) + [`detect_sensitive`].
/// **Blocking** — run on a background thread.
pub fn detect_in_image(image: &RgbaImage, kinds: &[SensitiveKind]) -> crate::Result<Vec<Detection>> {
    let lines = crate::ocr::recognize_text(image, &OcrOptions::for_redaction())?;
    Ok(detect_sensitive(&lines, kinds))
}

/// Build obfuscation annotations for the **selected** detections.
/// Apply with `Command::Add(..)` as one undo step.
pub fn detections_to_annotations(detections: &[Detection], style: RedactStyle) -> Vec<Annotation> {
    detections
        .iter()
        .filter(|d| d.selected)
        .map(|d| {
            let rect = d.bounds;
            Annotation::new(match style {
                RedactStyle::Solid(color) => AnnotationKind::Redact(RedactAnnotation { rect, color, corner_radius: 0.0 }),
                RedactStyle::Blur(strength) => {
                    AnnotationKind::Blur(BlurAnnotation { rect, strength, corner_radius: 0.0 })
                }
                RedactStyle::Pixelate(block_size) => {
                    AnnotationKind::Pixelate(PixelateAnnotation { rect, block_size: block_size.max(1) })
                }
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Text matching
// ---------------------------------------------------------------------------

/// Find sensitive substrings in one line of text. Results are non-overlapping
/// (higher-priority kinds win: card > key > email > IP > phone), sorted by
/// start offset.
pub fn find_in_text(text: &str, kinds: &[SensitiveKind]) -> Vec<TextMatch> {
    let mut cands: Vec<TextMatch> = Vec::new();
    for &k in kinds {
        let ranges = match k {
            SensitiveKind::Email => find_emails(text),
            SensitiveKind::ApiKey => find_api_keys(text),
            SensitiveKind::CreditCard => find_cards(text),
            SensitiveKind::IpAddress => find_ips(text),
            SensitiveKind::Phone => find_phones(text),
        };
        cands.extend(ranges.into_iter().map(|range| TextMatch { kind: k, range }));
    }
    // Priority, then longer first.
    cands.sort_by_key(|m| (m.kind.priority(), std::cmp::Reverse(m.range.len()), m.range.start));
    let mut accepted: Vec<TextMatch> = Vec::new();
    for c in cands {
        if !accepted.iter().any(|a| a.range.start < c.range.end && c.range.start < a.range.end) {
            accepted.push(c);
        }
    }
    accepted.sort_by_key(|m| m.range.start);
    accepted
}

fn re(s: &str) -> Regex {
    // Patterns are constants covered by tests; an invalid one degrades to a
    // never-matching regex (`a^` always compiles) instead of failing.
    Regex::new(s)
        .or_else(|e| {
            log::error!("rs-smart: bad regex {s:?}: {e}");
            Regex::new("a^")
        })
        .expect("`a^` is a valid regex")
}

fn prev_char(text: &str, i: usize) -> Option<char> {
    text[..i].chars().next_back()
}

fn next_char(text: &str, i: usize) -> Option<char> {
    text[i..].chars().next()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Neither neighbour is a word char.
fn isolated(text: &str, r: &Range<usize>) -> bool {
    !prev_char(text, r.start).is_some_and(is_word) && !next_char(text, r.end).is_some_and(is_word)
}

// ---- email ----------------------------------------------------------------

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)[a-z0-9](?:[a-z0-9._%+\-]*[a-z0-9_%+\-])?@[a-z0-9](?:[a-z0-9\-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9\-]*[a-z0-9])?)*\.[a-z]{2,24}")
});

/// Extensions that look like TLDs in `icon@2x.png`-style asset names.
const FILE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "webp", "pdf", "js", "ts", "tsx", "jsx", "css", "scss", "html", "json", "txt",
    "md", "rs", "py", "rb", "go", "swift", "kt", "java", "zip", "gz", "tar", "mp4", "mov", "heic", "tiff", "bmp",
    "ico", "icns", "lock", "yml", "yaml", "toml", "xml", "wasm", "mjs", "cjs", "vue", "map",
];

fn find_emails(text: &str) -> Vec<Range<usize>> {
    EMAIL
        .find_iter(text)
        .map(|m| m.range())
        .filter(|r| isolated(text, r))
        .filter(|r| {
            let s = &text[r.clone()];
            let tld = s.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            !FILE_EXTS.contains(&tld.as_str())
        })
        .collect()
}

// ---- API keys / secrets ---------------------------------------------------

static KEY_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // AWS access key ids.
        r"\b(?:AKIA|ASIA|ABIA|ACCA|AGPA|AIDA|AROA|ANPA|ANVA|APKA)[0-9A-Z]{16}\b",
        // GitHub tokens.
        r"\bgh[pousr]_[A-Za-z0-9]{30,255}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{22,255}\b",
        // GitLab.
        r"\bglpat-[A-Za-z0-9_\-]{20,}",
        // Slack tokens + webhooks.
        r"\bxox[abposre]-[A-Za-z0-9\-]{10,}",
        r"hooks\.slack\.com/services/[A-Za-z0-9/_\-]{20,}",
        // Stripe.
        r"\b(?:sk|pk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b",
        r"\bwhsec_[A-Za-z0-9]{24,}\b",
        // OpenAI / Anthropic / generic `sk-` keys.
        r"\bsk-(?:ant-|proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}",
        // Google API key.
        r"\bAIza[0-9A-Za-z_\-]{35}\b",
        // Twilio / SendGrid / npm / PyPI / DigitalOcean / Shopify.
        r"\bSK[0-9a-fA-F]{32}\b",
        r"\bSG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}",
        r"\bnpm_[A-Za-z0-9]{36}\b",
        r"\bpypi-[A-Za-z0-9_\-]{50,}",
        r"\bdop_v1_[a-f0-9]{64}\b",
        r"\bshp(?:at|ca|pa|ss)_[a-fA-F0-9]{32}\b",
        // JWT.
        r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        // PEM private key header.
        r"-----BEGIN (?:[A-Z]+ )?PRIVATE KEY-----",
    ]
    .iter()
    .map(|p| re(p))
    .collect()
});

/// `password = hunter22`, `api_key: "…"`, `Authorization: Bearer …` → value.
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?i)\b(?:api[_\-\s]?key|apikey|secret(?:[_\-]?key)?|client[_\-]?secret|access[_\-]?token|auth[_\-]?token|refresh[_\-]?token|bearer|token|password|passwd|pwd|private[_\-]?key|x-api-key)\b["']?\s*(?:[:=]|=>)?\s*["']?(?:bearer\s+)?([^\s"',;<>]{6,})"#)
});

/// Generic long token candidates.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z0-9+/_\-]{20,}={0,2}"));

static UUID_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"));

fn find_api_keys(text: &str) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for p in KEY_PATTERNS.iter() {
        out.extend(p.find_iter(text).map(|m| m.range()));
    }
    for c in KEY_VALUE.captures_iter(text) {
        if let Some(v) = c.get(1) {
            let r = trim_trailing_punct(text, v.range());
            let val = &text[r.clone()];
            // Skip obvious non-secrets: placeholders and plain words
            // ("password: required", "token = null").
            let lower = val.to_ascii_lowercase();
            let placeholder = ["null", "none", "true", "false", "undefined", "required", "optional", "string", "******", "xxxxxx", "<redacted>"]
                .iter()
                .any(|p| lower == *p)
                || val.chars().all(|c| c == '*' || c == '•' || c == 'x' || c == 'X');
            let wordy = val.chars().all(|c| c.is_ascii_lowercase()) && val.len() < 12;
            if !placeholder && !wordy && r.len() >= 6 {
                out.push(r);
            }
        }
    }
    for m in TOKEN.find_iter(text) {
        let r = m.range();
        if isolated_token(text, &r) && looks_random(&text[r.clone()]) {
            out.push(r);
        }
    }
    out
}

fn trim_trailing_punct(text: &str, r: Range<usize>) -> Range<usize> {
    let s = &text[r.clone()];
    let t = s.trim_end_matches(['.', ')', ']', '}', '!', '?']);
    r.start..r.start + t.len()
}

fn isolated_token(text: &str, r: &Range<usize>) -> bool {
    let p = prev_char(text, r.start);
    let n = next_char(text, r.end);
    let bad = |c: char| is_word(c) || matches!(c, '@' | '.' | '/' | '-' | '+');
    !p.is_some_and(bad) && !n.is_some_and(|c| bad(c) && c != '.')
}

/// Shannon entropy in bits per character.
pub(crate) fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    let mut n = 0u32;
    for b in s.bytes() {
        counts[b as usize] += 1;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    let n = n as f64;
    counts.iter().filter(|&&c| c > 0).map(|&c| {
        let p = c as f64 / n;
        -p * p.log2()
    }).sum()
}

/// Heuristic: does a ≥20-char token look like a random secret (not a word,
/// identifier, path, UUID or number)?
fn looks_random(tok: &str) -> bool {
    let body = tok.trim_end_matches('=');
    let len = body.len();
    if len < 20 || UUID_RE.is_match(body) {
        return false;
    }
    let digits = body.bytes().filter(u8::is_ascii_digit).count();
    let upper = body.bytes().filter(u8::is_ascii_uppercase).count();
    let lower = body.bytes().filter(u8::is_ascii_lowercase).count();
    let slashes = body.bytes().filter(|&b| b == b'/').count();
    if digits == len || digits == 0 {
        return false;
    }
    if slashes > 0 && slashes * 8 > len {
        return false; // path-like
    }
    let is_hex = body.bytes().all(|b| b.is_ascii_hexdigit());
    if is_hex {
        return len >= 32 && entropy(body) >= 3.0 && (upper == 0 || lower == 0);
    }
    if upper == 0 || lower == 0 || digits < 2 {
        return false;
    }
    // Identifiers / prose have long lowercase runs; random base62 rarely does.
    let mut run = 0;
    let mut max_run = 0;
    for b in body.bytes() {
        if b.is_ascii_lowercase() {
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 0;
        }
    }
    if max_run >= 8 {
        return false;
    }
    entropy(body) >= 3.6
}

// ---- credit cards ---------------------------------------------------------

static CARD: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\b\d{4}([ \-])\d{4}(?:[ \-])\d{4}(?:[ \-])\d{4}(?:[ \-]\d{1,3})?\b", // 16 (+ up to 3)
        r"\b\d{4}[ \-]\d{6}[ \-]\d{4,5}\b",                                   // Amex 15 / Diners 14
        r"\b\d{13,19}\b",
    ]
    .iter()
    .map(|p| re(p))
    .collect()
});

/// Luhn checksum over ASCII digits.
pub fn luhn_valid(digits: &str) -> bool {
    let ds: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if ds.len() < 2 {
        return false;
    }
    let sum: u32 = ds
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| if i % 2 == 1 { let x = d * 2; if x > 9 { x - 9 } else { x } } else { d })
        .sum();
    sum.is_multiple_of(10)
}

fn plausible_card_prefix(d: &str) -> bool {
    let b = d.as_bytes();
    let p2: u32 = d.get(..2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let p4: u32 = d.get(..4).and_then(|s| s.parse().ok()).unwrap_or(0);
    match b.first() {
        Some(b'4') => true,                                   // Visa
        Some(b'5') => (51..=55).contains(&p2),                // Mastercard
        Some(b'2') => (2221..=2720).contains(&p4),            // Mastercard 2-series
        Some(b'3') => matches!(p2, 34 | 37 | 30 | 36 | 38 | 39 | 35), // Amex, Diners, JCB
        Some(b'6') => true,                                   // Discover, UnionPay, Maestro
        _ => false,
    }
}

fn find_cards(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for p in CARD.iter() {
        for m in p.find_iter(text) {
            let r = m.range();
            let digits: String = text[r.clone()].chars().filter(char::is_ascii_digit).collect();
            if !(13..=19).contains(&digits.len()) {
                continue;
            }
            let first = digits.as_bytes()[0];
            if digits.bytes().all(|b| b == first) {
                continue;
            }
            if plausible_card_prefix(&digits) && luhn_valid(&digits) {
                out.push(r);
            }
        }
    }
    out
}

// ---- IP addresses ---------------------------------------------------------

static IPV4: LazyLock<Regex> = LazyLock::new(|| re(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b(?:/\d{1,2}\b)?"));
static IPV6: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)[0-9a-f:]*:[0-9a-f:]*:[0-9a-f:.]*(?:%[a-z0-9]+)?"));

fn find_ips(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for m in IPV4.find_iter(text) {
        let r = m.range();
        let s = &text[r.clone()];
        let addr = s.split('/').next().unwrap_or(s);
        if addr.parse::<std::net::Ipv4Addr>().is_err() {
            continue;
        }
        // No leading zeros like 01.02.03.04 (dates / versions).
        if addr.split('.').any(|o| o.len() > 1 && o.starts_with('0')) {
            continue;
        }
        // Part of a longer dotted sequence (version 1.2.3.4.5)?
        let before = &text[..r.start];
        let after = &text[r.end..];
        if before.ends_with('.') || (after.starts_with('.') && after[1..].starts_with(|c: char| c.is_ascii_digit())) {
            continue;
        }
        out.push(r);
    }
    for m in IPV6.find_iter(text) {
        let mut r = m.range();
        // Trim trailing punctuation (':' at end of sentence, '.').
        while r.end > r.start {
            let s = &text[r.clone()];
            if (s.ends_with(':') && !s.ends_with("::")) || s.ends_with('.') {
                r.end -= 1;
            } else {
                break;
            }
        }
        let s = &text[r.clone()];
        let addr = s.split('%').next().unwrap_or(s);
        let hex_digits = addr.chars().filter(|c| c.is_ascii_hexdigit()).count();
        if hex_digits < 2 || addr.matches(':').count() < 2 {
            continue;
        }
        if prev_char(text, r.start).is_some_and(|c| c.is_alphanumeric()) || next_char(text, r.end).is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        if addr.parse::<std::net::Ipv6Addr>().is_ok() {
            out.push(r);
        }
    }
    out
}

// ---- phone numbers --------------------------------------------------------

static PHONE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?:\+\d{1,3}[\s.\-]?)?(?:\(\d{1,5}\)[\s.\-]?)?\d{1,5}(?:[\s.\-]\d{2,8}){1,5}|\+\d{7,15}\b")
});
static DATE_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^(?:\d{4}[\-./]\d{1,2}[\-./]\d{1,2}|\d{1,2}[\-./]\d{1,2}[\-./]\d{2,4})$")
});

fn find_phones(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for m in PHONE.find_iter(text) {
        let r = m.range();
        let s = &text[r.clone()];
        let digits = s.chars().filter(char::is_ascii_digit).count();
        if !(7..=15).contains(&digits) {
            continue;
        }
        let plus = s.starts_with('+');
        let parens = s.contains('(');
        if !plus && !parens && digits < 9 {
            continue;
        }
        if DATE_LIKE.is_match(s) {
            continue;
        }
        // Dotted groups (1.23.456) are versions/IPs unless international.
        if !plus && s.contains('.') && !s.contains(['-', ' ']) && digits < 10 {
            continue;
        }
        // A single separator splitting two groups of 4+ digits without '+'
        // is usually an id/amount ("2024 123456").
        let groups: Vec<&str> = s.split(|c: char| !c.is_ascii_digit()).filter(|g| !g.is_empty()).collect();
        if !plus && !parens && groups.len() < 3 {
            continue;
        }
        // Boundaries: not glued to letters, other digits, or more separators+digits.
        let p = prev_char(text, r.start);
        let n = next_char(text, r.end);
        if p.is_some_and(|c| is_word(c) || matches!(c, '.' | '/' | '-' | '+' | '#' | '$' | '€' | '£' | ':' | ','))
            || n.is_some_and(|c| is_word(c) || c == '/' || c == '%')
        {
            continue;
        }
        let after = &text[r.end..];
        if (after.starts_with('.') || after.starts_with(',') || after.starts_with('-'))
            && after[1..].starts_with(|c: char| c.is_ascii_digit())
        {
            continue;
        }
        out.push(r);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::OcrWord;
    use SensitiveKind::*;

    fn kinds_of(text: &str) -> Vec<(SensitiveKind, String)> {
        find_in_text(text, &SensitiveKind::ALL).into_iter().map(|m| (m.kind, text[m.range].to_string())).collect()
    }

    fn only(text: &str, kind: SensitiveKind) -> Vec<String> {
        find_in_text(text, &[kind]).into_iter().map(|m| text[m.range].to_string()).collect()
    }

    fn none(text: &str) {
        assert!(kinds_of(text).is_empty(), "false positive in {text:?}: {:?}", kinds_of(text));
    }

    #[test]
    fn emails() {
        assert_eq!(only("contact: jane.doe+test@example.co.uk, thanks", Email), ["jane.doe+test@example.co.uk"]);
        assert_eq!(only("From: <ops@parloa.com>", Email), ["ops@parloa.com"]);
        assert_eq!(only("a@b.io and x_y@sub-domain.example.org", Email), ["a@b.io", "x_y@sub-domain.example.org"]);
        assert!(only("icon@2x.png", Email).is_empty());
        assert!(only("@mention and foo@ and @bar.com", Email).is_empty());
        assert!(only("npm i @types/node", Email).is_empty());
    }

    #[test]
    fn specific_api_keys() {
        for k in [
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGH",
            // Split so secret scanners don't flag these fake fixtures.
            concat!("xoxb-", "123456789012-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx"),
            concat!("sk_", "live_4eC39HqLyjWDarjtT1zdp7dc"),
            concat!("pk_", "test_TYooMQauvdEDq54NiTphI7jx"),
            "sk-proj-abcDEF123456ghiJKL789012mnoPQR",
            "sk-ant-api03-AbCdEf0123456789_GhIjKl-MnOpQr",
            "sk-abcdefghijklmnopqrstuvwxyz0123456789ABCD",
            "AIzaSyDaGmWKa4JsXZ-HjGw7ISLn_3namBGewQe",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
        ] {
            let text = format!("key: {k} end");
            let found = kinds_of(&text);
            assert_eq!(found, vec![(ApiKey, k.to_string())], "{k}");
        }
    }

    #[test]
    fn key_value_secrets() {
        assert_eq!(only("password = hunter22", ApiKey), ["hunter22"]);
        assert_eq!(only(r#"API_KEY="q8Zr2mWx9""#, ApiKey), ["q8Zr2mWx9"]);
        assert_eq!(only("Authorization: Bearer abc123DEF456ghi", ApiKey), ["abc123DEF456ghi"]);
        assert!(only("password: required", ApiKey).is_empty());
        assert!(only("token = null", ApiKey).is_empty());
        assert!(only("password: ********", ApiKey).is_empty());
    }

    #[test]
    fn generic_high_entropy() {
        assert_eq!(only("secret 9fQ2xL7vR4tK8mZ1pW3nB6yH end", ApiKey), ["9fQ2xL7vR4tK8mZ1pW3nB6yH"]);
        assert_eq!(
            only("sha d41d8cd98f00b204e9800998ecf8427e", ApiKey),
            ["d41d8cd98f00b204e9800998ecf8427e"]
        );
        // Guards.
        assert!(only("550e8400-e29b-41d4-a716-446655440000", ApiKey).is_empty(), "uuid");
        assert!(only("AbstractSingletonProxyFactoryBean2", ApiKey).is_empty(), "identifier");
        assert!(only("src/components/Header2/View1/index", ApiKey).is_empty(), "path");
        assert!(only("12345678901234567890123", ApiKey).is_empty(), "number");
        assert!(only("internationalization-localization", ApiKey).is_empty(), "words");
        assert!(only("my-component-name-2024-final-v2", ApiKey).is_empty(), "kebab");
        assert!(only("deadbeef", ApiKey).is_empty(), "short hex");
    }

    #[test]
    fn cards() {
        assert_eq!(only("Card 4242 4242 4242 4242 exp 12/34", CreditCard), ["4242 4242 4242 4242"]);
        assert_eq!(only("5555-5555-5555-4444", CreditCard), ["5555-5555-5555-4444"]);
        assert_eq!(only("amex 3782 822463 10005", CreditCard), ["3782 822463 10005"]);
        assert_eq!(only("4111111111111111", CreditCard), ["4111111111111111"]);
        // Luhn-invalid / implausible.
        assert!(only("4242 4242 4242 4241", CreditCard).is_empty());
        assert!(only("1234 5678 9012 3456", CreditCard).is_empty());
        assert!(only("0000000000000000", CreditCard).is_empty());
        assert!(only("order 12345678901234", CreditCard).is_empty());
    }

    #[test]
    fn luhn() {
        assert!(luhn_valid("4242424242424242"));
        assert!(luhn_valid("79927398713"));
        assert!(!luhn_valid("79927398710"));
        assert!(!luhn_valid("1"));
    }

    #[test]
    fn ips() {
        assert_eq!(only("server 192.168.1.10 up", IpAddress), ["192.168.1.10"]);
        assert_eq!(only("cidr 10.0.0.0/8,", IpAddress), ["10.0.0.0/8"]);
        assert_eq!(only("host 8.8.8.8:53", IpAddress), ["8.8.8.8"]);
        assert_eq!(only("v6 2001:db8:85a3::8a2e:370:7334 ok", IpAddress), ["2001:db8:85a3::8a2e:370:7334"]);
        assert_eq!(only("loopback ::1.", IpAddress), Vec::<String>::new()); // too few digits
        assert_eq!(only("link fe80::1ff:fe23:4567:890a%en0", IpAddress), ["fe80::1ff:fe23:4567:890a%en0"]);
        assert!(only("version 1.2.3.4.5", IpAddress).is_empty());
        assert!(only("999.1.1.1", IpAddress).is_empty());
        assert!(only("time 12:30:45", IpAddress).is_empty());
        assert!(only("mac 00:1A:2B:3C:4D:5E", IpAddress).is_empty());
        assert!(only("std::fs::read", IpAddress).is_empty());
        assert!(only("v1.2.3.4", IpAddress).is_empty());
    }

    #[test]
    fn phones() {
        assert_eq!(only("call +49 30 12345678 now", Phone), ["+49 30 12345678"]);
        assert_eq!(only("US (555) 123-4567", Phone), ["(555) 123-4567"]);
        assert_eq!(only("tel 555-123-4567.", Phone), ["555-123-4567"]);
        assert_eq!(only("+14155552671", Phone), ["+14155552671"]);
        assert_eq!(only("+44 20 7946 0958", Phone), ["+44 20 7946 0958"]);
        // Guards.
        assert!(only("2024-09-25", Phone).is_empty(), "date");
        assert!(only("25.09.2024", Phone).is_empty(), "date");
        assert!(only("version 1.23.456", Phone).is_empty(), "version");
        assert!(only("id 1727265600", Phone).is_empty(), "plain number");
        assert!(only("price $1 299 999", Phone).is_empty(), "amount");
        assert!(only("12:30:45", Phone).is_empty(), "time");
        assert!(only("2024 123456", Phone).is_empty(), "two groups");
    }

    #[test]
    fn prose_has_no_false_positives() {
        none("The quick brown fox jumps over the lazy dog.");
        none("Build finished in 2.35s — 1024 files, 3 warnings");
        none("fn main() { println!(\"hello\"); }");
        none("Meeting on 2024-09-25 at 14:30, room 4.12");
        none("https://github.com/rust-lang/rust/pull/12345");
        none("npm install --save-dev @types/node@20.11.5");
        none("Total: 1,234.56 EUR (incl. 19% VAT)");
        none("commit a1b2c3d Merge branch 'feature/login-page'");
        none("RapidSnap 2026-09-25 at 10.11.12.png");
    }

    #[test]
    fn overlap_priority() {
        // A card number inside a key=value is reported once, as a card.
        let found = kinds_of("card=4242424242424242");
        assert_eq!(found, vec![(CreditCard, "4242424242424242".into())]);
        // Mixed line.
        let found = kinds_of("mail bob@example.com from 10.1.2.3 key AKIAIOSFODNN7EXAMPLE");
        assert_eq!(
            found,
            vec![
                (Email, "bob@example.com".into()),
                (IpAddress, "10.1.2.3".into()),
                (ApiKey, "AKIAIOSFODNN7EXAMPLE".into())
            ]
        );
    }

    #[test]
    fn kind_filter() {
        let t = "bob@example.com 10.1.2.3";
        assert_eq!(find_in_text(t, &[IpAddress]).len(), 1);
        assert!(find_in_text(t, &[]).is_empty());
    }

    #[test]
    fn detections_have_tight_boxes() {
        // "ip 10.1.2.3 x": 13 chars, 10 px each.
        let text = "ip 10.1.2.3 x";
        let line = OcrLine {
            text: text.into(),
            bounds: Rect::new(0.0, 100.0, 130.0, 20.0),
            confidence: 1.0,
            words: vec![
                OcrWord { text: "ip".into(), range: 0..2, bounds: Rect::new(0.0, 100.0, 20.0, 20.0) },
                OcrWord { text: "10.1.2.3".into(), range: 3..11, bounds: Rect::new(30.0, 100.0, 80.0, 20.0) },
                OcrWord { text: "x".into(), range: 12..13, bounds: Rect::new(120.0, 100.0, 10.0, 20.0) },
            ],
        };
        let d = detect_sensitive(&[line], &SensitiveKind::ALL);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, IpAddress);
        let b = d[0].bounds;
        assert!(b.x < 30.0 && b.x > 25.0 && b.right() > 110.0 && b.right() < 115.0, "{b:?}");
        let anns = detections_to_annotations(&d, RedactStyle::Solid(Color::BLACK));
        assert_eq!(anns.len(), 1);
        let mut d2 = d.clone();
        d2[0].selected = false;
        assert!(detections_to_annotations(&d2, RedactStyle::Blur(8.0)).is_empty());
    }

    #[test]
    fn masking() {
        let d = Detection { id: Uuid::nil(), kind: ApiKey, text: "AKIAIOSFODNN7EXAMPLE".into(), bounds: Rect::default(), selected: true };
        assert_eq!(d.masked_text(), "AKI••••••••••••••PLE");
        assert_eq!(mask("abc"), "•••");
    }
}
