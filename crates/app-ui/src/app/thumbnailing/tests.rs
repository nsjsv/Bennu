use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use desktop_linux::{NetworkConnection, NetworkConnectionId, NetworkMountState, NetworkProtocol};
use file_core::{
    DirectoryEntry, DirectoryMetadataAvailability, EntryMetadata, FileKind,
    TransferConflictItem, TransferConflictMetadata,
};

use super::*;
use crate::config::ui_thread_startup_config;
use crate::model::{
    BrowserPaneLayout, BrowserTab, ExpandedDirectory, ExpandedDirectoryStatus,
    IconGridExpansionAnchor, IconGridExpansionContext, IconGridExpansionSessionId,
    IconGridExpansionState, IconGridViewport, SplitAxis, TransferConflictMode,
    TransferConflictState,
};
use crate::network_connections::NetworkConnectionState;
use crate::operation_queue::QueuedTransfer;

#[test]
fn missing_viewport_schedules_list_thumbnails_for_full_window_height() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.main_window_height = 2000.0;
    browser.view_mode = BrowserViewMode::List;
    let initial_rows = crate::list_view::list_initial_rows(
        browser.main_window_height,
        browser.user_config.list_view_density,
    );
    // 回归：初始预热行数由真实窗口高度推导（2000px 下远超旧固定 25 行），
    // 未产生滚动事件的折叠线以下条目也必须拿到缩略图调度。
    assert!(initial_rows > 25);
    browser.entries = (0..120)
        .map(|index| image_entry(&format!("/workspace/{index}.png")))
        .collect::<Vec<_>>()
        .into();

    browser.schedule_visible_list_thumbnail_range_for_pane(BrowserPaneId::PRIMARY, None);

    let scheduled = drain_scheduled_requests(&mut browser);
    assert_eq!(scheduled.len(), initial_rows.min(120));
}

#[test]
fn missing_viewport_schedules_column_thumbnails_for_full_window_height() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.main_window_height = 2000.0;
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = (0..120)
        .map(|index| image_entry(&format!("/workspace/{index}.png")))
        .collect::<Vec<_>>()
        .into();

    let requests = browser.thumbnail_requests_for_pane_directory_range(
        BrowserPaneId::PRIMARY,
        Path::new("/workspace"),
    );

    let initial_rows = crate::three_column_view::column_initial_rows(
        browser.main_window_height,
        browser.user_config.columns_view_density,
    );
    assert!(initial_rows > 25);
    assert_eq!(requests.len(), initial_rows.min(120));
}

fn drain_scheduled_requests(browser: &mut FileBrowser) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    loop {
        let batch = browser.thumbnail_cache.take_next_batch();
        if batch.is_empty() {
            break;
        }
        for work in batch {
            sources.push(work.request.source.clone());
            browser.thumbnail_cache.finish(&work.request.key());
        }
    }
    sources
}

#[test]
fn measured_viewport_schedules_list_rows_from_shared_geometry() {
    let entries = (0..120)
        .map(|index| image_entry(&format!("/workspace/{index}.png")))
        .collect::<Vec<_>>();
    let range = crate::visible_entries::list_entry_range_for_viewport(
        &entries,
        &HashMap::new(),
        crate::list_view::LIST_ROW_HEIGHT,
        crate::list_view::LIST_HEADER_HEIGHT,
        crate::list_view::LIST_HEADER_HEIGHT + crate::list_view::LIST_ROW_HEIGHT * 40.0,
        crate::list_view::LIST_ROW_HEIGHT * 3.0,
        OVERSCAN_ROWS,
    );

    assert_eq!((range.start, range.end), (28, 55));
}

#[test]
fn column_thumbnail_range_uses_shared_column_geometry() {
    let geometry = crate::three_column_view::ColumnGeometry::for_level(
        crate::config::ViewDensityLevel::DEFAULT,
    );
    let range = crate::three_column_view::column_virtual_range_for_viewport(
        120,
        geometry.entries_top_padding + geometry.entry_scroll_height * 40.0,
        geometry.entry_scroll_height * 3.0,
        OVERSCAN_ROWS,
        geometry,
    );

    assert_eq!((range.start, range.end), (28, 55));
}

