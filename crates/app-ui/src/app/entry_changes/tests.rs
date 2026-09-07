use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    discover_directory_with_progress, DirectoryDiscovery, DiscoveredDirectoryEntry,
    ResolvedEntryChange, ScanOptions,
};
use tokio_util::sync::CancellationToken;

use super::super::FileBrowser;
use crate::config;
use crate::model::{BrowserViewMode, DirectoryCollectionPhase, Message};

async fn discovery_for(path: &std::path::Path) -> DirectoryDiscovery {
    discover_directory_with_progress(
        path,
        ScanOptions::default(),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap()
}

fn entry_paths(browser: &FileBrowser) -> Vec<String> {
    browser
        .entries
        .iter()
        .map(|entry| entry.name().to_string_lossy().into_owned())
        .collect()
}

fn added(entry_path: PathBuf) -> ResolvedEntryChange {
    let metadata = std::fs::symlink_metadata(&entry_path).unwrap();
    let name = entry_path.file_name().unwrap().to_os_string();
    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        file_core::FileKind::Directory
    } else {
        file_core::FileKind::File
    };
    let is_hidden = name.to_string_lossy().starts_with('.');
    ResolvedEntryChange::Added(DiscoveredDirectoryEntry::with_complete_filesystem_metadata(
        entry_path,
        name,
        kind,
        is_hidden,
        file_type.is_symlink(),
        &metadata,
    ))
}

#[tokio::test]
async fn added_entries_land_in_sorted_position() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), b"a").unwrap();
    std::fs::write(workspace.path().join("c.txt"), b"c").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = workspace.path().to_path_buf();
    browser.sync_active_tab_state();
    browser.directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.directory_discovery.as_ref().unwrap(),
    )));

    std::fs::write(workspace.path().join("b.txt"), b"b").unwrap();
    let change = added(workspace.path().join("b.txt"));
    drop(browser.apply_entry_changes(workspace.path(), &[change]));

    assert_eq!(entry_paths(&browser), vec!["a.txt", "b.txt", "c.txt"]);
    // discovery 与显示列表同步：新增条目进入 discovery 且带正确的 discovery_index。
    let discovery = browser.directory_discovery.as_ref().unwrap();
    assert_eq!(discovery.entries.len(), 3);
    assert!(browser
        .entries
        .iter()
        .enumerate()
        .all(|(position, entry)| { entry.discovery_index == Some(discovery.order[position]) }));
}

#[tokio::test]
async fn removed_entries_drop_out_and_clear_selection() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), b"a").unwrap();
    std::fs::write(workspace.path().join("b.txt"), b"b").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = workspace.path().to_path_buf();
    browser.sync_active_tab_state();
    browser.view_mode = BrowserViewMode::List;
    browser.directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.directory_discovery.as_ref().unwrap(),
    )));
    let removed_path = workspace.path().join("a.txt");
    browser.select_path(removed_path.clone());

    drop(browser.apply_entry_changes(
        workspace.path(),
        &[ResolvedEntryChange::Removed {
            path: removed_path.clone(),
        }],
    ));

    assert_eq!(entry_paths(&browser), vec!["b.txt"]);
    assert!(browser.selected_paths.get(&removed_path).is_none());
    assert_eq!(browser.selected, None);
}

#[tokio::test]
async fn same_directory_rename_moves_entry_in_place() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("old.txt"), b"x").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = workspace.path().to_path_buf();
    browser.sync_active_tab_state();
    browser.directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.directory_discovery.as_ref().unwrap(),
    )));

    let from = workspace.path().join("old.txt");
    let to = workspace.path().join("new.txt");
    std::fs::rename(&from, &to).unwrap();
    let change = added(to.clone());
    let from_clone = from.clone();
    let resolved = match change {
        ResolvedEntryChange::Added(entry) => ResolvedEntryChange::Renamed {
            from: from_clone,
            to: entry,
        },
        other => other,
    };
    drop(browser.apply_entry_changes(workspace.path(), &[resolved]));

    assert_eq!(entry_paths(&browser), vec!["new.txt"]);
    assert!(browser.find_discovered_entry(&to).is_some());
    assert!(browser.find_discovered_entry(&from).is_none());
}

