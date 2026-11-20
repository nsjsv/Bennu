//! 回车打开索引里找到的文件时去哪。多栏视图可以选（用户 2026-11-20 定），别的视图总是进入所在
//! 文件夹并选中它。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoundInColumns {
    /// 第一栏不动，一栏一栏展开到所在文件夹；不在第一栏下面时从两者共同的上级开始。
    ExpandFromFirstColumn,
    /// 所在文件夹作为第一栏。
    FolderAsFirstColumn,
    /// 用默认程序打开文件。
    OpenFile,
}

impl FoundInColumns {
    pub(crate) const ALL: [Self; 3] = [
        Self::ExpandFromFirstColumn,
        Self::FolderAsFirstColumn,
        Self::OpenFile,
    ];

    pub(crate) fn label(self) -> String {
        match self {
            Self::ExpandFromFirstColumn => tr!("Expand from the first column"),
            Self::FolderAsFirstColumn => tr!("Its folder as the first column"),
            Self::OpenFile => tr!("Open the file"),
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::ExpandFromFirstColumn => "expand",
            Self::FolderAsFirstColumn => "folder_first",
            Self::OpenFile => "open",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|choice| choice.config_value() == value)
    }
}
