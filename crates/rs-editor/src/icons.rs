//! Embedded Lucide icons (ISC license, see icons/LICENSE-LUCIDE).

use gpui_kit::component::Icon;

/// Named embedded icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Ico {
    Bold,
    BringToFront,
    Circle,
    CircleDot,
    Copy,
    Crop,
    Download,
    Droplets,
    FileArchive,
    FolderOpen,
    Grid3x3,
    GripVertical,
    Hand,
    Highlighter,
    Image,
    Italic,
    Maximize,
    MessageSquareText,
    Minus,
    MousePointer2,
    MoveUpRight,
    PenLine,
    Pipette,
    Plus,
    RectangleHorizontal,
    Redo2,
    RotateCcw,
    RotateCw,
    Save,
    ScanText,
    SendToBack,
    ShieldCheck,
    Spotlight,
    Square,
    TextAlignCenter,
    TextAlignEnd,
    TextAlignStart,
    Trash,
    Type,
    Undo2,
    X,
    ZoomIn,
    ZoomOut,
}

impl Ico {
    fn bytes(self) -> &'static [u8] {
        match self {
            Ico::Bold => include_bytes!("../icons/bold.svg"),
            Ico::BringToFront => include_bytes!("../icons/bring-to-front.svg"),
            Ico::Circle => include_bytes!("../icons/circle.svg"),
            Ico::CircleDot => include_bytes!("../icons/circle-dot.svg"),
            Ico::Copy => include_bytes!("../icons/copy.svg"),
            Ico::Crop => include_bytes!("../icons/crop.svg"),
            Ico::Download => include_bytes!("../icons/download.svg"),
            Ico::Droplets => include_bytes!("../icons/droplets.svg"),
            Ico::FileArchive => include_bytes!("../icons/file-archive.svg"),
            Ico::FolderOpen => include_bytes!("../icons/folder-open.svg"),
            Ico::Grid3x3 => include_bytes!("../icons/grid-3x3.svg"),
            Ico::GripVertical => include_bytes!("../icons/grip-vertical.svg"),
            Ico::Hand => include_bytes!("../icons/hand.svg"),
            Ico::Highlighter => include_bytes!("../icons/highlighter.svg"),
            Ico::Image => include_bytes!("../icons/image.svg"),
            Ico::Italic => include_bytes!("../icons/italic.svg"),
            Ico::Maximize => include_bytes!("../icons/maximize.svg"),
            Ico::MessageSquareText => include_bytes!("../icons/message-square-text.svg"),
            Ico::Minus => include_bytes!("../icons/minus.svg"),
            Ico::MousePointer2 => include_bytes!("../icons/mouse-pointer-2.svg"),
            Ico::MoveUpRight => include_bytes!("../icons/move-up-right.svg"),
            Ico::PenLine => include_bytes!("../icons/pen-line.svg"),
            Ico::Pipette => include_bytes!("../icons/pipette.svg"),
            Ico::Plus => include_bytes!("../icons/plus.svg"),
            Ico::RectangleHorizontal => include_bytes!("../icons/rectangle-horizontal.svg"),
            Ico::Redo2 => include_bytes!("../icons/redo-2.svg"),
            Ico::RotateCcw => include_bytes!("../icons/rotate-ccw.svg"),
            Ico::RotateCw => include_bytes!("../icons/rotate-cw.svg"),
            Ico::Save => include_bytes!("../icons/save.svg"),
            Ico::ScanText => include_bytes!("../icons/scan-text.svg"),
            Ico::SendToBack => include_bytes!("../icons/send-to-back.svg"),
            Ico::ShieldCheck => include_bytes!("../icons/shield-check.svg"),
            Ico::Spotlight => include_bytes!("../icons/spotlight.svg"),
            Ico::Square => include_bytes!("../icons/square.svg"),
            Ico::TextAlignCenter => include_bytes!("../icons/text-align-center.svg"),
            Ico::TextAlignEnd => include_bytes!("../icons/text-align-end.svg"),
            Ico::TextAlignStart => include_bytes!("../icons/text-align-start.svg"),
            Ico::Trash => include_bytes!("../icons/trash.svg"),
            Ico::Type => include_bytes!("../icons/type.svg"),
            Ico::Undo2 => include_bytes!("../icons/undo-2.svg"),
            Ico::X => include_bytes!("../icons/x.svg"),
            Ico::ZoomIn => include_bytes!("../icons/zoom-in.svg"),
            Ico::ZoomOut => include_bytes!("../icons/zoom-out.svg"),
        }
    }

    /// A gpui-component `Icon` from the embedded SVG.
    pub fn icon(self) -> Icon {
        Icon::default().data(self.bytes())
    }
}
