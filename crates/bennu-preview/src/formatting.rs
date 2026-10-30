//! 预览文案用的格式化。文件大小只有 `bennu_localization` 一份实现，这里转出去，
//! 预览各处的既有调用路径不变。

pub use bennu_localization::{format_duration, format_file_size, format_file_size_compact};

use bennu_localization::tr;
use file_core::FileKind;

/// 属性窗口与右侧信息区的"类型"一栏，绘制时按当前语言取。
pub fn file_kind_label(kind: FileKind) -> String {
    match kind {
        FileKind::Directory => tr!("Folder"),
        FileKind::File => tr!("File"),
        FileKind::Symlink => tr!("Symbolic Link"),
        FileKind::Other => tr!("Other"),
    }
}
