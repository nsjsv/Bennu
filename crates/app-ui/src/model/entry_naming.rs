//! 界面只给首选名，被占用时由 file-core 在原子创建/提交时续编号
//! (`file_core::entry_name_candidates`),这里不查磁盘。

use file_core::InvalidEntryName;

/// 与 `operation_queue::NEW_DIRECTORY_NAME`(「New...」菜单的英文目录)区分:
/// 收纳选中项的新文件夹按工单固定使用中文名。
pub(crate) const GATHERED_FOLDER_BASE_NAME: &str = "新建文件夹";

/// 预览表格里非法名字的短标签（批量重命名、高级新建共用），英文为翻译键。
pub(crate) fn invalid_entry_name_label(reason: InvalidEntryName) -> &'static str {
    match reason {
        InvalidEntryName::Empty => "Empty name",
        InvalidEntryName::CurrentDirectory | InvalidEntryName::ParentDirectory => "Reserved name",
        InvalidEntryName::ContainsSlash => "Contains \"/\"",
        InvalidEntryName::ContainsNul => "Contains NUL",
    }
}
