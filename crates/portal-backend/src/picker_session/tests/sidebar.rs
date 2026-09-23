//! 侧边栏状态机测试：数据回填、trash 列表行构造、宽度 clamp、
//! 挂载交互（防重入/成功导航/失败提示）、SaveFile 回收站确认禁用。

use super::*;
use bennu_sidebar::{
    SavedNetworkConnection, SidebarDeviceEntry, SidebarLocation, SidebarLocationKind,
    SidebarNetworkConnectionEntry,
};
use desktop_linux::{
    NetworkConnection, NetworkConnectionId, NetworkMountState, NetworkProtocol,
    StorageDeviceAccess, StorageDeviceId,
};
use file_core::entry::{EntryMetadata, FileKind};
use file_core::trash_bin::TrashEntry;

fn home_location() -> SidebarLocation {
    SidebarLocation {
        label: "Home".to_string(),
        path: PathBuf::from("/home/user"),
        kind: SidebarLocationKind::Home,
    }
}

fn unmounted_device(id: &str) -> SidebarDeviceEntry {
    SidebarDeviceEntry {
        id: StorageDeviceId::new(id),
        label: id.to_string(),
        detail: None,
        size_bytes: 1024,
        mount_points: Vec::new(),
        access: StorageDeviceAccess::LocalFilesystem,
        can_mount: true,
        can_unmount: false,
        removal: None,
    }
}

fn saved_connection(id: &str) -> SidebarNetworkConnectionEntry {
    SidebarNetworkConnectionEntry::new(SavedNetworkConnection::new(
        NetworkConnection::new(
            NetworkConnectionId::new(id),
            id,
            NetworkProtocol::Smb,
            "smb://server/share",
        )
        .unwrap(),
        false,
    ))
}

fn sidebar_data() -> PickerSidebarData {
    PickerSidebarData {
        locations: vec![home_location()],
        connections: vec![saved_connection("nas")],
        devices: vec![unmounted_device("udisk")],
        provider_failures: Vec::new(),
    }
}

#[test]
fn sidebar_data_backfill_replaces_state_and_verifies_scrollbar() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    assert!(session.sidebar().locations.is_empty());

    let effect = session.update(SessionMessage::SidebarDataLoaded(Box::new(sidebar_data())));

    assert!(matches!(effect, SessionEffect::VerifyScrollbarLayout));
    assert_eq!(session.sidebar().locations.len(), 1);
    assert_eq!(session.sidebar().connections.len(), 1);
    assert_eq!(session.sidebar().devices.len(), 1);
}

#[test]
fn trash_row_entry_keeps_original_name_and_payload_path() {
    let entry = DirectoryEntry::new(
        PathBuf::from("/home/user/.local/share/Trash/files/report.pdf.2"),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    );
    let trash_entry = TrashEntry::from_historical_entry(
        PathBuf::from("/home/user/.local/share/Trash/files/report.pdf.2"),
        PathBuf::from("/home/user/.local/share/Trash/info/report.pdf.2.trashinfo"),
        PathBuf::from("/home/user/Documents/report.pdf"),
        None,
        entry,
    );

    let row = crate::picker_session::scan::trash_row_entry(trash_entry);

    // 名称展示原始文件名（丢弃冲突后缀），确认返回 files/ 真实载荷路径。
    assert_eq!(row.name, "report.pdf");
    assert_eq!(
        row.path,
        PathBuf::from("/home/user/.local/share/Trash/files/report.pdf.2")
    );
}

#[test]
fn trash_navigation_scans_and_openfile_confirms_payload_path() {
    let (mut session, mut receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });

    let effect = session.update(SessionMessage::SidebarTrashPressed);
    assert!(matches!(
        effect,
        SessionEffect::NavigateDirectory(ref path) if path.as_os_str() == TRASH_DIRECTORY
    ));
    assert!(session.is_trash_view());

    let trash_file = DirectoryEntry::new(
        PathBuf::from("/home/user/.local/share/Trash/files/old.txt"),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    );
    session.apply_scan(DirectoryScanResult {
        directory: PathBuf::from(TRASH_DIRECTORY),
        outcome: Ok(DirectoryScanOutcome {
            entries: vec![trash_file],
        }),
    });
    assert_eq!(session.rows().len(), 1);

    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    let effect = session.update(SessionMessage::ConfirmPressed);
    assert!(
        matches!(effect, SessionEffect::Confirmed(paths) if paths == vec![PathBuf::from(
            "/home/user/.local/share/Trash/files/old.txt"
        )])
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(PickerResolution::Confirmed(paths)) if paths.len() == 1
    ));
}

