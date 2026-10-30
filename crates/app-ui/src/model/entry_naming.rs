//! 界面只给首选名，被占用时由 file-core 在原子创建/提交时续编号
//! (`file_core::entry_name_candidates`),这里不查磁盘。

/// 与 `operation_queue::NEW_DIRECTORY_NAME`(「New...」菜单的英文目录)区分:
/// 收纳选中项的新文件夹按工单固定使用中文名。
pub(crate) const GATHERED_FOLDER_BASE_NAME: &str = "新建文件夹";