#[test]
fn inactive_pane_thumbnail_request_matches_current_entry() {
    let (browser, _, _, image_entry) = browser_with_inactive_image_pane();
    let request = request_for_entry(&image_entry, LIST_THUMBNAIL_EDGE).expect("image request");

    assert!(browser.is_current_thumbnail_request(&request));
}

#[test]
fn icons_pane_rejects_thumbnail_results_from_hidden_persistent_expansions() {
    let (mut browser, _, _, image_entry) = browser_with_inactive_image_pane();
    let request = request_for_entry(&image_entry, LIST_THUMBNAIL_EDGE).expect("image request");
    let pane = browser.panes.last_mut().unwrap();
    pane.view_mode = BrowserViewMode::Icons;
    pane.entries = Vec::new().into();
    pane.expanded_directories.insert(
        PathBuf::from("/inactive/hidden"),
        ExpandedDirectory {
            entries: vec![image_entry],
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
        },
    );

    assert!(!browser.is_current_thumbnail_request(&request));
}

#[test]
fn inactive_pane_thumbnail_range_uses_its_own_viewport() {
    let (browser, inactive_id, inactive_dir, image_entry) = browser_with_inactive_image_pane();

    let requests =
        browser.thumbnail_requests_for_pane_directory_range(inactive_id, inactive_dir.as_path());

    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].source, image_entry.path);
}

#[test]
fn inactive_pane_thumbnail_range_schedules_svg_request() {
    let (browser, inactive_id, inactive_dir, image_entry) =
        browser_with_inactive_pane_image("/inactive/vector.svg");

    let requests =
        browser.thumbnail_requests_for_pane_directory_range(inactive_id, inactive_dir.as_path());

    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].source, image_entry.path);
}

#[test]
fn list_scrolled_schedules_visible_list_thumbnail_requests() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![
        image_entry("/workspace/photo-0.png"),
        image_entry("/workspace/vector.svg"),
        image_entry("/workspace/photo-2.png"),
    ]
    .into();

    browser.schedule_visible_list_thumbnail_range_for_pane(
        BrowserPaneId::PRIMARY,
        Some(ColumnViewport {
            offset_y: 0.0,
            height: crate::list_view::LIST_ROW_HEIGHT * 3.0,
        }),
    );

    let batch = browser
        .thumbnail_cache
        .take_next_batch()
        .into_iter()
        .collect::<Vec<_>>();
    let queued_sources = batch
        .iter()
        .map(|work| work.request.source.clone())
        .collect::<HashSet<_>>();

    assert!(queued_sources.contains(&PathBuf::from("/workspace/photo-0.png")));
    assert!(queued_sources.contains(&PathBuf::from("/workspace/vector.svg")));
    assert!(batch
        .iter()
        .all(|work| work.load_policy == ThumbnailLoadPolicy::LoadOrGenerate));
}

#[test]
fn icon_grid_schedules_only_shared_visible_range_at_dynamic_edge() {
    let mut config = crate::config::default_user_config();
    config.set_icons_view_density(crate::config::ViewDensityLevel::from_icon_grid_size(112));
    let (mut browser, _) = FileBrowser::new(config);
    browser.current_dir = PathBuf::from("/workspace");
    browser.view_mode = BrowserViewMode::Icons;
    browser.main_window_width = 500.0;
    browser.sidebar_width = 0.0;
    browser.entries = (0..100)
        .map(|index| image_entry(&format!("/workspace/{index}.png")))
        .collect::<Vec<_>>()
        .into();
    browser.expanded_directories.insert(
        PathBuf::from("/workspace/subdir"),
        ExpandedDirectory {
            entries: vec![image_entry("/workspace/subdir/hidden.png")],
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
        },
    );
    let viewport = IconGridViewport {
        offset_y: crate::icon_grid_geometry::ICON_GRID_CONTENT_PADDING
            + crate::icon_grid_geometry::row_height(112) * 10.0,
        width: 500.0,
        height: crate::icon_grid_geometry::row_height(112) * 2.0,
    };
    browser.icon_grid_viewports.insert(
        BrowserPaneId::PRIMARY,
        PaneIconGridViewport {
            directory: browser.current_dir.clone(),
            viewport,
        },
    );
    let visible = crate::icon_grid_geometry::visible_entry_range(viewport, 100, 800.0, 112);

    browser.schedule_visible_icon_grid_thumbnails_for_pane(BrowserPaneId::PRIMARY);

    let mut queued = Vec::new();
    loop {
        let batch = browser.thumbnail_cache.take_next_batch();
        if batch.is_empty() {
            break;
        }
        for work in batch {
            browser.thumbnail_cache.finish(&work.key());
            queued.push(work);
        }
    }
    let queued_sources = queued
        .iter()
        .map(|work| work.request.source.clone())
        .collect::<HashSet<_>>();

    assert_eq!(queued.len(), visible.end_entry - visible.start_entry);
    assert!(queued.iter().all(|work| work.request.max_edge == 224));
    assert!(queued.iter().all(|work| {
        let file_name = work
            .request
            .source
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<usize>().ok());
        file_name.is_some_and(|index| (visible.start_entry..visible.end_entry).contains(&index))
    }));
    assert!(!queued_sources.contains(&PathBuf::from("/workspace/subdir/hidden.png")));
}

