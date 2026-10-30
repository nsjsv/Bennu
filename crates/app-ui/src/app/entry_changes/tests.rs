use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_core::{
    discover_directory_with_progress, DirectoryDiscovery, DiscoveredDirectoryEntry,
    ResolvedEntryChange, ScanOptions,
};
use tokio_util::sync::CancellationToken;

use super::super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode,
    DirectoryCollectionPhase, DirectoryExpansionLoadContext, DirectoryLoadRequest,
    ExpandedDirectoryStatus, PrebuiltDirectoryDiscovery, SplitAxis,
};
use crate::operation_history::CompletedTransfer;
use crate::thumbnail_cache::ColumnViewport;

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
        .tab()
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
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    browser.tab_mut().directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.tab().directory_discovery.as_ref().unwrap(),
    )));

    std::fs::write(workspace.path().join("b.txt"), b"b").unwrap();
    let change = added(workspace.path().join("b.txt"));
    drop(browser.apply_entry_changes(workspace.path(), &[change]));

    assert_eq!(entry_paths(&browser), vec!["a.txt", "b.txt", "c.txt"]);
    // discovery 与显示列表同步：新增条目进入 discovery 且带正确的 discovery_index。
    let discovery = browser.tab().directory_discovery.as_ref().unwrap();
    assert_eq!(discovery.entries.len(), 3);
    assert!(browser
        .tab()
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
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    browser.tab_mut().view_mode = BrowserViewMode::List;
    browser.tab_mut().directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.tab().directory_discovery.as_ref().unwrap(),
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
    assert!(!browser.tab().selected_paths.contains(&removed_path));
    assert_eq!(browser.tab().selected, None);
}

#[tokio::test]
async fn same_directory_rename_moves_entry_in_place() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("old.txt"), b"x").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    browser.tab_mut().directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.tab().directory_discovery.as_ref().unwrap(),
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
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    // 多栏视图场景：两个目录都在 expanded_directories 中显示。
    browser.tab_mut().expanded_directories.insert(
        source_dir.clone(),
        expanded_loaded(discovery_for(&source_dir).await),
    );
    browser.tab_mut().expanded_directories.insert(
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

    let source_entries = &browser.tab().expanded_directories[&source_dir].entries;
    let target_entries = &browser.tab().expanded_directories[&target_dir].entries;
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
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    browser.tab_mut().directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.tab().directory_discovery.as_ref().unwrap(),
    )));

    std::fs::write(workspace.path().join(".hidden"), b"h").unwrap();
    let change = added(workspace.path().join(".hidden"));
    drop(browser.apply_entry_changes(workspace.path(), &[change]));

    assert_eq!(entry_paths(&browser), vec!["a.txt"]);
}

#[test]
fn rescan_required_or_missing_targets_fall_back_to_full_reload() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().current_dir = PathBuf::from("/tmp/somewhere");

    // RescanRequired 必须返回 None 以触发全量重扫兜底。
    let current_dir = browser.tab().current_dir.clone();
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
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    browser.tab_mut().directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.tab().directory_discovery.as_ref().unwrap(),
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

// ---- 跨面板（分栏）场景：增量应用与对账的作用域是「所有窗格的所有标签」----
//
// 这些用例断言的是不变量本身：任何一个窗格（含后台标签）显示的目录都必须
// 命中增量应用目标，可见面板都必须参与对账。它们不是只覆盖某条触发路径，
// 因此移动完成入口（accept_file_operation_moves_renamed）与 watcher 入口
// （直接调 apply_entry_changes）分别有用例证明走的是同一共享不变量。

/// 分栏浏览器：活动窗格（PRIMARY）浏览 active_dir，另一窗格
/// （BrowserPaneId(1)）浏览 inactive_dir。两者的 discovery 与显示列表都
/// 来自真实目录扫描，保证增量应用路径与生产一致。
async fn split_browser_with_panes(active_dir: &Path, inactive_dir: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().current_dir = active_dir.to_path_buf();
    browser.tab_mut().directory_discovery = Some(discovery_for(active_dir).await);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.tab().directory_discovery.as_ref().unwrap(),
    )));

    let mut inactive_tab = BrowserTab::directory(1, inactive_dir.to_path_buf());
    inactive_tab.directory_discovery = Some(discovery_for(inactive_dir).await);
    inactive_tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
    inactive_tab.entries = crate::model::display_entries_in_discovery_order(
        inactive_tab.directory_discovery.as_ref().unwrap(),
    )
    .into();
    browser
        .panes
        .push(BrowserPane::new(BrowserPaneId(1), inactive_tab));
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
        .tab()
        .entries
        .iter()
        .map(|entry| entry.name().to_string_lossy().into_owned())
        .collect()
}

