use std::collections::HashSet;
use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::{keyboard, Point};

use crate::app::FileBrowser;
use crate::config;
use crate::model::BrowserLocation;
use crate::model::{ExpandedDirectory, ExpandedDirectoryStatus, SelectionMarqueePhase};

fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        path,
        kind,
        EntryMetadata {
            len: 0,
            modified: None,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}

fn loaded_directory(entries: Vec<DirectoryEntry>) -> ExpandedDirectory {
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
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

fn browser_with_entries(entries: Vec<DirectoryEntry>) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser.tab_mut().entries = entries.into();
    browser
}

#[test]
fn dragging_upper_column_entry_preserves_open_descendant_columns_after_release() {
    let root = PathBuf::from("/workspace");
    let source = root.join("report.txt");
    let project = root.join("project");
    let source_directory = project.join("src");
    let mut browser = browser_with_entries(vec![
        test_entry(source.clone(), FileKind::File),
        test_entry(project.clone(), FileKind::Directory),
    ]);
    browser.tab_mut().expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(
            source_directory.clone(),
            FileKind::Directory,
        )]),
    );
    browser
        .tab_mut()
        .expanded_directories
        .insert(source_directory.clone(), loaded_directory(Vec::new()));
    browser.tab_mut().deepest_open_column_directory = Some(source_directory.clone());
    let open_columns = vec![root, project, source_directory.clone()];

    drop(browser.handle_column_entry_clicked(source));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(source_directory.clone())
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        open_columns
    );

    drop(browser.update_file_drag(Point::new(10.0, 0.0)));
    assert!(browser
        .file_drag
        .as_ref()
        .is_some_and(|file_drag| file_drag.is_dragging()));
    drop(browser.finish_drag_selection(None));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(source_directory)
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        open_columns
    );
}

#[test]
fn stationary_upper_column_click_commits_new_column_context_on_release() {
    let root = PathBuf::from("/workspace");
    let source = root.join("report.txt");
    let project = root.join("project");
    let source_directory = project.join("src");
    let mut browser = browser_with_entries(vec![test_entry(source.clone(), FileKind::File)]);
    browser.tab_mut().deepest_open_column_directory = Some(source_directory.clone());

    drop(browser.handle_column_entry_clicked(source));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(source_directory)
    );

    drop(browser.finish_drag_selection(None));

    assert_eq!(browser.tab().deepest_open_column_directory, None);
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![root]
    );
}

#[test]
fn ctrl_and_shift_upper_column_selection_preserve_open_descendant_columns() {
    let root = PathBuf::from("/workspace");
    let first = root.join("first.txt");
    let second = root.join("second.txt");
    let project = root.join("project");
    let source_directory = project.join("src");
    let mut browser = browser_with_entries(vec![
        test_entry(first.clone(), FileKind::File),
        test_entry(second.clone(), FileKind::File),
        test_entry(project, FileKind::Directory),
    ]);
    browser.tab_mut().deepest_open_column_directory = Some(source_directory.clone());
    browser.select_path(first.clone());

    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
    drop(browser.handle_column_entry_clicked(second));
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    drop(browser.handle_column_entry_clicked(first));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(source_directory)
    );
}

#[test]
fn dragging_upper_column_blank_preserves_open_descendant_columns() {
    let current_dir = PathBuf::from("/workspace");
    let project = current_dir.join("project");
    let source_directory = project.join("src");
    let mut browser = browser_with_entries(vec![test_entry(project.clone(), FileKind::Directory)]);
    browser.tab_mut().deepest_open_column_directory = Some(source_directory.clone());

    drop(browser.start_column_blank_selection_marquee(current_dir.clone()));
    browser
        .selection_marquee
        .as_mut()
        .expect("selection marquee starts")
        .phase = SelectionMarqueePhase::Selecting;
    drop(browser.update_selection_from_column_entry_bounds(Vec::new()));
    drop(browser.finish_drag_selection(None));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(source_directory.clone())
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![current_dir, project, source_directory]
    );
}

