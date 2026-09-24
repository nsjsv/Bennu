//! drag.rs 的拖拽生命周期/意图/落地测试。drag.rs 主体受 800 行预算
//! 约束,测试按本目录惯例(activation_tests 等)拆成同级测试模块。

use std::path::PathBuf;

use iced::keyboard;

use super::drag::{in_place_duplicate_target, safe_file_drop_target};
use crate::model::{
    FileDragDropIntent, FileDragNativeDndState, FileDragStationaryAction, FileDropTarget,
    SidebarBookmarkDropSlot,
};
use crate::operation_queue::QueuedFileOperation;
use crate::sidebar_devices::SidebarDeviceEntry;
use desktop_linux::{StorageDeviceAccess, StorageDeviceId};

#[test]
fn drag_drop_intent_follows_live_modifiers() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let target = PathBuf::from("/tmp/drag-target");

    // 无修饰=移动意图(现状)。
    assert_eq!(
        browser.file_drag_drop_intent(&[], &target),
        FileDragDropIntent::Move
    );

    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
    assert_eq!(
        browser.file_drag_drop_intent(&[], &target),
        FileDragDropIntent::Copy
    );

    // Shift 与 Ctrl+Shift 都保持移动语义:Shift 本身就是移动修饰键。
    browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
    assert_eq!(
        browser.file_drag_drop_intent(&[], &target),
        FileDragDropIntent::Move
    );
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT;
    assert_eq!(
        browser.file_drag_drop_intent(&[], &target),
        FileDragDropIntent::Move
    );

    // Alt=创建链接;Ctrl 与 Alt 同时按住时 Ctrl 复制优先。
    browser.keyboard_modifiers = keyboard::Modifiers::ALT;
    assert_eq!(
        browser.file_drag_drop_intent(&[], &target),
        FileDragDropIntent::CreateLink
    );
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL | keyboard::Modifiers::ALT;
    assert_eq!(
        browser.file_drag_drop_intent(&[], &target),
        FileDragDropIntent::Copy
    );
}

#[test]
fn alt_drag_intent_falls_back_to_move_on_remote_mount() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    browser.sidebar_devices.devices = vec![SidebarDeviceEntry {
        id: StorageDeviceId::new("gvfs"),
        label: "gvfs".to_owned(),
        detail: None,
        size_bytes: 0,
        mount_points: vec![PathBuf::from("/run/user/1000/gvfs")],
        access: StorageDeviceAccess::RemoteFilesystem,
        can_mount: true,
        can_unmount: true,
        removal: None,
    }];
    browser.keyboard_modifiers = keyboard::Modifiers::ALT;

    let remote_source = PathBuf::from("/run/user/1000/gvfs/mtp/DCIM/photo.jpg");
    let local_source = PathBuf::from("/home/user/photo.jpg");
    let local_target = PathBuf::from("/home/user/photos");
    let remote_target = PathBuf::from("/run/user/1000/gvfs/mtp/DCIM");

    // 源或落点在远程挂载:gvfs 上 symlink 不可靠,回退移动。
    assert_eq!(
        browser.file_drag_drop_intent(&[remote_source], &local_target),
        FileDragDropIntent::Move
    );
    assert_eq!(
        browser.file_drag_drop_intent(std::slice::from_ref(&local_source), &remote_target),
        FileDragDropIntent::Move
    );
    assert_eq!(
        browser.file_drag_drop_intent(std::slice::from_ref(&local_source), &local_target),
        FileDragDropIntent::CreateLink
    );
}