#[test]
fn icon_grid_schedules_visible_expansion_thumbnail_requests() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let root = PathBuf::from("/workspace/root");
    let main_image = image_entry("/workspace/main.png");
    let child_image = image_entry("/workspace/root/child.png");
    browser.current_dir = PathBuf::from("/workspace");
    browser.view_mode = BrowserViewMode::Icons;
    browser.entries = vec![directory_entry(root.clone()), main_image.clone()].into();
    browser.icon_grid_expansion = Some(IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: browser.current_dir.clone(),
            session_id: IconGridExpansionSessionId::new(1),
        },
        IconGridExpansionAnchor {
            parent_directory: browser.current_dir.clone(),
            path: root.clone(),
            index: 0,
        },
        ExpandedDirectory {
            entries: vec![child_image.clone()],
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 1,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    ));
    browser.icon_grid_viewports.insert(
        BrowserPaneId::PRIMARY,
        PaneIconGridViewport {
            directory: browser.current_dir.clone(),
            viewport: IconGridViewport {
                offset_y: 0.0,
                width: 500.0,
                height: 800.0,
            },
        },
    );

    browser.schedule_visible_icon_grid_thumbnails_for_pane(BrowserPaneId::PRIMARY);

    let queued_sources = browser
        .thumbnail_cache
        .take_next_batch()
        .into_iter()
        .map(|work| work.request.source)
        .collect::<HashSet<_>>();
    assert!(queued_sources.contains(&main_image.path));
    assert!(queued_sources.contains(&child_image.path));
    let child_request = request_for_entry(
        &child_image,
        crate::icon_grid_geometry::thumbnail_edge(browser.user_config.icon_grid_size),
    )
    .unwrap();
    assert!(browser.is_current_thumbnail_request(&child_request));
}

#[test]
fn network_list_thumbnails_default_to_cache_only_policy() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![image_entry("/mnt/nas/photo.png")].into();
    mount_network_root(&mut browser, "/mnt/nas");

    browser.schedule_visible_list_thumbnail_range_for_pane(
        BrowserPaneId::PRIMARY,
        Some(ColumnViewport {
            offset_y: 0.0,
            height: crate::list_view::LIST_ROW_HEIGHT,
        }),
    );

    let batch = browser.thumbnail_cache.take_next_batch();

    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].load_policy, ThumbnailLoadPolicy::CacheOnly);
}

#[test]
fn enabled_network_list_thumbnails_use_generate_policy() {
    let mut config = crate::config::default_user_config();
    config.network_list_thumbnail_downloads_enabled = true;
    let (mut browser, _) = FileBrowser::new(config);
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![image_entry("/mnt/nas/photo.png")].into();
    mount_network_root(&mut browser, "/mnt/nas");

    browser.schedule_visible_list_thumbnail_range_for_pane(
        BrowserPaneId::PRIMARY,
        Some(ColumnViewport {
            offset_y: 0.0,
            height: crate::list_view::LIST_ROW_HEIGHT,
        }),
    );

    let batch = browser.thumbnail_cache.take_next_batch();

    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].load_policy, ThumbnailLoadPolicy::LoadOrGenerate);
}