/// 跨面板移动、活动面板 = 源侧：移动完成入口必须同时更新两侧，
/// 目标面板（非活动）的列表不能停留陈旧。
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

    drop(
        browser.accept_file_operation_moves_renamed(vec![CompletedTransfer {
            source: source.clone(),
            target: target.clone(),
        }]),
    );

    assert_eq!(entry_paths(&browser), Vec::<String>::new());
    // 目标面板（非活动）插入目标条目：不变量 —— 增量必须命中所有窗格。
    assert_eq!(
        pane_entry_names(&browser, BrowserPaneId(1)),
        vec!["moved.txt"]
    );
    // 双侧都已反映，对账通过；任一侧对不上都会重复触发全量兜底。
    assert!(browser.transfers_are_reflected_in_visible_directories(&[(source, target)]));
}

/// 跨面板移动、活动面板 = 目标侧：find_discovered_entry 必须能查到非活动
/// 面板里的源条目，否则整批移动被跳过、两个面板一起陈旧。源面板的选中
/// 集合也必须随源条目移除一并清理。
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
        let tab = browser.pane_by_id_mut(BrowserPaneId(1)).unwrap().tab_mut();
        tab.selected = Some(source.clone());
        tab.selected_paths.insert(source.clone());
        tab.selection_anchor = Some(source.clone());
    }

    std::fs::rename(&source, &target).unwrap();
    drop(
        browser.accept_file_operation_moves_renamed(vec![CompletedTransfer {
            source: source.clone(),
            target: target.clone(),
        }]),
    );

    assert_eq!(entry_paths(&browser), vec!["moved.txt"]);
    // 源面板（非活动）移除源条目，选中集合/锚点同步清理；活动与否不改变
    // 清理规则。
    let tab = browser.pane_by_id(BrowserPaneId(1)).unwrap().tab();
    assert!(tab.entries.iter().all(|entry| entry.path != source));
    assert_eq!(tab.selected, None);
    assert!(tab.selected_paths.is_empty());
    assert_eq!(tab.selection_anchor, None);
}

/// 对账不变量：源父目录仅被非活动面板显示且仍含源条目时必须判「未反映」，
/// 否则「无从核对」被误判为通过，全量兜底永不触发。
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

/// 后台标签（同一窗格里未激活的标签）与活动标签显示同一目录时，增量
/// 各应用一次：切回后台标签时列表已是最新，不依赖切换时的重扫。
#[tokio::test]
async fn changes_reach_background_tab_showing_the_same_directory() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), b"a").unwrap();
    let mut browser = split_browser_with_panes(workspace.path(), workspace.path()).await;
    let mut background_tab = BrowserTab::directory(9, workspace.path().to_path_buf());
    background_tab.directory_discovery = Some(discovery_for(workspace.path()).await);
    background_tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
    background_tab.entries = crate::model::display_entries_in_discovery_order(
        background_tab.directory_discovery.as_ref().unwrap(),
    )
    .into();
    browser.pane_mut().tabs.push(background_tab);

    std::fs::write(workspace.path().join("fresh.txt"), b"f").unwrap();
    let task = browser.apply_entry_changes(
        workspace.path(),
        &[added(workspace.path().join("fresh.txt"))],
    );

    assert!(task.is_some());
    for pane in &browser.panes {
        for tab in &pane.tabs {
            assert!(
                tab.entries
                    .iter()
                    .any(|entry| entry.path.ends_with("fresh.txt")),
                "pane {:?} tab {} missed the change",
                pane.id,
                tab.id
            );
        }
    }
}

