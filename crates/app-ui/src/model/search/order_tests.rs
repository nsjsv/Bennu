use std::path::PathBuf;

use file_core::SortDirection;
use file_search::{SearchOrder, SearchSortDirection, SearchSortField};

use crate::model::search::{SearchWorkspaceSessionId, SearchWorkspaceState};
use crate::model::{ListColumnKind, SearchDirectoryScope};

fn search_for(input: &str) -> SearchWorkspaceState {
    let mut search = SearchWorkspaceState::new(
        PathBuf::from("/home/user/docs"),
        PathBuf::from("/home/user"),
        None,
        SearchWorkspaceSessionId(1),
    );
    let _ = search.replace_input(input.to_owned());
    search
}

fn by_name(direction: SearchSortDirection) -> SearchOrder {
    SearchOrder::Field {
        field: SearchSortField::Name,
        direction,
        directories_first: true,
    }
}

#[test]
fn a_new_search_is_ordered_by_relevance() {
    assert_eq!(search_for("report").order(), SearchOrder::Relevance);
}

#[test]
fn clicking_a_column_cycles_ascending_descending_relevance() {
    let mut search = search_for("report");
    assert!(search.click_sort_column(ListColumnKind::Name, true));
    assert_eq!(search.order(), by_name(SearchSortDirection::Ascending));
    assert_eq!(
        search.column_sort_direction(ListColumnKind::Name),
        Some(SortDirection::Ascending)
    );
    assert_eq!(search.column_sort_direction(ListColumnKind::Size), None);

    assert!(search.click_sort_column(ListColumnKind::Name, true));
    assert_eq!(search.order(), by_name(SearchSortDirection::Descending));

    assert!(search.click_sort_column(ListColumnKind::Name, true));
    assert_eq!(search.order(), SearchOrder::Relevance);
    assert_eq!(search.column_sort_direction(ListColumnKind::Name), None);
}

#[test]
fn columns_the_index_cannot_order_are_ignored() {
    let mut search = search_for("report");
    assert!(!search.click_sort_column(ListColumnKind::Owner, true));
    assert!(!search.click_sort_column(ListColumnKind::Permissions, true));
    assert_eq!(search.order(), SearchOrder::Relevance);
}

#[test]
fn the_location_column_orders_by_containing_folder() {
    let mut search = search_for("report");
    assert!(search.click_sort_column(ListColumnKind::Location, false));
    assert_eq!(
        search.order(),
        SearchOrder::Field {
            field: SearchSortField::Location,
            direction: SearchSortDirection::Ascending,
            directories_first: false,
        }
    );
}

#[test]
fn refreshing_the_same_search_keeps_the_chosen_order() {
    let mut search = search_for("report");
    search.click_sort_column(ListColumnKind::Name, true);
    // 末尾空格不改变查询的词。
    let _ = search.replace_input("report ".to_owned());
    search.drop_order_of_previous_search();
    assert_eq!(search.order(), by_name(SearchSortDirection::Ascending));
}

#[test]
fn new_words_filters_or_scope_go_back_to_relevance_for_good() {
    let mut search = search_for("report");
    search.click_sort_column(ListColumnKind::Name, true);
    let _ = search.replace_input("reports".to_owned());
    assert_eq!(search.order(), SearchOrder::Relevance);
    search.drop_order_of_previous_search();
    let _ = search.replace_input("report".to_owned());
    assert_eq!(search.order(), SearchOrder::Relevance);

    search.click_sort_column(ListColumnKind::Name, true);
    search.filters.toggle_custom_extensions();
    assert_eq!(search.order(), SearchOrder::Relevance);

    let mut search = search_for("report");
    search.click_sort_column(ListColumnKind::Name, true);
    assert!(search.root.select_scope(SearchDirectoryScope::Home));
    assert_eq!(search.order(), SearchOrder::Relevance);
}

#[test]
fn going_back_to_a_search_restores_its_order() {
    let mut search = search_for("report");
    search.click_sort_column(ListColumnKind::Name, true);
    search.click_sort_column(ListColumnKind::Name, true);
    let restored = SearchWorkspaceState::from_recall(
        search.recall(),
        PathBuf::from("/home/user"),
        SearchWorkspaceSessionId(2),
    );
    assert_eq!(restored.order(), by_name(SearchSortDirection::Descending));
}