#[test]
fn cache_only_thumbnail_miss_does_not_block_later_generation() {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    let image_entry = image_entry("/mnt/nas/photo.png");
    let request = request_for_entry(&image_entry, LIST_THUMBNAIL_EDGE).expect("request");
    browser.thumbnail_cache.enqueue_cached_request(
        request.clone(),
        ThumbnailPurpose::List,
        ThumbnailPriority::Visible,
    );
    let work = browser
        .thumbnail_cache
        .take_next_batch()
        .pop()
        .expect("cache-only work");

    drop(browser.accept_thumbnail_batch(vec![ThumbnailLoadOutcome {
        work,
        result: ThumbnailLoadResult::CacheMiss,
    }]));
    browser.thumbnail_cache.enqueue_request(
        request,
        ThumbnailPurpose::List,
        ThumbnailPriority::Visible,
    );

    assert_eq!(browser.thumbnail_cache.take_next_batch().len(), 1);
}

#[test]
fn transfer_conflict_thumbnail_request_matches_current_conflict() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    let source = PathBuf::from("/incoming/photo.png");
    let target = PathBuf::from("/existing/photo.png");
    let source_metadata = transfer_conflict_metadata(11);
    let target_metadata = transfer_conflict_metadata(13);
    let source_request = request_for_transfer_conflict_path(
        &source,
        &source_metadata,
        TRANSFER_CONFLICT_THUMBNAIL_EDGE,
    )
    .expect("source request");
    let target_request = request_for_transfer_conflict_path(
        &target,
        &target_metadata,
        TRANSFER_CONFLICT_THUMBNAIL_EDGE,
    )
    .expect("target request");
    browser.transfer_conflict = Some(TransferConflictState {
        mode: TransferConflictMode::Copy,
        transfers: vec![QueuedTransfer::new(source.clone(), target.clone())],
        conflicts: vec![TransferConflictItem {
            source,
            target,
            source_metadata,
            target_metadata,
        }],
        current_index: 0,
        apply_to_all: false,
    });

    assert!(browser.is_current_thumbnail_request(&source_request));
    assert!(browser.is_current_thumbnail_request(&target_request));
}

#[test]
fn transfer_conflict_thumbnail_requests_are_queued() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    let source = PathBuf::from("/incoming/photo.png");
    let target = PathBuf::from("/existing/photo.png");
    browser.transfer_conflict = Some(TransferConflictState {
        mode: TransferConflictMode::Copy,
        transfers: vec![QueuedTransfer::new(source.clone(), target.clone())],
        conflicts: vec![TransferConflictItem {
            source: source.clone(),
            target: target.clone(),
            source_metadata: transfer_conflict_metadata(11),
            target_metadata: transfer_conflict_metadata(13),
        }],
        current_index: 0,
        apply_to_all: false,
    });

    browser.enqueue_current_transfer_conflict_thumbnail_requests();
    let batch = browser.thumbnail_cache.take_next_batch();
    let queued_sources = batch
        .iter()
        .map(|work| work.request.source.clone())
        .collect::<HashSet<_>>();

    assert_eq!(batch.len(), 2);
    assert!(queued_sources.contains(&source));
    assert!(queued_sources.contains(&target));
    assert!(batch
        .iter()
        .all(|work| work.purpose == ThumbnailPurpose::TransferConflict));
}

#[test]
fn drifted_thumbnail_result_requeues_with_current_entry_metadata() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    // 调度时条目元数据尚未落地（Pending：len=0），入队请求带着「空钥匙」。
    browser.entries = vec![pending_image_entry("/workspace/photo.png")].into();
    let pending_entry = browser.entries.first().unwrap().clone();
    let request =
        request_for_entry(&pending_entry, LIST_THUMBNAIL_EDGE).expect("pending request");
    let scope =
        thumbnail_scope_for_pane_directory(BrowserPaneId::PRIMARY, Path::new("/workspace"));
    browser.thumbnail_cache.enqueue_request_for_scope(
        request,
        ThumbnailPurpose::List,
        ThumbnailPriority::Visible,
        scope.clone(),
    );
    let work = browser
        .thumbnail_cache
        .take_next_batch()
        .pop()
        .expect("inflight work");
    let stale_key = work.key();

    // 加载期间元数据落地：条目被重建为带真实 len 的版本，key 随之漂移。
    browser.entries = vec![image_entry("/workspace/photo.png")].into();

    browser.requeue_drifted_thumbnail_request(&work);

    let requeued = browser.thumbnail_cache.take_next_batch();
    assert_eq!(requeued.len(), 1);
    assert_eq!(requeued[0].request.source, PathBuf::from("/workspace/photo.png"));
    assert_eq!(requeued[0].request.metadata.len, 10);
    assert_ne!(requeued[0].key(), stale_key);
    assert_eq!(requeued[0].purpose, work.purpose);
    assert_eq!(requeued[0].priority, work.priority);
    assert_eq!(requeued[0].scope, Some(scope));
}