/// 增量删除让内容缩短时，停在旧内容底部的视口要在同一转换里收回来，
/// 否则虚拟列表按旧偏移渲染出整屏空白。
#[tokio::test]
async fn incremental_removals_reclamp_a_viewport_left_past_the_content() {
    let workspace = tempfile::tempdir().unwrap();
    for index in 0..5 {
        std::fs::write(workspace.path().join(format!("f{index}.txt")), b"x").unwrap();
    }
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    browser.tab_mut().view_mode = BrowserViewMode::List;
    browser.tab_mut().directory_discovery = Some(discovery_for(workspace.path()).await);
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    browser.set_entries(Arc::new(crate::model::display_entries_in_discovery_order(
        browser.tab().directory_discovery.as_ref().unwrap(),
    )));
    browser.pane_mut().column_viewports.insert(
        workspace.path().to_path_buf(),
        ColumnViewport {
            offset_y: 22000.0,
            height: 800.0,
        },
    );

    let removals = (0..3)
        .map(|index| removed(workspace.path().join(format!("f{index}.txt"))))
        .collect::<Vec<_>>();
    drop(browser.apply_entry_changes(workspace.path(), &removals));

    assert_eq!(
        browser.pane().column_viewports[workspace.path()].offset_y,
        0.0
    );
}

// ---- 扫描期间到达的变化：排队，落地后按到达顺序重放 ----
//
// 扫描读到的是启动那一刻的磁盘，期间的变化若直接改旧列表，会被落地的结果
// 整体覆盖。这里断言的不变量：扫描中不碰列表、落地后补上、顺序与到达一致、
// 被新扫描取代的队列不再重放。

/// 让当前目录开始一次扫描，并取得「启动那一刻」的磁盘快照。
async fn start_current_scan(
    browser: &mut FileBrowser,
    directory: &Path,
) -> (DirectoryLoadRequest, DirectoryDiscovery) {
    browser.tab_mut().current_dir = directory.to_path_buf();
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Discovering;
    let request = browser.next_directory_load_request(directory.to_path_buf());
    (request, discovery_for(directory).await)
}

fn removed(path: PathBuf) -> ResolvedEntryChange {
    ResolvedEntryChange::Removed { path }
}

#[tokio::test]
async fn changes_during_a_scan_are_applied_after_it_lands() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), b"a").unwrap();
    std::fs::write(workspace.path().join("b.txt"), b"b").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let (request, snapshot) = start_current_scan(&mut browser, workspace.path()).await;

    std::fs::write(workspace.path().join("c.txt"), b"c").unwrap();
    std::fs::remove_file(workspace.path().join("a.txt")).unwrap();
    let added_c = added(workspace.path().join("c.txt"));
    assert!(browser
        .apply_entry_changes(workspace.path(), &[added_c])
        .is_some());
    assert!(browser
        .apply_entry_changes(workspace.path(), &[removed(workspace.path().join("a.txt"))])
        .is_some());
    // 扫描没落地之前，增量不碰列表。
    assert!(entry_paths(&browser).is_empty());

    drop(browser.accept_directory_discovery(request, PrebuiltDirectoryDiscovery::build(snapshot)));

    assert_eq!(entry_paths(&browser), vec!["b.txt", "c.txt"]);
    assert!(browser.tab().pending_changes.is_empty());
}

#[tokio::test]
async fn queued_changes_replay_in_arrival_order() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), b"a").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let (request, snapshot) = start_current_scan(&mut browser, workspace.path()).await;

    // 先出现再消失：反过来重放会留下 c.txt。
    let c = workspace.path().join("c.txt");
    std::fs::write(&c, b"c").unwrap();
    drop(browser.apply_entry_changes(workspace.path(), &[added(c.clone())]));
    std::fs::remove_file(&c).unwrap();
    drop(browser.apply_entry_changes(workspace.path(), &[removed(c)]));

    drop(browser.accept_directory_discovery(request, PrebuiltDirectoryDiscovery::build(snapshot)));

    assert_eq!(entry_paths(&browser), vec!["a.txt"]);
}

#[tokio::test]
async fn rescan_required_during_a_scan_still_falls_back_to_full_reload() {
    let workspace = tempfile::tempdir().unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let _scan = start_current_scan(&mut browser, workspace.path()).await;

    assert!(browser
        .apply_entry_changes(workspace.path(), &[ResolvedEntryChange::RescanRequired])
        .is_none());
}

#[tokio::test]
async fn a_newer_scan_discards_changes_queued_for_the_older_one() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a.txt"), b"a").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let _superseded = start_current_scan(&mut browser, workspace.path()).await;
    let c = workspace.path().join("c.txt");
    std::fs::write(&c, b"c").unwrap();
    drop(browser.apply_entry_changes(workspace.path(), &[added(c.clone())]));

    // c.txt 在第二次扫描启动前已经没了：第二次扫描读到的磁盘早于旧队列
    // 里的事件之后，重放旧事件会把已删除的文件重新变出来。
    std::fs::remove_file(&c).unwrap();
    let (request, snapshot) = start_current_scan(&mut browser, workspace.path()).await;
    drop(browser.accept_directory_discovery(request, PrebuiltDirectoryDiscovery::build(snapshot)));

    assert_eq!(entry_paths(&browser), vec!["a.txt"]);
    assert!(browser.tab().pending_changes.is_empty());
}

