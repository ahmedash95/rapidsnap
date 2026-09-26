//! # rs-core — RapidSnap core data model
//!
//! Pure data, no UI or platform dependencies. Everything is
//! `serde::{Serialize, Deserialize}` (project files and preferences depend on
//! it). See `CONTRACTS.md` at the repo root for the full contract.
//!
//! **FROZEN**: other crates' owners may only *add* items here (never rename or
//! remove), and must record additions in CONTRACTS.md → Changelog.

pub mod annotation;
pub mod assets;
pub mod beautify;
pub mod color;
pub mod document;
pub mod export;
pub mod geometry;
pub mod history;
pub mod prefs;
pub mod sanitize;
pub mod style;
pub mod text;
pub mod tool;

pub use annotation::*;
pub use beautify::*;
pub use color::*;
pub use document::*;
pub use export::*;
pub use geometry::*;
pub use history::{apply_command, zorder, Command, History, HistoryError};
pub use prefs::*;
pub use style::*;
pub use text::*;
pub use tool::*;

/// Fixtures shared by tests in this and other crates.
pub mod test_support {
    use crate::*;

    /// One instance of every [`AnnotationKind`] (for serde / render smoke tests).
    pub fn all_kinds() -> Vec<AnnotationKind> {
        let st = ToolStyle::default();
        let mut v: Vec<AnnotationKind> = ToolKind::ALL
            .into_iter()
            .filter_map(|t| st.create(t, Point::new(10.0, 10.0), Point::new(80.0, 60.0)))
            .map(|a| a.kind)
            .collect();
        // Add variants with optional fields set.
        v.push(AnnotationKind::Arrow(ArrowAnnotation {
            start: Point::new(0.0, 0.0),
            end: Point::new(50.0, 50.0),
            control: Some(Point::new(40.0, 5.0)),
            stroke: Stroke { dash: DashStyle::Dashed, ..Stroke::default() },
            head: ArrowHead::Open,
            double_headed: true,
            tapered: true,
            head_scale: 1.5,
        }));
        v.push(AnnotationKind::Text(TextAnnotation {
            position: Point::new(5.0, 5.0),
            width: Some(120.0),
            content: "Hello\nworld".into(),
            style: TextPreset::Code.style(),
            preset: Some(TextPreset::Code),
        }));
        v
    }

    /// A small test document (200×100 white image, scale 2) with one of every kind.
    pub fn sample_document() -> Document {
        let mut doc = Document::new(
            RgbaImage::filled(200, 100, [255, 255, 255, 255]),
            DocumentMetadata { scale_factor: 2.0, ..Default::default() },
        );
        doc.annotations = all_kinds().into_iter().map(Annotation::new).collect();
        doc
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sample_document_roundtrip() {
        let doc = crate::test_support::sample_document();
        let s = serde_json::to_string_pretty(&doc).unwrap();
        let back: crate::Document = serde_json::from_str(&s).unwrap();
        assert_eq!(back.annotations, doc.annotations);
        assert_eq!(back.metadata, doc.metadata);
    }
}
