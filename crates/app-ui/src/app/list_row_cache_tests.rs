//! 列表行流按输入复用的不变量：输入没变拿到的是同一份；任何一项输入变了都重排，
//! 而且与清掉缓存从头排的一模一样。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};
use file_operations::test_persistence::{attach_store, enqueue_persisted};

use super::FileBrowser;
use crate::config;
use crate::model::search::{SearchWorkspaceSessionId, SearchWorkspaceState};
use crate::model::{
    BrowserLocation, BrowserPaneId, DirectoryOrderPhase, ExpandedDirectory,
    ExpandedDirectoryStatus, FileGroupingMode,
};
use crate::thumbnail_cache::ColumnViewport;
use crate::transfer_placeholder_view::{list_row_layout, ListRowLayout, ListTransferRow};

const ROW_HEIGHT: f32 = 40.0;

fn entry(path: &str, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        kind,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn expanded(entries: Vec<DirectoryEntry>, animation_progress: f32) -> ExpandedDirectory {
    ExpandedDirectory {
        entries: entries.into(),
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
        is_expanded: true,
        is_collapsing: false,
        animation_progress,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: DirectoryOrderPhase::Ready {
            field: SortField::Name,
            direction: SortDirection::Ascending,
        },
    }
}

/// /workspace：project（展开到一半）、a.txt、m.txt。
fn browser() -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = vec![
        entry("/workspace/project", FileKind::Directory),
        entry("/workspace/a.txt", FileKind::File),
        entry("/workspace/m.txt", FileKind::File),
    ]
    .into();
    browser.tab_mut().expanded_directories.insert(
        PathBuf::from("/workspace/project"),
        expanded(
            vec![entry("/workspace/project/main.rs", FileKind::File)],
            0.5,
        ),
    );
    browser
}

fn layout(browser: &FileBrowser) -> Rc<ListRowLayout> {
    list_row_layout(
        browser,
        browser.pane_view(BrowserPaneId::PRIMARY).unwrap(),
        ROW_HEIGHT,
    )
}

/// 每行画出来是什么（种类、名字、条纹、深度、动画进度、摘要高、传输进度）加总高。
fn describe(browser: &FileBrowser, layout: &ListRowLayout) -> (Vec<String>, f32) {
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let rows = (0..layout.len())
        .map(|row| match layout.row(pane, row).unwrap() {
            ListTransferRow::Entry {
                visible,
                transfer,
                stripe_index,
                snippet_height,
            } => format!(
                "entry {} stripe={stripe_index} depth={} progress={} snippet={snippet_height} transfer={:?}",
                visible.entry.path().display(),
                visible.depth,
                visible.animation_progress,
                transfer.map(|transfer| transfer.progress),
            ),
            ListTransferRow::DirectoryStatusRow {
                status,
                depth,
                height,
                stripe_index,
            } => format!("status {status:?} depth={depth} height={height} stripe={stripe_index}"),
            ListTransferRow::Placeholder {
                placeholder,
                stripe_index,
            } => format!("placeholder {} stripe={stripe_index}", placeholder.name),
            ListTransferRow::GroupHeader { title, count, .. } => format!("header {title} {count}"),
        })
        .collect();
    (rows, layout.content_height())
}

/// 清掉窗格的缓存从头排。
fn fresh(browser: &FileBrowser) -> (Vec<String>, f32) {
    *browser.pane().list_rows.borrow_mut() = None;
    describe(browser, &layout(browser))
}

#[test]
fn unchanged_inputs_reuse_the_rows_already_laid_out() {
    let mut browser = browser();
    let before = layout(&browser);

    // 滚动、悬停、选中、可见行补上文件信息（没按大小、日期分组）都不改行流。
    browser.pane_mut().list_viewport = Some(ColumnViewport {
        offset_y: 120.0,
        height: 300.0,
    });
    let a = PathBuf::from("/workspace/a.txt");
    browser.hovered_entry = Some(a.clone());
    browser.tab_mut().selected_paths.insert(a.clone());
    browser.tab_mut().selected = Some(a);
    browser.entry_metadata_revision += 1;

    assert!(Rc::ptr_eq(&before, &layout(&browser)));
}