#[test]
fn thumbnail_result_for_removed_entry_is_not_requeued() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry("/workspace/photo.png")].into();
    let request = request_for_entry(browser.entries.first().unwrap(), LIST_THUMBNAIL_EDGE)
        .expect("request");
    browser.thumbnail_cache.enqueue_request(
        request,
        ThumbnailPurpose::List,
        ThumbnailPriority::Visible,
    );
    let work = browser
        .thumbnail_cache
        .take_next_batch()
        .pop()
        .expect("inflight work");

    // 条目已离开当前目录：结果照旧丢弃，不允许重排。
    browser.entries = vec![image_entry("/elsewhere/photo.png")].into();

    browser.requeue_drifted_thumbnail_request(&work);

    assert!(browser.thumbnail_cache.take_next_batch().is_empty());
}

fn browser_with_inactive_image_pane() -> (FileBrowser, BrowserPaneId, PathBuf, DirectoryEntry) {
    browser_with_inactive_pane_image("/inactive/photo.png")
}

fn browser_with_inactive_pane_image(
    image_path: &str,
) -> (FileBrowser, BrowserPaneId, PathBuf, DirectoryEntry) {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    let inactive_id = BrowserPaneId(1);
    let inactive_dir = PathBuf::from("/inactive");
    let image_entry = image_entry(image_path);
    let tab = BrowserTab::directory(1, inactive_dir.clone());

    browser.panes.push(BrowserPane {
        id: inactive_id,
        current_dir: inactive_dir.clone(),
        is_trash_view: false,
        entries: vec![image_entry.clone()].into(),
        directory_discovery: None,
        directory_loading_placeholder: None,
        trash_entries: Vec::new(),
        selected: None,
        selected_paths: HashSet::new(),
        selection_anchor: None,
        deepest_open_column_directory: None,
        expanded_directories: HashMap::new(),
        view_mode: crate::model::BrowserViewMode::Columns,
        column_browser_viewport: Default::default(),
        column_viewports: HashMap::from([(
            inactive_dir.clone(),
            ColumnViewport {
                offset_y: 0.0,
                height: crate::list_view::LIST_ROW_HEIGHT,
            },
        )]),
        tabs: vec![tab.clone()],
        active_tab_id: tab.id,
        directory_load_generation: 0,
        directory_load_cancel: None,
        back_stack: Vec::new(),
        forward_stack: Vec::new(),
        directory_collection_phase: crate::model::DirectoryCollectionPhase::Ready,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    });
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: inactive_id,
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };

    (browser, inactive_id, inactive_dir, image_entry)
}

fn directory_entry(path: PathBuf) -> DirectoryEntry {
    DirectoryEntry::new(
        path,
        FileKind::Directory,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn image_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata {
            len: 10,
            modified: None,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}

fn pending_image_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata {
            filesystem_availability: DirectoryMetadataAvailability::Pending,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}

fn transfer_conflict_metadata(len: u64) -> TransferConflictMetadata {
    TransferConflictMetadata {
        is_directory: false,
        len,
        modified: None,
    }
}

fn mount_network_root(browser: &mut FileBrowser, mount_path: &str) {
    let id = NetworkConnectionId::new("nas");
    let connection = NetworkConnection::new(
        id.clone(),
        "NAS",
        NetworkProtocol::Smb,
        "smb://server/share",
    )
    .expect("network connection");
    browser.network_connections = NetworkConnectionState::from_connections(vec![connection]);
    browser.network_connections.accept_loaded(vec![(
        id,
        NetworkMountState::Mounted(PathBuf::from(mount_path)),
    )]);
}
