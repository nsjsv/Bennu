use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use desktop_linux::{
    FileClipboardOperation, FileClipboardSelection, WaylandDndController, WaylandDndDropOrigin,
    WaylandDndDropPosition, WaylandDndFileDrop, WaylandFileDragIcon, WaylandFileDragSourceEvent,
    WaylandFileDropTargetEvent, WaylandFileDropTargetSessionId,
};
use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind};
use iced::futures::StreamExt;
use iced::{Point, Rectangle, Size, Task};
use iced_runtime::Action;

use super::super::FileBrowser;
use super::file_drop::tab_file_drop_hover_delay_elapsed;
use crate::config;
use crate::model::{
    BrowserPaneId, DirectoryFileDragTargetBounds, DirectoryLoadRequest, FileDragGestureId,
    FileDragNativeDndState, FileDragPhase, FileDragState, FileDropLayoutState, FileDropPrompt,
    FileDropSessionIdentity, Message, TabDropDestination, TabFileDropTarget,
    TabFileDropTargetBounds, TransferConflictMode,
};
use crate::operation_queue::{QueuedFileOperation, QueuedTransfer};

fn rectangle(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(width, height))
}

fn external_drop(
    target_session_id: WaylandFileDropTargetSessionId,
    paths: Vec<PathBuf>,
    position: Point,
) -> WaylandDndFileDrop {
    WaylandDndFileDrop {
        target_session_id,
        selection: FileClipboardSelection::new(FileClipboardOperation::Copy, paths),
        origin: WaylandDndDropOrigin::External,
        position: Some(WaylandDndDropPosition {
            x: position.x as f64,
            y: position.y as f64,
        }),
    }
}

fn internal_drop(
    target_session_id: WaylandFileDropTargetSessionId,
    source_session_id: desktop_linux::WaylandFileDragSessionId,
    paths: Vec<PathBuf>,
    position: Point,
) -> WaylandDndFileDrop {
    WaylandDndFileDrop {
        target_session_id,
        selection: FileClipboardSelection::new(FileClipboardOperation::Move, paths),
        origin: WaylandDndDropOrigin::Internal(source_session_id),
        position: Some(WaylandDndDropPosition {
            x: position.x as f64,
            y: position.y as f64,
        }),
    }
}

fn begin_external_session(
    browser: &mut FileBrowser,
    target_session_id: WaylandFileDropTargetSessionId,
    position: Point,
) -> crate::model::FileDropLayoutRequest {
    drop(
        browser.accept_wayland_target_event(WaylandFileDropTargetEvent::Entered {
            target_session_id,
            origin: WaylandDndDropOrigin::External,
            position: WaylandDndDropPosition {
                x: position.x as f64,
                y: position.y as f64,
            },
        }),
    );
    let session = browser.file_drop_session.as_ref().expect("drop session");
    match session.layout {
        FileDropLayoutState::Pending(request) => request,
        FileDropLayoutState::Ready { .. } => panic!("external session must measure layout"),
    }
}

fn begin_internal_session(
    browser: &mut FileBrowser,
    target_session_id: WaylandFileDropTargetSessionId,
    source_session_id: desktop_linux::WaylandFileDragSessionId,
    position: Point,
) -> crate::model::FileDropLayoutRequest {
    drop(
        browser.accept_wayland_target_event(WaylandFileDropTargetEvent::Entered {
            target_session_id,
            origin: WaylandDndDropOrigin::Internal(source_session_id),
            position: WaylandDndDropPosition {
                x: position.x as f64,
                y: position.y as f64,
            },
        }),
    );
    let session = browser.file_drop_session.as_ref().expect("drop session");
    match session.layout {
        FileDropLayoutState::Pending(request) => request,
        FileDropLayoutState::Ready { .. } => panic!("internal session must measure current layout"),
    }
}

fn drop_target_event(
    target_session_id: WaylandFileDropTargetSessionId,
    position: Point,
) -> WaylandFileDropTargetEvent {
    WaylandFileDropTargetEvent::Dropped {
        target_session_id,
        position: Some(WaylandDndDropPosition {
            x: position.x as f64,
            y: position.y as f64,
        }),
    }
}

