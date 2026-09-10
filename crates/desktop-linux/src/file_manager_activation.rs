use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::sync::mpsc::{self, Receiver, Sender};
use url::Url;
use zbus::fdo::RequestNameFlags;
use zbus::proxy::MethodFlags;

pub const FILE_MANAGER_ACTIVATION_BUS_NAME: &str = "io.github.nsjsv.Bennu";
pub const FILE_MANAGER_ACTIVATION_OBJECT_PATH: &str = "/io/github/nsjsv/Bennu";
pub const FILE_MANAGER1_BUS_NAME: &str = "org.freedesktop.FileManager1";
pub const FILE_MANAGER1_OBJECT_PATH: &str = "/org/freedesktop/FileManager1";
const FILE_MANAGER_ACTIVATION_INTERFACE: &str = "io.github.nsjsv.Bennu.Activation1";
const ACTIVATION_CHANNEL_CAPACITY: usize = 64;
const MAX_ACTIVATION_TARGETS: usize = 256;
const MAX_ACTIVATION_PATH_BYTES: usize = 1024 * 1024;
const MAX_STARTUP_ID_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopStartupId(String);

impl DesktopStartupId {
    fn parse(startup_id: String) -> Result<Self, LocalRequestError> {
        if startup_id.len() > MAX_STARTUP_ID_BYTES {
            return Err(LocalRequestError::StartupIdTooLong {
                bytes: startup_id.len(),
                maximum: MAX_STARTUP_ID_BYTES,
            });
        }
        Ok(Self(startup_id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorkspaceRequest {
    tabs: Vec<LocalWorkspaceTab>,
}

impl LocalWorkspaceRequest {
    pub fn from_cli_paths(paths: Vec<PathBuf>) -> Result<Self, LocalRequestError> {
        validate_path_batch_shape(&paths)?;
        classify_workspace(paths, WorkspaceClassification::CliPaths)
    }

    fn from_folders(paths: Vec<PathBuf>) -> Result<Self, LocalRequestError> {
        validate_path_batch_shape(&paths)?;
        classify_workspace(paths, WorkspaceClassification::Folders)
    }

    fn from_items(paths: Vec<PathBuf>) -> Result<Self, LocalRequestError> {
        validate_path_batch_shape(&paths)?;
        classify_workspace(paths, WorkspaceClassification::Items)
    }

    pub fn tabs(&self) -> &[LocalWorkspaceTab] {
        &self.tabs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorkspaceTab {
    directory: PathBuf,
    selected_paths: Vec<PathBuf>,
}

impl LocalWorkspaceTab {
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn selected_paths(&self) -> &[PathBuf] {
        &self.selected_paths
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPropertyTargets {
    paths: Vec<PathBuf>,
}

impl LocalPropertyTargets {
    fn new(paths: Vec<PathBuf>) -> Result<Self, LocalRequestError> {
        validate_path_batch_shape(&paths)?;
        let mut deduplicated = Vec::new();
        let mut seen = HashSet::new();
        for path in paths {
            validate_absolute_path(&path)?;
            let metadata = std::fs::symlink_metadata(&path).map_err(|source| {
                LocalRequestError::PathUnavailable {
                    path: path.clone(),
                    source,
                }
            })?;
            if !metadata.file_type().is_file()
                && !metadata.file_type().is_dir()
                && !metadata.file_type().is_symlink()
            {
                return Err(LocalRequestError::UnsupportedPath { path });
            }
            if seen.insert(path.clone()) {
                deduplicated.push(path);
            }
        }
        Ok(Self {
            paths: deduplicated,
        })
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopActivationEvent {
    FocusMainWindow(DesktopStartupId),
    MergeWorkspace(LocalWorkspaceRequest, DesktopStartupId),
    OpenProperties(LocalPropertyTargets, DesktopStartupId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandardFileManagerServiceStatus {
    Owned,
    Occupied(String),
}

/// 二次启动时新进程的策略：转发给主实例合并，或自己独立运行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopLaunchWindowPolicy {
    MergeIntoExisting,
    OpenNewWindow,
}

#[derive(Debug)]
pub enum FileManagerActivationClaim {
    Primary(Arc<DesktopActivationRuntime>),
    Forwarded,
    /// 主实例已存在且策略要求独立窗口：不转发、不注册激活名，进程继续普通启动。
    Detached,
}

#[derive(Debug)]
pub struct DesktopActivationRuntime {
    // zbus 内部任务只持弱引用：没有这个字段，连接会在 claim_or_forward 返回时被
    // drop，总线名随即注销、激活事件通道立即关闭。
    _connection: zbus::blocking::Connection,
    event_receiver: Mutex<Option<Receiver<DesktopActivationEvent>>>,
    standard_service_status: StandardFileManagerServiceStatus,
}

impl DesktopActivationRuntime {
    pub fn claim_or_forward(
        paths: &[PathBuf],
        policy: DesktopLaunchWindowPolicy,
    ) -> Result<FileManagerActivationClaim, FileManagerActivationError> {
        let (event_sender, event_receiver) = mpsc::channel(ACTIVATION_CHANNEL_CAPACITY);
        match build_primary_connection(event_sender) {
            Ok(connection) => {
                let standard_service_status = claim_file_manager1_name(&connection);
                Ok(FileManagerActivationClaim::Primary(Arc::new(Self {
                    _connection: connection,
                    event_receiver: Mutex::new(Some(event_receiver)),
                    standard_service_status,
                })))
            }
            Err(zbus::Error::NameTaken) => {
                resolve_name_taken_claim(paths, policy, forward_paths_to_primary)
            }
            Err(source) => Err(FileManagerActivationError::Connect { source }),
        }
    }

    pub fn wait_for_initial_event(
        &self,
    ) -> Result<DesktopActivationEvent, FileManagerActivationError> {
        let mut receiver = self
            .event_receiver
            .lock()
            .expect("desktop activation receiver lock")
            .take()
            .ok_or(FileManagerActivationError::ReceiverUnavailable)?;
        let event = receiver
            .blocking_recv()
            .ok_or(FileManagerActivationError::ReceiverClosed)?;
        self.event_receiver
            .lock()
            .expect("desktop activation receiver lock")
            .replace(receiver);
        Ok(event)
    }

    pub fn take_event_receiver(&self) -> Option<Receiver<DesktopActivationEvent>> {
        self.event_receiver
            .lock()
            .expect("desktop activation receiver lock")
            .take()
    }

    pub fn standard_service_status(&self) -> &StandardFileManagerServiceStatus {
        &self.standard_service_status
    }

    pub fn identity(&self) -> usize {
        self as *const Self as usize
    }
}
#[derive(Debug, Error)]
pub enum FileManagerActivationError {
    #[error("could not connect to the desktop session bus: {source}")]
    Connect {
        #[source]
        source: zbus::Error,
    },
    #[error("could not forward activation to the running file manager: {source}")]
    Forward {
        #[source]
        source: zbus::Error,
    },
    #[error("desktop activation receiver was already taken")]
    ReceiverUnavailable,
    #[error("desktop activation service stopped before receiving its first request")]
    ReceiverClosed,
}

#[derive(Debug, Error)]
pub enum LocalRequestError {
    #[error("activation target list must not be empty")]
    EmptyBatch,
    #[error("activation target count {count} exceeds the maximum {maximum}")]
    TooManyTargets { count: usize, maximum: usize },
    #[error("activation path payload size {bytes} exceeds the maximum {maximum}")]
    PathPayloadTooLarge { bytes: usize, maximum: usize },
    #[error("desktop startup id size {bytes} exceeds the maximum {maximum}")]
    StartupIdTooLong { bytes: usize, maximum: usize },
    #[error("activation path must be absolute: {}", path.display())]
    RelativePath { path: PathBuf },
    #[error("cannot access '{}': {source}", path.display())]
    PathUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsupported activation target: {}", path.display())]
    UnsupportedPath { path: PathBuf },
    #[error("cannot reveal '{}': no parent directory", path.display())]
    MissingParent { path: PathBuf },
    #[error("cannot read directory '{}': {source}", path.display())]
    DirectoryUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid URI '{uri}': {details}")]
    InvalidUri { uri: String, details: String },
    #[error("unsupported URI scheme '{scheme}': only local file URIs are accepted")]
    UnsupportedUriScheme { scheme: String },
    #[error("file URI is not a local absolute path: {uri}")]
    NonLocalFileUri { uri: String },
}

#[derive(Debug, Clone, Copy)]
enum WorkspaceClassification {
    CliPaths,
    Folders,
    Items,
}

fn classify_workspace(
    paths: Vec<PathBuf>,
    classification: WorkspaceClassification,
) -> Result<LocalWorkspaceRequest, LocalRequestError> {
    let mut tabs = Vec::<LocalWorkspaceTab>::new();
    let mut tab_indices = HashMap::<PathBuf, usize>::new();
    let mut seen_targets = HashSet::new();

    for path in paths {
        validate_absolute_path(&path)?;
        if !seen_targets.insert(path.clone()) {
            continue;
        }
        let (directory, selected_path) = classify_target(&path, classification)?;
        let tab_index = match tab_indices.get(&directory) {
            Some(index) => *index,
            None => {
                let index = tabs.len();
                tab_indices.insert(directory.clone(), index);
                tabs.push(LocalWorkspaceTab {
                    directory,
                    selected_paths: Vec::new(),
                });
                index
            }
        };
        if let Some(selected_path) = selected_path {
            tabs[tab_index].selected_paths.push(selected_path);
        }
    }

    Ok(LocalWorkspaceRequest { tabs })
}

fn classify_target(
    path: &Path,
    classification: WorkspaceClassification,
) -> Result<(PathBuf, Option<PathBuf>), LocalRequestError> {
    match classification {
        WorkspaceClassification::CliPaths => {
            let metadata =
                std::fs::metadata(path).map_err(|source| LocalRequestError::PathUnavailable {
                    path: path.to_path_buf(),
                    source,
                })?;
            if metadata.is_dir() {
                ensure_directory_readable(path)?;
                Ok((path.to_path_buf(), None))
            } else if metadata.is_file() {
                let parent = parent_directory(path)?;
                ensure_directory_readable(&parent)?;
                Ok((parent, Some(path.to_path_buf())))
            } else {
                Err(LocalRequestError::UnsupportedPath {
                    path: path.to_path_buf(),
                })
            }
        }
        WorkspaceClassification::Folders => {
            let metadata =
                std::fs::metadata(path).map_err(|source| LocalRequestError::PathUnavailable {
                    path: path.to_path_buf(),
                    source,
                })?;
            if !metadata.is_dir() {
                return Err(LocalRequestError::UnsupportedPath {
                    path: path.to_path_buf(),
                });
            }
            ensure_directory_readable(path)?;
            Ok((path.to_path_buf(), None))
        }
        WorkspaceClassification::Items => {
            let metadata = std::fs::symlink_metadata(path).map_err(|source| {
                LocalRequestError::PathUnavailable {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            if !metadata.file_type().is_file()
                && !metadata.file_type().is_dir()
                && !metadata.file_type().is_symlink()
            {
                return Err(LocalRequestError::UnsupportedPath {
                    path: path.to_path_buf(),
                });
            }
            let parent = parent_directory(path)?;
            ensure_directory_readable(&parent)?;
            Ok((parent, Some(path.to_path_buf())))
        }
    }
}

fn validate_path_batch_shape(paths: &[PathBuf]) -> Result<(), LocalRequestError> {
    if paths.is_empty() {
        return Err(LocalRequestError::EmptyBatch);
    }
    if paths.len() > MAX_ACTIVATION_TARGETS {
        return Err(LocalRequestError::TooManyTargets {
            count: paths.len(),
            maximum: MAX_ACTIVATION_TARGETS,
        });
    }
    let bytes = paths.iter().try_fold(0usize, |total, path| {
        total.checked_add(path.as_os_str().as_bytes().len()).ok_or(
            LocalRequestError::PathPayloadTooLarge {
                bytes: usize::MAX,
                maximum: MAX_ACTIVATION_PATH_BYTES,
            },
        )
    })?;
    if bytes > MAX_ACTIVATION_PATH_BYTES {
        return Err(LocalRequestError::PathPayloadTooLarge {
            bytes,
            maximum: MAX_ACTIVATION_PATH_BYTES,
        });
    }
    Ok(())
}

fn validate_absolute_path(path: &Path) -> Result<(), LocalRequestError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(LocalRequestError::RelativePath {
            path: path.to_path_buf(),
        })
    }
}

fn parent_directory(path: &Path) -> Result<PathBuf, LocalRequestError> {
    path.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| LocalRequestError::MissingParent {
            path: path.to_path_buf(),
        })
}

fn ensure_directory_readable(path: &Path) -> Result<(), LocalRequestError> {
    std::fs::read_dir(path)
        .map(drop)
        .map_err(|source| LocalRequestError::DirectoryUnreadable {
            path: path.to_path_buf(),
            source,
        })
}

fn build_primary_connection(
    event_sender: Sender<DesktopActivationEvent>,
) -> zbus::Result<zbus::blocking::Connection> {
    let connection = zbus::blocking::connection::Builder::session()?
        .serve_at(
            FILE_MANAGER1_OBJECT_PATH,
            FileManager1Interface {
                event_sender: event_sender.clone(),
            },
        )?
        .serve_at(
            FILE_MANAGER_ACTIVATION_OBJECT_PATH,
            BrandedActivationInterface { event_sender },
        )?
        .build()?;
    connection.request_name_with_flags(
        FILE_MANAGER_ACTIVATION_BUS_NAME,
        RequestNameFlags::DoNotQueue.into(),
    )?;
    Ok(connection)
}

fn claim_file_manager1_name(
    connection: &zbus::blocking::Connection,
) -> StandardFileManagerServiceStatus {
    match connection
        .request_name_with_flags(FILE_MANAGER1_BUS_NAME, RequestNameFlags::DoNotQueue.into())
    {
        Ok(_) => StandardFileManagerServiceStatus::Owned,
        Err(error) => StandardFileManagerServiceStatus::Occupied(error.to_string()),
    }
}

fn forward_paths_to_primary(paths: &[PathBuf]) -> Result<(), FileManagerActivationError> {
    let connection = zbus::blocking::Connection::session()
        .map_err(|source| FileManagerActivationError::Forward { source })?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        FILE_MANAGER_ACTIVATION_BUS_NAME,
        FILE_MANAGER_ACTIVATION_OBJECT_PATH,
        FILE_MANAGER_ACTIVATION_INTERFACE,
    )
    .map_err(|source| FileManagerActivationError::Forward { source })?;
    let response = if paths.is_empty() {
        proxy.call_with_flags::<_, _, ()>(
            "Activate",
            MethodFlags::NoAutoStart.into(),
            &(String::new(),),
        )
    } else {
        let encoded_paths = paths
            .iter()
            .map(|path| path.as_os_str().as_bytes().to_vec())
            .collect::<Vec<_>>();
        proxy.call_with_flags::<_, _, ()>(
            "OpenPaths",
            MethodFlags::NoAutoStart.into(),
            &(encoded_paths, String::new()),
        )
    };
    response.map_err(|source| FileManagerActivationError::Forward { source })?;
    Ok(())
}

/// NameTaken 时的分流决策：独立窗口策略直接 Detached，不触碰 D-Bus；
/// 合并策略转发路径给主实例。转发动作注入以便无 session bus 时测试。
fn resolve_name_taken_claim(
    paths: &[PathBuf],
    policy: DesktopLaunchWindowPolicy,
    forward: impl FnOnce(&[PathBuf]) -> Result<(), FileManagerActivationError>,
) -> Result<FileManagerActivationClaim, FileManagerActivationError> {
    match policy {
        DesktopLaunchWindowPolicy::OpenNewWindow => Ok(FileManagerActivationClaim::Detached),
        DesktopLaunchWindowPolicy::MergeIntoExisting => {
            forward(paths)?;
            Ok(FileManagerActivationClaim::Forwarded)
        }
    }
}

#[derive(Debug, Clone)]
struct FileManager1Interface {
    event_sender: Sender<DesktopActivationEvent>,
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager1Interface {
    fn show_items(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
        let startup_id = DesktopStartupId::parse(startup_id).map_err(fdo_request_error)?;
        let workspace = LocalWorkspaceRequest::from_items(local_paths_from_uris(&uris)?)
            .map_err(fdo_request_error)?;
        try_send_event(
            &self.event_sender,
            DesktopActivationEvent::MergeWorkspace(workspace, startup_id),
        )
    }

    fn show_folders(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
        let startup_id = DesktopStartupId::parse(startup_id).map_err(fdo_request_error)?;
        let workspace = LocalWorkspaceRequest::from_folders(local_paths_from_uris(&uris)?)
            .map_err(fdo_request_error)?;
        try_send_event(
            &self.event_sender,
            DesktopActivationEvent::MergeWorkspace(workspace, startup_id),
        )
    }

    fn show_item_properties(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
        let startup_id = DesktopStartupId::parse(startup_id).map_err(fdo_request_error)?;
        let targets =
            LocalPropertyTargets::new(local_paths_from_uris(&uris)?).map_err(fdo_request_error)?;
        try_send_event(
            &self.event_sender,
            DesktopActivationEvent::OpenProperties(targets, startup_id),
        )
    }
}

#[derive(Debug, Clone)]
struct BrandedActivationInterface {
    event_sender: Sender<DesktopActivationEvent>,
}

#[zbus::interface(name = "io.github.nsjsv.Bennu.Activation1")]
impl BrandedActivationInterface {
    fn activate(&self, startup_id: String) -> zbus::fdo::Result<()> {
        let startup_id = DesktopStartupId::parse(startup_id).map_err(fdo_request_error)?;
        try_send_event(
            &self.event_sender,
            DesktopActivationEvent::FocusMainWindow(startup_id),
        )
    }

    fn open_paths(&self, encoded_paths: Vec<Vec<u8>>, startup_id: String) -> zbus::fdo::Result<()> {
        let startup_id = DesktopStartupId::parse(startup_id).map_err(fdo_request_error)?;
        let paths = encoded_paths
            .into_iter()
            .map(|bytes| PathBuf::from(OsString::from_vec(bytes)))
            .collect::<Vec<_>>();
        let workspace = LocalWorkspaceRequest::from_cli_paths(paths).map_err(fdo_request_error)?;
        try_send_event(
            &self.event_sender,
            DesktopActivationEvent::MergeWorkspace(workspace, startup_id),
        )
    }
}

fn local_paths_from_uris(uris: &[String]) -> zbus::fdo::Result<Vec<PathBuf>> {
    if uris.is_empty() {
        return Err(fdo_request_error(LocalRequestError::EmptyBatch));
    }
    if uris.len() > MAX_ACTIVATION_TARGETS {
        return Err(fdo_request_error(LocalRequestError::TooManyTargets {
            count: uris.len(),
            maximum: MAX_ACTIVATION_TARGETS,
        }));
    }
    uris.iter().map(|uri| local_path_from_uri(uri)).collect()
}

fn local_path_from_uri(uri: &str) -> zbus::fdo::Result<PathBuf> {
    validate_uri_percent_encoding(uri)?;
    let parsed = Url::parse(uri).map_err(|error| {
        fdo_request_error(LocalRequestError::InvalidUri {
            uri: uri.to_owned(),
            details: error.to_string(),
        })
    })?;
    if parsed.scheme() != "file" {
        return Err(fdo_request_error(LocalRequestError::UnsupportedUriScheme {
            scheme: parsed.scheme().to_owned(),
        }));
    }
    parsed.to_file_path().map_err(|()| {
        fdo_request_error(LocalRequestError::NonLocalFileUri {
            uri: uri.to_owned(),
        })
    })
}

fn validate_uri_percent_encoding(uri: &str) -> zbus::fdo::Result<()> {
    let bytes = uri.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let valid_escape = bytes
                .get(index + 1..index + 3)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit));
            if !valid_escape {
                return Err(fdo_request_error(LocalRequestError::InvalidUri {
                    uri: uri.to_owned(),
                    details: "malformed percent encoding".to_owned(),
                }));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn fdo_request_error(error: LocalRequestError) -> zbus::fdo::Error {
    match error {
        LocalRequestError::TooManyTargets { .. }
        | LocalRequestError::PathPayloadTooLarge { .. }
        | LocalRequestError::StartupIdTooLong { .. } => {
            zbus::fdo::Error::LimitsExceeded(error.to_string())
        }
        _ => zbus::fdo::Error::InvalidArgs(error.to_string()),
    }
}

fn try_send_event(
    event_sender: &Sender<DesktopActivationEvent>,
    event: DesktopActivationEvent,
) -> zbus::fdo::Result<()> {
    event_sender.try_send(event).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => {
            zbus::fdo::Error::LimitsExceeded("desktop activation queue is full".to_owned())
        }
        mpsc::error::TrySendError::Closed(_) => {
            zbus::fdo::Error::Failed("desktop activation receiver is unavailable".to_owned())
        }
    })
}

#[cfg(test)]
#[path = "file_manager_activation_tests.rs"]
mod tests;
