use std::path::{Path, PathBuf};

use desktop_linux::{
    StorageDevice, StorageDeviceAccess, StorageDeviceId, StorageDeviceProviderFailure,
    StorageDeviceRemoval, StorageDeviceSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarDeviceAction {
    Mount,
    Unmount,
    Eject,
}

impl SidebarDeviceAction {
    pub(crate) fn label(self, device: &SidebarDeviceEntry) -> &'static str {
        match self {
            Self::Mount => "Mount",
            Self::Unmount => "Unmount",
            Self::Eject if device.removal == Some(StorageDeviceRemoval::SafelyRemove) => {
                "Safely Remove"
            }
            Self::Eject => "Eject",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarDeviceEntry {
    pub(crate) id: StorageDeviceId,
    pub(crate) label: String,
    pub(crate) detail: Option<String>,
    pub(crate) size_bytes: u64,
    pub(crate) mount_points: Vec<PathBuf>,
    pub(crate) access: StorageDeviceAccess,
    pub(crate) can_mount: bool,
    pub(crate) can_unmount: bool,
    pub(crate) removal: Option<StorageDeviceRemoval>,
}

impl SidebarDeviceEntry {
    pub(crate) fn from_storage_device(device: StorageDevice) -> Self {
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

    pub(crate) fn primary_mount_path(&self) -> Option<&Path> {
        self.mount_points.first().map(PathBuf::as_path)
    }

    pub(crate) fn is_mounted(&self) -> bool {
        self.primary_mount_path().is_some()
    }

    pub(crate) fn available_actions(&self) -> Vec<SidebarDeviceAction> {
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

#[derive(Debug, Clone, Default)]
pub(crate) struct SidebarDeviceState {
    pub(crate) devices: Vec<SidebarDeviceEntry>,
    pub(crate) provider_failures: Vec<StorageDeviceProviderFailure>,
    pub(crate) is_loading: bool,
    pub(crate) pending_action: Option<SidebarDeviceActionRequest>,
    next_action_generation: u64,
}

impl SidebarDeviceState {
    pub(crate) fn loading() -> Self {
        Self {
            is_loading: true,
            ..Self::default()
        }
    }

    pub(crate) fn accept_loaded(&mut self, snapshot: StorageDeviceSnapshot) {
        self.devices = snapshot
            .devices
            .into_iter()
            .map(SidebarDeviceEntry::from_storage_device)
            .collect();
        self.provider_failures = snapshot.provider_failures;
        self.is_loading = false;
    }

    pub(crate) fn begin_refresh(&mut self) -> bool {
        if self.is_loading {
            false
        } else {
            self.is_loading = true;
            true
        }
    }

    pub(crate) fn device(&self, id: &StorageDeviceId) -> Option<&SidebarDeviceEntry> {
        self.devices.iter().find(|device| &device.id == id)
    }

    pub(crate) fn selected_device_id(&self, current_dir: &Path) -> Option<&StorageDeviceId> {
        selected_sidebar_device(&self.devices, current_dir).map(|device| &device.id)
    }

    pub(crate) fn path_is_remote_mount(&self, path: &Path) -> bool {
        self.devices.iter().any(|device| {
            device.access == StorageDeviceAccess::RemoteFilesystem
                && device
                    .mount_points
                    .iter()
                    .any(|mount_point| path.starts_with(mount_point))
        })
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

pub(crate) fn selected_sidebar_device<'a>(
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

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_linux::StorageDeviceMountState;

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
    fn portable_mount_is_remote_without_affecting_local_mounts() {
        let mut device = device("phone", vec![PathBuf::from("/run/user/1000/gvfs/mtp")]);
        device.access = StorageDeviceAccess::RemoteFilesystem;
        let state = SidebarDeviceState {
            devices: vec![device],
            ..SidebarDeviceState::default()
        };

        assert!(state.path_is_remote_mount(Path::new("/run/user/1000/gvfs/mtp/DCIM")));
        assert!(!state.path_is_remote_mount(Path::new("/home/user/DCIM")));
    }

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
