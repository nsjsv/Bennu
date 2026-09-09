use std::path::PathBuf;
use std::time::{Duration, Instant};

use desktop_linux::{
    X11DndDropPosition, X11DndEvent, X11DndFileDrop, X11DndWindowHandle, X11FileDropTargetEvent,
    X11FileDropTargetSessionId,
};
use iced::{Point, Rectangle, Size};

use super::file_drop::tab_file_drop_hover_delay_elapsed;
use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPaneId, DirectoryFileDragTargetBounds, FileDragHitTestBounds, FileDropLayoutState,
    FileDropPrompt, FileDropSessionIdentity, TabDropDestination, TabFileDropTarget,
    TabFileDropTargetBounds,
};
use crate::operation_queue::QueuedFileOperation;

#[test]
fn x11_multi_file_directory_drop_opens_one_existing_prompt() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let clipboard_directory = PathBuf::from("/workspace/clipboard-level");
    browser.deepest_open_column_directory = Some(clipboard_directory.clone());
    let id = X11FileDropTargetSessionId::unique();
    let request = begin_session(&mut browser, id, position(1, 40, 40), 1.0, 1);
    let destination = PathBuf::from("/workspace/destination");
    drop(browser.accept_drop_layout(
        request,
        directory_bounds(
            browser.active_pane_id(),
            destination.clone(),
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));
    assert_eq!(browser.paste_target_directory(), clipboard_directory);
    drop(browser.accept_x11_target_event(
        X11FileDropTargetEvent::Dropped {
            target_session_id: id,
            position: position(1, 40, 40),
        },
        1.0,
        1,
    ));
    let paths = vec![
        PathBuf::from("/outside/first.txt"),
        PathBuf::from("/outside/second.txt"),
    ];
    drop(browser.accept_x11_file_drop(X11DndFileDrop {
        target_session_id: id,
        paths: paths.clone(),
    }));
    drop(browser.accept_x11_file_drop(X11DndFileDrop {
        target_session_id: id,
        paths: vec![PathBuf::from("/outside/late.txt")],
    }));

    assert!(matches!(
        &browser.file_drop_prompt,
        Some(FileDropPrompt { paste_directory, paths: actual })
            if paste_directory == &destination && actual == &paths
    ));
    assert_eq!(browser.paste_target_directory(), clipboard_directory);
    assert!(browser.file_drop_session.is_none());
}

#[test]
fn x11_multi_file_trash_tab_queues_one_explicit_path_batch() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let target = add_inactive_trash_tab(&mut browser);
    let id = X11FileDropTargetSessionId::unique();
    let request = begin_session(&mut browser, id, position(1, 50, 20), 1.0, 1);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target, rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    drop(browser.accept_x11_target_event(
        X11FileDropTargetEvent::Dropped {
            target_session_id: id,
            position: position(1, 50, 20),
        },
        1.0,
        1,
    ));
    let paths = vec![
        PathBuf::from("/tmp/dropped-a.txt"),
        PathBuf::from("/tmp/dropped-b.txt"),
    ];
    let tasks_before = browser.operation_queue.tasks().len();
    drop(browser.accept_x11_file_drop(X11DndFileDrop {
        target_session_id: id,
        paths: paths.clone(),
    }));
    drop(browser.accept_x11_file_drop(X11DndFileDrop {
        target_session_id: id,
        paths: paths.clone(),
    }));

    assert_eq!(browser.operation_queue.tasks().len(), tasks_before + 1);
    assert!(matches!(
        browser.operation_queue.tasks().last().map(|task| &task.operation),
        Some(QueuedFileOperation::Trash { paths: actual }) if actual == &paths
    ));
}

#[test]
fn scale_change_invalidates_old_position_layout_and_drop() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.accept_x11_dnd_handle(Ok(Some(X11DndWindowHandle::new(10, 0))), 1.0));
    let id = X11FileDropTargetSessionId::unique();
    let old_request = begin_session(&mut browser, id, position(1, 20, 20), 1.0, 1);
    drop(browser.accept_x11_scale_factor(browser.main_window, 2.0));
    let new_request = match browser
        .file_drop_session
        .as_ref()
        .map(|session| &session.layout)
    {
        Some(FileDropLayoutState::Pending(request)) if *request != old_request => *request,
        state => panic!("expected new pending layout after scale change, got {state:?}"),
    };
    drop(browser.accept_drop_layout(
        old_request,
        directory_bounds(
            browser.active_pane_id(),
            "/workspace/stale",
            rectangle(0.0, 0.0, 100.0, 100.0),
        ),
    ));
    assert!(matches!(
        browser.file_drop_session.as_ref().map(|session| &session.layout),
        Some(FileDropLayoutState::Pending(request)) if *request == new_request
    ));

    drop(browser.accept_x11_target_event(
        X11FileDropTargetEvent::Dropped {
            target_session_id: id,
            position: position(1, 20, 20),
        },
        2.0,
        2,
    ));
    assert!(browser.file_drop_session.is_none());
    assert!(browser.file_drop_prompt.is_none());
}

