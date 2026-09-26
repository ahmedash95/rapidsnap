//! Extra inline icons for the side panel and toolbar overflow (Lucide-style 24×24 strokes; the
//! Lucide-derived ones are ISC licensed, see icons/LICENSE-LUCIDE). Kept in
//! the panel's own module so the shared `icons.rs` stays untouched.

use gpui_kit::component::Icon;

use crate::icons::Ico;

/// Panel-only icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PIco {
    Ban,
    SquareFilled,
    SquareBoth,
    Gradient,
    ImagePlus,
    Monitor,
    Images,
    HeadFilled,
    HeadOpen,
    HeadRound,
    HeadNone,
    DashSolid,
    DashDashed,
    DashDotted,
    CapRound,
    CapSquare,
    CapButt,
    ChevronDown,
    ChevronRight,
    ArrowUp,
    ArrowDown,
    Palette,
    Sliders,
    Frame,
    Shadow,
    Ratio,
    AppWindow,
    Globe,
    Stamp,
    Layers,
    Padding,
    Bookmark,
    Share,
    CornerTL,
    CornerTR,
    CornerBL,
    CornerBR,
    Ellipsis,
}

const HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#;

impl PIco {
    fn body(self) -> &'static str {
        match self {
            PIco::Ban => r#"<circle cx="12" cy="12" r="9"/><path d="m5.6 5.6 12.8 12.8"/>"#,
            PIco::SquareFilled => r#"<rect x="4" y="4" width="16" height="16" rx="2.5" fill="currentColor"/>"#,
            PIco::SquareBoth => r#"<rect x="4" y="4" width="16" height="16" rx="2.5" fill="currentColor" fill-opacity="0.35"/>"#,
            PIco::Gradient => r#"<rect x="3" y="3" width="18" height="18" rx="3"/><path d="M3 15 15 3M3 21 21 3M9 21 21 9" stroke-width="1.5" stroke-opacity="0.6"/>"#,
            PIco::ImagePlus => r#"<path d="M16 5h6M19 2v6"/><path d="M21 11.5V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7.5"/><path d="m21 15-3.1-3.1a2 2 0 0 0-2.8 0L6 21"/><circle cx="9" cy="9" r="2"/>"#,
            PIco::Monitor => r#"<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>"#,
            PIco::Images => r#"<path d="M18 22H4a2 2 0 0 1-2-2V6"/><path d="m22 13-1.3-1.3a2.4 2.4 0 0 0-3.4 0L11 18"/><circle cx="12" cy="8" r="2"/><rect x="6" y="2" width="16" height="16" rx="2"/>"#,
            PIco::HeadFilled => r#"<path d="M3 12h11"/><path d="M13 6.5 20.5 12 13 17.5Z" fill="currentColor"/>"#,
            PIco::HeadOpen => r#"<path d="M3 12h17"/><path d="m14 6 6 6-6 6"/>"#,
            PIco::HeadRound => r#"<path d="M3 12h12"/><circle cx="17.5" cy="12" r="3.2" fill="currentColor"/>"#,
            PIco::HeadNone => r#"<path d="M3 12h18"/>"#,
            PIco::DashSolid => r#"<path d="M3 12h18" stroke-width="2.5"/>"#,
            PIco::DashDashed => r#"<path d="M3 12h4M10 12h4M17 12h4" stroke-width="2.5" stroke-linecap="butt"/>"#,
            PIco::DashDotted => r#"<path d="M4 12h.01M8 12h.01M12 12h.01M16 12h.01M20 12h.01" stroke-width="3"/>"#,
            PIco::CapRound => r#"<path d="M6 12h12" stroke-width="7" stroke-linecap="round"/>"#,
            PIco::CapSquare => r#"<path d="M6 12h12" stroke-width="7" stroke-linecap="square"/>"#,
            PIco::CapButt => r#"<path d="M6 12h12" stroke-width="7" stroke-linecap="butt"/>"#,
            PIco::ChevronDown => r#"<path d="m6 9 6 6 6-6"/>"#,
            PIco::ChevronRight => r#"<path d="m9 18 6-6-6-6"/>"#,
            PIco::ArrowUp => r#"<path d="m5 12 7-7 7 7M12 19V5"/>"#,
            PIco::ArrowDown => r#"<path d="M12 5v14M19 12l-7 7-7-7"/>"#,
            PIco::Palette => r#"<path d="M12 22a10 10 0 1 1 10-10c0 2.8-2.2 4-4 4h-1.8a1.7 1.7 0 0 0-1.2 2.9c.4.4.5 1 .2 1.5-.5 1-1.8 1.6-3.2 1.6Z"/><circle cx="13.5" cy="6.5" r="1" fill="currentColor"/><circle cx="17.5" cy="10.5" r="1" fill="currentColor"/><circle cx="8.5" cy="7.5" r="1" fill="currentColor"/><circle cx="6.5" cy="12.5" r="1" fill="currentColor"/>"#,
            PIco::Sliders => r#"<path d="M21 4h-7M10 4H3M21 12h-9M8 12H3M21 20h-5M12 20H3M14 2v4M8 10v4M16 18v4"/>"#,
            PIco::Frame => r#"<rect x="3" y="3" width="18" height="18" rx="5"/><rect x="8" y="8" width="8" height="8" rx="1.5"/>"#,
            PIco::Shadow => r#"<rect x="3" y="3" width="13" height="13" rx="2"/><path d="M20 8v10a2 2 0 0 1-2 2H8" stroke-opacity="0.55"/>"#,
            PIco::Ratio => r#"<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10v4M18 10v4"/>"#,
            PIco::AppWindow => r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M2 9h20"/><path d="M6 6.5h.01M9 6.5h.01"/>"#,
            PIco::Globe => r#"<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20M2 12h20"/>"#,
            PIco::Stamp => r#"<path d="M5 22h14"/><path d="M19.3 14.3A2 2 0 0 0 18 14H6a2 2 0 0 0-2 2v2h16v-2a2 2 0 0 0-.7-1.7Z"/><path d="M15 14V9.5a3 3 0 1 0-6 0V14"/>"#,
            PIco::Layers => r#"<path d="m12 2 9.5 5L12 12 2.5 7Z"/><path d="m2.5 12 9.5 5 9.5-5"/><path d="m2.5 17 9.5 5 9.5-5"/>"#,
            PIco::Padding => r#"<rect x="3" y="3" width="18" height="18" rx="2" stroke-dasharray="3 3"/><rect x="8" y="8" width="8" height="8" rx="1"/>"#,
            PIco::Bookmark => r#"<path d="m19 21-7-4-7 4V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2Z"/>"#,
            PIco::Share => r#"<path d="M12 3v12M7 8l5-5 5 5"/><path d="M5 13v6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-6"/>"#,
            PIco::CornerTL => r#"<rect x="3" y="3" width="18" height="18" rx="2" stroke-opacity="0.45"/><rect x="6" y="6" width="5" height="5" rx="1" fill="currentColor"/>"#,
            PIco::CornerTR => r#"<rect x="3" y="3" width="18" height="18" rx="2" stroke-opacity="0.45"/><rect x="13" y="6" width="5" height="5" rx="1" fill="currentColor"/>"#,
            PIco::CornerBL => r#"<rect x="3" y="3" width="18" height="18" rx="2" stroke-opacity="0.45"/><rect x="6" y="13" width="5" height="5" rx="1" fill="currentColor"/>"#,
            PIco::Ellipsis => r#"<circle cx="12" cy="12" r="1.2" fill="currentColor"/><circle cx="19" cy="12" r="1.2" fill="currentColor"/><circle cx="5" cy="12" r="1.2" fill="currentColor"/>"#,
            PIco::CornerBR => r#"<rect x="3" y="3" width="18" height="18" rx="2" stroke-opacity="0.45"/><rect x="13" y="13" width="5" height="5" rx="1" fill="currentColor"/>"#,
        }
    }

    pub fn icon(self) -> Icon {
        let svg = format!("{HEAD}{}</svg>", self.body());
        Icon::default().data(svg.as_bytes())
    }
}

/// Either a shared toolbar icon or a panel-only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnyIco {
    T(Ico),
    P(PIco),
}

impl AnyIco {
    pub fn icon(self) -> Icon {
        match self {
            AnyIco::T(i) => i.icon(),
            AnyIco::P(p) => p.icon(),
        }
    }
}

impl From<Ico> for AnyIco {
    fn from(i: Ico) -> Self {
        AnyIco::T(i)
    }
}

impl From<PIco> for AnyIco {
    fn from(p: PIco) -> Self {
        AnyIco::P(p)
    }
}