#[tokio::test]
async fn scan_in_a_pane_without_focus_queues_and_replays_its_own_changes() {
    let workspace = tempfile::tempdir().unwrap();
    let active_dir = workspace.path().join("active");
    let other_dir = workspace.path().join("other");
    std::fs::create_dir_all(&active_dir).unwrap();
    std::fs::create_dir_all(&other_dir).unwrap();
    std::fs::write(other_dir.join("a.txt"), b"a").unwrap();
    let mut browser = split_browser_with_panes(&active_dir, &other_dir).await;
    let other_pane = BrowserPaneId(1);
    browser
        .pane_by_id_mut(other_pane)
        .unwrap()
        .tab_mut()
        .directory_collection_phase = DirectoryCollectionPhase::Discovering;
    let (request, _cancel) = browser.begin_directory_load(other_pane, other_dir.clone());
    let snapshot = discovery_for(&other_dir).await;

    std::fs::write(other_dir.join("b.txt"), b"b").unwrap();
    drop(browser.apply_entry_changes(&other_dir, &[added(other_dir.join("b.txt"))]));
    assert_eq!(pane_entry_names(&browser, other_pane), vec!["a.txt"]);

    drop(browser.accept_directory_discovery(request, PrebuiltDirectoryDiscovery::build(snapshot)));

    assert_eq!(
        pane_entry_names(&browser, other_pane),
        vec!["a.txt", "b.txt"]
    );
}

/// 监听范围 = 所有可见窗格的目录：扫描期间不拆订阅，否则期间的变化没人收；
/// 非活动窗格的目录也要监听，回收站视图不走文件监听。
#[tokio::test]
async fn watched_directories_cover_every_visible_pane_even_while_scanning() {
    let workspace = tempfile::tempdir().unwrap();
    let active_dir = workspace.path().join("active");
    let other_dir = workspace.path().join("other");
    std::fs::create_dir_all(&active_dir).unwrap();
    std::fs::create_dir_all(&other_dir).unwrap();
    let mut browser = split_browser_with_panes(&active_dir, &other_dir).await;
    for pane in &mut browser.panes {
        pane.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Discovering;
    }

    let watched = browser.watched_directories();
    assert!(watched.contains(&active_dir));
    assert!(watched.contains(&other_dir));

    browser
        .pane_by_id_mut(BrowserPaneId(1))
        .unwrap()
        .tab_mut()
        .is_trash_view = true;
    assert!(!browser.watched_directories().contains(&other_dir));
}

#[tokio::test]
async fn changes_during_an_expanded_directory_scan_are_applied_after_it_lands() {
    let workspace = tempfile::tempdir().unwrap();
    let child = workspace.path().join("child");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("a.txt"), b"a").unwrap();
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().current_dir = workspace.path().to_path_buf();
    browser.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
    let snapshot = discovery_for(&child).await;
    let mut expanded = expanded_loaded(snapshot.clone());
    expanded.status = ExpandedDirectoryStatus::Loading;
    expanded.entries.clear();
    expanded.directory_discovery = None;
    let (request, _cancel) = FileBrowser::next_expanded_directory_load_request(
        DirectoryExpansionLoadContext::BrowserTree {
            pane_id: BrowserPaneId::PRIMARY,
        },
        child.clone(),
        &mut expanded,
    );
    browser
        .tab_mut()
        .expanded_directories
        .insert(child.clone(), expanded);

    std::fs::write(child.join("b.txt"), b"b").unwrap();
    assert!(browser
        .apply_entry_changes(&child, &[added(child.join("b.txt"))])
        .is_some());
    assert!(browser.tab().expanded_directories[&child]
        .entries
        .is_empty());

    drop(browser.accept_expanded_directory_discovery(
        request,
        Ok(PrebuiltDirectoryDiscovery::build(snapshot)),
    ));

    let names = browser.tab().expanded_directories[&child]
        .entries
        .iter()
        .map(|entry| entry.name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["a.txt", "b.txt"]);
    assert!(browser.tab().pending_changes.is_empty());
}