#[tokio::test]
async fn cross_directory_move_updates_both_expanded_lists_without_trespass() {
    let workspace = tempfile::tempdir().unwrap();
    let source_dir = workspace.path().join("src");
    let target_dir = workspace.path().join("dst");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(source_dir.join("moved.txt"), b"x").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = workspace.path().to_path_buf();
    browser.sync_active_tab_state();
    // 多栏视图场景：两个目录都在 expanded_directories 中显示。
    browser.expanded_directories.insert(
        source_dir.clone(),
        expanded_loaded(discovery_for(&source_dir).await),
    );
    browser.expanded_directories.insert(
        target_dir.clone(),
        expanded_loaded(discovery_for(&target_dir).await),
    );

    let source = source_dir.join("moved.txt");
    let target = target_dir.join("moved.txt");
    std::fs::rename(&source, &target).unwrap();
    // 操作来源的跨目录变更：源目录 Removed + 目标目录 Added（各自的组）。
    drop(browser.apply_entry_changes(
        &source_dir,
        &[ResolvedEntryChange::Removed {
            path: source.clone(),
        }],
    ));
    drop(browser.apply_entry_changes(&target_dir, &[added(target.clone())]));

    let source_entries = &browser.expanded_directories[&source_dir].entries;
    let target_entries = &browser.expanded_directories[&target_dir].entries;
    assert!(source_entries.iter().all(|entry| entry.path != source));
    assert!(target_entries.iter().any(|entry| entry.path == target));
    // 目标条目不得混入源目录列表（父目录错误的幽灵条目）。
    assert!(source_entries
        .iter()
        .all(|entry| entry.path.starts_with(&source_dir)));
    assert!(browser.transfers_are_reflected_in_visible_directories(&[(source, target)]));
}

fn expanded_loaded(discovery: DirectoryDiscovery) -> crate::model::ExpandedDirectory {
    crate::model::ExpandedDirectory {
        entries: crate::model::display_entries_in_discovery_order(&discovery),
        directory_discovery: Some(discovery),
        status: crate::model::ExpandedDirectoryStatus::Loaded,
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

#[tokio::test]
async fn hidden_additions_are_ignored_when_hidden_files_are_excluded() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), b"a").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = workspace.path().to_path_buf();
    browser.sync_active_tab_state();
    browser.directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.directory_discovery.as_ref().unwrap(),
    )));

    std::fs::write(workspace.path().join(".hidden"), b"h").unwrap();
    let change = added(workspace.path().join(".hidden"));
    drop(browser.apply_entry_changes(workspace.path(), &[change]));

    assert_eq!(entry_paths(&browser), vec!["a.txt"]);
}

#[test]
fn rescan_required_or_missing_targets_fall_back_to_full_reload() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/tmp/somewhere");
    browser.sync_active_tab_state();

    // RescanRequired 必须返回 None 以触发全量重扫兜底。
    let current_dir = browser.current_dir.clone();
    assert!(browser
        .apply_entry_changes(&current_dir, &[ResolvedEntryChange::RescanRequired])
        .is_none());
    // 目录既非当前目录也非展开目录：无应用目标，返回 None。
    assert!(browser
        .apply_entry_changes(
            Path::new("/tmp/unrelated"),
            &[ResolvedEntryChange::Removed {
                path: PathBuf::from("/tmp/unrelated/x"),
            }],
        )
        .is_none());
}

#[tokio::test]
async fn transfer_reconciliation_requires_source_gone_and_target_visible() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("kept.txt"), b"k").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = workspace.path().to_path_buf();
    browser.sync_active_tab_state();
    browser.directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.directory_discovery.as_ref().unwrap(),
    )));

    let moved_out = workspace.path().join("kept.txt");
    let moved_in = workspace.path().join("fresh.txt");
    // 源还在、目标不在：对账失败。
    assert!(!browser
        .transfers_are_reflected_in_visible_directories(&[(moved_out.clone(), moved_in.clone())]));

    std::fs::write(&moved_in, b"new").unwrap();
    drop(browser.apply_entry_changes(
        workspace.path(),
        &[
            ResolvedEntryChange::Removed {
                path: moved_out.clone(),
            },
            added(moved_in.clone()),
        ],
    ));
    // 源消失、目标可见：对账通过，无需全量重扫。
    assert!(browser.transfers_are_reflected_in_visible_directories(&[(moved_out, moved_in)]));
}