fn directory_bounds(
    pane_id: BrowserPaneId,
    directory: impl Into<PathBuf>,
    bounds: Rectangle,
) -> crate::model::FileDragHitTestBounds {
    crate::model::FileDragHitTestBounds {
        directory_targets: vec![DirectoryFileDragTargetBounds {
            pane_id,
            directory: directory.into(),
            bounds,
        }],
        ..crate::model::FileDragHitTestBounds::default()
    }
}

fn tab_bounds(target: TabFileDropTarget, bounds: Rectangle) -> crate::model::FileDragHitTestBounds {
    crate::model::FileDragHitTestBounds {
        tabs: vec![TabFileDropTargetBounds { target, bounds }],
        ..crate::model::FileDragHitTestBounds::default()
    }
}

fn add_inactive_directory_tab(browser: &mut FileBrowser, directory: PathBuf) -> TabFileDropTarget {
    let original_tab_id = browser.active_tab_id;
    drop(browser.open_directory_in_new_tab(directory.clone()));
    let target_tab_id = browser.active_tab_id;
    drop(browser.select_tab(original_tab_id));
    TabFileDropTarget {
        pane_id: browser.active_pane_id(),
        tab_id: target_tab_id,
        destination: TabDropDestination::Directory(directory),
    }
}

fn add_inactive_trash_tab(browser: &mut FileBrowser) -> TabFileDropTarget {
    let original_tab_id = browser.active_tab_id;
    drop(browser.open_trash_in_new_tab());
    let target_tab_id = browser.active_tab_id;
    drop(browser.select_tab(original_tab_id));
    TabFileDropTarget {
        pane_id: browser.active_pane_id(),
        tab_id: target_tab_id,
        destination: TabDropDestination::Trash,
    }
}