#[test]
fn drag_action_capsule_label_follows_target_and_intent() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("report.pdf");
    std::fs::write(&source, b"data").unwrap();
    let project = directory.path().join("project");
    std::fs::create_dir(&project).unwrap();

    // 无拖拽会话:不渲染。
    assert!(browser.file_drag_action_capsule_label().is_none());

    // entries 为空时 sources 回退到 self.selected。
    browser.selected = Some(source.clone());
    browser.cursor_position = iced::Point::new(0.0, 0.0);
    drop(browser.start_file_drag(
        source.clone(),
        FileDragStationaryAction::SelectionOnly,
        Vec::new(),
    ));
    drop(browser.update_file_drag(iced::Point::new(10.0, 0.0)));

    // 拖拽中但无悬停落点:不渲染。
    assert!(browser.file_drag_action_capsule_label().is_none());

    drop(browser.handle_drop_target_hovered(project.clone()));
    assert_eq!(
        browser.file_drag_action_capsule_label().as_deref(),
        Some("Move to project")
    );

    // 光标悬停回被拖的源条目:提起的内容悬在自己身上,落点无意义,不显示。
    drop(browser.handle_entry_hovered(source.clone()));
    assert!(browser.file_drag_action_capsule_label().is_none());

    // 悬停源父目录(当前目录空白处):移动落地是空操作,不显示;
    // Ctrl 复制在同目录有原位副本,照常显示。
    let current_directory = directory.path().to_path_buf();
    let directory_name = current_directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap()
        .to_owned();
    drop(browser.handle_drop_target_hovered(current_directory.clone()));
    assert!(browser.file_drag_action_capsule_label().is_none());
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
    assert_eq!(
        browser.file_drag_action_capsule_label().as_deref(),
        Some(format!("Copy to {directory_name}").as_str())
    );
    browser.keyboard_modifiers = keyboard::Modifiers::empty();

    // 离开源条目回到文件夹落点:恢复显示。
    drop(browser.handle_entry_hover_cleared(source.clone()));
    drop(browser.handle_drop_target_hovered(project.clone()));
    assert_eq!(
        browser.file_drag_action_capsule_label().as_deref(),
        Some("Move to project")
    );
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
    assert_eq!(
        browser.file_drag_action_capsule_label().as_deref(),
        Some("Copy to project")
    );
    browser.keyboard_modifiers = keyboard::Modifiers::ALT;
    assert_eq!(
        browser.file_drag_action_capsule_label().as_deref(),
        Some("Create link to project")
    );

    // 回收站落点固定移动文案,修饰键不影响(落地行为同样不看)。
    let session = browser.file_drop_session.as_mut().unwrap();
    session.hovered_target = Some(FileDropTarget::Trash);
    assert_eq!(
        browser.file_drag_action_capsule_label().as_deref(),
        Some("Move to Trash")
    );

    // 书签槽不是传输语义:不渲染。
    let session = browser.file_drop_session.as_mut().unwrap();
    session.hovered_target = Some(FileDropTarget::SidebarBookmarkSlot(
        SidebarBookmarkDropSlot::Insert { index: 0 },
    ));
    assert!(browser.file_drag_action_capsule_label().is_none());
}

#[test]
fn move_drag_with_fully_no_op_batch_does_nothing() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("project");
    std::fs::create_dir(&folder).unwrap();

    // 全部都是空操作(目录移入自身):不入队任何传输。
    drop(browser.move_dragged_files(vec![folder.clone()], folder.clone()));
    assert!(browser.operation_queue.tasks().is_empty());
}

#[test]
fn drag_action_capsule_shows_when_batch_partially_no_op() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("project");
    std::fs::create_dir(&folder).unwrap();
    let sibling = directory.path().join("report.pdf");
    std::fs::write(&sibling, b"data").unwrap();

    // 多选批 = 落点目录自身 + 可移动条目:其余条目仍可移动,胶囊
    // 照常显示;整批都是空操作才隐藏。
    browser.file_drag = Some(crate::model::FileDragState {
        gesture_id: crate::model::FileDragGestureId(1),
        source_pane_id: browser.active_pane_id(),
        source_tab_id: browser.active_tab_id,
        sources: vec![folder.clone(), sibling.clone()],
        pressed_path: sibling.clone(),
        bookmark_source: None,
        stationary_action: FileDragStationaryAction::SelectionOnly,
        phase: crate::model::FileDragPhase::WaitingForMovement {
            origin: iced::Point::new(0.0, 0.0),
        },
        native_dnd: crate::model::FileDragNativeDndState::NotRequested,
        column_directories_snapshot: Vec::new(),
        press_origin: iced::Point::new(0.0, 0.0),
        preview_entries: Vec::new(),
        wayland_drag_icon: None,
    });
    // 激活拖拽会话(测试环境无 wayland 句柄,走应用内拖拽回退),
    // 落点悬停会话由此建立。
    drop(browser.update_file_drag(iced::Point::new(10.0, 0.0)));
    drop(browser.handle_drop_target_hovered(folder.clone()));
    assert_eq!(
        browser.file_drag_action_capsule_label().as_deref(),
        Some("Move to project")
    );

    // 整批都是空操作(悬停回源父目录空白):隐藏。
    drop(browser.handle_drop_target_hovered(directory.path().to_path_buf()));
    assert!(browser.file_drag_action_capsule_label().is_none());
}

