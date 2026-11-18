//! 状态栏的当前目录统计由显示着的条目求和：文件信息补齐前显示 "-"，补齐后是可见文件的总大小；
//! 不显示隐藏文件时隐藏条目数取扫描时见到的；多栏取最深那一栏；目录变化后只在内存里重算。

use std::path::Path;

use file_core::{
    discover_directory_with_progress, resolve_directory_metadata, DirectoryDiscovery,
    DirectoryEntry, DirectoryMetadataRequest, DirectoryMetadataRequirement, EntryMetadata,
    FileKind, ScanOptions,
};
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::model::{
    BrowserLocation, BrowserViewMode, DirectoryOrderPhase, ExpandedDirectory,
    ExpandedDirectoryStatus,
};

async fn discover(path: &Path) -> DirectoryDiscovery {
    discover_directory_with_progress(
        path,
        ScanOptions::default(),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap()
}

async fn resolve_all(discovery: &DirectoryDiscovery) {
    resolve_directory_metadata(
        discovery.metadata_resolver.clone(),
        DirectoryMetadataRequest {
            request_generation: 0,
            requirement: DirectoryMetadataRequirement::Filesystem,
            targets: (0..discovery.entries.len()).collect(),
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
}

/// 一个目录：a.txt（3 字节）、b.txt（5 字节）、文件夹 sub、隐藏文件 .h（7 字节）。
fn write_fixture(path: &Path) {
    std::fs::write(path.join("a.txt"), b"aaa").unwrap();
    std::fs::write(path.join("b.txt"), b"bbbbb").unwrap();
    std::fs::create_dir(path.join("sub")).unwrap();
    std::fs::write(path.join(".h"), b"hhhhhhh").unwrap();
}

fn browser_showing(path: &Path, discovery: DirectoryDiscovery) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.tab_mut().choose_view_mode(BrowserViewMode::Icons);
    browser.tab_mut().location = BrowserLocation::Directory(path.to_path_buf());
    let request = browser.next_directory_load_request(path.to_path_buf());
    drop(browser.accept_directory_discovery(
        request,
        crate::model::PrebuiltDirectoryDiscovery::build(discovery),
    ));
    browser
}

fn status(browser: &FileBrowser) -> (Option<u64>, Option<usize>) {
    let entry = browser.pane_status_strip_entries()[0];
    (
        entry.visible_files_total_size_bytes,
        entry.hidden_entry_count,
    )
}

#[tokio::test]
async fn totals_wait_for_every_file_size_then_sum_the_visible_files() {
    let directory = tempfile::tempdir().unwrap();
    write_fixture(directory.path());
    let discovery = discover(directory.path()).await;
    let browser = browser_showing(directory.path(), discovery.clone());

    // 文件信息还在补：总大小未知；隐藏文件不显示，个数是扫描时见到的。
    assert_eq!(status(&browser), (None, Some(1)));

    resolve_all(&discovery).await;
    // 补信息的结果作为消息到达时文件信息版本号加一（`accept_directory_metadata_resolution`）。
    let mut browser = browser;
    browser.entry_metadata_revision += 1;
    assert_eq!(status(&browser), (Some(3 + 5), Some(1)));
}

#[tokio::test]
async fn shown_hidden_files_count_in_the_total() {
    let directory = tempfile::tempdir().unwrap();
    write_fixture(directory.path());
    let discovery = discover_directory_with_progress(
        directory.path(),
        ScanOptions {
            include_hidden: true,
            ..ScanOptions::default()
        },
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    resolve_all(&discovery).await;
    let mut browser = browser_showing(directory.path(), discovery);
    browser.user_config.show_hidden_files = true;

    assert_eq!(status(&browser), (Some(3 + 5 + 7), Some(1)));
}

/// 目录里新增文件（监视到的条目自带文件信息）：总大小跟着变，不用再读盘。
#[tokio::test]
async fn an_added_file_changes_the_total_without_reading_the_directory_again() {
    let directory = tempfile::tempdir().unwrap();
    write_fixture(directory.path());
    let discovery = discover(directory.path()).await;
    resolve_all(&discovery).await;
    let mut browser = browser_showing(directory.path(), discovery);
    assert_eq!(status(&browser).0, Some(8));

    browser
        .tab_mut()
        .entries
        .make_mut()
        .push(DirectoryEntry::new(
            directory.path().join("c.txt"),
            FileKind::File,
            EntryMetadata {
                len: 100,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        ));

    assert_eq!(status(&browser).0, Some(108));
}

/// 多栏模式下「当前目录」与地址栏同源：最深打开的那一栏，而非栏浏览根。
#[tokio::test]
async fn columns_report_the_deepest_open_column() {
    let root = tempfile::tempdir().unwrap();
    write_fixture(root.path());
    let column = root.path().join("sub");
    std::fs::write(column.join("inner.bin"), [0_u8; 256]).unwrap();
    let root_discovery = discover(root.path()).await;
    let column_discovery = discover(&column).await;
    resolve_all(&root_discovery).await;
    resolve_all(&column_discovery).await;
    let mut browser = browser_showing(root.path(), root_discovery);
    browser.tab_mut().choose_view_mode(BrowserViewMode::Columns);
    browser.tab_mut().deepest_open_column_directory = Some(column.clone());
    browser.tab_mut().expanded_directories.insert(
        column,
        ExpandedDirectory {
            entries: crate::model::display_entries_in_discovery_order(&column_discovery).into(),
            directory_discovery: Some(column_discovery),
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
        },
    );

    assert_eq!(status(&browser), (Some(256), Some(0)));
}

/// 按大小、修改时间排序要等全部条目（文件夹也算）的文件信息：图标、分栏视图也得有人去要，
/// 原来只有列表视图要，别的视图顺序一直停在「等文件信息」。
#[tokio::test]
async fn size_order_in_icon_view_demands_every_entry() {
    let directory = tempfile::tempdir().unwrap();
    write_fixture(directory.path());
    let options = ScanOptions {
        sort_field: file_core::SortField::Size,
        ..ScanOptions::default()
    };
    let discovery = discover_directory_with_progress(
        directory.path(),
        options.clone(),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.options = options;
    browser.tab_mut().choose_view_mode(BrowserViewMode::Icons);
    browser.tab_mut().location = BrowserLocation::Directory(directory.path().to_path_buf());
    let request = browser.next_directory_load_request(directory.path().to_path_buf());
    drop(browser.accept_directory_discovery(
        request,
        crate::model::PrebuiltDirectoryDiscovery::build(discovery.clone()),
    ));

    let demanded = browser
        .directory_metadata_in_flight
        .iter()
        .map(|key| key.index)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(demanded, (0..discovery.entries.len()).collect());
}
