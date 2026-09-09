use std::collections::HashMap;
use std::convert::Infallible;
use std::ffi::OsString;
use std::fmt;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::gvfs_devices;
use crate::storage_device_visibility::{
    udisks_device_is_visible, DesktopUserContext, UdisksFstabEntry, UdisksVisibilityFacts,
};
use thiserror::Error;
use udisks2::zbus::names::OwnedInterfaceName;
use udisks2::zbus::zvariant::{OwnedObjectPath, OwnedValue};
use udisks2::{standard_options, Client};

const FILESYSTEM_INTERFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const LOOP_INTERFACE: &str = "org.freedesktop.UDisks2.Loop";

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StorageDeviceId {
    UdisksObject(String),
    GvfsVolume(String),
}

impl StorageDeviceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self::UdisksObject(value.into())
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::UdisksObject(value) | Self::GvfsVolume(value) => value,
        }
    }
}

impl fmt::Display for StorageDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UdisksObject(value) => write!(formatter, "udisks:{value}"),
            Self::GvfsVolume(value) => write!(formatter, "gvfs:{value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageDeviceAccess {
    LocalFilesystem,
    RemoteFilesystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageDeviceRemoval {
    Eject,
    SafelyRemove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageDeviceMountState {
    Mounted(Vec<PathBuf>),
    Unmounted,
}

impl StorageDeviceMountState {
    pub fn mount_points(&self) -> &[PathBuf] {
        match self {
            Self::Mounted(paths) => paths,
            Self::Unmounted => &[],
        }
    }

    pub fn primary_mount_path(&self) -> Option<&Path> {
        self.mount_points().first().map(PathBuf::as_path)
    }

    pub fn is_mounted(&self) -> bool {
        matches!(self, Self::Mounted(paths) if !paths.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageDevice {
    pub id: StorageDeviceId,
    pub label: String,
    pub device_path: Option<PathBuf>,
    pub filesystem_type: String,
    pub size_bytes: u64,
    pub mount_state: StorageDeviceMountState,
    pub access: StorageDeviceAccess,
    pub is_removable: bool,
    pub can_mount: bool,
    pub can_unmount: bool,
    pub can_eject: bool,
    pub can_power_off: bool,
    pub removal: Option<StorageDeviceRemoval>,
}

impl StorageDevice {
    pub fn is_mounted(&self) -> bool {
        self.mount_state.is_mounted()
    }

    pub fn primary_mount_path(&self) -> Option<&Path> {
        self.mount_state.primary_mount_path()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageDeviceProvider {
    Udisks,
    Gvfs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageDeviceProviderFailure {
    pub provider: StorageDeviceProvider,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StorageDeviceSnapshot {
    pub devices: Vec<StorageDevice>,
    pub provider_failures: Vec<StorageDeviceProviderFailure>,
}

#[derive(Debug, Error)]
pub enum StorageDeviceError {
    #[error("UDisks2 error: {0}")]
    UDisks(#[from] udisks2::Error),
    #[error("D-Bus error: {0}")]
    Dbus(#[from] udisks2::zbus::Error),
    #[error("D-Bus object manager error: {0}")]
    Fdo(#[from] udisks2::zbus::fdo::Error),
    #[error("D-Bus object path error: {0}")]
    ObjectPath(#[from] udisks2::zbus::zvariant::Error),
    #[error("GVfs error: {0}")]
    Gvfs(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("storage device action is unavailable: {0}")]
    ActionUnavailable(&'static str),
}

impl From<Infallible> for StorageDeviceError {
    fn from(value: Infallible) -> Self {
        match value {}
    }
}

pub async fn load_storage_devices() -> StorageDeviceSnapshot {
    let (udisks_result, gvfs_result) = tokio::join!(
        load_udisks_storage_devices(),
        gvfs_devices::load_gvfs_storage_devices(),
    );
    merge_storage_device_results(
        udisks_result.map_err(|error| error.to_string()),
        gvfs_result.map_err(|error| error.to_string()),
    )
}

fn merge_storage_device_results(
    udisks_result: Result<Vec<StorageDevice>, String>,
    gvfs_result: Result<Vec<StorageDevice>, String>,
) -> StorageDeviceSnapshot {
    let mut snapshot = StorageDeviceSnapshot::default();
    match udisks_result {
        Ok(devices) => snapshot.devices.extend(devices),
        Err(error) => snapshot
            .provider_failures
            .push(StorageDeviceProviderFailure {
                provider: StorageDeviceProvider::Udisks,
                message: error,
            }),
    }
    match gvfs_result {
        Ok(devices) => snapshot.devices.extend(devices),
        Err(error) => snapshot
            .provider_failures
            .push(StorageDeviceProviderFailure {
                provider: StorageDeviceProvider::Gvfs,
                message: error,
            }),
    }
    snapshot.devices.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    snapshot
}

async fn load_udisks_storage_devices() -> Result<Vec<StorageDevice>, StorageDeviceError> {
    let client = Client::new().await?;
    let managed_objects = client.object_manager().get_managed_objects().await?;
    let user = DesktopUserContext::from_process_environment();
    let mut devices = Vec::new();

    for (object_path, interfaces) in managed_objects {
        if !has_interface(&interfaces, FILESYSTEM_INTERFACE) {
            continue;
        }
        let is_loop_device = has_interface(&interfaces, LOOP_INTERFACE);
        let object = client.object(object_path.clone())?;
        if let Some(device) =
            storage_device_from_object(&client, object_path, object, is_loop_device, &user).await?
        {
            devices.push(device);
        }
    }

    devices.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(devices)
}

pub async fn mount_storage_device(id: StorageDeviceId) -> Result<PathBuf, StorageDeviceError> {
    match id {
        StorageDeviceId::GvfsVolume(identity) => gvfs_devices::mount_gvfs_storage_device(identity)
            .await
            .map_err(|error| StorageDeviceError::Gvfs(Box::new(error))),
        StorageDeviceId::UdisksObject(object_path) => {
            let client = Client::new().await?;
            let object = client.object(object_path)?;
            let filesystem = object.filesystem().await?;
            let mount_path = filesystem.mount(standard_options(false)).await?;
            Ok(PathBuf::from(mount_path))
        }
    }
}

pub async fn unmount_storage_device(id: StorageDeviceId) -> Result<(), StorageDeviceError> {
    match id {
        StorageDeviceId::GvfsVolume(identity) => {
            gvfs_devices::unmount_gvfs_storage_device(identity)
                .await
                .map_err(|error| StorageDeviceError::Gvfs(Box::new(error)))
        }
        StorageDeviceId::UdisksObject(object_path) => {
            let client = Client::new().await?;
            let object = client.object(object_path)?;
            let filesystem = object.filesystem().await?;
            filesystem.unmount(standard_options(false)).await?;
            Ok(())
        }
    }
}

pub async fn eject_or_power_off_storage_device(
    id: StorageDeviceId,
) -> Result<(), StorageDeviceError> {
    match id {
        StorageDeviceId::GvfsVolume(identity) => gvfs_devices::remove_gvfs_storage_device(identity)
            .await
            .map_err(|error| StorageDeviceError::Gvfs(Box::new(error))),
        StorageDeviceId::UdisksObject(object_path) => {
            let client = Client::new().await?;
            let object = client.object(object_path)?;
            let filesystem = object.filesystem().await?;
            let block = object.block().await?;
            let drive = client.drive_for_block(&block).await?;

            if !filesystem.mount_points().await?.is_empty() {
                match filesystem.unmount(standard_options(false)).await {
                    Ok(()) | Err(udisks2::Error::NotMounted) => {}
                    Err(error) => return Err(error.into()),
                }
            }

            if drive.can_power_off().await.unwrap_or(false) {
                drive.power_off(standard_options(false)).await?;
                return Ok(());
            }
            if drive.ejectable().await.unwrap_or(false) {
                drive.eject(standard_options(false)).await?;
                return Ok(());
            }

            Err(StorageDeviceError::ActionUnavailable(
                "device cannot be ejected or safely removed",
            ))
        }
    }
}

async fn storage_device_from_object(
    client: &Client,
    object_path: OwnedObjectPath,
    object: udisks2::Object,
    is_loop_device: bool,
    user: &DesktopUserContext,
) -> Result<Option<StorageDevice>, StorageDeviceError> {
    let block = object.block().await?;
    let filesystem = object.filesystem().await?;
    let mount_points = filesystem
        .mount_points()
        .await?
        .into_iter()
        .filter_map(path_from_udisks_bytes)
        .collect::<Vec<_>>();
    let facts = UdisksVisibilityFacts {
        hint_ignore: block.hint_ignore().await.unwrap_or(false),
        fstab_entries: if mount_points.is_empty() {
            unmounted_udisks_fstab_entries(&block).await
        } else {
            Vec::new()
        },
        mount_points: mount_points.clone(),
        loop_setup_by_uid: if is_loop_device {
            match object.r#loop().await {
                Ok(loop_device) => loop_device.setup_by_uid().await.ok(),
                Err(_) => None,
            }
        } else {
            None
        },
    };
    if !udisks_device_is_visible(&facts, user) {
        return Ok(None);
    }

    let drive = client.drive_for_block(&block).await.ok();
    let is_removable = if let Some(drive) = drive.as_ref() {
        drive.removable().await.unwrap_or(false)
            || drive.media_removable().await.unwrap_or(false)
            || matches!(
                drive.connection_bus().await.as_deref(),
                Ok("usb") | Ok("firewire")
            )
    } else {
        false
    };
    let device_path = match block
        .preferred_device()
        .await
        .ok()
        .and_then(path_from_udisks_bytes)
    {
        Some(path) => Some(path),
        None => block.device().await.ok().and_then(path_from_udisks_bytes),
    };
    let filesystem_type = block.id_type().await.unwrap_or_default();
    let size_bytes = match block.size().await {
        Ok(size) => size,
        Err(_) => filesystem.size().await.unwrap_or(0),
    };
    let (can_eject, can_power_off, drive_label) = if let Some(drive) = drive.as_ref() {
        (
            drive.ejectable().await.unwrap_or(false),
            drive.can_power_off().await.unwrap_or(false),
            Some(DriveLabelParts {
                vendor: drive.vendor().await.unwrap_or_default(),
                model: drive.model().await.unwrap_or_default(),
            }),
        )
    } else {
        (false, false, None)
    };
    let label = storage_device_label(
        block.hint_name().await.unwrap_or_default(),
        block.id_label().await.unwrap_or_default(),
        drive_label,
        device_path.as_deref(),
        &filesystem_type,
    );
    let mount_state = if mount_points.is_empty() {
        StorageDeviceMountState::Unmounted
    } else {
        StorageDeviceMountState::Mounted(mount_points)
    };

    let can_unmount = mount_state.is_mounted();
    Ok(Some(StorageDevice {
        id: StorageDeviceId::UdisksObject(object_path.to_string()),
        label,
        device_path,
        filesystem_type,
        size_bytes,
        mount_state,
        access: StorageDeviceAccess::LocalFilesystem,
        is_removable,
        can_mount: true,
        can_unmount,
        can_eject,
        can_power_off,
        removal: if can_power_off {
            Some(StorageDeviceRemoval::SafelyRemove)
        } else if can_eject {
            Some(StorageDeviceRemoval::Eject)
        } else {
            None
        },
    }))
}

/// fstab rows tracked by UDisks only decide visibility while the volume is
/// unmounted, mirroring GVfs `should_include_volume_check_configuration()`.
async fn unmounted_udisks_fstab_entries(
    block: &udisks2::block::BlockProxy<'_>,
) -> Vec<UdisksFstabEntry> {
    let Ok(configuration) = block.configuration().await else {
        return Vec::new();
    };
    configuration
        .iter()
        .filter(|(entry_type, _)| entry_type == "fstab")
        .filter_map(|(_, values)| fstab_entry_from_udisks_values(values))
        .collect()
}

fn fstab_entry_from_udisks_values(
    values: &HashMap<String, OwnedValue>,
) -> Option<UdisksFstabEntry> {
    let dir = path_from_udisks_bytes(udisks_bytestring_value(values.get("dir")?)?)?;
    let options = values
        .get("opts")
        .and_then(udisks_bytestring_value)
        .map(|bytes| String::from_utf8_lossy(&trim_nul_bytes(bytes)).into_owned())
        .unwrap_or_default();
    let visibility_override = if options.split(',').any(|option| option == "x-gvfs-show") {
        Some(true)
    } else if options.split(',').any(|option| option == "x-gvfs-hide") {
        Some(false)
    } else {
        None
    };
    Some(UdisksFstabEntry {
        dir,
        visibility_override,
    })
}

/// UDisks encodes `fstab` fields as `ay` bytestrings, which zvariant surfaces
/// as an array of `u8` values.
fn udisks_bytestring_value(value: &OwnedValue) -> Option<Vec<u8>> {
    let array = value
        .downcast_ref::<udisks2::zbus::zvariant::Array>()
        .ok()?;
    array
        .iter()
        .map(|item| item.downcast_ref::<u8>())
        .collect::<Result<Vec<u8>, _>>()
        .ok()
}

fn has_interface(
    interfaces: &HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
    expected: &str,
) -> bool {
    interfaces.keys().any(|name| name.as_str() == expected)
}

struct DriveLabelParts {
    vendor: String,
    model: String,
}

fn storage_device_label(
    hint_name: String,
    id_label: String,
    drive: Option<DriveLabelParts>,
    device_path: Option<&Path>,
    filesystem_type: &str,
) -> String {
    first_non_empty([
        hint_name,
        id_label,
        drive
            .map(|drive| [drive.vendor, drive.model].join(" "))
            .unwrap_or_default(),
        device_path
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        filesystem_type.to_owned(),
    ])
    .unwrap_or_else(|| "Storage Device".to_owned())
}

fn first_non_empty(values: impl IntoIterator<Item = String>) -> Option<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
}

fn path_from_udisks_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    let bytes = trim_nul_bytes(bytes);
    if bytes.is_empty() {
        None
    } else {
        Some(PathBuf::from(OsString::from_vec(bytes)))
    }
}

fn trim_nul_bytes(mut bytes: Vec<u8>) -> Vec<u8> {
    if let Some(index) = bytes.iter().position(|byte| *byte == 0) {
        bytes.truncate(index);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udisks_byte_paths_ignore_trailing_nul() {
        let path = path_from_udisks_bytes(b"/run/media/user/DISK\0ignored".to_vec());

        assert_eq!(path, Some(PathBuf::from("/run/media/user/DISK")));
    }

    #[test]
    fn label_prefers_hint_then_filesystem_label_then_drive_then_path() {
        let label = storage_device_label(
            "".to_owned(),
            "Photos".to_owned(),
            Some(DriveLabelParts {
                vendor: "Vendor".to_owned(),
                model: "Model".to_owned(),
            }),
            Some(Path::new("/dev/sdb1")),
            "vfat",
        );

        assert_eq!(label, "Photos");
    }

    #[test]
    fn label_falls_back_to_device_path() {
        let label = storage_device_label(
            String::new(),
            String::new(),
            None,
            Some(Path::new("/dev/sdb1")),
            "vfat",
        );

        assert_eq!(label, "sdb1");
    }

    #[test]
    fn provider_merge_keeps_both_successful_sources() {
        let local = StorageDevice {
            id: StorageDeviceId::UdisksObject("disk".to_owned()),
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
        };
        let portable = StorageDevice {
            id: StorageDeviceId::GvfsVolume("activation:mtp://phone/".to_owned()),
            label: "Phone".to_owned(),
            device_path: None,
            filesystem_type: "GVfs".to_owned(),
            size_bytes: 0,
            mount_state: StorageDeviceMountState::Unmounted,
            access: StorageDeviceAccess::RemoteFilesystem,
            is_removable: true,
            can_mount: true,
            can_unmount: false,
            can_eject: true,
            can_power_off: false,
            removal: Some(StorageDeviceRemoval::Eject),
        };

        let snapshot = merge_storage_device_results(Ok(vec![local]), Ok(vec![portable]));

        assert_eq!(snapshot.devices.len(), 2);
        assert!(snapshot.provider_failures.is_empty());
    }

    #[test]
    fn provider_merge_keeps_successful_devices_when_other_provider_fails() {
        let device = StorageDevice {
            id: StorageDeviceId::UdisksObject("disk".to_owned()),
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
        };
        let snapshot = merge_storage_device_results(
            Ok(vec![device]),
            Err("GVfs backend unavailable".to_owned()),
        );

        assert_eq!(snapshot.devices.len(), 1);
        assert_eq!(snapshot.provider_failures.len(), 1);
        assert_eq!(
            snapshot.provider_failures[0].provider,
            StorageDeviceProvider::Gvfs
        );
    }

    #[test]
    fn provider_merge_keeps_both_provider_failures() {
        let snapshot = merge_storage_device_results(
            Err("UDisks unavailable".to_owned()),
            Err("GVfs unavailable".to_owned()),
        );

        assert!(snapshot.devices.is_empty());
        assert_eq!(snapshot.provider_failures.len(), 2);
    }
}
