use std::path::PathBuf;

use desktop_linux::{
    eject_or_power_off_storage_device, load_storage_devices, mount_storage_device,
    unmount_storage_device, StorageDeviceId, StorageDeviceSnapshot,
};
use iced::Task;

use crate::model::Message;
use crate::sidebar_devices::{SidebarDeviceAction, SidebarDeviceActionRequest};

pub(crate) fn sidebar_devices_command() -> Task<Message> {
    Task::perform(load_sidebar_devices(), Message::SidebarDevicesLoaded)
}

pub(crate) fn sidebar_device_action_command(
    request: SidebarDeviceActionRequest,
    action: SidebarDeviceAction,
) -> Task<Message> {
    let task_request = request.clone();
    Task::perform(
        perform_sidebar_device_action(request.id.clone(), action),
        move |outcome| Message::SidebarDeviceActionFinished(task_request, action, outcome),
    )
}

async fn load_sidebar_devices() -> StorageDeviceSnapshot {
    load_storage_devices().await
}

async fn perform_sidebar_device_action(
    id: StorageDeviceId,
    action: SidebarDeviceAction,
) -> Result<Option<PathBuf>, String> {
    match action {
        SidebarDeviceAction::Mount => mount_storage_device(id)
            .await
            .map(Some)
            .map_err(|error| error.to_string()),
        SidebarDeviceAction::Unmount => unmount_storage_device(id)
            .await
            .map(|()| None)
            .map_err(|error| error.to_string()),
        SidebarDeviceAction::Eject => eject_or_power_off_storage_device(id)
            .await
            .map(|()| None)
            .map_err(|error| error.to_string()),
    }
}