#[test]
fn every_input_change_lays_out_again() {
    let project = Path::new("/workspace/project");
    let cases: Vec<(&str, Box<dyn Fn(&mut FileBrowser)>)> = vec![
        (
            "根层条目",
            Box::new(|browser| {
                browser
                    .tab_mut()
                    .entries
                    .make_mut()
                    .push(entry("/workspace/z.txt", FileKind::File));
            }),
        ),
        (
            "展开子目录的条目",
            Box::new(move |browser| {
                let expanded = browser
                    .tab_mut()
                    .expanded_directories
                    .get_mut(project)
                    .unwrap();
                expanded
                    .entries
                    .make_mut()
                    .push(entry("/workspace/project/lib.rs", FileKind::File));
            }),
        ),
        (
            "展开动画",
            Box::new(move |browser| {
                let expanded = browser
                    .tab_mut()
                    .expanded_directories
                    .get_mut(project)
                    .unwrap();
                expanded.animation_progress = 1.0;
            }),
        ),
        (
            "折叠",
            Box::new(move |browser| {
                let expanded = browser
                    .tab_mut()
                    .expanded_directories
                    .get_mut(project)
                    .unwrap();
                expanded.is_expanded = false;
            }),
        ),
        (
            "展开目录读不出来",
            Box::new(move |browser| {
                let expanded = browser
                    .tab_mut()
                    .expanded_directories
                    .get_mut(project)
                    .unwrap();
                expanded.status = ExpandedDirectoryStatus::Error;
            }),
        ),
        (
            "分组方式",
            Box::new(|browser| browser.user_config.file_grouping = FileGroupingMode::NameInitial),
        ),
        (
            "排序",
            Box::new(|browser| browser.options.sort_direction = SortDirection::Descending),
        ),
        (
            "传入占位",
            Box::new(|browser| {
                let mut transfer = file_operations::QueuedTransfer::new(
                    PathBuf::from("/source/incoming.txt"),
                    PathBuf::from("/workspace/incoming.txt"),
                );
                transfer.conflict_strategy = file_core::TransferConflictStrategy::Fail;
                enqueue_persisted(
                    &mut browser.operation_queue,
                    file_operations::QueuedFileOperation::Copy {
                        transfers: vec![transfer],
                        verification: file_core::FileOperationVerification::BasicMetadata,
                    },
                );
            }),
        ),
    ];
    for (input, change) in cases {
        let storage = tempfile::tempdir().unwrap();
        let mut browser = browser();
        attach_store(
            &mut browser.operation_queue,
            bennu_state::TaskQueueStore::new(storage.path().join("state.sqlite")).unwrap(),
        );
        let before = layout(&browser);

        change(&mut browser);
        let after = layout(&browser);

        assert!(!Rc::ptr_eq(&before, &after), "{input} 变了没重排");
        assert_eq!(describe(&browser, &after), fresh(&browser), "{input}");
    }
}

#[test]
fn entry_metadata_counts_only_when_grouping_reads_it() {
    // 按大小分组读条目里的大小：补上文件信息（快照不变）也要重排。
    let mut browser = browser();
    browser.user_config.file_grouping = FileGroupingMode::Size;
    let before = layout(&browser);

    browser.entry_metadata_revision += 1;

    assert!(!Rc::ptr_eq(&before, &layout(&browser)));
}

#[test]
fn row_height_change_lays_out_again() {
    let browser = browser();
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let before = list_row_layout(&browser, pane, ROW_HEIGHT);

    let taller = list_row_layout(&browser, pane, ROW_HEIGHT + 10.0);

    assert!(!Rc::ptr_eq(&before, &taller));
    assert!(taller.content_height() > before.content_height());
}

#[test]
fn search_snippets_change_lays_out_again() {
    // 搜索结果里有摘要的行高一行；摘要清掉（新一轮搜索开始）条目不变，行高要跟着变。
    let mut browser = browser();
    let mut search = SearchWorkspaceState::new(
        PathBuf::from("/workspace"),
        PathBuf::from("/home/test"),
        None,
        SearchWorkspaceSessionId(1),
    );
    search.remember_details(
        &[file_search::SearchHit {
            path: PathBuf::from("/workspace/a.txt"),
            display_name: "a.txt".to_owned(),
            kind: file_search::SearchFileKind::File,
            size: 0,
            modified_ms: None,
            accessed_ms: None,
            created_ms: None,
            rank: 1.0,
            snippet: Some("a report line".to_owned()),
            match_source: file_search::MatchSource::Content,
        }],
        &["a.txt".to_owned()],
    );
    browser.tab_mut().location = BrowserLocation::Search(Box::new(search));
    let before = layout(&browser);

    browser
        .tab_mut()
        .location
        .search_mut()
        .unwrap()
        .details_mut()
        .clear();
    let after = layout(&browser);

    assert!(after.content_height() < before.content_height());
    assert_eq!(describe(&browser, &after), fresh(&browser));
}
