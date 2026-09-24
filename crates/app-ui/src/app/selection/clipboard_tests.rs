//! clipboard.rs 的粘贴/删除/收纳测试。clipboard.rs 主体受 800 行预算
//! 约束,测试按本目录惯例拆成同级测试模块。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::clipboard::move_paste_transfers;
use super::gather_sources_in_directory;
use crate::app::FileBrowser;
use crate::config;
use crate::model::{
    BrowserViewMode, DestructiveActionConfirmation, FileDragStationaryAction, FileDropTarget,
    FileEntryContentModifier, PendingOperation,
};
use crate::operation_queue::{QueuedFileOperation, QueuedTransfer};
use desktop_linux::{NetworkConnection, NetworkConnectionId, NetworkMountState, NetworkProtocol};
use file_core::{DirectoryEntry, EntryMetadata, FileKind};

fn test_entry(path: &Path) -> DirectoryEntry {
    test_entry_with_kind(path, FileKind::File)
}

fn test_entry_with_kind(path: &Path, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        path.to_path_buf(),
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

fn browser_with_entries(paths: &[PathBuf]) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = paths
        .iter()
        .map(|path| test_entry(path))
        .collect::<Vec<_>>()
        .into();
    browser.selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
    browser.selected = paths.first().cloned();
    browser
}

fn mount_network_connection(browser: &mut FileBrowser, mount_path: PathBuf) {
    let connection = NetworkConnection::new(
        NetworkConnectionId::new("nas"),
        "NAS",
        NetworkProtocol::Smb,
        "smb://server/share",
    )
    .unwrap();
    let id = connection.id.clone();
    browser.network_connections =
        crate::network_connections::NetworkConnectionState::from_connections(vec![connection]);
    browser
        .network_connections
        .accept_loaded(vec![(id, NetworkMountState::Mounted(mount_path))]);
}

#[test]
fn entry_hover_uses_current_level_for_paste_and_directory_for_drop() {
    let current_dir = PathBuf::from("/workspace");
    let source = current_dir.join("report.txt");
    let directory = current_dir.join("project");
    let mut browser = browser_with_entries(std::slice::from_ref(&source));
    browser.view_mode = BrowserViewMode::List;
    browser.entries = vec![
        test_entry(&source),
        test_entry_with_kind(&directory, FileKind::Directory),
    ]
    .into();

    drop(browser.handle_entry_hovered(directory.clone()));
    assert_eq!(browser.paste_target_directory(), current_dir);
    drop(browser.handle_entry_hovered(source.clone()));
    assert_eq!(browser.paste_target_directory(), current_dir);

    browser.cursor_position = iced::Point::new(0.0, 0.0);
    drop(browser.start_file_drag(source, FileDragStationaryAction::SelectionOnly, Vec::new()));
    drop(browser.update_file_drag(iced::Point::new(10.0, 0.0)));
    drop(browser.handle_entry_hovered(directory.clone()));

    assert_eq!(browser.paste_target_directory(), current_dir);
    drop(browser.handle_drop_target_hovered(directory.clone()));
    assert_eq!(browser.paste_target_directory(), current_dir);
    assert!(matches!(
        browser
            .file_drop_session
            .as_ref()
            .and_then(|session| session.hovered_target.as_ref()),
        Some(FileDropTarget::Directory(target)) if target == &directory
    ));
}

#[test]
fn cut_visual_modifier_requires_exact_move_source_membership() {
    let source = PathBuf::from("/workspace/report.txt");
    let move_operation = PendingOperation::Move(vec![source.clone()]);

    assert_eq!(
        move_operation.content_modifier_for_path(&source),
        FileEntryContentModifier::Cut
    );
    assert_eq!(
        move_operation.content_modifier_for_path(Path::new("/workspace/report.txt.bak")),
        FileEntryContentModifier::None
    );
    assert_eq!(
        move_operation.content_modifier_for_path(Path::new("/archive/report.txt")),
        FileEntryContentModifier::None
    );
    assert_eq!(
        PendingOperation::Copy(vec![source.clone()]).content_modifier_for_path(&source),
        FileEntryContentModifier::Copied
    );
    assert_eq!(
        PendingOperation::Copy(vec![source.clone()])
            .content_modifier_for_path(Path::new("/workspace/report.txt.bak")),
        FileEntryContentModifier::None
    );
}

