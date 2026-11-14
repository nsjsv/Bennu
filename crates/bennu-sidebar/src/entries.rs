//! 设备/网络连接的侧边栏条目视图模型。字段全 pub：宿主进程
//! （app-ui 的 NetworkConnectionState / SidebarDeviceState、portal 的
//! PickerSidebarState）拥有各自的会话状态机并直接读写这些数据字段。

use std::path::{Path, PathBuf};

use bennu_localization::tr;
use desktop_linux::{
    NetworkConnection, NetworkConnectionId, NetworkMountCredentials, NetworkMountState,
    StorageDevice, StorageDeviceAccess, StorageDeviceId, StorageDeviceRemoval,
};

use crate::saved_connections::SavedNetworkConnection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarDeviceAction {
    Mount,
    Unmount,
    Eject,
}

impl SidebarDeviceAction {
    pub fn label(self, device: &SidebarDeviceEntry) -> String {
        match self {
            Self::Mount => tr!("Mount"),
            Self::Unmount => tr!("Unmount"),
            Self::Eject if device.removal == Some(StorageDeviceRemoval::SafelyRemove) => {
                tr!("Safely Remove")
            }
            Self::Eject => tr!("Eject"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarDeviceEntry {
    pub id: StorageDeviceId,
    pub label: String,
    pub detail: Option<String>,
    pub size_bytes: u64,
    pub mount_points: Vec<PathBuf>,
    pub access: StorageDeviceAccess,
    pub can_mount: bool,
    pub can_unmount: bool,
    pub removal: Option<StorageDeviceRemoval>,
}

impl SidebarDeviceEntry {
    pub fn from_storage_device(device: StorageDevice) -> Self {
        let mount_points = device.mount_state.mount_points().to_vec();
        let detail = device
            .primary_mount_path()
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| {
                device
                    .device_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .filter(|detail| !detail.is_empty());

        Self {
            id: device.id,
            label: device.label,
            detail,
            size_bytes: device.size_bytes,
            mount_points,
            access: device.access,
            can_mount: device.can_mount,
            can_unmount: device.can_unmount,
            removal: device.removal,
        }
    }

    pub fn primary_mount_path(&self) -> Option<&Path> {
        self.mount_points.first().map(PathBuf::as_path)
    }

    pub fn is_mounted(&self) -> bool {
        self.primary_mount_path().is_some()
    }

    pub fn available_actions(&self) -> Vec<SidebarDeviceAction> {
        let mut actions = Vec::new();
        if self.is_mounted() && self.can_unmount {
            actions.push(SidebarDeviceAction::Unmount);
        }
        if !self.is_mounted() && self.can_mount {
            actions.push(SidebarDeviceAction::Mount);
        }
        if self.removal.is_some() {
            actions.push(SidebarDeviceAction::Eject);
        }
        actions
    }
}

/// 当前目录所属设备：取最长挂载点前缀命中（嵌套挂载时选最深者）。
pub fn selected_sidebar_device<'a>(
    devices: &'a [SidebarDeviceEntry],
    current_dir: &Path,
) -> Option<&'a SidebarDeviceEntry> {
    devices
        .iter()
        .flat_map(|device| {
            device
                .mount_points
                .iter()
                .filter(move |mount_point| current_dir.starts_with(mount_point))
                .map(move |mount_point| (device, mount_point.components().count()))
        })
        .max_by_key(|(_, depth)| *depth)
        .map(|(device, _)| device)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarNetworkConnectionAction {
    Connect,
    Disconnect,
    Edit,
    Remove,
}

impl SidebarNetworkConnectionAction {
    pub fn label(self) -> String {
        match self {
            Self::Connect => tr!("Connect"),
            Self::Disconnect => tr!("Disconnect"),
            Self::Edit => tr!("Edit"),
            Self::Remove => tr!("Remove"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SidebarNetworkConnectionEntry {
    pub connection: NetworkConnection,
    pub auto_connect: bool,
    pub state: NetworkMountState,
    /// 会话内凭据缓存（宿主进程私有语义：只在挂载流程中记忆，编辑远端
    /// 身份或挂载失败时由宿主状态机清理）。
    pub remembered_credentials: Option<NetworkMountCredentials>,
}

impl SidebarNetworkConnectionEntry {
    pub fn new(saved: SavedNetworkConnection) -> Self {
        Self {
            connection: saved.connection,
            auto_connect: saved.auto_connect,
            state: NetworkMountState::Disconnected,
            remembered_credentials: None,
        }
    }

    pub fn id(&self) -> &NetworkConnectionId {
        &self.connection.id
    }

    pub fn label(&self) -> String {
        self.connection.label_or_default()
    }

    pub fn mount_path(&self) -> Option<&Path> {
        match &self.state {
            NetworkMountState::Mounted(path) => Some(path.as_path()),
            _ => None,
        }
    }

    pub fn available_actions(&self) -> Vec<SidebarNetworkConnectionAction> {
        match &self.state {
            NetworkMountState::Mounted(_) => vec![
                SidebarNetworkConnectionAction::Disconnect,
                SidebarNetworkConnectionAction::Edit,
                SidebarNetworkConnectionAction::Remove,
            ],
            NetworkMountState::Disconnected | NetworkMountState::Error(_) => [
                SidebarNetworkConnectionAction::Connect,
                SidebarNetworkConnectionAction::Edit,
                SidebarNetworkConnectionAction::Remove,
            ]
            .to_vec(),
            NetworkMountState::Connecting => vec![
                SidebarNetworkConnectionAction::Edit,
                SidebarNetworkConnectionAction::Remove,
            ],
        }
    }
}

/// 路径是不是在网络位置上：已挂载的网络连接里，或标成远程文件系统的设备（gvfs 的
/// MTP/SMB/SFTP 等）里。读这种路径要走网络，整份读取（预览下载、波形、搜索遍历）前先问它。
/// 主程序和文件选择对话框共用这一条规则。
pub fn path_is_on_network(
    path: &Path,
    connections: &[SidebarNetworkConnectionEntry],
    devices: &[SidebarDeviceEntry],
) -> bool {
    connections.iter().any(|entry| {
        entry
            .mount_path()
            .is_some_and(|mount_path| path.starts_with(mount_path))
    }) || devices.iter().any(|device| {
        device.access == StorageDeviceAccess::RemoteFilesystem
            && device
                .mount_points
                .iter()
                .any(|mount_point| path.starts_with(mount_point))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_device_mounts_are_network_but_local_mounts_are_not() {
        let mut phone = device("phone", vec![PathBuf::from("/run/user/1000/gvfs/mtp")]);
        phone.access = StorageDeviceAccess::RemoteFilesystem;
        let disk = device("disk", vec![PathBuf::from("/run/media/user/disk")]);
        let devices = [phone, disk];

        assert!(path_is_on_network(
            Path::new("/run/user/1000/gvfs/mtp/DCIM"),
            &[],
            &devices
        ));
        assert!(!path_is_on_network(
            Path::new("/run/media/user/disk/song.flac"),
            &[],
            &devices
        ));
        assert!(!path_is_on_network(
            Path::new("/home/user/DCIM"),
            &[],
            &devices
        ));
    }

    fn device(id: &str, mount_points: Vec<PathBuf>) -> SidebarDeviceEntry {
        SidebarDeviceEntry {
            id: StorageDeviceId::new(id),
            label: id.to_owned(),
            detail: None,
            size_bytes: 0,
            mount_points,
            access: StorageDeviceAccess::LocalFilesystem,
            can_mount: true,
            can_unmount: true,
            removal: None,
        }
    }

    #[test]
    fn selected_device_uses_longest_matching_mount_prefix() {
        let devices = vec![
            device("outer", vec![PathBuf::from("/run/media/user")]),
            device("inner", vec![PathBuf::from("/run/media/user/photos")]),
        ];

        let selected = selected_sidebar_device(&devices, Path::new("/run/media/user/photos/raw"))
            .expect("selected device");

        assert_eq!(selected.id, StorageDeviceId::new("inner"));
    }

    #[test]
    fn unmounted_device_offers_mount_action() {
        let device = device("disk", Vec::new());

        assert_eq!(device.available_actions(), vec![SidebarDeviceAction::Mount]);
    }

    #[test]
    fn mounted_removable_device_offers_unmount_and_eject() {
        let mut device = device("disk", vec![PathBuf::from("/media/disk")]);
        device.removal = Some(StorageDeviceRemoval::Eject);

        assert_eq!(
            device.available_actions(),
            vec![SidebarDeviceAction::Unmount, SidebarDeviceAction::Eject]
        );
    }

    #[test]
    fn network_entry_mount_path_tracks_mount_state() {
        let connection = NetworkConnection::new(
            NetworkConnectionId::new("nas"),
            "nas",
            desktop_linux::NetworkProtocol::Smb,
            "smb://server/share",
        )
        .unwrap();
        let mut entry =
            SidebarNetworkConnectionEntry::new(SavedNetworkConnection::new(connection, false));

        assert!(entry.mount_path().is_none());
        entry.state = NetworkMountState::Mounted(PathBuf::from("/run/gvfs/server"));
        assert_eq!(entry.mount_path(), Some(Path::new("/run/gvfs/server")));
    }
}
