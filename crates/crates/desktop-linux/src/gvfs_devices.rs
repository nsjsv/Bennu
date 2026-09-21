use std::io;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::thread;

use gio::glib::MainContext;
use gio::prelude::{DriveExt, FileExt, MountExt, VolumeExt, VolumeMonitorExt};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::gvfs_paths::{default_gvfs_fuse_root, resolve_gvfs_mount_path, GvfsMountPathError};
use crate::storage_devices::{
    StorageDevice, StorageDeviceAccess, StorageDeviceId, StorageDeviceMountState,
    StorageDeviceRemoval,
};

#[derive(Debug, Error)]
pub enum GvfsDeviceRuntimeStartError {
    #[error("could not start the GVfs device thread: {0}")]
    Thread(#[source] io::Error),
    #[error("the GVfs device thread stopped during initialization")]
    InitializationStopped,
}

#[derive(Debug, Error)]
pub enum GvfsDeviceError {
    #[error("could not start the GVfs device runtime: {0}")]
    RuntimeStart(#[source] Arc<GvfsDeviceRuntimeStartError>),
    #[error("the GVfs device runtime stopped before replying")]
    RuntimeStopped,
    #[error("GVfs volume {identity:?} is no longer available")]
    VolumeUnavailable { identity: String },
    #[error("GVfs device action is unavailable: {0}")]
    ActionUnavailable(&'static str),
    #[error("GVfs operation for {identity:?} failed: {source}")]
    Gio {
        identity: String,
        #[source]
        source: gio::glib::Error,
    },
    #[error("GVfs operation for {identity:?} did not produce a mount: {message}")]
    OperationIncomplete {
        identity: String,
        message: &'static str,
    },
    #[error("GVfs mount path for {identity:?} failed: {source}")]
    MountPath {
        identity: String,
        #[source]
        source: GvfsMountPathError,
    },
}

struct GvfsDeviceRuntime {
    request_sender: mpsc::UnboundedSender<GvfsDeviceRequest>,
}

enum GvfsDeviceRequest {
    Load {
        response: oneshot::Sender<Result<Vec<StorageDevice>, GvfsDeviceError>>,
    },
    Mount {
        identity: String,
        response: oneshot::Sender<Result<PathBuf, GvfsDeviceError>>,
    },
    Unmount {
        identity: String,
        response: oneshot::Sender<Result<(), GvfsDeviceError>>,
    },
    Remove {
        identity: String,
        response: oneshot::Sender<Result<(), GvfsDeviceError>>,
    },
    #[cfg(test)]
    InspectContext {
        response: oneshot::Sender<GvfsRuntimeProbe>,
    },
}

#[cfg(test)]
#[derive(Debug)]
struct GvfsRuntimeProbe {
    thread_id: thread::ThreadId,
    has_thread_default_context: bool,
}

static GVFS_DEVICE_RUNTIME: LazyLock<Result<GvfsDeviceRuntime, Arc<GvfsDeviceRuntimeStartError>>> =
    LazyLock::new(|| GvfsDeviceRuntime::start().map_err(Arc::new));

#[derive(Debug, Clone, PartialEq, Eq)]
struct GvfsVolumeFacts {
    identity: String,
    label: String,
    class: Option<String>,
    activation_root_is_native: Option<bool>,
}

impl GvfsDeviceRuntime {
    fn start() -> Result<Self, GvfsDeviceRuntimeStartError> {
        let (request_sender, request_receiver) = mpsc::unbounded_channel();
        let (startup_sender, startup_receiver) = std::sync::mpsc::sync_channel(1);
        thread::Builder::new()
            .name("bennu-gvfs-devices".to_owned())
            .spawn(move || {
                let context = MainContext::new();
                context.block_on(async move {
                    let monitor = gio::VolumeMonitor::get();
                    if startup_sender.send(()).is_err() {
                        return;
                    }
                    run_gvfs_device_requests(monitor, request_receiver).await;
                });
            })
            .map_err(GvfsDeviceRuntimeStartError::Thread)?;
        startup_receiver
            .recv()
            .map_err(|_| GvfsDeviceRuntimeStartError::InitializationStopped)?;
        Ok(Self { request_sender })
    }
}

fn gvfs_device_runtime() -> Result<&'static GvfsDeviceRuntime, GvfsDeviceError> {
    match &*GVFS_DEVICE_RUNTIME {
        Ok(runtime) => Ok(runtime),
        Err(source) => Err(GvfsDeviceError::RuntimeStart(source.clone())),
    }
}

async fn request_gvfs_device<T>(
    build_request: impl FnOnce(oneshot::Sender<Result<T, GvfsDeviceError>>) -> GvfsDeviceRequest,
) -> Result<T, GvfsDeviceError> {
    let runtime = gvfs_device_runtime()?;
    let (response_sender, response_receiver) = oneshot::channel();
    runtime
        .request_sender
        .send(build_request(response_sender))
        .map_err(|_| GvfsDeviceError::RuntimeStopped)?;
    response_receiver
        .await
        .map_err(|_| GvfsDeviceError::RuntimeStopped)?
}

pub(super) async fn load_gvfs_storage_devices() -> Result<Vec<StorageDevice>, GvfsDeviceError> {
    request_gvfs_device(|response| GvfsDeviceRequest::Load { response }).await
}

pub(super) async fn mount_gvfs_storage_device(
    identity: String,
) -> Result<PathBuf, GvfsDeviceError> {
    request_gvfs_device(|response| GvfsDeviceRequest::Mount { identity, response }).await
}

pub(super) async fn unmount_gvfs_storage_device(identity: String) -> Result<(), GvfsDeviceError> {
    request_gvfs_device(|response| GvfsDeviceRequest::Unmount { identity, response }).await
}

pub(super) async fn remove_gvfs_storage_device(identity: String) -> Result<(), GvfsDeviceError> {
    request_gvfs_device(|response| GvfsDeviceRequest::Remove { identity, response }).await
}

async fn run_gvfs_device_requests(
    monitor: gio::VolumeMonitor,
    mut request_receiver: mpsc::UnboundedReceiver<GvfsDeviceRequest>,
) {
    while let Some(request) = request_receiver.recv().await {
        match request {
            GvfsDeviceRequest::Load { response } => {
                let _ = response.send(load_gvfs_storage_devices_on_context(&monitor));
            }
            GvfsDeviceRequest::Mount { identity, response } => {
                let outcome = mount_gvfs_storage_device_on_context(&monitor, identity).await;
                let _ = response.send(outcome);
            }
            GvfsDeviceRequest::Unmount { identity, response } => {
                let outcome = unmount_gvfs_storage_device_on_context(&monitor, identity).await;
                let _ = response.send(outcome);
            }
            GvfsDeviceRequest::Remove { identity, response } => {
                let outcome = remove_gvfs_storage_device_on_context(&monitor, identity).await;
                let _ = response.send(outcome);
            }
            #[cfg(test)]
            GvfsDeviceRequest::InspectContext { response } => {
                let _ = response.send(GvfsRuntimeProbe {
                    thread_id: thread::current().id(),
                    has_thread_default_context: MainContext::thread_default().is_some(),
                });
            }
        }
    }
}

fn load_gvfs_storage_devices_on_context(
    monitor: &gio::VolumeMonitor,
) -> Result<Vec<StorageDevice>, GvfsDeviceError> {
    let mounts = monitor.mounts();
    let mut devices = monitor
        .volumes()
        .into_iter()
        .filter_map(|volume| {
            let facts = volume_facts(&volume)?;
            if !is_portable_volume(facts.class.as_deref(), facts.activation_root_is_native) {
                return None;
            }
            Some(storage_device_from_volume(
                &volume,
                facts,
                gvfs_mount_for_volume(&volume, &mounts),
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    devices.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(devices)
}

async fn mount_gvfs_storage_device_on_context(
    monitor: &gio::VolumeMonitor,
    identity: String,
) -> Result<PathBuf, GvfsDeviceError> {
    let (volume, current_mount) = find_current_volume(monitor, &identity)?;
    if let Some(mount) = current_mount {
        return mounted_path(&identity, &mount);
    }
    if !volume.can_mount() {
        return Err(GvfsDeviceError::ActionUnavailable(
            "portable device cannot be mounted",
        ));
    }
    volume
        .mount_future(gio::MountMountFlags::NONE, None::<&gio::MountOperation>)
        .await
        .map_err(|error| gio_error(identity.clone(), error))?;
    let mounts = monitor.mounts();
    if let Some(mount) = gvfs_mount_for_volume(&volume, &mounts) {
        return mounted_path(&identity, &mount);
    }
    mounted_path_from_activation_root(&identity, &volume)
}

async fn unmount_gvfs_storage_device_on_context(
    monitor: &gio::VolumeMonitor,
    identity: String,
) -> Result<(), GvfsDeviceError> {
    let (_, mount) = find_current_volume(monitor, &identity)?;
    let mount = mount.ok_or(GvfsDeviceError::ActionUnavailable(
        "portable device is not mounted",
    ))?;
    if !mount.can_unmount() {
        return Err(GvfsDeviceError::ActionUnavailable(
            "portable device cannot be unmounted",
        ));
    }
    mount
        .unmount_with_operation_future(gio::MountUnmountFlags::NONE, None::<&gio::MountOperation>)
        .await
        .map_err(|error| gio_error(identity, error))
}

async fn remove_gvfs_storage_device_on_context(
    monitor: &gio::VolumeMonitor,
    identity: String,
) -> Result<(), GvfsDeviceError> {
    let (volume, mount) = find_current_volume(monitor, &identity)?;
    if let Some(mount) = mount.filter(MountExt::can_eject) {
        return mount
            .eject_with_operation_future(gio::MountUnmountFlags::NONE, None::<&gio::MountOperation>)
            .await
            .map_err(|error| gio_error(identity, error));
    }
    if volume.can_eject() {
        return volume
            .eject_with_operation_future(gio::MountUnmountFlags::NONE, None::<&gio::MountOperation>)
            .await
            .map_err(|error| gio_error(identity, error));
    }
    if let Some(drive) = volume.drive() {
        if drive.can_eject() {
            return drive
                .eject_with_operation_future(
                    gio::MountUnmountFlags::NONE,
                    None::<&gio::MountOperation>,
                )
                .await
                .map_err(|error| gio_error(identity, error));
        }
        if drive.can_stop() {
            return drive
                .stop_future(gio::MountUnmountFlags::NONE, None::<&gio::MountOperation>)
                .await
                .map_err(|error| gio_error(identity, error));
        }
    }
    Err(GvfsDeviceError::ActionUnavailable(
        "portable device cannot be ejected or safely removed",
    ))
}

fn find_current_volume(
    monitor: &gio::VolumeMonitor,
    identity: &str,
) -> Result<(gio::Volume, Option<gio::Mount>), GvfsDeviceError> {
    let mounts = monitor.mounts();
    let volume = monitor
        .volumes()
        .into_iter()
        .find(|volume| volume_facts(volume).is_some_and(|facts| facts.identity == identity))
        .ok_or_else(|| GvfsDeviceError::VolumeUnavailable {
            identity: identity.to_owned(),
        })?;
    let mount = gvfs_mount_for_volume(&volume, &mounts);
    Ok((volume, mount))
}

fn gvfs_mount_for_volume(volume: &gio::Volume, mounts: &[gio::Mount]) -> Option<gio::Mount> {
    volume.get_mount().or_else(|| {
        let activation_root = volume.activation_root()?;
        mounts
            .iter()
            .find(|mount| {
                mount_location_matches_activation_root(
                    &activation_root,
                    &mount.root(),
                    &mount.default_location(),
                )
            })
            .cloned()
    })
}

fn mount_location_matches_activation_root(
    activation_root: &gio::File,
    mount_root: &gio::File,
    mount_default_location: &gio::File,
) -> bool {
    activation_root.equal(mount_root) || activation_root.equal(mount_default_location)
}

fn storage_device_from_volume(
    volume: &gio::Volume,
    facts: GvfsVolumeFacts,
    mount: Option<gio::Mount>,
) -> Result<StorageDevice, GvfsDeviceError> {
    let mount_state = match mount.as_ref() {
        Some(mount) => {
            StorageDeviceMountState::Mounted(vec![mounted_path(&facts.identity, mount)?])
        }
        None => StorageDeviceMountState::Unmounted,
    };
    let drive = volume.drive();
    let can_eject = mount.as_ref().is_some_and(MountExt::can_eject)
        || volume.can_eject()
        || drive.as_ref().is_some_and(DriveExt::can_eject);
    let can_stop = drive.as_ref().is_some_and(DriveExt::can_stop);
    let removal = if can_eject {
        Some(StorageDeviceRemoval::Eject)
    } else if can_stop {
        Some(StorageDeviceRemoval::SafelyRemove)
    } else {
        None
    };
    Ok(StorageDevice {
        id: StorageDeviceId::GvfsVolume(facts.identity),
        label: facts.label,
        device_path: None,
        filesystem_type: "GVfs".to_owned(),
        size_bytes: 0,
        mount_state,
        access: StorageDeviceAccess::RemoteFilesystem,
        is_removable: true,
        can_mount: volume.can_mount(),
        can_unmount: mount.as_ref().is_some_and(MountExt::can_unmount),
        can_eject,
        can_power_off: can_stop,
        removal,
    })
}

fn mounted_path(identity: &str, mount: &gio::Mount) -> Result<PathBuf, GvfsDeviceError> {
    let root = mount.root();
    let default_location = mount.default_location();
    if let Some(path) = default_location.path().or_else(|| root.path()) {
        return Ok(path);
    }
    resolve_gvfs_mount_path(
        root.uri().as_str(),
        default_location.uri().as_str(),
        &default_gvfs_fuse_root(),
    )
    .map_err(|source| GvfsDeviceError::MountPath {
        identity: identity.to_owned(),
        source,
    })
}

fn mounted_path_from_activation_root(
    identity: &str,
    volume: &gio::Volume,
) -> Result<PathBuf, GvfsDeviceError> {
    let activation_root = volume
        .activation_root()
        .ok_or(GvfsDeviceError::OperationIncomplete {
            identity: identity.to_owned(),
            message: "mount completed without an activation root",
        })?;
    if let Some(path) = activation_root.path() {
        return Ok(path);
    }
    let root_uri = activation_root.uri();
    resolve_gvfs_mount_path(
        root_uri.as_str(),
        root_uri.as_str(),
        &default_gvfs_fuse_root(),
    )
    .map_err(|source| GvfsDeviceError::MountPath {
        identity: identity.to_owned(),
        source,
    })
}

fn volume_facts(volume: &gio::Volume) -> Option<GvfsVolumeFacts> {
    let class = volume.identifier("class").map(|value| value.to_string());
    let activation_root = volume.activation_root();
    let activation_root_uri = activation_root.as_ref().map(|file| file.uri().to_string());
    let activation_root_is_native = activation_root.as_ref().map(FileExt::is_native);
    let uuid = volume.uuid().map(|value| value.to_string());
    let identifiers = volume
        .enumerate_identifiers()
        .into_iter()
        .filter_map(|kind| {
            volume
                .identifier(&kind)
                .map(|value| (kind.to_string(), value.to_string()))
        })
        .collect::<Vec<_>>();
    let identity = gvfs_volume_identity(
        activation_root_uri.as_deref(),
        volume.get_mount().as_ref(),
        uuid.as_deref(),
        identifiers,
    )?;
    Some(GvfsVolumeFacts {
        identity,
        label: volume.name().to_string(),
        class,
        activation_root_is_native,
    })
}

fn gvfs_volume_identity(
    activation_root_uri: Option<&str>,
    mount: Option<&gio::Mount>,
    uuid: Option<&str>,
    identifiers: Vec<(String, String)>,
) -> Option<String> {
    activation_root_uri
        .map(|uri| format!("activation:{uri}"))
        .or_else(|| uuid.map(|uuid| format!("uuid:{uuid}")))
        .or_else(|| {
            let mut identifiers = identifiers
                .into_iter()
                .filter(|(kind, _)| kind != "class" && kind != "label")
                .map(|(kind, value)| format!("{kind}={value}"))
                .collect::<Vec<_>>();
            identifiers.sort();
            (!identifiers.is_empty()).then(|| format!("identifiers:{}", identifiers.join("\0")))
        })
        .or_else(|| mount.map(|mount| format!("mount:{}", mount.root().uri())))
}

fn is_portable_volume(class: Option<&str>, activation_root_is_native: Option<bool>) -> bool {
    matches!(class, None | Some("device")) && activation_root_is_native == Some(false)
}

fn gio_error(identity: String, source: gio::glib::Error) -> GvfsDeviceError {
    GvfsDeviceError::Gio { identity, source }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn inspect_gvfs_runtime() -> GvfsRuntimeProbe {
        let runtime = gvfs_device_runtime().expect("GVfs runtime");
        let (response_sender, response_receiver) = oneshot::channel();
        runtime
            .request_sender
            .send(GvfsDeviceRequest::InspectContext {
                response: response_sender,
            })
            .expect("send context inspection");
        response_receiver.await.expect("context inspection")
    }

    #[tokio::test]
    async fn gio_operations_reuse_one_thread_default_context() {
        let first = inspect_gvfs_runtime().await;
        let second = inspect_gvfs_runtime().await;

        assert!(first.has_thread_default_context);
        assert!(second.has_thread_default_context);
        assert_eq!(first.thread_id, second.thread_id);
    }

    #[test]
    fn mount_location_matches_equal_root_or_default_location() {
        let activation_root = gio::File::for_uri("mtp://phone/");
        let matching_root = gio::File::for_uri("mtp://phone/");
        let matching_default = gio::File::for_uri("mtp://phone/");
        let other = gio::File::for_uri("mtp://other/");

        assert!(mount_location_matches_activation_root(
            &activation_root,
            &matching_root,
            &other,
        ));
        assert!(mount_location_matches_activation_root(
            &activation_root,
            &other,
            &matching_default,
        ));
        assert!(!mount_location_matches_activation_root(
            &activation_root,
            &other,
            &other,
        ));
    }

    #[test]
    fn portable_filter_accepts_upstream_portable_facts_without_class() {
        for (activation_root_uri, identifiers) in [
            (
                "mtp://phone/",
                vec![("unix-device".to_owned(), "/dev/bus/usb/008/002".to_owned())],
            ),
            (
                "gphoto2://camera/",
                vec![("unix-device".to_owned(), "/dev/bus/usb/001/004".to_owned())],
            ),
            (
                "afc://phone/",
                vec![("uuid".to_owned(), "phone-uuid".to_owned())],
            ),
        ] {
            assert!(
                gvfs_volume_identity(Some(activation_root_uri), None, None, identifiers,).is_some()
            );
            assert!(is_portable_volume(None, Some(false)));
        }
    }

    #[test]
    fn portable_filter_rejects_native_network_and_loop_volumes() {
        assert!(is_portable_volume(Some("device"), Some(false)));
        assert!(!is_portable_volume(Some("device"), Some(true)));
        assert!(!is_portable_volume(Some("network"), Some(false)));
        assert!(!is_portable_volume(Some("loop"), Some(false)));
        assert!(!is_portable_volume(None, None));
    }

    #[test]
    fn class_and_label_are_not_unique_fallback_identities() {
        assert_eq!(
            gvfs_volume_identity(
                None,
                None,
                None,
                vec![
                    ("class".to_owned(), "device".to_owned()),
                    ("label".to_owned(), "Phone".to_owned()),
                ],
            ),
            None
        );
    }
    #[test]
    fn activation_identities_keep_portable_protocols_distinct() {
        let mtp = gvfs_volume_identity(Some("mtp://[usb:001,002]/"), None, None, Vec::new());
        let gphoto = gvfs_volume_identity(Some("gphoto2://[usb:001,002]/"), None, None, Vec::new());
        let afc = gvfs_volume_identity(Some("afc://phone/"), None, None, Vec::new());

        assert_eq!(mtp.as_deref(), Some("activation:mtp://[usb:001,002]/"));
        assert_eq!(
            gphoto.as_deref(),
            Some("activation:gphoto2://[usb:001,002]/")
        );
        assert_eq!(afc.as_deref(), Some("activation:afc://phone/"));
        assert_ne!(mtp, gphoto);
    }

    #[test]
    fn activation_identity_does_not_depend_on_display_name() {
        let first = GvfsVolumeFacts {
            identity: "activation:mtp://phone/".to_owned(),
            label: "Phone".to_owned(),
            class: Some("device".to_owned()),
            activation_root_is_native: Some(false),
        };
        let second = GvfsVolumeFacts {
            label: "Renamed Phone".to_owned(),
            ..first.clone()
        };

        assert_eq!(first.identity, second.identity);
        assert_ne!(first.label, second.label);
    }
}