#[test]
fn alt_drag_release_queues_symlinks_into_target_directory() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("report.pdf");
    std::fs::write(&source, b"data").unwrap();
    let target = directory.path().join("archive");
    std::fs::create_dir(&target).unwrap();

    browser.keyboard_modifiers = keyboard::Modifiers::ALT;
    drop(browser.move_dragged_files(vec![source.clone()], target.clone()));

    assert_eq!(browser.operation_queue.tasks().len(), 1);
    match &browser.operation_queue.tasks()[0].operation {
        QueuedFileOperation::CreateSymbolicLinks { links } => {
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target_path, source);
            assert_eq!(links[0].link_path.parent(), Some(target.as_path()));
        }
        other => panic!("expected symlink creation, got {other:?}"),
    }
}

#[test]
fn ctrl_drag_inside_same_directory_plans_in_place_duplicate() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("report.pdf");
    std::fs::write(&source, b"data").unwrap();
    let directory_path = directory.path().to_path_buf();

    let target = in_place_duplicate_target(&source, &directory_path, false);

    assert_eq!(target, directory.path().join("report副本.pdf"));
    assert!(!target.exists());
}

#[test]
fn unsafe_directory_targets_are_rejected_for_every_source_relationship() {
    let file_source = PathBuf::from("/workspace/report.txt");
    let directory_source = PathBuf::from("/workspace/project");

    for (sources, target) in [
        (vec![file_source.clone()], PathBuf::from("/workspace")),
        (vec![directory_source.clone()], directory_source.clone()),
        (
            vec![directory_source.clone()],
            directory_source.join("nested"),
        ),
    ] {
        assert!(
            safe_file_drop_target(&sources, Some(FileDropTarget::Directory(target)),).is_none()
        );
    }
}

#[test]
fn expanded_subdirectory_source_can_move_back_to_tab_root() {
    let source = PathBuf::from("/workspace/root/expanded/report.txt");
    let root = PathBuf::from("/workspace/root");

    assert_eq!(
        safe_file_drop_target(
            std::slice::from_ref(&source),
            Some(FileDropTarget::Directory(root.clone())),
        ),
        Some(FileDropTarget::Directory(root))
    );
}

#[test]
fn activation_hands_file_drag_to_native_wayland_session() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    browser.wayland_dnd = Some(crate::app::wayland_dnd::WaylandDndRuntime {
        window_handle: desktop_linux::WaylandDndWindowHandle::new(0x1, 0x2),
        controller: desktop_linux::WaylandDndController::new(),
    });
    let source = PathBuf::from("/workspace/report.txt");
    browser.selected = Some(source.clone());
    browser.cursor_position = iced::Point::new(0.0, 0.0);
    drop(browser.start_file_drag(source, FileDragStationaryAction::SelectionOnly, Vec::new()));

    drop(browser.update_file_drag(iced::Point::new(10.0, 0.0)));

    let file_drag = browser
        .file_drag
        .as_ref()
        .expect("drag survives activation");
    assert!(matches!(
        file_drag.native_dnd,
        FileDragNativeDndState::Requested(_)
    ));
    // 原生会话接管:自绘预览退场,应用内落点会话不创建。
    assert!(!file_drag.displays_iced_drag_preview());
    assert!(browser.file_drop_session.is_none());
}

#[test]
fn activation_without_wayland_runtime_falls_back_to_iced_drag() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let source = PathBuf::from("/workspace/report.txt");
    browser.selected = Some(source.clone());
    browser.cursor_position = iced::Point::new(0.0, 0.0);
    drop(browser.start_file_drag(source, FileDragStationaryAction::SelectionOnly, Vec::new()));

    drop(browser.update_file_drag(iced::Point::new(10.0, 0.0)));

    let file_drag = browser
        .file_drag
        .as_ref()
        .expect("drag survives activation");
    assert_eq!(file_drag.native_dnd, FileDragNativeDndState::NotRequested);
    assert!(matches!(
        browser
            .file_drop_session
            .as_ref()
            .map(|session| session.identity),
        Some(crate::model::FileDropSessionIdentity::Iced(_))
    ));
}

#[test]
fn preview_offsets_fill_while_waiting_for_movement() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let source = PathBuf::from("/workspace/report.txt");
    browser.selected = Some(source.clone());
    browser.cursor_position = iced::Point::new(20.0, 30.0);
    drop(browser.start_file_drag(
        source.clone(),
        FileDragStationaryAction::SelectionOnly,
        Vec::new(),
    ));
    // 按下时发起的测量在激活前到达:此刻仍是 WaitingForMovement。
    let press_origin = browser.file_drag.as_ref().unwrap().press_origin;
    let bounds = vec![crate::model::ColumnEntryBounds {
        pane_id: browser.active_pane_id(),
        path: source,
        bounds: iced::Rectangle::new(iced::Point::new(15.0, 22.0), iced::Size::new(100.0, 20.0)),
    }];

    // 测试环境无 wayland 句柄,填充不发起预渲染。
    drop(browser.refresh_file_drag_preview_layout(&bounds));

    let file_drag = browser.file_drag.as_ref().unwrap();
    assert!(!file_drag.is_dragging());
    assert_eq!(file_drag.preview_entries.len(), 1);
    assert_eq!(
        file_drag.preview_entries[0].offset,
        iced::Vector::new(15.0 - press_origin.x, 22.0 - press_origin.y)
    );
}