#[test]
fn new_clipboard_operation_replaces_cut_visual_membership() {
    let first = PathBuf::from("/workspace/first.txt");
    let second = PathBuf::from("/workspace/second.txt");
    let mut browser = browser_with_entries(&[first.clone(), second.clone()]);
    browser.selected_paths = HashSet::from([first.clone()]);
    drop(browser.move_selected());
    assert_eq!(
        browser.file_entry_content_modifier(&first),
        FileEntryContentModifier::Cut
    );

    browser.selected = Some(second.clone());
    browser.selected_paths = HashSet::from([second.clone()]);
    drop(browser.move_selected());
    assert_eq!(
        browser.file_entry_content_modifier(&first),
        FileEntryContentModifier::None
    );
    assert_eq!(
        browser.file_entry_content_modifier(&second),
        FileEntryContentModifier::Cut
    );

    drop(browser.copy_selected());
    assert_eq!(
        browser.file_entry_content_modifier(&second),
        FileEntryContentModifier::Copied
    );
}

#[test]
fn move_paste_consumes_cut_visual_membership() {
    let source = PathBuf::from("/workspace/report.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&source));
    drop(browser.move_selected());
    assert_eq!(
        browser.file_entry_content_modifier(&source),
        FileEntryContentModifier::Cut
    );

    drop(browser.paste_operation(
        PathBuf::from("/destination"),
        PendingOperation::Move(vec![source.clone()]),
    ));
    assert!(browser.pending_operation.is_none());
    assert_eq!(
        browser.file_entry_content_modifier(&source),
        FileEntryContentModifier::None
    );
}

#[test]
fn move_paste_into_own_subtree_enqueues_nothing() {
    let project = PathBuf::from("/workspace/project");
    let mut browser = browser_with_entries(std::slice::from_ref(&project));

    // 与拖拽落地同一不变量:目录移入自身子树是空操作,直接跳过,
    // 不入队由传输引擎报错。
    drop(browser.paste_operation(
        project.join("inner"),
        PendingOperation::Move(vec![project.clone()]),
    ));
    assert!(browser.operation_queue.tasks().is_empty());
}

#[test]
fn move_paste_transfers_skips_no_op_sources_and_keeps_the_rest() {
    let already_there = PathBuf::from("/workspace/project");
    let outsider = PathBuf::from("/other/notes");

    // 混合选择:已在落点目录内的源是空操作,其余条目照常移动。
    let transfers =
        move_paste_transfers(Path::new("/workspace"), &[already_there, outsider.clone()]);

    assert_eq!(
        transfers,
        vec![QueuedTransfer::new(
            outsider,
            PathBuf::from("/workspace/notes")
        )]
    );
}

#[test]
fn local_delete_still_uses_trash_operation() {
    let local_path = PathBuf::from("/workspace/local.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&local_path));

    let command = browser.trash_selected();
    drop(command);

    assert!(browser.destructive_action_confirmation.is_none());
    assert_eq!(browser.operation_queue.tasks().len(), 1);
    assert!(matches!(
        &browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::Trash { paths } if paths == &vec![local_path]
    ));
}

#[test]
fn network_delete_requests_permanent_delete_confirmation() {
    let mount_path = PathBuf::from("/run/user/1000/gvfs/smb-share:server=server,share=share");
    let network_path = mount_path.join("remote.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&network_path));
    mount_network_connection(&mut browser, mount_path);

    let command = browser.trash_selected();
    drop(command);

    assert_eq!(browser.operation_queue.tasks().len(), 0);
    assert!(matches!(
        &browser.destructive_action_confirmation,
        Some(DestructiveActionConfirmation::DeletePermanently { paths })
            if paths == &vec![network_path]
    ));
}

#[test]
fn mixed_local_and_network_delete_is_rejected() {
    let mount_path = PathBuf::from("/run/user/1000/gvfs/smb-share:server=server,share=share");
    let network_path = mount_path.join("remote.txt");
    let local_path = PathBuf::from("/workspace/local.txt");
    let mut browser = browser_with_entries(&[local_path, network_path]);
    mount_network_connection(&mut browser, mount_path);

    let command = browser.trash_selected();
    drop(command);

    assert_eq!(browser.operation_queue.tasks().len(), 0);
    assert!(browser.destructive_action_confirmation.is_none());
    assert_eq!(
        browser.current_error(),
        Some("Delete local and remote items separately so local files can use Trash")
    );
}

#[test]
fn columns_keyboard_paste_targets_focused_column_not_pointer_hover() {
    let project = PathBuf::from("/workspace/project");
    let mut browser = browser_with_entries(&[PathBuf::from("/workspace/a.txt")]);
    browser.view_mode = BrowserViewMode::Columns;
    browser.deepest_open_column_directory = Some(project.clone());
    browser.focused_column_directory = Some(project.clone());
    browser.cursor_paste_directory = Some(PathBuf::from("/workspace"));

    assert_eq!(browser.paste_target_directory(), project);
}

#[test]
fn columns_keyboard_paste_falls_back_to_deepest_open_column_without_focus() {
    let project = PathBuf::from("/workspace/project");
    let mut browser = browser_with_entries(&[PathBuf::from("/workspace/a.txt")]);
    browser.view_mode = BrowserViewMode::Columns;
    browser.deepest_open_column_directory = Some(project.clone());
    browser.cursor_paste_directory = Some(PathBuf::from("/workspace"));

    assert_eq!(browser.paste_target_directory(), project);
}

#[test]
fn columns_keyboard_paste_falls_back_to_current_dir_when_nothing_open() {
    let mut browser = browser_with_entries(&[PathBuf::from("/workspace/a.txt")]);
    browser.view_mode = BrowserViewMode::Columns;

    assert_eq!(
        browser.paste_target_directory(),
        PathBuf::from("/workspace")
    );
}

#[test]
fn duplicate_selected_enqueues_in_place_copies_for_whole_selection() {
    let report = PathBuf::from("/workspace/report.pdf");
    let notes = PathBuf::from("/workspace/notes");
    let mut browser = browser_with_entries(&[report.clone(), notes.clone()]);
    // 副本走复制管线,恢复日志需要任务存储。
    let state_directory = tempfile::tempdir().unwrap();
    browser.operation_queue.set_store(
        file_operation_store::TaskQueueStore::new(state_directory.path().join("state.sqlite"))
            .unwrap(),
    );

    drop(browser.duplicate_selected());

    assert_eq!(browser.operation_queue.tasks().len(), 1);
    assert!(matches!(
        &browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::Duplicate { transfers, .. }
            if transfers == &vec![
                QueuedTransfer::new(report.clone(), PathBuf::from("/workspace/report副本.pdf")),
                QueuedTransfer::new(notes.clone(), PathBuf::from("/workspace/notes副本")),
            ]
    ));
}

#[test]
fn duplicate_completion_selects_the_new_copies() {
    let first_copy = PathBuf::from("/workspace/report副本.pdf");
    let second_copy = PathBuf::from("/workspace/notes副本");
    let mut browser = browser_with_entries(&[PathBuf::from("/workspace/report.pdf")]);
    let operation = QueuedFileOperation::Duplicate {
        transfers: vec![
            QueuedTransfer::new(PathBuf::from("/workspace/report.pdf"), first_copy.clone()),
            QueuedTransfer::new(PathBuf::from("/workspace/notes"), second_copy.clone()),
        ],
        verification: browser.file_operation_verification(),
    };

    let targets = operation.duplicate_selection_targets().unwrap();
    browser.select_operation_result_paths(targets);

    assert!(browser.selected_paths.contains(&first_copy));
    assert!(browser.selected_paths.contains(&second_copy));
    assert_eq!(browser.selected, Some(second_copy));
}

#[test]
fn gather_sources_keeps_only_entries_inside_the_target_directory() {
    let in_current = PathBuf::from("/workspace/report.txt");
    let in_other_column = PathBuf::from("/workspace/project/plan.txt");

    let sources = gather_sources_in_directory(
        &[in_current.clone(), in_other_column],
        &PathBuf::from("/workspace"),
    );

    assert_eq!(sources, vec![in_current]);
}

#[test]
fn new_folder_from_selection_enqueues_one_undoable_move_batch() {
    let workspace = tempfile::tempdir().unwrap();
    let report = workspace.path().join("report.txt");
    let mut browser = browser_with_entries(std::slice::from_ref(&report));
    browser.current_dir = workspace.path().to_path_buf();

    drop(browser.new_folder_from_selection());

    assert_eq!(browser.operation_queue.tasks().len(), 1);
    assert!(matches!(
        &browser.operation_queue.tasks()[0].operation,
        QueuedFileOperation::GatherSelectionIntoNewFolder { directory, sources }
            if directory == &workspace.path().join("新建文件夹")
                && sources == &vec![report.clone()]
    ));
}
