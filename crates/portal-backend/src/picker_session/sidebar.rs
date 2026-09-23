//! 侧边栏状态机与数据加载（design.md 第 3 节）。纯逻辑 + 桌面平台层
//! 调用，不 import iced/zbus 运行时类型；挂载/数据加载效果以
//! `SessionEffect` 输出由 main 层翻译执行。
//!
//! 数据时机与主程序一致：每次窗口打开读一次（`LoadSidebarData`），
//! 打开期间主程序改收藏/连接不影响已开窗口。宽度当次窗口有效，
//! 默认 180、clamp 140–360（bennu-sidebar 共享常量），不持久化。

use std::collections::HashSet;
use std::path::PathBuf;

use bennu_sidebar::{
    normalize_sidebar_width, read_sidebar_config, sidebar_locations, SidebarDeviceEntry,
    SidebarLocation, SidebarNetworkConnectionEntry, DEFAULT_SIDEBAR_WIDTH,
};
use desktop_linux::{
    load_network_mount_states, load_storage_devices, NetworkConnection, NetworkConnectionId,
    NetworkMountState, StorageDeviceId, StorageDeviceProviderFailure,
};

use super::{PickerSession, SessionEffect};

/// 回收站虚拟视图的当前目录标识（与主程序 `trash_location_path` 同值）。
pub(crate) const TRASH_DIRECTORY: &str = "trash:///";

/// 侧边栏行悬停身份（驱动 hovered 行样式，对齐主程序悬停反馈）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SidebarEntryId {
    Location(PathBuf),
    Trash,
    Device(desktop_linux::StorageDeviceId),
    Connection(desktop_linux::NetworkConnectionId),
}

/// PickerSession 的侧边栏子状态。
pub(crate) struct PickerSidebarState {
    pub(crate) locations: Vec<SidebarLocation>,
    pub(crate) connections: Vec<SidebarNetworkConnectionEntry>,
    pub(crate) devices: Vec<SidebarDeviceEntry>,
    pub(crate) provider_failures: Vec<StorageDeviceProviderFailure>,
    /// 挂载进行中的设备（防重入：busy 期间同条目点击忽略）。
    mounting_devices: HashSet<StorageDeviceId>,
    pub(crate) width: f32,
    /// 悬停行（None = 无悬停）。
    pub(crate) hovered: Option<SidebarEntryId>,
    /// 最近一次失败提示（挂载失败等）；下次成功操作清除。
    pub(crate) notice: Option<String>,
}

impl Default for PickerSidebarState {
    fn default() -> Self {
        Self {
            locations: Vec::new(),
            connections: Vec::new(),
            devices: Vec::new(),
            provider_failures: Vec::new(),
            mounting_devices: HashSet::new(),
            width: DEFAULT_SIDEBAR_WIDTH,
            hovered: None,
            notice: None,
        }
    }
}

/// 窗口打开时一次性读取的侧边栏数据载荷（main 层 Task::perform 回填）。
#[derive(Debug, Clone)]
pub(crate) struct PickerSidebarData {
    pub(crate) locations: Vec<SidebarLocation>,
    pub(crate) connections: Vec<SidebarNetworkConnectionEntry>,
    pub(crate) devices: Vec<SidebarDeviceEntry>,
    pub(crate) provider_failures: Vec<StorageDeviceProviderFailure>,
}

/// 异步读取侧边栏数据：config.toml（收藏 + 保存的网络连接）→ 位置/收藏
/// 计算 → 设备快照 + 网络挂载状态批量查询。任何一步失败都降级为空集
/// （portal 常驻进程不能因配置/系统异常 panic），与主程序启动读一次的
/// 容忍语义一致。
pub(crate) async fn load_sidebar_data() -> PickerSidebarData {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let config = read_sidebar_config();
    let locations = sidebar_locations(&home, config.favorites.as_deref());

    let mut connections: Vec<SidebarNetworkConnectionEntry> = config
        .network_connections
        .into_iter()
        .map(SidebarNetworkConnectionEntry::new)
        .collect();
    let mount_states = load_network_mount_states(
        connections
            .iter()
            .map(|entry| entry.connection.clone())
            .collect(),
    )
    .await
    .unwrap_or_default();
    for entry in &mut connections {
        if let Some((_, state)) = mount_states
            .iter()
            .find(|(id, _)| id == &entry.connection.id)
        {
            entry.state = state.clone();
        }
    }

    let snapshot = load_storage_devices().await;
    PickerSidebarData {
        locations,
        connections,
        devices: snapshot
            .devices
            .into_iter()
            .map(SidebarDeviceEntry::from_storage_device)
            .collect(),
        provider_failures: snapshot.provider_failures,
    }
}

impl PickerSession {
    /// 回收站虚拟视图：当前目录为 `trash:///`。
    pub(crate) fn is_trash_view(&self) -> bool {
        self.directory.as_os_str() == TRASH_DIRECTORY
    }

    pub(crate) fn sidebar(&self) -> &PickerSidebarState {
        &self.sidebar
    }

    pub(crate) fn sidebar_width(&self) -> f32 {
        self.sidebar.width
    }