#[test]
fn move_drag_of_archive_members_extracts_into_target_directory() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    // 虚拟包内路径只要求归档边界是真实存在的归档文件,成员条目按
    // 路径形态判定,无需真实解包。
    let archive = directory.path().join("docs.zip");
    std::fs::write(&archive, b"payload").unwrap();
    let member = archive.join("photos/1.txt");
    let destination = directory.path().join("landing");
    std::fs::create_dir(&destination).unwrap();

    drop(browser.move_dragged_files(vec![member.clone()], destination.clone()));

    assert_eq!(browser.operation_queue.tasks().len(), 1);
    assert!(matches!(
        &browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::ExtractArchiveMembers { sources, destination: target }
            if sources == &vec![member.clone()] && target == &destination
    ));
}

#[test]
fn mixed_drag_extracts_members_alongside_real_file_transfers() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("docs.zip");
    std::fs::write(&archive, b"payload").unwrap();
    let member = archive.join("photos/1.txt");
    let real_file = directory.path().join("notes.txt");
    std::fs::write(&real_file, b"data").unwrap();
    let destination = directory.path().join("landing");
    std::fs::create_dir(&destination).unwrap();

    drop(browser.move_dragged_files(vec![member.clone(), real_file], destination.clone()));

    // 包内成员进了提取队列而不是真实传输队列(后者会对虚拟路径
    // symlink_metadata 报 os error 20);真实文件照常走冲突检查管线,
    // 不在此断言。混合多选时两路并行,提取已在任务表里。
    assert_eq!(browser.operation_queue.tasks().len(), 1);
    assert!(matches!(
        &browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::ExtractArchiveMembers { sources, .. } if sources == &vec![member]
    ));
}

#[test]
fn ctrl_drag_of_archive_members_extracts_into_target_directory() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("docs.zip");
    std::fs::write(&archive, b"payload").unwrap();
    let member = archive.join("report.pdf");
    let destination = directory.path().join("landing");
    std::fs::create_dir(&destination).unwrap();
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;

    drop(browser.move_dragged_files(vec![member], destination));

    // Ctrl 复制意图与移动同样分流:包内成员复制不出,降级为提取。
    assert_eq!(browser.operation_queue.tasks().len(), 1);
    assert!(matches!(
        &browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::ExtractArchiveMembers { .. }
    ));
}

#[test]
fn drag_landing_inside_archive_does_nothing() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("docs.zip");
    std::fs::write(&archive, b"payload").unwrap();
    let real_file = directory.path().join("notes.txt");
    std::fs::write(&real_file, b"data").unwrap();
    let inside_archive = archive.join("photos");

    // 落点在包内:与粘贴的包内只读门控同源,移动意图不执行。
    drop(browser.move_dragged_files(vec![real_file.clone()], inside_archive));
    assert!(browser.operation_queue.tasks().is_empty());

    // 包根(归档文件本身视作目录的浏览入口)同样是包内落点,
    // 复制意图同样被吞掉。
    browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
    drop(browser.move_dragged_files(vec![real_file], archive));
    assert!(browser.operation_queue.tasks().is_empty());
}

#[test]
fn alt_drag_with_archive_members_queues_nothing() {
    let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("docs.zip");
    std::fs::write(&archive, b"payload").unwrap();
    let member = archive.join("report.pdf");
    let real_file = directory.path().join("notes.txt");
    std::fs::write(&real_file, b"data").unwrap();
    let target = directory.path().join("landing");
    std::fs::create_dir(&target).unwrap();
    browser.keyboard_modifiers = keyboard::Modifiers::ALT;

    // 建链意图遇包内成员整批不做:不为虚拟路径创建悬空符号链接,
    // 也不悄悄退化成对剩余真实文件的移动(意图中途变卦更误导)。
    drop(browser.move_dragged_files(vec![real_file, member], target));
    assert!(browser.operation_queue.tasks().is_empty());
}
