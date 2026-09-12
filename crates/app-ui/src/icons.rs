use std::ffi::OsStr;
use std::path::Path;

use file_core::FileKind;
use iced::widget::{svg, Svg};
use iced::{Length, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconSymbol {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Bookmark,
    Check,
    FileSolid,
    FolderSolid,
    ChevronRight,
    Close,
    Columns,
    Copy,
    Download,
    Eye,
    EyeOff,
    File,
    FileArchive,
    FileCode,
    FileImage,
    FileText,
    Folder,
    FolderOpen,
    Grid,
    Ghost,
    GhostX,
    GripVertical,
    HardDrive,
    Hash,
    House,
    Link,
    List,
    Minus,
    Monitor,
    Music,
    Pause,
    PanelRight,
    Pencil,
    Pin,
    Play,
    Plus,
    Regex,
    RestoreWindow,
    Search,
    Scissors,
    Settings,
    Square,
    Terminal,
    Trash,
    TriangleAlert,
    Video,
    Volume2,
}

pub(crate) fn file_entry_icon_symbol(kind: FileKind, name: &OsStr) -> IconSymbol {
    file_kind_icon_symbol(kind, Path::new(name).extension().and_then(OsStr::to_str))
}

pub(crate) fn preview_entry_icon_symbol(kind: FileKind, name: &str) -> IconSymbol {
    file_kind_icon_symbol(kind, Path::new(name).extension().and_then(OsStr::to_str))
}

pub(crate) fn rotated_chevron_right_view(rotation_degrees: f32, size: f32) -> Svg<'static, Theme> {
    Svg::new(svg::Handle::from_memory(rotated_chevron_right_bytes(
        rotation_degrees,
    )))
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
}

fn rotated_chevron_right_bytes(rotation_degrees: f32) -> Vec<u8> {
    let rotation_degrees = rotation_degrees.clamp(-90.0, 90.0);
    let Some(path_start) = CHEVRON_RIGHT_SVG.find("<path") else {
        return CHEVRON_RIGHT_SVG.as_bytes().to_vec();
    };
    let Some(svg_end) = CHEVRON_RIGHT_SVG.rfind("</svg>") else {
        return CHEVRON_RIGHT_SVG.as_bytes().to_vec();
    };

    format!(
        "{}<g transform=\"rotate({rotation_degrees:.2} 12 12)\">{}</g>{}",
        &CHEVRON_RIGHT_SVG[..path_start],
        &CHEVRON_RIGHT_SVG[path_start..svg_end],
        &CHEVRON_RIGHT_SVG[svg_end..]
    )
    .into_bytes()
}

/// 拖拽堆叠用实心图标:中空线条图标叠放会互相穿透,实心形状靠
/// 遮挡产生层次。
pub(crate) fn file_kind_solid_symbol(kind: FileKind) -> IconSymbol {
    match kind {
        FileKind::Directory => IconSymbol::FolderSolid,
        FileKind::Symlink | FileKind::Other | FileKind::File => IconSymbol::FileSolid,
    }
}

fn file_kind_icon_symbol(kind: FileKind, extension: Option<&str>) -> IconSymbol {
    match kind {
        FileKind::Directory => IconSymbol::Folder,
        FileKind::Symlink => IconSymbol::Link,
        FileKind::Other => IconSymbol::File,
        FileKind::File => file_extension_icon_symbol(extension),
    }
}

fn file_extension_icon_symbol(extension: Option<&str>) -> IconSymbol {
    let Some(extension) = extension else {
        return IconSymbol::File;
    };

    if is_image_extension(extension) {
        IconSymbol::FileImage
    } else if is_archive_extension(extension) {
        IconSymbol::FileArchive
    } else if is_code_extension(extension) {
        IconSymbol::FileCode
    } else if is_text_extension(extension) {
        IconSymbol::FileText
    } else {
        IconSymbol::File
    }
}

