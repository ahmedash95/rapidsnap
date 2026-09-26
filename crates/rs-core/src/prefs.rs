//! User preferences (persisted as JSON by `rs_smart::prefs`).
//!
//! Every struct uses `#[serde(default)]` so older/newer files load without
//! error: unknown fields are ignored, missing fields take defaults.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{BeautifySettings, Color, ExportSettings, Rect, StylePreset, ToolKind, ToolStyle};

/// Keyboard modifiers of a hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Modifiers {
    /// ⌘ Command.
    pub cmd: bool,
    /// ⇧ Shift.
    pub shift: bool,
    /// ⌥ Option / Alt.
    pub alt: bool,
    /// ⌃ Control.
    pub ctrl: bool,
}

/// A global hotkey.
///
/// `key` is a W3C `KeyboardEvent.code` name — exactly what
/// `global_hotkey::hotkey::Code::from_str` accepts, e.g. `"Digit2"`, `"KeyA"`,
/// `"F5"`, `"Space"`. Build a `global_hotkey::HotKey` with
/// `HotKey::new(Some(mods), Code::from_str(&key)?)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: String,
}

impl Hotkey {
    pub fn new(modifiers: Modifiers, key: impl Into<String>) -> Self {
        Self { modifiers, key: key.into() }
    }

    /// Human-readable, macOS style: `"⌃⌥⇧⌘2"`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.modifiers.ctrl {
            s.push('⌃');
        }
        if self.modifiers.alt {
            s.push('⌥');
        }
        if self.modifiers.shift {
            s.push('⇧');
        }
        if self.modifiers.cmd {
            s.push('⌘');
        }
        let k = self.key.as_str();
        let label = k
            .strip_prefix("Digit")
            .or_else(|| k.strip_prefix("Key"))
            .unwrap_or(k);
        s.push_str(label);
        s
    }
}

/// Configurable global hotkeys per capture mode. `None` = unassigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub region: Option<Hotkey>,
    pub window: Option<Hotkey>,
    pub fullscreen: Option<Hotkey>,
    pub repeat_last: Option<Hotkey>,
}

impl Default for Hotkeys {
    /// Region ⇧⌘2, Window ⌥⇧⌘2, Full screen ⇧⌘1, Repeat last: unassigned.
    fn default() -> Self {
        let cs = Modifiers { cmd: true, shift: true, ..Default::default() };
        let acs = Modifiers { alt: true, ..cs };
        Self {
            region: Some(Hotkey::new(cs, "Digit2")),
            window: Some(Hotkey::new(acs, "Digit2")),
            fullscreen: Some(Hotkey::new(cs, "Digit1")),
            repeat_last: None,
        }
    }
}

/// Delay before capturing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDelay {
    #[default]
    None,
    S3,
    S5,
    S10,
}

impl CaptureDelay {
    pub fn seconds(self) -> u32 {
        match self {
            CaptureDelay::None => 0,
            CaptureDelay::S3 => 3,
            CaptureDelay::S5 => 5,
            CaptureDelay::S10 => 10,
        }
    }
}

/// The last region captured (for "repeat last region").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureRegion {
    /// `CGDirectDisplayID` of the display the region was on (None = main).
    pub display_id: Option<u32>,
    /// Region in **global screen points** (macOS global coordinates, origin
    /// top-left of the main display, y down — same convention as rs-capture).
    pub rect: Rect,
}

/// Maximum entries kept in `recent_colors`.
pub const MAX_RECENT_COLORS: usize = 16;