fn test_entry(path: &Path) -> DirectoryEntry {
    DirectoryEntry::new(
        path.to_path_buf(),
        FileKind::File,
        EntryMetadata {
            len: 1,
            modified: None,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}

fn source_session_id(paths: Vec<PathBuf>) -> desktop_linux::WaylandFileDragSessionId {
    WaylandDndController::new()
        .start_file_drag(
            paths,
            WaylandFileDragIcon::new(1, 1, vec![0, 0, 0, 0]).expect("test icon"),
        )
        .expect("source session")
}

fn install_internal_source(
    browser: &mut FileBrowser,
    source_session_id: desktop_linux::WaylandFileDragSessionId,
    sources: Vec<PathBuf>,
) {
    browser.file_drag = Some(FileDragState {
        gesture_id: FileDragGestureId(1),
        source_pane_id: browser.active_pane_id(),
        source_tab_id: browser.active_tab_id,
        pressed_path: sources[0].clone(),
        sources,
        bookmark_source: None,
        stationary_action: crate::model::FileDragStationaryAction::SelectionOnly,
        phase: FileDragPhase::Dragging,
        native_dnd: FileDragNativeDndState::Started(source_session_id),
        column_directories_snapshot: Vec::new(),
        press_origin: iced::Point::ORIGIN,
        preview_entries: Vec::new(),
    });
}

async fn transfer_request(task: Task<Message>) -> (TransferConflictMode, Vec<QueuedTransfer>) {
    let mut stream = iced_runtime::task::into_stream(task).expect("transfer task");
    while let Some(action) = stream.next().await {
        if let Action::Output(Message::TransferConflictsChecked {
            mode, transfers, ..
        }) = action
        {
            return (mode, transfers);
        }
    }
    panic!("expected transfer conflict request");
}

#[test]
fn payload_before_layout_is_consumed_after_matching_snapshot() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(40.0, 40.0);
    let destination = PathBuf::from("/workspace/project");
    let source = PathBuf::from("/outside/report.txt");
    let request = begin_external_session(&mut browser, target_session_id, position);

    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    drop(browser.accept_wayland_file_drop(external_drop(
        target_session_id,
        vec![source.clone()],
        position,
    )));
    assert!(browser.file_drop_prompt.is_none());

    drop(browser.accept_drop_layout(
        request,
        directory_bounds(
            browser.active_pane_id(),
            destination.clone(),
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));

    assert!(matches!(
        &browser.file_drop_prompt,
        Some(FileDropPrompt { paste_directory, paths })
            if paste_directory == &destination && paths == &vec![source]
    ));
    assert!(browser.file_drop_session.is_none());
}

#[test]
fn layout_before_payload_freezes_target_until_payload_arrives() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(30.0, 30.0);
    let destination = PathBuf::from("/workspace/destination");
    let source = PathBuf::from("/outside/photo.png");
    let request = begin_external_session(&mut browser, target_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        directory_bounds(
            browser.active_pane_id(),
            destination.clone(),
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));

    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    assert!(browser.file_drop_prompt.is_none());
    assert!(browser
        .file_drop_session
        .as_ref()
        .is_some_and(|session| session.hovered_target.is_none()));
    drop(browser.accept_wayland_file_drop(external_drop(
        target_session_id,
        vec![source.clone()],
        Point::new(500.0, 500.0),
    )));

    assert!(matches!(
        &browser.file_drop_prompt,
        Some(FileDropPrompt { paste_directory, paths })
            if paste_directory == &destination && paths == &vec![source]
    ));
}

#[test]
fn old_measurement_payload_and_leave_do_not_clear_new_session() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let old_id = WaylandFileDropTargetSessionId::unique();
    let new_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(20.0, 20.0);
    let old_request = begin_external_session(&mut browser, old_id, position);
    let new_request = begin_external_session(&mut browser, new_id, position);

    drop(browser.accept_drop_layout(
        old_request,
        directory_bounds(
            browser.active_pane_id(),
            "/old",
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));
    drop(
        browser.accept_wayland_target_event(WaylandFileDropTargetEvent::Left {
            target_session_id: old_id,
        }),
    );
    drop(browser.accept_wayland_file_drop(external_drop(
        old_id,
        vec![PathBuf::from("/old/source")],
        position,
    )));
    drop(browser.accept_wayland_drop_failure(old_id, "late failure".to_owned()));

    let session = browser
        .file_drop_session
        .as_ref()
        .expect("new session survives");
    assert_eq!(session.identity, FileDropSessionIdentity::Wayland(new_id));
    assert!(
        matches!(session.layout, FileDropLayoutState::Pending(request) if request == new_request)
    );
}

#[test]
fn wayland_drop_on_inactive_tab_uses_tab_destination() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target = add_inactive_directory_tab(&mut browser, PathBuf::from("/workspace/tab"));
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(50.0, 20.0);
    let sources = vec![
        PathBuf::from("/outside/source.txt"),
        PathBuf::from("/outside/second.txt"),
    ];
    let request = begin_external_session(&mut browser, target_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target.clone(), rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    drop(browser.accept_wayland_file_drop(external_drop(
        target_session_id,
        sources.clone(),
        position,
    )));

    assert!(matches!(
        &browser.file_drop_prompt,
        Some(FileDropPrompt { paste_directory, paths })
            if paste_directory == &PathBuf::from("/workspace/tab") && paths == &sources
    ));
}

#[tokio::test]
async fn internal_wayland_drop_on_inactive_tab_moves_once() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let source_directory = temp_dir.path().join("source");
    let destination = temp_dir.path().join("destination");
    fs::create_dir_all(&source_directory).expect("source directory");
    fs::create_dir_all(&destination).expect("destination directory");
    let source = source_directory.join("report.txt");
    fs::write(&source, b"content").expect("source file");

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target = add_inactive_directory_tab(&mut browser, destination.clone());
    let source_session_id = source_session_id(vec![source.clone()]);
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(50.0, 20.0);
    install_internal_source(&mut browser, source_session_id, vec![source.clone()]);
    let request =
        begin_internal_session(&mut browser, target_session_id, source_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target, rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    let task = browser.accept_wayland_file_drop(internal_drop(
        target_session_id,
        source_session_id,
        vec![source.clone()],
        position,
    ));
    let (mode, transfers) = transfer_request(task).await;

    assert_eq!(mode, TransferConflictMode::Move);
    assert_eq!(
        transfers,
        vec![QueuedTransfer::new(source, destination.join("report.txt"))]
    );
    assert!(browser.file_drop_session.is_none());
}

#[test]
fn tab_hover_threshold_is_500ms_and_switch_invalidates_old_layout() {
    let started_at = Instant::now();
    assert!(!tab_file_drop_hover_delay_elapsed(
        started_at,
        started_at + Duration::from_millis(499),
    ));
    assert!(tab_file_drop_hover_delay_elapsed(
        started_at,
        started_at + Duration::from_millis(500),
    ));

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let original_tab_id = browser.active_tab_id;
    let target = add_inactive_directory_tab(&mut browser, PathBuf::from("/workspace/hover"));
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(50.0, 20.0);
    let request = begin_external_session(&mut browser, target_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target.clone(), rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    let initial_hover = browser
        .file_drop_session
        .as_ref()
        .and_then(|session| session.tab_hover.clone())
        .expect("tab hover token");
    drop(
        browser.accept_wayland_target_event(WaylandFileDropTargetEvent::Moved {
            target_session_id,
            position: WaylandDndDropPosition { x: 50.0, y: 20.0 },
        }),
    );
    assert_eq!(
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| session.tab_hover.as_ref()),
        Some(&initial_hover)
    );
    let mut hover = initial_hover;
    hover.started_at = Instant::now() - Duration::from_millis(500);
    browser
        .file_drop_session
        .as_mut()
        .expect("drop session")
        .tab_hover = Some(hover.clone());

    drop(browser.accept_tab_hover_elapsed(hover));

    assert_ne!(browser.active_tab_id, original_tab_id);
    assert_eq!(browser.active_tab_id, target.tab_id);
    let switched_request = match browser
        .file_drop_session
        .as_ref()
        .map(|session| &session.layout)
    {
        Some(FileDropLayoutState::Pending(request))
            if request.pane_id == target.pane_id && request.tab_id == target.tab_id =>
        {
            *request
        }
        state => panic!("expected switched pending layout, got {state:?}"),
    };

    drop(browser.accept_drop_layout(
        request,
        directory_bounds(
            target.pane_id,
            "/workspace/stale",
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));
    assert!(matches!(
        browser.file_drop_session.as_ref().map(|session| &session.layout),
        Some(FileDropLayoutState::Pending(pending)) if *pending == switched_request
    ));

    let nested_destination = PathBuf::from("/workspace/hover/nested");
    drop(browser.accept_drop_layout(
        switched_request,
        directory_bounds(
            target.pane_id,
            nested_destination.clone(),
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));

    let reloaded_directory = PathBuf::from("/workspace/hover");
    let directory_load_generation = browser.directory_load_generation;
    drop(browser.accept_complete_directory_fixture(
        DirectoryLoadRequest {
            pane_id: target.pane_id,
            path: reloaded_directory.clone(),
            generation: directory_load_generation,
        },
        DirectoryScan {
            path: reloaded_directory,
            entries: Vec::new(),
            skipped: Vec::new(),
        },
    ));
    let reloaded_request = match browser
        .file_drop_session
        .as_ref()
        .map(|session| &session.layout)
    {
        Some(FileDropLayoutState::Pending(request)) if *request != switched_request => *request,
        state => panic!("expected reloaded pending layout, got {state:?}"),
    };
    drop(browser.accept_drop_layout(
        switched_request,
        directory_bounds(
            target.pane_id,
            "/workspace/stale-after-reload",
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));
    assert!(matches!(
        browser.file_drop_session.as_ref().map(|session| &session.layout),
        Some(FileDropLayoutState::Pending(pending)) if *pending == reloaded_request
    ));
    drop(browser.accept_drop_layout(
        reloaded_request,
        directory_bounds(
            target.pane_id,
            nested_destination.clone(),
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));
    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    let source = PathBuf::from("/outside/after-switch.txt");
    drop(browser.accept_wayland_file_drop(external_drop(
        target_session_id,
        vec![source.clone()],
        position,
    )));
    assert!(matches!(
        &browser.file_drop_prompt,
        Some(FileDropPrompt { paste_directory, paths })
            if paste_directory == &nested_destination && paths == &vec![source]
    ));
}

#[test]
fn trash_tab_uses_dropped_paths_instead_of_live_trash_selection() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target = add_inactive_trash_tab(&mut browser);
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(50.0, 20.0);
    let dropped_path = PathBuf::from("/tmp/dropped.txt");
    browser
        .selected_paths
        .insert(PathBuf::from("/tmp/unrelated.txt"));
    let request = begin_external_session(&mut browser, target_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target, rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    drop(browser.accept_wayland_file_drop(external_drop(
        target_session_id,
        vec![dropped_path.clone()],
        position,
    )));

    assert!(matches!(
        browser.operation_queue.tasks().last().map(|task| &task.operation),
        Some(QueuedFileOperation::Trash { paths }) if paths == &vec![dropped_path]
    ));
}

#[test]
fn internal_wayland_trash_tab_uses_frozen_source_paths() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target = add_inactive_trash_tab(&mut browser);
    let source = PathBuf::from("/tmp/internal-trash.txt");
    let source_session_id = source_session_id(vec![source.clone()]);
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(50.0, 20.0);
    install_internal_source(&mut browser, source_session_id, vec![source.clone()]);
    let request =
        begin_internal_session(&mut browser, target_session_id, source_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target, rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    drop(browser.accept_wayland_file_drop(internal_drop(
        target_session_id,
        source_session_id,
        vec![source.clone()],
        position,
    )));

    assert!(matches!(
        browser.operation_queue.tasks().last().map(|task| &task.operation),
        Some(QueuedFileOperation::Trash { paths }) if paths == &vec![source]
    ));
}

#[test]
fn closing_tab_after_physical_drop_rejects_late_payload() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let original_tab_id = browser.active_tab_id;
    let target = add_inactive_directory_tab(&mut browser, PathBuf::from("/workspace/closing"));
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(50.0, 20.0);
    let request = begin_external_session(&mut browser, target_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target.clone(), rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    drop(browser.close_tab(target.tab_id));
    drop(browser.accept_wayland_file_drop(external_drop(
        target_session_id,
        vec![PathBuf::from("/outside/late.txt")],
        position,
    )));

    assert_eq!(browser.active_tab_id, original_tab_id);
    assert!(browser.file_drop_prompt.is_none());
    assert!(browser.file_drop_session.is_none());
}

#[test]
fn source_terminal_only_clears_matching_source_state() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/source.txt");
    let active_source_id = source_session_id(vec![source.clone()]);
    let stale_source_id = source_session_id(vec![PathBuf::from("/workspace/stale.txt")]);
    install_internal_source(&mut browser, active_source_id, vec![source.clone()]);
    browser.drag_selection_anchor = Some(source);

    drop(
        browser.accept_wayland_source_event(WaylandFileDragSourceEvent::Rejected {
            session_id: stale_source_id,
            details: "stale rejection".to_owned(),
        }),
    );
    assert_eq!(
        browser
            .file_drag
            .as_ref()
            .and_then(|drag| drag.native_dnd.session_id()),
        Some(active_source_id)
    );
    assert!(browser.current_error().is_none());
    assert!(browser.drag_selection_anchor.is_some());

    drop(
        browser
            .accept_wayland_source_event(WaylandFileDragSourceEvent::Cancelled(active_source_id)),
    );
    assert!(browser.file_drag.is_none());
    assert!(browser.drag_selection_anchor.is_none());
}

#[tokio::test]
async fn source_finished_before_payload_preserves_dropped_internal_dispatch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let source_directory = temp_dir.path().join("source");
    let destination = temp_dir.path().join("destination");
    fs::create_dir_all(&source_directory).expect("source directory");
    fs::create_dir_all(&destination).expect("destination directory");
    let source = source_directory.join("report.txt");
    fs::write(&source, b"content").expect("source file");
    let source_session_id = source_session_id(vec![source.clone()]);
    let target_session_id = WaylandFileDropTargetSessionId::unique();
    let position = Point::new(50.0, 50.0);
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let pane_id = browser.active_pane_id();
    install_internal_source(&mut browser, source_session_id, vec![source.clone()]);
    let request =
        begin_internal_session(&mut browser, target_session_id, source_session_id, position);
    drop(browser.accept_drop_layout(
        request,
        directory_bounds(
            pane_id,
            destination.clone(),
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));
    drop(browser.accept_wayland_target_event(drop_target_event(target_session_id, position)));
    drop(
        browser
            .accept_wayland_source_event(WaylandFileDragSourceEvent::Finished(source_session_id)),
    );
    assert!(browser.file_drag.is_none());
    assert!(browser.file_drop_session.is_some());

    let task = browser.accept_wayland_file_drop(internal_drop(
        target_session_id,
        source_session_id,
        vec![source.clone()],
        position,
    ));
    let (mode, transfers) = transfer_request(task).await;

    assert_eq!(mode, TransferConflictMode::Move);
    assert_eq!(
        transfers,
        vec![QueuedTransfer::new(source, destination.join("report.txt"))]
    );
    assert!(browser.file_drop_session.is_none());
}

#[tokio::test]
async fn iced_fallback_tab_release_uses_same_internal_dispatch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let source_directory = temp_dir.path().join("source");
    let destination = temp_dir.path().join("destination");
    fs::create_dir_all(&source_directory).expect("source directory");
    fs::create_dir_all(&destination).expect("destination directory");
    let source = source_directory.join("iced.txt");
    fs::write(&source, b"content").expect("source file");

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target = add_inactive_directory_tab(&mut browser, destination.clone());
    browser.entries = vec![test_entry(&source)].into();
    browser.selected_paths.insert(source.clone());
    browser.cursor_position = Point::new(0.0, 0.0);
    browser.start_file_drag(
        source.clone(),
        crate::model::FileDragStationaryAction::SelectionOnly,
        Vec::new(),
    );
    drop(browser.update_file_drag(Point::new(10.0, 0.0)));

    let gesture_id = browser
        .file_drag
        .as_ref()
        .expect("Iced file drag")
        .gesture_id;
    drop(browser.accept_tab_file_drop_entered(gesture_id, target.clone()));
    drop(browser.accept_tab_file_drop_released(
        FileDragGestureId(gesture_id.0.wrapping_add(1)),
        target.clone(),
    ));
    assert!(browser.file_drag.is_some());
    assert!(browser.file_drop_session.is_some());

    let task = browser.accept_tab_file_drop_released(gesture_id, target);
    let (mode, transfers) = transfer_request(task).await;

    assert_eq!(mode, TransferConflictMode::Move);
    assert_eq!(
        transfers,
        vec![QueuedTransfer::new(source, destination.join("iced.txt"))]
    );
    assert!(browser.file_drag.is_none());
    assert!(browser.file_drop_session.is_none());
}

#[test]
fn iced_fallback_trash_tab_uses_drag_snapshot_paths() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target = add_inactive_trash_tab(&mut browser);
    let source = PathBuf::from("/tmp/iced-trash.txt");
    browser.entries = vec![test_entry(&source)].into();
    browser.selected_paths.insert(source.clone());
    browser.cursor_position = Point::new(0.0, 0.0);
    browser.start_file_drag(
        source.clone(),
        crate::model::FileDragStationaryAction::SelectionOnly,
        Vec::new(),
    );
    drop(browser.update_file_drag(Point::new(10.0, 0.0)));
    let gesture_id = browser
        .file_drag
        .as_ref()
        .expect("Iced file drag")
        .gesture_id;
    drop(browser.accept_tab_file_drop_entered(gesture_id, target.clone()));
    drop(browser.accept_tab_file_drop_released(gesture_id, target));

    assert!(matches!(
        browser.operation_queue.tasks().last().map(|task| &task.operation),
        Some(QueuedFileOperation::Trash { paths }) if paths == &vec![source]
    ));
    assert!(browser.file_drag.is_none());
    assert!(browser.file_drop_session.is_none());
}
