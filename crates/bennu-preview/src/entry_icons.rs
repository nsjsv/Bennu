//! 目录条目按种类选图标：主程序列表/多栏/大图、portal 选择窗和预览目录树共用。
//! 放在这里是因为它同时需要 file-core 的 `FileKind` 与 bennu-theme 的图标词汇，
//! 而 bennu-theme 不依赖 file-core（与 `formatting::file_kind_label` 同理）。

use std::ffi::OsStr;
use std::path::Path;

use bennu_theme::icons::{file_extension_icon_symbol, IconSymbol};
use file_core::FileKind;

pub fn file_entry_icon_symbol(kind: FileKind, name: &OsStr) -> IconSymbol {
    file_kind_icon_symbol(kind, Path::new(name).extension().and_then(OsStr::to_str))
}

pub fn preview_entry_icon_symbol(kind: FileKind, name: &str) -> IconSymbol {
    file_kind_icon_symbol(kind, Path::new(name).extension().and_then(OsStr::to_str))
}

/// 拖拽堆叠用实心图标:中空线条图标叠放会互相穿透,实心形状靠
/// 遮挡产生层次。
pub fn file_kind_solid_symbol(kind: FileKind) -> IconSymbol {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_follows_kind_first_then_extension() {
        assert_eq!(
            file_entry_icon_symbol(FileKind::Directory, OsStr::new("photos.png")),
            IconSymbol::Folder
        );
        assert_eq!(
            file_entry_icon_symbol(FileKind::File, OsStr::new("photo.PNG")),
            IconSymbol::FileImage
        );
        assert_eq!(
            preview_entry_icon_symbol(FileKind::Symlink, "main.rs"),
            IconSymbol::Link
        );
        assert_eq!(
            file_kind_solid_symbol(FileKind::Other),
            IconSymbol::FileSolid
        );
    }
}