/// All user preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Schema version (currently 1).
    pub version: u32,
    pub hotkeys: Hotkeys,
    pub launch_at_login: bool,
    /// Close the editor after ⌘C copy (v1 default: true).
    pub auto_close_on_copy: bool,
    /// Instant mode: capture → copy PNG to clipboard, skip the editor.
    pub instant_copy: bool,
    pub capture_delay: CaptureDelay,
    /// Capture window with its shadow (window mode).
    pub window_capture_shadow: bool,
    /// Capture window with a transparent background (window mode; alpha
    /// around rounded corners). Added 2026-09-25 — files written before it
    /// existed used `window_capture_shadow` for both; `rs_smart::prefs`
    /// migrates a missing value from `window_capture_shadow`.
    pub window_capture_transparent: bool,
    pub export: ExportSettings,
    /// Last style used per tool (logical points).
    pub tool_styles: BTreeMap<ToolKind, ToolStyle>,
    pub style_presets: Vec<StylePreset>,
    /// Most-recent-first, deduplicated, at most [`MAX_RECENT_COLORS`].
    pub recent_colors: Vec<Color>,
    /// Applied to new documents.
    pub last_beautify: BeautifySettings,
    pub last_capture_region: Option<CaptureRegion>,
    pub last_tool: ToolKind,
    /// Whether the welcome / permission screen has been completed.
    pub onboarding_done: bool,
    /// Keep N most recent captures in history (0 = disabled).
    pub history_limit: u32,
    /// **Deprecated / unused.** Was the editor side-panel design variant
    /// (0 = A Inspector, 1 = B Cards, 2 = C Minimal) while the layouts were
    /// A/B tested (2026-09-25). The editor now has a single panel layout
    /// and neither reads nor writes this; the field stays (add-only rule) so
    /// existing preference files keep loading.
    pub panel_variant: u8,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            hotkeys: Hotkeys::default(),
            launch_at_login: false,
            auto_close_on_copy: true,
            instant_copy: false,
            capture_delay: CaptureDelay::None,
            window_capture_shadow: true,
            window_capture_transparent: true,
            export: ExportSettings::default(),
            tool_styles: BTreeMap::new(),
            style_presets: Vec::new(),
            recent_colors: Vec::new(),
            last_beautify: BeautifySettings::default(),
            last_capture_region: None,
            last_tool: ToolKind::Arrow,
            onboarding_done: false,
            history_limit: 50,
            panel_variant: 0,
        }
    }
}

impl Preferences {
    /// Remembered style for `tool`, or its default.
    pub fn tool_style(&self, tool: ToolKind) -> ToolStyle {
        self.tool_styles.get(&tool).cloned().unwrap_or_else(|| ToolStyle::default_for(tool))
    }

    pub fn remember_tool_style(&mut self, tool: ToolKind, style: ToolStyle) {
        self.tool_styles.insert(tool, style);
    }

    /// Push a color to the front of `recent_colors` (dedup, truncate).
    pub fn push_recent_color(&mut self, c: Color) {
        self.recent_colors.retain(|x| *x != c);
        self.recent_colors.insert(0, c);
        self.recent_colors.truncate(MAX_RECENT_COLORS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_roundtrip_and_tolerant() {
        let mut p = Preferences::default();
        p.push_recent_color(Color::RED);
        p.push_recent_color(Color::BLUE);
        p.push_recent_color(Color::RED);
        assert_eq!(p.recent_colors, vec![Color::RED, Color::BLUE]);
        p.remember_tool_style(ToolKind::Arrow, ToolStyle::default());
        let s = serde_json::to_string_pretty(&p).unwrap();
        let back: Preferences = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
        // Missing fields + unknown fields are fine.
        let partial: Preferences = serde_json::from_str(r#"{"instant_copy":true,"zzz":1}"#).unwrap();
        assert!(partial.instant_copy);
        assert_eq!(partial.hotkeys, Hotkeys::default());
    }

    #[test]
    fn panel_variant_defaults_and_roundtrips() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(p.panel_variant, 0);
        let p = Preferences { panel_variant: 2, ..Default::default() };
        let back: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.panel_variant, 2);
    }

    #[test]
    fn hotkey_display() {
        assert_eq!(Hotkeys::default().region.unwrap().display(), "⇧⌘2");
    }
}
