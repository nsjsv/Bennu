use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    discover_directory_with_progress, DirectoryDiscovery, DiscoveredDirectoryEntry,
    ResolvedEntryChange, ScanOptions,
};
use tokio_util::sync::CancellationToken;

use super::super::FileBrowser;
use crate::config;
use crate::model::{BrowserPaneId, BrowserPaneLayout, BrowserViewMode, SplitAxis};
use crate::operation_history::CompletedTransfer;

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
    assert!(!browser.selected_paths.contains(&removed_path));
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

// ---- 跨面板（分栏）场景：增量应用与对账的作用域是「所有可见面板」----
//
// 这些用例断言的是被修复的不变量本身：任何一个可见面板显示的目录都必须
// 能命中增量应用目标、都必须参与对账。它们不是只覆盖某条触发路径，因此
// 移动完成入口（accept_file_operation_moves_renamed）与 watcher 入口
// （直接调 apply_entry_changes）分别有用例证明走的是同一共享不变量。

/// 分栏浏览器：活动面板（PRIMARY，由顶层镜像代表）浏览 active_dir，
/// 非活动面板（BrowserPaneId(1)，状态独立持有）浏览 inactive_dir。
/// 两者的 discovery 与显示列表都来自真实目录扫描，保证增量应用路径与
/// 生产一致：应用目标枚举、discovery 重建、tab 回写全部走真实数据。
async fn split_browser_with_panes(active_dir: &Path, inactive_dir: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = active_dir.to_path_buf();
    browser.sync_active_tab_state();
    browser.directory_discovery = Some(discovery_for(active_dir).await);
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.directory_discovery.as_ref().unwrap(),
    )));

    // 非活动面板由镜像快照改造而来：覆盖目录与扫描数据后，它与活动镜像
    // 互不共享可变状态 —— 这正是生产中 split 之后两个面板的关系。
    let mut inactive = browser.capture_active_pane_snapshot();
    inactive.id = BrowserPaneId(1);
    inactive.current_dir = inactive_dir.to_path_buf();
    inactive.directory_discovery = Some(discovery_for(inactive_dir).await);
    inactive.entries = crate::model::display_entries_in_discovery_order(
        inactive.directory_discovery.as_ref().unwrap(),
    )
    .into();
    inactive.selected = None;
    inactive.selected_paths = HashSet::new();
    inactive.selection_anchor = None;
    inactive.sync_active_tab_state();
    browser.panes.push(inactive);
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: BrowserPaneId(1),
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };
    browser
}

fn pane_entry_names(browser: &FileBrowser, pane_id: BrowserPaneId) -> Vec<String> {
    browser
        .pane_by_id(pane_id)
        .expect("inactive pane must exist")
        .entries
        .iter()
        .map(|entry| entry.name().to_string_lossy().into_owned())
        .collect()
}

/// 跨面板移动、活动面板 = 源侧：移动完成入口必须同时更新两侧。
/// 修复前增量只作用于活动镜像，目标面板（非活动）的列表永久陈旧 ——
/// 正是用户报告的 bug 方向之一。
#[tokio::test]
async fn cross_pane_move_updates_active_source_and_inactive_target() {
    let workspace = tempfile::tempdir().unwrap();
    let source_dir = workspace.path().join("src");
    let target_dir = workspace.path().join("dst");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(source_dir.join("moved.txt"), b"x").unwrap();
    let mut browser = split_browser_with_panes(&source_dir, &target_dir).await;

    let source = source_dir.join("moved.txt");
    let target = target_dir.join("moved.txt");
    std::fs::rename(&source, &target).unwrap();

    drop(browser.accept_file_operation_moves_renamed(
        1,
        vec![CompletedTransfer {
            source: source.clone(),
            target: target.clone(),
        }],
    ));

    // 源面板（活动镜像）移除源条目：这是修复前唯一生效的一侧。
    assert_eq!(entry_paths(&browser), Vec::<String>::new());
    // 目标面板（非活动）插入目标条目：不变量 —— 增量必须命中所有可见面板。
    assert_eq!(
        pane_entry_names(&browser, BrowserPaneId(1)),
        vec!["moved.txt"]
    );
    // 双侧都已反映，对账通过；任一侧对不上都会重复触发全量兜底。
    assert!(browser.transfers_are_reflected_in_visible_directories(&[(source, target)]));
}

