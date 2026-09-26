//! Colors.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 8-bit sRGB color with **straight (non-premultiplied)** alpha.
///
/// Serializes as a hex string `"#RRGGBBAA"` (deserialization also accepts
/// `"#RRGGBB"`, `"#RGB"`, with or without `#`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    /// Default annotation color (vivid red, like macOS system red).
    pub const RED: Color = Color::rgb(0xFF, 0x3B, 0x30);
    pub const YELLOW: Color = Color::rgb(0xFF, 0xD6, 0x0A);
    pub const GREEN: Color = Color::rgb(0x34, 0xC7, 0x59);
    pub const BLUE: Color = Color::rgb(0x0A, 0x84, 0xFF);
    pub const PURPLE: Color = Color::rgb(0xBF, 0x5A, 0xF2);
    pub const ORANGE: Color = Color::rgb(0xFF, 0x9F, 0x0A);
    pub const GRAY: Color = Color::rgb(0x8E, 0x8E, 0x93);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Same color with alpha replaced by `alpha` in 0..=1.
    pub fn with_alpha(self, alpha: f32) -> Self {
        Self { a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, ..self }
    }

    /// Alpha as 0..=1.
    pub fn alpha_f32(self) -> f32 {
        self.a as f32 / 255.0
    }

    /// `[r, g, b, a]` in 0..=1.
    pub fn to_f32(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        ]
    }

    /// `"#RRGGBBAA"`, uppercase.
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
    }

    /// Parse `#RGB`, `#RRGGBB` or `#RRGGBBAA` (leading `#` optional).
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let hex = |i: usize, n: usize| u8::from_str_radix(s.get(i..i + n)?, 16).ok();
        match s.len() {
            3 => {
                let r = hex(0, 1)?;
                let g = hex(1, 1)?;
                let b = hex(2, 1)?;
                Some(Self::rgb(r * 17, g * 17, b * 17))
            }
            6 => Some(Self::rgb(hex(0, 2)?, hex(2, 2)?, hex(4, 2)?)),
            8 => Some(Self::rgba(hex(0, 2)?, hex(2, 2)?, hex(4, 2)?, hex(6, 2)?)),
            _ => None,
        }
    }

    /// Relative luminance (0..=1), useful for choosing a contrasting text color.
    pub fn luminance(self) -> f32 {
        let [r, g, b, _] = self.to_f32();
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::from_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color {s:?}")))
    }
}

/// The default palette shown in the style panel (v1 parity + extras).
pub const DEFAULT_PALETTE: [Color; 10] = [
    Color::RED,
    Color::ORANGE,
    Color::YELLOW,
    Color::GREEN,
    Color::BLUE,
    Color::PURPLE,
    Color::rgb(0xFF, 0x2D, 0x55),
    Color::WHITE,
    Color::GRAY,
    Color::BLACK,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Color::rgba(1, 2, 250, 128);
        assert_eq!(Color::from_hex(&c.to_hex()), Some(c));
        assert_eq!(Color::from_hex("#fff"), Some(Color::WHITE));
        assert_eq!(Color::from_hex("000000"), Some(Color::BLACK));
        assert_eq!(Color::from_hex("zz"), None);
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"#0102FA80\"");
        assert_eq!(serde_json::from_str::<Color>(&json).unwrap(), c);
    }
}