fn is_image_extension(extension: &str) -> bool {
    [
        "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "tiff", "avif",
    ]
    .iter()
    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn is_archive_extension(extension: &str) -> bool {
    [
        "zip", "tar", "gz", "tgz", "xz", "bz2", "7z", "rar", "zst", "deb", "rpm",
    ]
    .iter()
    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn is_code_extension(extension: &str) -> bool {
    [
        "rs", "py", "js", "ts", "tsx", "jsx", "c", "h", "cpp", "hpp", "go", "java", "kt", "swift",
        "sh", "bash", "zsh", "fish", "html", "css", "scss", "xml",
    ]
    .iter()
    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn is_text_extension(extension: &str) -> bool {
    [
        "txt", "md", "markdown", "rst", "log", "toml", "yaml", "yml", "json", "csv",
    ]
    .iter()
    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

impl IconSymbol {
    pub(crate) fn view(self, size: f32) -> Svg<'static, Theme> {
        Svg::new(svg::Handle::from_memory(self.bytes()))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
    }

    pub(crate) fn bytes(self) -> &'static [u8] {
        match self {
            Self::ArrowDown => include_bytes!("../assets/icons/lucide/arrow-down.svg"),
            Self::ArrowLeft => include_bytes!("../assets/icons/lucide/arrow-left.svg"),
            Self::ArrowRight => include_bytes!("../assets/icons/lucide/arrow-right.svg"),
            Self::ArrowUp => include_bytes!("../assets/icons/lucide/arrow-up.svg"),
            Self::Bookmark => include_bytes!("../assets/icons/lucide/bookmark.svg"),
            Self::Check => include_bytes!("../assets/icons/lucide/check.svg"),
            Self::ChevronRight => CHEVRON_RIGHT_ICON,
            Self::Close => CLOSE_ICON,
            Self::Columns => COLUMNS_ICON,
            Self::Copy => include_bytes!("../assets/icons/lucide/copy.svg"),
            Self::Download => include_bytes!("../assets/icons/lucide/download.svg"),
            Self::Eye => include_bytes!("../assets/icons/lucide/eye.svg"),
            Self::EyeOff => include_bytes!("../assets/icons/lucide/eye-off.svg"),
            Self::File => include_bytes!("../assets/icons/lucide/file.svg"),
            Self::FileSolid => include_bytes!("../assets/icons/solid/file.svg"),
            Self::FileArchive => include_bytes!("../assets/icons/lucide/file-archive.svg"),
            Self::FileCode => include_bytes!("../assets/icons/lucide/file-code.svg"),
            Self::FileImage => include_bytes!("../assets/icons/lucide/file-image.svg"),
            Self::FileText => include_bytes!("../assets/icons/lucide/file-text.svg"),
            Self::Folder => include_bytes!("../assets/icons/lucide/folder.svg"),
            Self::FolderSolid => include_bytes!("../assets/icons/solid/folder.svg"),
            Self::FolderOpen => include_bytes!("../assets/icons/lucide/folder-open.svg"),
            Self::Grid => GRID_ICON,
            Self::Ghost => include_bytes!("../assets/icons/lucide/ghost.svg"),
            Self::GhostX => include_bytes!("../assets/icons/lucide/ghost-x.svg"),
            Self::GripVertical => include_bytes!("../assets/icons/lucide/grip-vertical.svg"),
            Self::HardDrive => HARD_DRIVE_ICON,
            Self::Hash => include_bytes!("../assets/icons/lucide/hash.svg"),
            Self::House => include_bytes!("../assets/icons/lucide/house.svg"),
            Self::Link => include_bytes!("../assets/icons/lucide/link.svg"),
            Self::List => LIST_ICON,
            Self::Minus => MINUS_ICON,
            Self::Monitor => include_bytes!("../assets/icons/lucide/monitor.svg"),
            Self::Music => include_bytes!("../assets/icons/lucide/music.svg"),
            Self::PanelRight => include_bytes!("../assets/icons/lucide/panel-right.svg"),
            Self::Pause => PAUSE_ICON,
            Self::Pencil => include_bytes!("../assets/icons/lucide/pencil.svg"),
            Self::Pin => include_bytes!("../assets/icons/lucide/pin.svg"),
            Self::Play => PLAY_ICON,
            Self::Plus => include_bytes!("../assets/icons/lucide/plus.svg"),
            Self::Regex => include_bytes!("../assets/icons/lucide/regex.svg"),
            Self::RestoreWindow => include_bytes!("../assets/icons/lucide/copy.svg"),
            Self::Search => include_bytes!("../assets/icons/lucide/search.svg"),
            Self::Scissors => include_bytes!("../assets/icons/lucide/scissors.svg"),
            Self::Settings => SETTINGS_ICON,
            Self::Square => SQUARE_ICON,
            Self::Terminal => TERMINAL_ICON,
            Self::Trash => include_bytes!("../assets/icons/lucide/trash-2.svg"),
            Self::TriangleAlert => include_bytes!("../assets/icons/lucide/triangle-alert.svg"),
            Self::Video => include_bytes!("../assets/icons/lucide/video.svg"),
            Self::Volume2 => VOLUME_2_ICON,
        }
    }
}

const CHEVRON_RIGHT_ICON: &[u8] = include_bytes!("../assets/icons/lucide/chevron-right.svg");
const CHEVRON_RIGHT_SVG: &str = include_str!("../assets/icons/lucide/chevron-right.svg");
const CLOSE_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"#;
const COLUMNS_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/><path d="M15 3v18"/></svg>"#;
const HARD_DRIVE_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="22" x2="2" y1="12" y2="12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/><line x1="6" x2="6.01" y1="16" y2="16"/><line x1="10" x2="10.01" y1="16" y2="16"/></svg>"#;
const GRID_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/></svg>"#;
const LIST_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 6h13"/><path d="M8 12h13"/><path d="M8 18h13"/><path d="M3 6h.01"/><path d="M3 12h.01"/><path d="M3 18h.01"/></svg>"#;
const MINUS_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/></svg>"#;
const PAUSE_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/></svg>"#;
const PLAY_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 3 20 12 6 21 6 3"/></svg>"#;
const SETTINGS_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.32-1.915"/><circle cx="12" cy="12" r="3"/></svg>"#;
const SQUARE_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/></svg>"#;
const TERMINAL_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/></svg>"#;
const VOLUME_2_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4.702a1 1 0 0 0-1.707-.707L5.586 7.702A1 1 0 0 1 4.879 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h1.879a1 1 0 0 1 .707.298l3.707 3.707A1 1 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/></svg>"#;