/// 跨面板移动、活动面板 = 目标侧：修复前 find_discovered_entry 只查活动
/// 镜像，找不到非活动面板里的源条目导致整批移动被跳过，两个面板一起
/// 陈旧。修复后不仅双向视图更新，源面板的选中集合也必须随源条目移除
/// 一并清理，且 tab 快照同步回写（切走再切回不回到陈旧列表）。
#[tokio::test]
async fn cross_pane_move_updates_inactive_source_and_active_target() {
    let workspace = tempfile::tempdir().unwrap();
    let source_dir = workspace.path().join("src");
    let target_dir = workspace.path().join("dst");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(source_dir.join("moved.txt"), b"x").unwrap();
    let mut browser = split_browser_with_panes(&target_dir, &source_dir).await;

    let source = source_dir.join("moved.txt");
    let target = target_dir.join("moved.txt");
    // 源面板（非活动）预先选中被移走的条目：移动后选中必须一并清理。
    {
        let pane = browser.pane_by_id_mut(BrowserPaneId(1)).unwrap();
        pane.selected = Some(source.clone());
        pane.selected_paths.insert(source.clone());
        pane.selection_anchor = Some(source.clone());
    }

    std::fs::rename(&source, &target).unwrap();
    drop(browser.accept_file_operation_moves_renamed(
        1,
        vec![CompletedTransfer {
            source: source.clone(),
            target: target.clone(),
        }],
    ));

    // 目标面板（活动镜像）插入目标条目。
    assert_eq!(entry_paths(&browser), vec!["moved.txt"]);
    // 源面板（非活动）移除源条目，选中集合/锚点同步清理 —— 与活动镜像
    // 的增量应用语义保持一致，不允许面板之间出现两套选中清理规则。
    let pane = browser.pane_by_id(BrowserPaneId(1)).unwrap();
    assert!(pane.entries.iter().all(|entry| entry.path != source));
    assert_eq!(pane.selected, None);
    assert!(pane.selected_paths.is_empty());
    assert_eq!(pane.selection_anchor, None);
    // pane.sync_active_tab_state() 必须把新列表回写活动标签页。
    let tab = pane
        .tabs
        .iter()
        .find(|tab| tab.id == pane.active_tab_id)
        .expect("active tab must exist");
    assert!(tab.entries.iter().all(|entry| entry.path != source));
}

/// 对账不变量：源父目录仅被非活动面板显示且仍含源条目时必须判「未反映」。
/// 修复前 displayed_entries_for 只认活动镜像，「无从核对」被误判为通过，
/// 全量兜底永不触发 —— 这正是 bug 中两面板永久陈旧的另一半原因。
#[tokio::test]
async fn reconciliation_fails_when_source_parent_only_visible_in_inactive_pane() {
    let workspace = tempfile::tempdir().unwrap();
    let source_dir = workspace.path().join("src");
    let target_dir = workspace.path().join("dst");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(source_dir.join("moved.txt"), b"x").unwrap();
    // 活动面板浏览无关目录：源父目录只出现在非活动面板里，目标父目录
    // 不被任何面板显示（该侧无从核对视为通过）。
    let unrelated_dir = workspace.path().join("other");
    std::fs::create_dir_all(&unrelated_dir).unwrap();
    let mut browser = split_browser_with_panes(&unrelated_dir, &source_dir).await;

    let source = source_dir.join("moved.txt");
    let target = target_dir.join("moved.txt");

    // 源条目仍在非活动面板列表里：对账必须失败，让调用方走全量兜底。
    assert!(!browser
        .transfers_are_reflected_in_visible_directories(&[(source.clone(), target.clone())]));

    // 正向对照：增量真正应用后（源面板移除 + 目标插入）对账恢复通过。
    std::fs::rename(&source, &target).unwrap();
    drop(browser.apply_entry_changes(
        &source_dir,
        &[ResolvedEntryChange::Removed {
            path: source.clone(),
        }],
    ));
    drop(browser.apply_entry_changes(&target_dir, &[added(target.clone())]));
    assert!(browser.transfers_are_reflected_in_visible_directories(&[(source, target)]));
}

/// 同一目录同时显示在两个面板（两栏打开同一目录很常见）：变更必须应用
/// 到每一个显示目标。漏掉任何一个目标都会让该面板停留陈旧列表，这断言
/// 的是「同一目录可命中多个应用目标，全部应用」的目标枚举语义。
#[tokio::test]
async fn changes_reach_every_pane_displaying_the_same_directory() {
    let workspace = tempfile::tempdir().unwrap();
    let shared_dir = workspace.path().join("shared");
    std::fs::create_dir_all(&shared_dir).unwrap();
    std::fs::write(shared_dir.join("a.txt"), b"a").unwrap();
    let mut browser = split_browser_with_panes(&shared_dir, &shared_dir).await;

    std::fs::write(shared_dir.join("fresh.txt"), b"f").unwrap();
    // 至少一个目标应用成功就必须返回 Some，调用方才不会重复触发全量兜底。
    let task = browser.apply_entry_changes(&shared_dir, &[added(shared_dir.join("fresh.txt"))]);
    assert!(task.is_some());

    assert!(entry_paths(&browser).contains(&"fresh.txt".to_string()));
    assert!(pane_entry_names(&browser, BrowserPaneId(1)).contains(&"fresh.txt".to_string()));
}

/// watcher 外部变更与移动完成共用 apply_entry_changes 单一入口：事件命中
/// 非活动面板显示的目录时，该面板必须增量更新并返回 Some。返回 None 会
/// 让调用方误触发全量重扫兜底，等于该入口仍在走「活动面板作用域」。
#[tokio::test]
async fn watcher_event_updates_directory_displayed_only_by_inactive_pane() {
    let workspace = tempfile::tempdir().unwrap();
    let active_dir = workspace.path().join("active");
    let inactive_dir = workspace.path().join("inactive");
    std::fs::create_dir_all(&active_dir).unwrap();
    std::fs::create_dir_all(&inactive_dir).unwrap();
    let mut browser = split_browser_with_panes(&active_dir, &inactive_dir).await;

    std::fs::write(inactive_dir.join("external.txt"), b"e").unwrap();
    let task =
        browser.apply_entry_changes(&inactive_dir, &[added(inactive_dir.join("external.txt"))]);

    // Some = 走增量路径；活动面板不受无关目录事件影响。
    assert!(task.is_some());
    assert!(entry_paths(&browser).is_empty());
    assert_eq!(
        pane_entry_names(&browser, BrowserPaneId(1)),
        vec!["external.txt"]
    );
}