    /// 拖宽写入收口：宽度永远经过共享 clamp（140–360），非有限值回落默认。
    pub(crate) fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar.width = normalize_sidebar_width(width);
    }

    /// 设备挂载 busy 态（视图行内「处理中...」与防重入同一来源）。
    pub(crate) fn sidebar_device_is_mounting(&self, id: &StorageDeviceId) -> bool {
        self.sidebar.mounting_devices.contains(id)
    }

    /// 挂载中的连接凭证：main 层翻译 `MountConnection` 时取完整连接
    /// 身份做密钥环查找与挂载（先查主程序已存凭证，查不到按无 app
    /// 凭证挂载——选择器没有凭据表单，失败只提示）。
    pub(crate) fn sidebar_connection(&self, id: &NetworkConnectionId) -> Option<NetworkConnection> {
        self.sidebar
            .connections
            .iter()
            .find(|entry| entry.id() == id)
            .map(|entry| entry.connection.clone())
    }

    pub(crate) fn accept_sidebar_data(&mut self, data: PickerSidebarData) -> SessionEffect {
        self.sidebar.locations = data.locations;
        self.sidebar.connections = data.connections;
        self.sidebar.devices = data.devices;
        self.sidebar.provider_failures = data.provider_failures;
        // 侧栏内容高度骤变：核实滚动条溢出。
        SessionEffect::VerifyScrollbarLayout
    }

    pub(crate) fn sidebar_location_pressed(&mut self, path: PathBuf) -> SessionEffect {
        self.begin_navigation(path)
    }

    pub(crate) fn sidebar_trash_pressed(&mut self) -> SessionEffect {
        self.begin_navigation(PathBuf::from(TRASH_DIRECTORY))
    }

    /// 已挂载设备点击导航挂载点；未挂载且可挂载则置 busy 并发起挂载
    /// 效果；busy 期间同条目点击忽略（防重入）。
    pub(crate) fn sidebar_device_pressed(&mut self, id: StorageDeviceId) -> SessionEffect {
        let Some(device) = self.sidebar.devices.iter().find(|device| device.id == id) else {
            return SessionEffect::None;
        };
        if let Some(mount_path) = device.primary_mount_path() {
            return self.begin_navigation(mount_path.to_path_buf());
        }
        if !device.can_mount || self.sidebar.mounting_devices.contains(&id) {
            return SessionEffect::None;
        }
        self.sidebar.mounting_devices.insert(id.clone());
        SessionEffect::MountDevice(id)
    }

    /// 挂载回信：成功则刷新该条目挂载状态并导航（清除提示），失败只
    /// 在侧栏底部提示。数据重载后条目已消失的迟到回信直接丢弃。
    pub(crate) fn sidebar_device_mount_finished(
        &mut self,
        id: StorageDeviceId,
        outcome: Result<PathBuf, String>,
    ) -> SessionEffect {
        self.sidebar.mounting_devices.remove(&id);
        let Some(device) = self
            .sidebar
            .devices
            .iter_mut()
            .find(|device| device.id == id)
        else {
            return SessionEffect::None;
        };
        match outcome {
            Ok(mount_path) => {
                device.mount_points = vec![mount_path.clone()];
                self.sidebar.notice = None;
                self.begin_navigation(mount_path)
            }
            Err(details) => {
                self.sidebar.notice = Some(details);
                SessionEffect::None
            }
        }
    }

    pub(crate) fn sidebar_connection_pressed(&mut self, id: NetworkConnectionId) -> SessionEffect {
        let Some(entry) = self
            .sidebar
            .connections
            .iter_mut()
            .find(|entry| entry.id() == &id)
        else {
            return SessionEffect::None;
        };
        if let NetworkMountState::Mounted(mount_path) = &entry.state {
            let mount_path = mount_path.clone();
            return self.begin_navigation(mount_path);
        }
        if matches!(entry.state, NetworkMountState::Connecting) {
            return SessionEffect::None;
        }
        entry.state = NetworkMountState::Connecting;
        SessionEffect::MountConnection(id)
    }

    /// 连接挂载回信：成功置 Mounted 并导航；失败回落未连接 + 侧栏提示。
    pub(crate) fn sidebar_connection_mount_finished(
        &mut self,
        id: NetworkConnectionId,
        outcome: Result<PathBuf, String>,
    ) -> SessionEffect {
        let Some(entry) = self
            .sidebar
            .connections
            .iter_mut()
            .find(|entry| entry.id() == &id)
        else {
            return SessionEffect::None;
        };
        if !matches!(entry.state, NetworkMountState::Connecting) {
            return SessionEffect::None;
        }
        match outcome {
            Ok(mount_path) => {
                entry.state = NetworkMountState::Mounted(mount_path.clone());
                self.sidebar.notice = None;
                self.begin_navigation(mount_path)
            }
            Err(details) => {
                entry.state = NetworkMountState::Disconnected;
                self.sidebar.notice = Some(details);
                SessionEffect::None
            }
        }
    }

    pub(crate) fn sidebar_hover_changed(&mut self, entry: Option<SidebarEntryId>) {
        self.sidebar.hovered = entry;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker_request::PickerKind;
    use crate::picker_session::tests::session;
    use bennu_sidebar::{MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH};

    #[test]
    fn resize_clamps_into_shared_bounds() {
        let (mut session, _receiver) = session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        session.set_sidebar_width(-1000.0);
        assert_eq!(session.sidebar_width(), MIN_SIDEBAR_WIDTH);
        session.set_sidebar_width(10000.0);
        assert_eq!(session.sidebar_width(), MAX_SIDEBAR_WIDTH);
        session.set_sidebar_width(f32::NAN);
        assert_eq!(session.sidebar_width(), DEFAULT_SIDEBAR_WIDTH);
    }
}
