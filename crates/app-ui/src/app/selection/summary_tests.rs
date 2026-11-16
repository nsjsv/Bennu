use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use crate::app::FileBrowser;
use crate::config;
use crate::model::{
    BrowserLocation, BrowserPaneId, BrowserViewMode, DirectoryCollectionPhase, DirectoryOrderPhase,
    ExpandedDirectory, ExpandedDirectoryStatus,
};
use crate::selection_summary::PaneSelectionSummary;

fn entry(path: &str, kind: FileKind, len: u64) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        kind,
        EntryMetadata {
            len,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}

fn loaded(entries: Vec<DirectoryEntry>) -> ExpandedDirectory {
    ExpandedDirectory {
        entries: entries.into(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

/// `/workspace` 里一个文件夹、一个文件，文件夹展开着；选中项里有直接条目、展开栏里的条目，
/// 还有已经不在任何一栏里的路径。
fn summary_in(view_mode: BrowserViewMode) -> Option<PaneSelectionSummary> {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let tab = browser.tab_mut();
    tab.choose_view_mode(view_mode);
    tab.location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
    tab.entries = vec![
        entry("/workspace/project", FileKind::Directory, 4096),
        entry("/workspace/notes.txt", FileKind::File, 10),
    ]
    .into();
    tab.expanded_directories.insert(
        PathBuf::from("/workspace/project"),
        loaded(vec![
            entry("/workspace/project/a.txt", FileKind::File, 20),
            entry("/workspace/project/src", FileKind::Directory, 4096),
        ]),
    );
    tab.selected_paths = [
        "/workspace/notes.txt",
        "/workspace/project/a.txt",
        "/workspace/project/src",
        "/workspace/gone.txt",
        "/elsewhere/x.txt",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    browser.pane_selection_summary(browser.pane_view(BrowserPaneId::PRIMARY).unwrap())
}

/// 列表、多栏：直接条目加展开栏里的条目；不在任何一栏里的选中路径不计。
#[test]
fn list_and_columns_count_selected_entries_in_expanded_directories() {
    for view_mode in [BrowserViewMode::List, BrowserViewMode::Columns] {
        assert_eq!(
            summary_in(view_mode),
            Some(PaneSelectionSummary {
                directory_count: 1,
                file_count: 2,
                file_total_bytes: 30,
            }),
            "{view_mode:?}"
        );
    }
}

/// 图标视图只有直接条目和图标交互展开的目录；列表的展开栏不算。
#[test]
fn icons_count_only_their_own_entries() {
    assert_eq!(
        summary_in(BrowserViewMode::Icons),
        Some(PaneSelectionSummary {
            directory_count: 0,
            file_count: 1,
            file_total_bytes: 10,
        })
    );
}