#[test]
fn stale_runtime_failure_does_not_replace_current_runtime() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.accept_x11_dnd_handle(Ok(Some(X11DndWindowHandle::new(10, 0))), 1.0));
    let stale_id = browser
        .x11_dnd
        .as_ref()
        .expect("first X11 runtime")
        .controller
        .id();
    drop(browser.accept_x11_dnd_handle(Ok(Some(X11DndWindowHandle::new(11, 0))), 1.0));
    let current_id = browser
        .x11_dnd
        .as_ref()
        .expect("current X11 runtime")
        .controller
        .id();

    drop(browser.accept_x11_dnd_event(
        stale_id,
        X11DndEvent::RuntimeFailed("stale failure".to_owned()),
    ));
    assert_eq!(
        browser
            .x11_dnd
            .as_ref()
            .map(|runtime| runtime.controller.id()),
        Some(current_id)
    );
    assert!(browser.current_error().is_none());
}

#[test]
fn runtime_replacement_clears_old_session_and_equal_scale_keeps_generation() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.accept_x11_dnd_handle(Ok(Some(X11DndWindowHandle::new(10, 0))), 1.0));
    let id = X11FileDropTargetSessionId::unique();
    begin_session(&mut browser, id, position(1, 20, 20), 1.0, 1);

    drop(browser.accept_x11_scale_factor(browser.main_window, 1.0));
    assert_eq!(
        browser
            .x11_dnd
            .as_ref()
            .map(|runtime| runtime.scale_generation),
        Some(1)
    );
    assert!(browser.file_drop_session.is_some());

    drop(browser.accept_x11_dnd_handle(Ok(Some(X11DndWindowHandle::new(11, 0))), 1.0));
    assert!(browser.file_drop_session.is_none());
}

#[test]
fn x11_tab_hover_uses_shared_boundary_and_continues_on_new_page() {
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
    let target = add_inactive_directory_tab(&mut browser, "/workspace/hover");
    let id = X11FileDropTargetSessionId::unique();
    let request = begin_session(&mut browser, id, position(1, 50, 20), 1.0, 1);
    drop(browser.accept_drop_layout(
        request,
        tab_bounds(target.clone(), rectangle(0.0, 0.0, 100.0, 40.0)),
    ));
    let mut hover = browser
        .file_drop_session
        .as_ref()
        .and_then(|session| session.tab_hover.clone())
        .expect("shared X11 tab hover token");
    assert!(matches!(hover.identity, FileDropSessionIdentity::X11(value) if value == id));
    hover.started_at = Instant::now() - Duration::from_millis(500);
    browser
        .file_drop_session
        .as_mut()
        .expect("X11 session")
        .tab_hover = Some(hover.clone());
    drop(browser.accept_tab_hover_elapsed(hover));
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
        state => panic!("expected switched X11 layout request, got {state:?}"),
    };
    let nested_destination = PathBuf::from("/workspace/hover/nested");
    drop(browser.accept_drop_layout(
        switched_request,
        directory_bounds(
            target.pane_id,
            nested_destination.clone(),
            rectangle(0.0, 50.0, 100.0, 100.0),
        ),
    ));
    drop(browser.accept_x11_target_event(
        X11FileDropTargetEvent::Moved {
            target_session_id: id,
            position: position(1, 50, 80),
        },
        1.0,
        1,
    ));
    drop(browser.accept_x11_target_event(
        X11FileDropTargetEvent::Dropped {
            target_session_id: id,
            position: position(1, 50, 80),
        },
        1.0,
        1,
    ));
    let source = PathBuf::from("/outside/after-switch.txt");
    drop(browser.accept_x11_file_drop(X11DndFileDrop {
        target_session_id: id,
        paths: vec![source.clone()],
    }));

    assert!(matches!(
        &browser.file_drop_prompt,
        Some(FileDropPrompt { paste_directory, paths })
            if paste_directory == &nested_destination && paths == &vec![source]
    ));
}

fn begin_session(
    browser: &mut FileBrowser,
    id: X11FileDropTargetSessionId,
    position: X11DndDropPosition,
    scale_factor: f32,
    scale_generation: u64,
) -> crate::model::FileDropLayoutRequest {
    drop(browser.accept_x11_target_event(
        X11FileDropTargetEvent::Entered {
            target_session_id: id,
            position,
        },
        scale_factor,
        scale_generation,
    ));
    match browser
        .file_drop_session
        .as_ref()
        .expect("X11 drop session")
        .layout
    {
        FileDropLayoutState::Pending(request) => request,
        FileDropLayoutState::Ready { .. } => panic!("new X11 session must measure layout"),
    }
}

fn add_inactive_directory_tab(
    browser: &mut FileBrowser,
    directory: impl Into<PathBuf>,
) -> TabFileDropTarget {
    let original_tab_id = browser.active_tab_id;
    let directory = directory.into();
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

fn directory_bounds(
    pane_id: BrowserPaneId,
    directory: impl Into<PathBuf>,
    bounds: Rectangle,
) -> FileDragHitTestBounds {
    FileDragHitTestBounds {
        directory_targets: vec![DirectoryFileDragTargetBounds {
            pane_id,
            directory: directory.into(),
            bounds,
        }],
        ..FileDragHitTestBounds::default()
    }
}

fn tab_bounds(target: TabFileDropTarget, bounds: Rectangle) -> FileDragHitTestBounds {
    FileDragHitTestBounds {
        tabs: vec![TabFileDropTargetBounds { target, bounds }],
        ..FileDragHitTestBounds::default()
    }
}

fn rectangle(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(width, height))
}

fn position(scale_generation: u64, client_x: i16, client_y: i16) -> X11DndDropPosition {
    X11DndDropPosition {
        root_x: client_x,
        root_y: client_y,
        client_x,
        client_y,
        timestamp: 7,
        scale_generation,
    }
}
