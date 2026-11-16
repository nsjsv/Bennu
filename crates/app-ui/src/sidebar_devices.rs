use std::path::Path;

use desktop_linux::{StorageDeviceId, StorageDeviceProviderFailure, StorageDeviceSnapshot};

// 纯搬移：SidebarDeviceEntry/SidebarDeviceAction（含 available_actions 与
// 最长前缀选中判定）已下沉 bennu-sidebar（portal 侧栏与主程序共用）；
// re-export 维持 crate::sidebar_devices::* 既有路径。
pub(crate) use bennu_sidebar::{selected_sidebar_device, SidebarDeviceAction, SidebarDeviceEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarDeviceActionRequest {
    pub(crate) id: StorageDeviceId,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarDeviceContextMenuState {
    pub(crate) device: SidebarDeviceEntry,
    pub(crate) position: iced::Point,
}

/// 设备列表只在有变化时重新加载，没有下一次轮询来补漏：加载途中又来的变化
/// 不能丢，只能记下来，等这次跑完立刻再跑一次。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum DeviceListLoad {
    #[default]
    Idle,
    Running,
    RunningStale,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SidebarDeviceState {
    pub(crate) devices: Vec<SidebarDeviceEntry>,
    pub(crate) provider_failures: Vec<StorageDeviceProviderFailure>,
    load: DeviceListLoad,
    pub(crate) pending_action: Option<SidebarDeviceActionRequest>,
    next_action_generation: u64,
}

impl SidebarDeviceState {
    pub(crate) fn loading() -> Self {
        Self {
            load: DeviceListLoad::Running,
            ..Self::default()
        }
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.load != DeviceListLoad::Idle
    }

    /// 返回 true 表示加载途中又有变化，调用方要马上再加载一次。
    pub(crate) fn accept_loaded(&mut self, snapshot: StorageDeviceSnapshot) -> bool {
        self.devices = snapshot
            .devices
            .into_iter()
            .map(SidebarDeviceEntry::from_storage_device)
            .collect();
        self.provider_failures = snapshot.provider_failures;
        let reload = self.load == DeviceListLoad::RunningStale;
        self.load = if reload {
            DeviceListLoad::Running
        } else {
            DeviceListLoad::Idle
        };
        reload
    }

    /// 返回 true 表示调用方要发起加载；已有加载在跑时只记下「跑完再来一次」。
    pub(crate) fn begin_refresh(&mut self) -> bool {
        match self.load {
            DeviceListLoad::Idle => {
                self.load = DeviceListLoad::Running;
                true
            }
            DeviceListLoad::Running | DeviceListLoad::RunningStale => {
                self.load = DeviceListLoad::RunningStale;
                false
            }
        }
    }

    pub(crate) fn device(&self, id: &StorageDeviceId) -> Option<&SidebarDeviceEntry> {
        self.devices.iter().find(|device| &device.id == id)
    }

    pub(crate) fn selected_device_id(&self, current_dir: &Path) -> Option<&StorageDeviceId> {
        selected_sidebar_device(&self.devices, current_dir).map(|device| &device.id)
    }

    pub(crate) fn begin_action(
        &mut self,
        id: StorageDeviceId,
    ) -> Option<SidebarDeviceActionRequest> {
        if self.pending_action.is_some() || self.device(&id).is_none() {
            return None;
        }

        self.next_action_generation = self.next_action_generation.wrapping_add(1);
        let request = SidebarDeviceActionRequest {
            id,
            generation: self.next_action_generation,
        };
        self.pending_action = Some(request.clone());
        Some(request)
    }

    pub(crate) fn accept_action_finished(&mut self, request: &SidebarDeviceActionRequest) -> bool {
        if self.pending_action.as_ref() != Some(request) {
            return false;
        }
        self.pending_action = None;
        true
    }

    pub(crate) fn is_action_pending(&self, id: &StorageDeviceId) -> bool {
        self.pending_action
            .as_ref()
            .is_some_and(|request| &request.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_linux::{StorageDevice, StorageDeviceAccess, StorageDeviceMountState};
    use std::path::PathBuf;

    #[test]
    fn stale_action_result_cannot_clear_new_request_for_same_device() {
        let mut state = SidebarDeviceState::default();
        state.accept_loaded(StorageDeviceSnapshot {
            devices: vec![StorageDevice {
                id: StorageDeviceId::new("disk"),
                label: "Disk".to_owned(),
                device_path: Some(PathBuf::from("/dev/sdb1")),
                filesystem_type: "vfat".to_owned(),
                size_bytes: 1,
                mount_state: StorageDeviceMountState::Unmounted,
                access: StorageDeviceAccess::LocalFilesystem,
                is_removable: true,
                can_mount: true,
                can_unmount: false,
                can_eject: false,
                can_power_off: false,
                removal: None,
            }],
            provider_failures: Vec::new(),
        });
        let first = state
            .begin_action(StorageDeviceId::new("disk"))
            .expect("first action request");
        assert!(state.accept_action_finished(&first));
        let second = state
            .begin_action(StorageDeviceId::new("disk"))
            .expect("second action request");

        assert!(!state.accept_action_finished(&first));
        assert_eq!(state.pending_action, Some(second));
    }
    #[test]
    fn change_during_a_load_triggers_exactly_one_more_load() {
        let mut state = SidebarDeviceState::loading();
        assert!(!state.begin_refresh());
        assert!(!state.begin_refresh());

        assert!(state.accept_loaded(StorageDeviceSnapshot::default()));
        assert!(state.is_loading());
        assert!(!state.accept_loaded(StorageDeviceSnapshot::default()));
        assert!(!state.is_loading());
        assert!(state.begin_refresh());
    }

    #[test]
    fn partial_provider_failure_keeps_devices_and_failure() {
        let storage = StorageDeviceSnapshot {
            devices: vec![StorageDevice {
                id: StorageDeviceId::new("disk"),
                label: "Disk".to_owned(),
                device_path: Some(PathBuf::from("/dev/sdb1")),
                filesystem_type: "vfat".to_owned(),
                size_bytes: 8,
                mount_state: StorageDeviceMountState::Unmounted,
                access: StorageDeviceAccess::LocalFilesystem,
                is_removable: true,
                can_mount: true,
                can_unmount: false,
                can_eject: false,
                can_power_off: false,
                removal: None,
            }],
            provider_failures: vec![StorageDeviceProviderFailure {
                provider: desktop_linux::StorageDeviceProvider::Gvfs,
                message: "gvfs backend unavailable".to_owned(),
            }],
        };
        let mut state = SidebarDeviceState::default();

        state.accept_loaded(storage);

        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.provider_failures.len(), 1);
    }
}