#[test]
fn control_marquee_in_upper_column_preserves_open_descendant_columns() {
    let current_dir = PathBuf::from("/workspace");
    let preserved = current_dir.join("preserved.txt");
    let project = current_dir.join("project");
    let source_directory = project.join("src");
    let mut browser = browser_with_entries(vec![
        test_entry(preserved.clone(), FileKind::File),
        test_entry(project.clone(), FileKind::Directory),
    ]);
    browser.tab_mut().deepest_open_column_directory = Some(source_directory.clone());
    browser.tab_mut().selected = Some(preserved.clone());
    *browser.tab_mut().selected_paths = HashSet::from([preserved.clone()]);
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;

    drop(browser.start_column_blank_selection_marquee(project));
    browser
        .selection_marquee
        .as_mut()
        .expect("selection marquee starts")
        .phase = SelectionMarqueePhase::Selecting;
    drop(browser.update_selection_from_column_entry_bounds(Vec::new()));
    drop(browser.finish_drag_selection(None));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(source_directory)
    );
    assert_eq!(browser.tab().selected_paths, HashSet::from([preserved]));
}

#[test]
fn archive_root_column_blank_click_keeps_archive_column_chain() {
    let current_dir = PathBuf::from("/workspace");
    let archive = current_dir.join("bundle.zip");
    let inner = archive.join("docs");
    let mut browser = browser_with_entries(vec![test_entry(archive.clone(), FileKind::File)]);
    browser.tab_mut().expanded_directories.insert(
        archive.clone(),
        loaded_directory(vec![test_entry(inner.clone(), FileKind::Directory)]),
    );
    browser.tab_mut().deepest_open_column_directory = Some(inner.clone());

    drop(browser.handle_column_blank_clicked(archive.clone()));

    // 包根在真实文件系统中是文件:空白点击必须收敛到包根栏,
    // 而不是清空栏目链退回原目录。
    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(archive.clone())
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![current_dir, archive]
    );
}

#[test]
fn inside_archive_column_blank_click_collapses_to_inner_directory() {
    let current_dir = PathBuf::from("/workspace");
    let archive = current_dir.join("bundle.zip");
    let inner = archive.join("docs");
    let mut browser = browser_with_entries(vec![test_entry(archive.clone(), FileKind::File)]);
    browser.tab_mut().expanded_directories.insert(
        archive.clone(),
        loaded_directory(vec![test_entry(inner.clone(), FileKind::Directory)]),
    );
    browser.tab_mut().deepest_open_column_directory = Some(inner.clone());

    drop(browser.handle_column_blank_clicked(inner.clone()));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(inner.clone())
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![current_dir, archive, inner]
    );
}

/// 按下与松开之间，当前目录在磁盘上变了（家目录里一直被写的日志）并整目录重扫：
/// 重扫若收掉按下的手势，松开时打不开新栏，留下「选中 A、仍打开 B」。
#[tokio::test]
async fn disk_rescan_between_press_and_release_still_opens_the_pressed_column() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    let bin = root.join("bin");
    let tmpfs = root.join("tmpfs");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&tmpfs).unwrap();
    let scan = || {
        file_core::discover_directory_with_progress(
            &root,
            file_core::ScanOptions::default(),
            tokio_util::sync::CancellationToken::new(),
            |_| {},
        )
    };
    let mut browser = browser_with_entries(Vec::new());
    browser.tab_mut().location = BrowserLocation::Directory(root.clone());
    let discovery = scan().await.unwrap();
    browser.set_entries(crate::model::display_entries_in_discovery_order(&discovery).into());
    browser.tab_mut().directory_discovery = Some(discovery);
    browser
        .tab_mut()
        .expanded_directories
        .insert(tmpfs.clone(), loaded_directory(Vec::new()));
    browser.tab_mut().deepest_open_column_directory = Some(tmpfs);

    drop(browser.handle_column_entry_clicked(bin.clone()));
    drop(
        browser.reload_observed_directory(file_core::DirectoryEntryChanges {
            directory: root.clone(),
            changes: vec![file_core::ResolvedEntryChange::RescanRequired],
        }),
    );
    let request = crate::model::DirectoryLoadRequest {
        pane_id: browser.active_pane_id(),
        path: root.clone(),
        generation: browser.pane().directory_load_generation,
    };
    let landed = scan().await.unwrap();
    drop(browser.accept_directory_discovery(
        request,
        crate::model::PrebuiltDirectoryDiscovery::build(landed),
    ));
    drop(browser.finish_drag_selection(None));

    assert_eq!(
        browser.tab().deepest_open_column_directory,
        Some(bin.clone())
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![root, bin]
    );
}