#[test]
fn savefile_cannot_confirm_in_trash_view() {
    let (mut session, mut receiver) = session(PickerKind::SaveFile { default_name: None });
    session.update(SessionMessage::SidebarTrashPressed);
    session.apply_scan(DirectoryScanResult {
        directory: PathBuf::from(TRASH_DIRECTORY),
        outcome: Ok(DirectoryScanOutcome {
            entries: Vec::new(),
        }),
    });
    session.update(SessionMessage::NameInputChanged("new.txt".to_string()));

    assert!(!session.can_confirm());
    // 回车（输入框 on_submit）也不得消费 reply。
    let effect = session.update(SessionMessage::ConfirmPressed);
    assert!(matches!(effect, SessionEffect::None));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn unmounted_device_press_mounts_once_and_navigates_on_success() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::SidebarDataLoaded(Box::new(sidebar_data())));

    let effect = session.update(SessionMessage::SidebarDevicePressed {
        id: StorageDeviceId::new("udisk"),
    });
    assert!(matches!(effect, SessionEffect::MountDevice(_)));
    assert!(session.sidebar_device_is_mounting(&StorageDeviceId::new("udisk")));

    // busy 防重入：挂载期间重复点击忽略。
    let effect = session.update(SessionMessage::SidebarDevicePressed {
        id: StorageDeviceId::new("udisk"),
    });
    assert!(matches!(effect, SessionEffect::None));

    let effect = session.update(SessionMessage::SidebarDeviceMountFinished {
        id: StorageDeviceId::new("udisk"),
        mount_path: Ok(PathBuf::from("/run/media/user/udisk")),
    });
    assert!(matches!(
        effect,
        SessionEffect::NavigateDirectory(ref path) if *path == PathBuf::from("/run/media/user/udisk")
    ));
    assert!(!session.sidebar_device_is_mounting(&StorageDeviceId::new("udisk")));
    assert_eq!(
        session.sidebar().devices[0].mount_points,
        vec![PathBuf::from("/run/media/user/udisk")]
    );
}

#[test]
fn device_mount_failure_shows_notice_and_clears_on_next_success() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::SidebarDataLoaded(Box::new(sidebar_data())));

    session.update(SessionMessage::SidebarDevicePressed {
        id: StorageDeviceId::new("udisk"),
    });
    session.update(SessionMessage::SidebarDeviceMountFinished {
        id: StorageDeviceId::new("udisk"),
        mount_path: Err("mount failed".to_string()),
    });

    assert_eq!(session.sidebar().notice.as_deref(), Some("mount failed"));
    assert!(!session.sidebar_device_is_mounting(&StorageDeviceId::new("udisk")));

    // 下次成功操作清除提示。
    session.update(SessionMessage::SidebarDevicePressed {
        id: StorageDeviceId::new("udisk"),
    });
    session.update(SessionMessage::SidebarDeviceMountFinished {
        id: StorageDeviceId::new("udisk"),
        mount_path: Ok(PathBuf::from("/media/udisk")),
    });
    assert!(session.sidebar().notice.is_none());
}

#[test]
fn connection_press_navigates_mounted_and_mounts_disconnected() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    let mut data = sidebar_data();
    data.connections[0].state = NetworkMountState::Mounted(PathBuf::from("/run/gvfs/server"));
    session.update(SessionMessage::SidebarDataLoaded(Box::new(data)));

    // 已挂载：直接导航挂载点。
    let effect = session.update(SessionMessage::SidebarConnectionPressed {
        id: NetworkConnectionId::new("nas"),
    });
    assert!(matches!(
        effect,
        SessionEffect::NavigateDirectory(ref path) if *path == PathBuf::from("/run/gvfs/server")
    ));

    // 未挂载：置 Connecting + 挂载效果；Connecting 期间点击忽略。
    let mut data = sidebar_data();
    data.connections[0].state = NetworkMountState::Disconnected;
    session.update(SessionMessage::SidebarDataLoaded(Box::new(data)));
    let effect = session.update(SessionMessage::SidebarConnectionPressed {
        id: NetworkConnectionId::new("nas"),
    });
    assert!(matches!(effect, SessionEffect::MountConnection(_)));
    let effect = session.update(SessionMessage::SidebarConnectionPressed {
        id: NetworkConnectionId::new("nas"),
    });
    assert!(matches!(effect, SessionEffect::None));

    // 失败：回落未连接 + 提示；条目被数据重载顶掉的迟到回信丢弃。
    session.update(SessionMessage::SidebarConnectionMountFinished {
        id: NetworkConnectionId::new("nas"),
        mount_path: Err("unreachable".to_string()),
    });
    assert!(matches!(
        session.sidebar().connections[0].state,
        NetworkMountState::Disconnected
    ));
    assert_eq!(session.sidebar().notice.as_deref(), Some("unreachable"));

    session.update(SessionMessage::SidebarConnectionMountFinished {
        id: NetworkConnectionId::new("gone"),
        mount_path: Ok(PathBuf::from("/run/gvfs/other")),
    });
    assert!(matches!(
        session.sidebar().connections[0].state,
        NetworkMountState::Disconnected
    ));
}

#[test]
fn location_press_navigates_and_records_history() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::SidebarDataLoaded(Box::new(sidebar_data())));

    let effect = session.update(SessionMessage::SidebarLocationPressed {
        path: PathBuf::from("/home/user"),
    });
    assert!(matches!(
        effect,
        SessionEffect::NavigateDirectory(ref path) if *path == PathBuf::from("/home/user")
    ));
    assert!(session.can_navigate_back());
}
