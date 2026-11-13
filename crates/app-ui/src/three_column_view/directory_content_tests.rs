use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::*;
use crate::app::FileBrowser;
use crate::model::BrowserLocation;
use crate::model::{ExpandedDirectory, ExpandedDirectoryStatus};

fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(path, kind, EntryMetadata::default(), false, false, false)
}

#[test]
fn current_column_distinguishes_pending_empty_and_streamed_entries() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let pane_id = browser.active_pane_id();
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = Vec::new().into();
    browser.tab_mut().directory_collection_phase =
        crate::model::DirectoryCollectionPhase::Discovering;

    assert!(matches!(
        column_content(
            browser.pane_view(pane_id).expect("active pane"),
            browser.tab().current_directory().unwrap(),
        ),
        ColumnContent::Pending
    ));

    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    assert!(matches!(
        column_content(
            browser.pane_view(pane_id).expect("active pane"),
            browser.tab().current_directory().unwrap(),
        ),
        ColumnContent::Empty
    ));

    browser.tab_mut().directory_collection_phase =
        crate::model::DirectoryCollectionPhase::Discovering;
    browser.tab_mut().entries = vec![test_entry(
        browser
            .tab()
            .current_directory()
            .unwrap()
            .join("streamed.txt"),
        FileKind::File,
    )]
    .into();
    let ColumnContent::Entries(entries) = column_content(
        browser.pane_view(pane_id).expect("active pane"),
        browser.tab().current_directory().unwrap(),
    ) else {
        panic!("streamed entries must be visible before load completion");
    };
    assert_eq!(entries.len(), 1);
}

#[test]
fn child_column_preserves_pending_loaded_and_failed_mappings() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let pane_id = browser.active_pane_id();
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    let directory = browser.tab().current_directory().unwrap().join("project");
    browser.tab_mut().entries = vec![test_entry(directory.clone(), FileKind::Directory)].into();
    browser.tab_mut().expanded_directories.insert(
        directory.clone(),
        ExpandedDirectory {
            entries: Vec::new(),
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loading,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    );

    assert!(matches!(
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory,),
        ColumnContent::Pending
    ));

    let expanded = browser
        .tab_mut()
        .expanded_directories
        .get_mut(&directory)
        .expect("expanded directory");
    expanded
        .entries
        .push(test_entry(directory.join("src"), FileKind::Directory));
    let ColumnContent::Entries(entries) =
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory)
    else {
        panic!("an in-flight child column must retain existing entries");
    };
    assert_eq!(entries.len(), 1);

    let expanded = browser
        .tab_mut()
        .expanded_directories
        .get_mut(&directory)
        .expect("expanded directory");
    expanded.entries.clear();
    expanded.status = ExpandedDirectoryStatus::Loaded;
    assert!(matches!(
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory,),
        ColumnContent::Empty
    ));

    browser
        .tab_mut()
        .expanded_directories
        .get_mut(&directory)
        .expect("expanded directory")
        .status = ExpandedDirectoryStatus::Error;
    assert!(matches!(
        column_content(browser.pane_view(pane_id).expect("active pane"), &directory,),
        ColumnContent::Empty
    ));
}

/// 回归：栏实际排出的内容高度与虚拟窗口位置无关，恒等于 `content_height`，
/// 首个渲染行的顶边恒为 `entries_top_padding + start * entry_scroll_height`。
/// 用 iced 真实布局（空渲染器）量，覆盖前 spacer 为 0、中段、后 spacer 为 0
/// 与整栏都在窗口内几种情况；手写公式测不出 `Column::push` 丢零高 spacer。
#[test]
fn laid_out_column_height_is_independent_of_the_virtual_window() {
    use iced::advanced::layout::Limits;
    use iced::advanced::widget::Tree;
    use iced::Size;

    for level_index in [3, 6] {
        let geometry = ColumnGeometry::for_level(ViewDensityLevel::from_index(level_index));
        for total in [10_usize, 200] {
            let viewport_height = 600.0;
            let bottom = (geometry.content_height(total) - viewport_height).max(0.0);
            let mut offsets: Vec<f32> = (0..)
                .map(|step| step as f32 * 37.0)
                .take_while(|offset| *offset < bottom)
                .collect();
            offsets.push(bottom);
            for offset in offsets {
                let range = column_virtual_range_for_viewport(
                    total,
                    offset,
                    viewport_height,
                    COLUMN_OVERSCAN_ROWS,
                    geometry,
                );
                let rows = (range.start..range.end).map(|_| {
                    Space::new()
                        .width(Length::Fill)
                        .height(Length::Fixed(geometry.entry_height))
                        .into()
                });
                let mut content: Element<'_, (), iced::Theme, ()> =
                    virtual_column_content(range, rows, geometry).into();
                let mut tree = Tree::new(&content);
                let node = content.as_widget_mut().layout(
                    &mut tree,
                    &(),
                    &Limits::new(Size::ZERO, Size::new(240.0, f32::INFINITY)),
                );
                let case = format!("level {level_index}, {total} rows, offset {offset}");
                assert_eq!(node.size().height, geometry.content_height(total), "{case}");
                let first_row = if range.before_height > 0.0 { 1 } else { 0 };
                assert_eq!(
                    node.children()[first_row].bounds().y,
                    geometry.entries_top_padding
                        + range.start as f32 * geometry.entry_scroll_height,
                    "{case}"
                );
            }
        }
    }
}
