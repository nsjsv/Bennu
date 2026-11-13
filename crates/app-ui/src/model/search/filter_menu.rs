//! 搜索筛选栏上的下拉。一次只开一个；点外面、按 Esc、离开搜索都会关掉。

use file_search::SearchTextScope;

use super::{SearchDatePreset, SearchDirectoryScope, SearchSizePreset};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchFilterMenu {
    Scope,
    Types,
    TextScope,
    Time,
    Size,
}

/// 下拉里点一下就定下来的选项。类型（多选）和时间种类（选完还要挑范围）不在这里，
/// 点了下拉不关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchFilterPick {
    Scope(SearchDirectoryScope),
    TextScope(SearchTextScope),
    DatePreset(SearchDatePreset),
    Size(SearchSizePreset),
}

impl SearchFilterPick {
    /// 选完就关；只有“自定义大小”要接着在下拉里填最小、最大。
    pub(crate) fn closes_menu(self) -> bool {
        !matches!(self, Self::Size(SearchSizePreset::Custom))
    }
}
