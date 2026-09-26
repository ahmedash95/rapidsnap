//! Canvas fit / initial window size math (pure, unit-tested).
//!
//! Zoom unit: **window points per image point** (`1.0` = "Actual size": a
//! Retina capture shows at its natural point size, i.e. one image pixel per
//! physical pixel on a Retina display).
//!
//! Fit behaviour (initial zoom and ⌘0):
//! * The whole canvas (image + beautify frame) fits the canvas area with a
//!   [`FIT_MARGIN`] on every side.
//! * Large images scale down as far as needed (down to the global minimum).
//! * Small images may scale **up** to fill the area, but never beyond
//!   [`fit_zoom_cap`]: `scale_factor` clamped to 1…2. So a 1× image (file,
//!   clipboard) is never blown up (1 image px = 1 pt, like Preview), and a 2×
//!   Retina capture can grow to at most 1 image px per screen point (each
//!   image pixel = an exact 2×2 block of device pixels — crisp, not blurry).

/// Minimum zoom anywhere in the editor.
pub const MIN_ZOOM: f32 = 0.1;
/// Maximum manual zoom.
pub const MAX_ZOOM: f32 = 5.0;
/// Free space kept around the fitted canvas (window pt, per side).
pub const FIT_MARGIN: f32 = 24.0;

/// Largest zoom "Fit" may pick for a document of `scale_factor` px per pt.
pub fn fit_zoom_cap(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() { scale_factor.clamp(1.0, 2.0) } else { 1.0 }
}

/// Fit zoom for a canvas of `canvas_pt` (w, h in image points) inside a
/// viewport of `view` (w, h in window points).
pub fn fit_zoom(view: (f32, f32), canvas_pt: (f32, f32), scale_factor: f32) -> f32 {
    let aw = (view.0 - 2.0 * FIT_MARGIN).max(1.0);
    let ah = (view.1 - 2.0 * FIT_MARGIN).max(1.0);
    let cw = canvas_pt.0.max(1.0);
    let ch = canvas_pt.1.max(1.0);
    let z = (aw / cw).min(ah / ch);
    if !z.is_finite() {
        return 1.0;
    }
    z.clamp(MIN_ZOOM, fit_zoom_cap(scale_factor).max(MIN_ZOOM))
}

/// Extra window space around the canvas area (side panel, toolbar, status
/// bar), in window points.
#[derive(Debug, Clone, Copy)]
pub struct Chrome {
    pub panel_w: f32,
    pub bars_h: f32,
}

/// Smallest editor window (pt); toolbar and side panel are laid out to stay
/// usable down to this size.
pub const MIN_WINDOW: (f32, f32) = (720.0, 500.0);
/// Preferred minimum initial window size (pt) when the screen allows it.
pub const MIN_INITIAL: (f32, f32) = (1100.0, 700.0);
/// Fraction of the screen's visible area the initial window may use.
pub const MAX_SCREEN_FRACTION: f32 = 0.9;

/// Initial editor window size (pt): big enough to show the canvas at
/// actual size (plus chrome and margins) when it fits, never smaller than
/// [`MIN_INITIAL`] and never larger than [`MAX_SCREEN_FRACTION`] of the
/// visible screen (`screen = None` → assume 1728×1080, a 14" MacBook).
pub fn initial_window_size(canvas_pt: (f32, f32), chrome: Chrome, screen: Option<(f32, f32)>) -> (f32, f32) {
    let (sw, sh) = screen.filter(|s| s.0 > 100.0 && s.1 > 100.0).unwrap_or((1728.0, 1080.0));
    let max_w = sw * MAX_SCREEN_FRACTION;
    let max_h = sh * MAX_SCREEN_FRACTION;
    let want_w = canvas_pt.0.max(1.0) + chrome.panel_w + 2.0 * FIT_MARGIN;
    let want_h = canvas_pt.1.max(1.0) + chrome.bars_h + 2.0 * FIT_MARGIN;
    let w = want_w.max(MIN_INITIAL.0).min(max_w).max(MIN_WINDOW.0);
    let h = want_h.max(MIN_INITIAL.1).min(max_h).max(MIN_WINDOW.1);
    (w.round(), h.round())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME: Chrome = Chrome { panel_w: 256.0, bars_h: 78.0 };

    #[test]
    fn retina_capture_fills_large_viewport_up_to_cap() {
        // 1440×900 px @2x = 720×450 pt in a 3000×1700 pt canvas area.
        let z = fit_zoom((3000.0, 1700.0), (720.0, 450.0), 2.0);
        assert_eq!(z, 2.0, "capped at 1 image px per pt");
        // A viewport that is only a bit larger: fills it (scale up < cap).
        let z = fit_zoom((1200.0, 800.0), (720.0, 450.0), 2.0);
        let expect = ((1200.0 - 48.0) / 720.0f32).min((800.0 - 48.0) / 450.0);
        assert!((z - expect).abs() < 1e-5 && z > 1.0, "{z}");
    }

    #[test]
    fn one_x_images_are_not_upscaled() {
        assert_eq!(fit_zoom((3000.0, 2000.0), (400.0, 300.0), 1.0), 1.0);
        assert_eq!(fit_zoom_cap(3.0), 2.0);
        assert_eq!(fit_zoom_cap(f32::NAN), 1.0);
    }

    #[test]
    fn big_images_scale_down_with_margin() {
        let z = fit_zoom((1000.0, 700.0), (3000.0, 2000.0), 1.0);
        assert!((z - (952.0 / 3000.0)).abs() < 1e-5, "{z}");
        // Displayed size + margins fits both axes.
        assert!(3000.0 * z + 48.0 <= 1000.0 + 1e-3 && 2000.0 * z + 48.0 <= 700.0 + 1e-3);
        assert_eq!(fit_zoom((10.0, 10.0), (1e6, 1e6), 2.0), MIN_ZOOM);
    }

    #[test]
    fn initial_window_respects_screen_and_image() {
        // Small capture → minimum size.
        assert_eq!(initial_window_size((400.0, 300.0), CHROME, Some((1728.0, 1117.0))), (1100.0, 700.0));
        // Medium → actual size + chrome.
        assert_eq!(initial_window_size((900.0, 600.0), CHROME, Some((2560.0, 1440.0))), (1204.0, 726.0));
        // Huge → capped at 90% of the screen.
        let (w, h) = initial_window_size((5000.0, 3000.0), CHROME, Some((1728.0, 1117.0)));
        assert_eq!((w, h), ((1728.0f32 * 0.9).round(), (1117.0f32 * 0.9).round()));
        // Tiny screen: never larger than the screen allows.
        let (w, h) = initial_window_size((400.0, 300.0), CHROME, Some((1024.0, 640.0)));
        assert!(w <= 1024.0 * 0.9 + 0.5 && h <= 640.0 * 0.9 + 0.5);
    }
}
