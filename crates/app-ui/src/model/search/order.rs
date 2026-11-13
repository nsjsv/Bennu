//! 这次搜索的结果顺序。默认按相关度；点表头选的顺序只属于选它时的词、筛选和范围，三者
//! 任一变了就是一次新搜索，顺序自然回到相关度——不必在每个改词、改筛选的入口各清一次。

use file_core::SortDirection;
use file_search::{SearchOrder, SearchSortDirection, SearchSortField};

use super::{SearchDirectoryScope, SearchFilterPresetState, SearchWorkspaceState};
use crate::model::ListColumnKind;

#[derive(Debug, Clone)]
pub(super) struct ChosenSearchOrder {
    input: String,
    filters: SearchFilterPresetState,
    scope: SearchDirectoryScope,
    order: SearchOrder,
}

impl SearchWorkspaceState {
    /// 下一次查询用的顺序。
    pub(crate) fn order(&self) -> SearchOrder {
        self.chosen_order
            .as_ref()
            .filter(|chosen| {
                chosen.input == self.input.trim()
                    && chosen.filters == self.filters
                    && chosen.scope == self.root.selected_scope()
            })
            .map_or(SearchOrder::Relevance, |chosen| chosen.order)
    }

    /// 发起查询前调用：词、筛选或范围已经换了，之前点表头选的顺序作废。之后改回原来的词
    /// 也是一次新搜索，不再沿用它。
    pub(crate) fn drop_order_of_previous_search(&mut self) {
        if self.order() == SearchOrder::Relevance {
            self.chosen_order = None;
        }
    }

    pub(super) fn choose_order(&mut self, order: SearchOrder) {
        self.chosen_order = Some(ChosenSearchOrder {
            input: self.input.trim().to_owned(),
            filters: self.filters.clone(),
            scope: self.root.selected_scope(),
            order,
        });
    }

    /// 点表头：没按这一列排 → 升序，升序 → 降序，降序 → 回到相关度，与文件夹表头同一节奏。
    /// 这一列给不了搜索结果顺序时返回 false。
    pub(crate) fn click_sort_column(
        &mut self,
        column: ListColumnKind,
        directories_first: bool,
    ) -> bool {
        let Some(field) = search_sort_field(column) else {
            return false;
        };
        let ascending = SearchOrder::Field {
            field,
            direction: SearchSortDirection::Ascending,
            directories_first,
        };
        let next = match self.order() {
            SearchOrder::Field {
                field: current,
                direction,
                ..
            } if current == field => match direction {
                SearchSortDirection::Ascending => SearchOrder::Field {
                    field,
                    direction: SearchSortDirection::Descending,
                    directories_first,
                },
                SearchSortDirection::Descending => SearchOrder::Relevance,
            },
            _ => ascending,
        };
        self.choose_order(next);
        true
    }

    /// 表头上这一列的排序指示；按相关度排时哪列都没有。
    pub(crate) fn column_sort_direction(&self, column: ListColumnKind) -> Option<SortDirection> {
        let SearchOrder::Field {
            field, direction, ..
        } = self.order()
        else {
            return None;
        };
        (search_sort_field(column) == Some(field)).then_some(match direction {
            SearchSortDirection::Ascending => SortDirection::Ascending,
            SearchSortDirection::Descending => SortDirection::Descending,
        })
    }
}

/// 列表列对应的搜索服务排序字段；属主、权限这类索引里没有的列不能排。
fn search_sort_field(column: ListColumnKind) -> Option<SearchSortField> {
    match column {
        ListColumnKind::Name => Some(SearchSortField::Name),
        ListColumnKind::Location => Some(SearchSortField::Location),
        ListColumnKind::Size => Some(SearchSortField::Size),
        ListColumnKind::Kind => Some(SearchSortField::Kind),
        ListColumnKind::Modified => Some(SearchSortField::Modified),
        ListColumnKind::Owner
        | ListColumnKind::Group
        | ListColumnKind::Permissions
        | ListColumnKind::Accessed
        | ListColumnKind::Created => None,
    }
}

#[cfg(test)]
#[path = "order_tests.rs"]
mod tests;
