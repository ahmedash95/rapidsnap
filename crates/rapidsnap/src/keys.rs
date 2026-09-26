//! GPUI keystroke → `rs_core::Hotkey` (W3C `KeyboardEvent.code`) mapping for
//! the shortcut recorder, and hotkey → tray-menu accelerator.

use std::str::FromStr;

use rs_core::{Hotkey, Modifiers};

/// W3C code for a GPUI key name (US layout). `None` = unsupported key.
pub fn code_for_key(key: &str) -> Option<String> {
    let k = key.to_ascii_lowercase();
    let code = match k.as_str() {
        "space" => "Space",
        "enter" => "Enter",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "escape" => "Escape",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "-" | "_" => "Minus",
        "=" | "+" => "Equal",
        "[" | "{" => "BracketLeft",
        "]" | "}" => "BracketRight",
        "\\" | "|" => "Backslash",
        ";" | ":" => "Semicolon",
        "'" | "\"" => "Quote",
        "," | "<" => "Comma",
        "." | ">" => "Period",
        "/" | "?" => "Slash",
        "`" | "~" => "Backquote",
        // Shifted digits (in case the platform reports the produced char).
        "!" => "Digit1",
        "@" => "Digit2",
        "#" => "Digit3",
        "$" => "Digit4",
        "%" => "Digit5",
        "^" => "Digit6",
        "&" => "Digit7",
        "*" => "Digit8",
        "(" => "Digit9",
        ")" => "Digit0",
        _ => {
            let mut chars = k.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_lowercase() => Some(format!("Key{}", c.to_ascii_uppercase())),
                (Some(c), None) if c.is_ascii_digit() => Some(format!("Digit{c}")),
                (Some('f'), Some(_)) => {
                    let n: u32 = k[1..].parse().ok()?;
                    (1..=20).contains(&n).then(|| format!("F{n}"))
                }
                _ => None,
            };
        }
    };
    Some(code.to_string())
}

/// Result of feeding a keystroke to the recorder.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordOutcome {
    /// A valid hotkey.
    Hotkey(Hotkey),
    /// Esc without modifiers: cancel recording.
    Cancel,
    /// Backspace/Delete without modifiers: clear the hotkey.
    Clear,
    /// Needs at least one modifier (except F-keys) or unsupported key.
    Invalid(String),
}

/// Interpret a keystroke typed into the shortcut recorder.
pub fn record(ks: &gpui_kit::Keystroke) -> RecordOutcome {
    let m = &ks.modifiers;
    let mods = Modifiers { cmd: m.platform, shift: m.shift, alt: m.alt, ctrl: m.control };
    let none = !(mods.cmd || mods.shift || mods.alt || mods.ctrl);
    match ks.key.as_str() {
        "escape" if none => return RecordOutcome::Cancel,
        "backspace" | "delete" if none => return RecordOutcome::Clear,
        _ => {}
    }
    let Some(code) = code_for_key(&ks.key) else {
        return RecordOutcome::Invalid(format!("Unsupported key “{}”", ks.key));
    };
    let is_fkey = code.starts_with('F') && code.len() <= 3;
    if !is_fkey && !(mods.cmd || mods.alt || mods.ctrl) {
        return RecordOutcome::Invalid("Use at least ⌘, ⌥ or ⌃ (or an F-key)".into());
    }
    RecordOutcome::Hotkey(Hotkey::new(mods, code))
}

/// Tray-menu accelerator shown next to a menu item.
pub fn accelerator(h: &Hotkey) -> Option<tray_icon::menu::accelerator::Accelerator> {
    use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers as M};
    let code = Code::from_str(&h.key).ok()?;
    let mut m = M::empty();
    if h.modifiers.cmd {
        m |= M::META;
    }
    if h.modifiers.shift {
        m |= M::SHIFT;
    }
    if h.modifiers.alt {
        m |= M::ALT;
    }
    if h.modifiers.ctrl {
        m |= M::CONTROL;
    }
    Some(Accelerator::new(m, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keys() {
        assert_eq!(code_for_key("a").as_deref(), Some("KeyA"));
        assert_eq!(code_for_key("2").as_deref(), Some("Digit2"));
        assert_eq!(code_for_key("@").as_deref(), Some("Digit2"));
        assert_eq!(code_for_key("f5").as_deref(), Some("F5"));
        assert_eq!(code_for_key("space").as_deref(), Some("Space"));
        assert_eq!(code_for_key("fn"), None);
        for c in ["KeyA", "Digit2", "F5", "Space", "Minus", "ArrowLeft"] {
            assert!(global_hotkey::hotkey::Code::from_str(c).is_ok(), "{c}");
        }
    }
}
