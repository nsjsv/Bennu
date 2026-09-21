use std::path::PathBuf;

use desktop_linux::{StorageDeviceId, StorageDeviceSnapshot};
use iced::Task;

use super::FileBrowser;
use crate::commands::{sidebar_device_action_command, sidebar_devices_command};
use crate::model::{BrowserPaneId, ContextMenuState, Message, NavigationMode};
use crate::sidebar_devices::{
    SidebarDeviceAction, SidebarDeviceActionRequest, SidebarDeviceContextMenuState,
};

impl FileBrowser {
    pub(crate) fn sidebar_device_is_selected(&self, id: &StorageDeviceId) -> bool {
        self.sidebar_devices
            .selected_device_id(&self.current_dir)
            .is_some_and(|selected_id| selected_id == id)
    }

    pub(super) fn accept_sidebar_devices(
        &mut self,
        result: StorageDeviceSnapshot,
    ) -> Task<Message> {
        self.sidebar_devices.accept_loaded(result);
        Task::none()
    }

    pub(super) fn refresh_sidebar_devices(&mut self) -> Task<Message> {
        if self.sidebar_devices.begin_refresh() {
            sidebar_devices_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn handle_sidebar_device_hovered(&mut self, id: StorageDeviceId) -> Task<Message> {
        self.hovered_sidebar_device = Some(id);
        Task::none()
    }

    pub(super) fn handle_sidebar_device_hover_cleared(
        &mut self,
        id: StorageDeviceId,
    ) -> Task<Message> {
        if self.hovered_sidebar_device.as_ref() == Some(&id) {
            self.hovered_sidebar_device = None;
        }
        Task::none()
    }

    pub(super) fn handle_sidebar_device_pressed(&mut self, id: StorageDeviceId) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        let Some(device) = self.sidebar_devices.device(&id).cloned() else {
            return rename_command;
        };
        self.context_menu = None;

        if let Some(path) = device.primary_mount_path().map(PathBuf::from) {
            return Task::batch([
                rename_command,
                self.navigate_to(path, NavigationMode::RecordHistory),
            ]);
        }
        if !device.can_mount {
            return rename_command;
        }

        let Some(request) = self.sidebar_devices.begin_action(id) else {
            return rename_command;
        };
        Task::batch([
            rename_command,
            sidebar_device_action_command(request, SidebarDeviceAction::Mount),
        ])
    }

    pub(super) fn handle_sidebar_device_middle_pressed(
        &mut self,
        pane_id: BrowserPaneId,
        id: StorageDeviceId,
    ) -> Task<Message> {
        self.activate_pane(pane_id);
        let rename_command = self.commit_rename_if_active();
        let Some(path) = self
            .sidebar_devices
            .device(&id)
            .and_then(|device| device.primary_mount_path().map(PathBuf::from))
        else {
            return rename_command;
        };

        Task::batch([rename_command, self.open_directory_from_middle_click(path)])
    }

    pub(super) fn handle_sidebar_device_right_clicked(
        &mut self,
        id: StorageDeviceId,
    ) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        let Some(device) = self.sidebar_devices.device(&id).cloned() else {
            return rename_command;
        };

        self.clear_preview();
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.selection_marquee = None;
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        let _ = self.cancel_address_editing();
        // 无可用动作的设备保留占位文案;有动作但全部被配置隐藏时不弹菜单。
        let device_has_actions = device.available_actions().is_empty();
        let actions_visible = !self
            .user_config
            .context_menus
            .sidebar_device_items(device.available_actions())
            .is_empty();
        if device_has_actions || actions_visible {
            self.context_menu = Some(ContextMenuState::SidebarDevice(
                SidebarDeviceContextMenuState {
                    device,
                    position: self.cursor_position,
                },
            ));
        }
        rename_command
    }

    pub(super) fn perform_sidebar_device_action(
        &mut self,
        id: StorageDeviceId,
        action: SidebarDeviceAction,
    ) -> Task<Message> {
        self.context_menu = None;
        let Some(request) = self.sidebar_devices.begin_action(id) else {
            return Task::none();
        };
        sidebar_device_action_command(request, action)
    }

    pub(super) fn accept_sidebar_device_action_finished(
        &mut self,
        request: SidebarDeviceActionRequest,
        action: SidebarDeviceAction,
        result: Result<Option<PathBuf>, String>,
    ) -> Task<Message> {
        let is_current = self.sidebar_devices.accept_action_finished(&request);
        let refresh_command = self.refresh_sidebar_devices();
        if !is_current {
            return refresh_command;
        }

        match result {
            Ok(Some(path)) if action == SidebarDeviceAction::Mount => {
                if self.sidebar_devices.device(&request.id).is_some() {
                    Task::batch([
                        refresh_command,
                        self.navigate_to(path, NavigationMode::RecordHistory),
                    ])
                } else {
                    refresh_command
                }
            }
            Ok(_) => refresh_command,
            Err(error) => {
                self.show_global_error(format!(
                    "Could not {} storage device: {error}",
                    sidebar_device_action_error_verb(action)
                ));
                refresh_command
            }
        }
    }
}
fn sidebar_device_action_error_verb(action: SidebarDeviceAction) -> &'static str {
    match action {
        SidebarDeviceAction::Mount => "mount",
        SidebarDeviceAction::Unmount => "unmount",
        SidebarDeviceAction::Eject => "eject",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use desktop_linux::{StorageDevice, StorageDeviceAccess, StorageDeviceMountState};

    #[test]
    fn file_browser_remote_mount_includes_portable_device_path() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser
            .sidebar_devices
            .accept_loaded(StorageDeviceSnapshot {
                devices: vec![StorageDevice {
                    id: StorageDeviceId::GvfsVolume("activation:mtp://phone/".to_owned()),
                    label: "Phone".to_owned(),
                    device_path: None,
                    filesystem_type: "GVfs".to_owned(),
                    size_bytes: 0,
                    mount_state: StorageDeviceMountState::Mounted(vec![PathBuf::from(
                        "/run/user/1000/gvfs/mtp:host=phone",
                    )]),
                    access: StorageDeviceAccess::RemoteFilesystem,
                    is_removable: true,
                    can_mount: true,
                    can_unmount: true,
                    can_eject: true,
                    can_power_off: false,
                    removal: Some(desktop_linux::StorageDeviceRemoval::Eject),
                }],
                provider_failures: Vec::new(),
            });

        assert!(browser.path_is_remote_mount(Path::new("/run/user/1000/gvfs/mtp:host=phone/DCIM")));
        assert!(!browser.path_is_remote_mount(Path::new("/home/user/DCIM")));
    }
}
