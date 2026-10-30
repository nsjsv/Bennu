//! 界面只给首选名，被占用时由 file-core 在原子创建/提交时续编号
//! (`file_core::entry_name_candidates`),这里不查磁盘。

use file_core::InvalidEntryName;

/// 预览表格里非法名字的短标签（批量重命名、高级新建共用）。
pub(crate) fn invalid_entry_name_label(reason: InvalidEntryName) -> String {
    match reason {
        InvalidEntryName::Empty => tr!("Empty name"),
        InvalidEntryName::CurrentDirectory | InvalidEntryName::ParentDirectory => {
            tr!("Reserved name")
        }
        InvalidEntryName::ContainsSlash => tr!("Contains \"/\""),
        InvalidEntryName::ContainsNul => tr!("Contains NUL"),
    }
}
