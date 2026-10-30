use std::cell::{Cell, RefCell};
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

use gio::glib;
use gio::prelude::{FileExt, MountExt, MountOperationExt, ObjectExt, VolumeMonitorExt};
use thiserror::Error;

use crate::gio_runtime::{run_on_gio_thread, GioRuntimeError};

mod uri;
use uri::parse_network_uri;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NetworkConnectionId(String);

impl NetworkConnectionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NetworkConnectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkProtocol {
    Smb,
    WebDav,
    Sftp,
}

impl NetworkProtocol {
    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "smb" => Some(Self::Smb),
            "webdav" => Some(Self::WebDav),
            "sftp" => Some(Self::Sftp),
            _ => None,
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Smb => "smb",
            Self::WebDav => "webdav",
            Self::Sftp => "sftp",
        }
    }

    fn mount_scheme(self, scheme: &str) -> Option<&'static str> {
        match self {
            Self::Smb if scheme == "smb" => Some("smb"),
            Self::WebDav => match scheme {
                "dav" | "http" => Some("dav"),
                "davs" | "https" => Some("davs"),
                _ => None,
            },
            Self::Sftp if scheme == "sftp" => Some("sftp"),
            _ => None,
        }
    }

    fn backend_name(self) -> &'static str {
        match self {
            Self::Smb => "gvfs-smb",
            Self::WebDav => "gvfs-dav",
            Self::Sftp => "gvfs-sftp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConnection {
    pub id: NetworkConnectionId,
    pub label: String,
    pub protocol: NetworkProtocol,
    pub uri: String,
}

impl NetworkConnection {
    pub fn new(
        id: NetworkConnectionId,
        label: impl Into<String>,
        protocol: NetworkProtocol,
        uri: impl Into<String>,
    ) -> Result<Self, NetworkMountError> {
        let uri = normalize_network_connection_uri(protocol, &uri.into())?;
        Ok(Self {
            id,
            label: label.into(),
            protocol,
            uri,
        })
    }

    pub fn new_with_username(
        id: NetworkConnectionId,
        label: impl Into<String>,
        protocol: NetworkProtocol,
        uri: impl Into<String>,
        username: Option<String>,
    ) -> Result<Self, NetworkMountError> {
        let uri = normalize_network_connection_uri(protocol, &uri.into())?;
        let parts = parse_network_uri(&uri)?;
        Ok(Self {
            id,
            label: label.into(),
            protocol,
            uri: parts.to_uri_with_username(
                protocol
                    .mount_scheme(&parts.scheme)
                    .unwrap_or(parts.canonical_scheme()),
                username.as_deref(),
            ),
        })
    }

    pub fn label_or_default(&self) -> String {
        let label = self.label.trim();
        if label.is_empty() {
            network_connection_label_from_uri(&self.uri)
        } else {
            label.to_owned()
        }
    }

    pub fn username(&self) -> Option<String> {
        parse_network_uri(&self.uri)
            .ok()
            .and_then(|parts| parts.username())
    }

    pub fn uri_without_username(&self) -> String {
        parse_network_uri(&self.uri)
            .map(|parts| parts.uri_without_username(parts.canonical_scheme()))
            .unwrap_or_else(|_| self.uri.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkMountState {
    Disconnected,
    Connecting,
    Mounted(PathBuf),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedNetworkConnection {
    pub connection: NetworkConnection,
    pub mount_path: PathBuf,
}

#[derive(Clone, PartialEq, Eq)]
pub struct NetworkMountCredentials {
    pub username: Option<String>,
    pub password: String,
}

impl NetworkMountCredentials {
    pub fn new(username: Option<String>, password: impl Into<String>) -> Self {
        Self {
            username: username
                .map(|username| username.trim().to_owned())
                .filter(|username| !username.is_empty()),
            password: password.into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.password.is_empty()
    }
}

impl fmt::Debug for NetworkMountCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkMountCredentials")
            .field("username", &self.username)
            .field(
                "password",
                if self.password.is_empty() {
                    &"<empty>"
                } else {
                    &"<redacted>"
                },
            )
            .finish()
    }
}

/// 挂载失败按 GIO 错误码分类，与系统界面语言无关；`source` 的消息由 GVfs 按系统语言给出，
/// 原样留给用户看。
#[derive(Debug, Error)]
pub enum NetworkMountError {
    #[error("invalid network URI {uri:?}: {message}")]
    InvalidUri { uri: String, message: String },
    #[error(transparent)]
    Runtime(#[from] GioRuntimeError),
    #[error("{backend} backend is unavailable for {uri:?}: {source}")]
    BackendUnavailable {
        uri: String,
        backend: &'static str,
        #[source]
        source: glib::Error,
    },
    #[error("could not sign in to {uri:?}: {source}")]
    AuthenticationFailed {
        uri: String,
        #[source]
        source: glib::Error,
    },
    #[error("mounting {uri:?} needs an answer to {question:?}")]
    QuestionDeclined {
        uri: String,
        question: String,
        #[source]
        source: glib::Error,
    },
    #[error("could not mount {uri:?}: {source}")]
    MountFailed {
        uri: String,
        #[source]
        source: glib::Error,
    },
    #[error("{uri:?} is not mounted")]
    NotMounted { uri: String },
    #[error("could not unmount {uri:?}: {source}")]
    UnmountFailed {
        uri: String,
        #[source]
        source: glib::Error,
    },
    #[error("{uri:?} is mounted but has no local path; GVfs FUSE may not be running")]
    MountPathUnavailable { uri: String },
}

pub async fn load_network_mount_states(
    connections: Vec<NetworkConnection>,
) -> Result<Vec<(NetworkConnectionId, NetworkMountState)>, NetworkMountError> {
    run_on_gio_thread(move |monitor| async move {
        let mounted_uris = mounted_network_uris(&monitor);
        connections
            .into_iter()
            .map(|connection| {
                let state = if mounted_uris
                    .iter()
                    .any(|uri| network_connection_matches_mounted_uri(&connection, uri))
                {
                    let location = gio::File::for_uri(&connection.uri);
                    NetworkMountState::Mounted(local_path_of_location(&connection, &location)?)
                } else {
                    NetworkMountState::Disconnected
                };
                Ok((connection.id, state))
            })
            .collect()
    })
    .await?
}

pub async fn mount_network_connection(
    connection: NetworkConnection,
) -> Result<MountedNetworkConnection, NetworkMountError> {
    mount_network_connection_with_credentials(connection, None).await
}

pub async fn mount_network_connection_with_credentials(
    connection: NetworkConnection,
    credentials: Option<NetworkMountCredentials>,
) -> Result<MountedNetworkConnection, NetworkMountError> {
    validate_network_connection_uri(connection.protocol, &connection.uri)?;
    let mount_uri = mount_uri_for_credentials(&connection, credentials.as_ref())?;
    let mounting = connection.clone();
    let mount_path = run_on_gio_thread(move |_| async move {
        mount_on_gio_thread(&mounting, &mount_uri, credentials).await
    })
    .await??;
    Ok(MountedNetworkConnection {
        connection,
        mount_path,
    })
}

/// 挂载并直接从挂载结果取本地路径：GVfs 位置的 `File::path()` 就是它在 FUSE 下的路径
/// （含 SMB 子目录、SFTP 远端路径），不扫 FUSE 目录、不轮询。
async fn mount_on_gio_thread(
    connection: &NetworkConnection,
    mount_uri: &str,
    credentials: Option<NetworkMountCredentials>,
) -> Result<PathBuf, NetworkMountError> {
    let location = gio::File::for_uri(mount_uri);
    let declined = Rc::new(RefCell::new(None));
    let operation = mount_operation(credentials, declined.clone());
    location
        .mount_enclosing_volume_future(gio::MountMountFlags::NONE, Some(&operation))
        .await
        .or_else(already_mounted_is_success)
        .map_err(|error| classify_mount_error(connection, error, declined.take()))?;
    local_path_of_location(connection, &location)
}

/// 位置已经挂载是幂等成功，直接取它的本地路径。
fn already_mounted_is_success(error: glib::Error) -> Result<(), glib::Error> {
    if error.matches(gio::IOErrorEnum::AlreadyMounted) {
        Ok(())
    } else {
        Err(error)
    }
}

fn local_path_of_location(
    connection: &NetworkConnection,
    location: &gio::File,
) -> Result<PathBuf, NetworkMountError> {
    location
        .path()
        .ok_or_else(|| NetworkMountError::MountPathUnavailable {
            uri: connection.uri.clone(),
        })
}

/// 挂载过程中替用户放弃回答的提问；挂载以 `FailedHandled` 结束时据此区分原因。
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclinedPrompt {
    Password,
    Question(String),
}

/// 回答挂载时的提问。密码只回答一次：密码错时 GVfs 会再问，第二次放弃，挂载以认证失败
/// 结束（原先往 gio 标准输入写一次密码也是这样结束）。没有凭据、以及其它提问（如 SFTP
/// 主机指纹）一律放弃，不替用户做决定。
fn mount_operation(
    credentials: Option<NetworkMountCredentials>,
    declined: Rc<RefCell<Option<DeclinedPrompt>>>,
) -> gio::MountOperation {
    let operation = gio::MountOperation::new();
    let answered = Cell::new(false);
    let password_declined = declined.clone();
    operation.connect_ask_password(move |operation, _message, user, domain, flags| {
        let answer = password_answer(
            credentials.as_ref(),
            answered.replace(true),
            user,
            domain,
            flags,
        );
        reply_to_password_request(operation, answer, &password_declined);
    });
    operation.connect_local("ask-question", false, move |values| {
        let question = values
            .get(1)
            .and_then(|value| value.get::<String>().ok())
            .unwrap_or_default();
        declined.replace(Some(DeclinedPrompt::Question(question)));
        if let Some(operation) = values
            .first()
            .and_then(|value| value.get::<gio::MountOperation>().ok())
        {
            operation.reply(gio::MountOperationResult::Aborted);
        }
        None
    });
    operation
}

fn reply_to_password_request(
    operation: &gio::MountOperation,
    answer: PasswordAnswer,
    declined: &RefCell<Option<DeclinedPrompt>>,
) {
    match answer {
        PasswordAnswer::Provide {
            username,
            domain,
            password,
        } => {
            operation.set_username(username.as_deref());
            operation.set_domain(domain.as_deref());
            operation.set_password(password.as_deref());
            operation.set_password_save(gio::PasswordSave::Never);
            operation.reply(gio::MountOperationResult::Handled);
        }
        PasswordAnswer::Decline => {
            declined.replace(Some(DeclinedPrompt::Password));
            operation.reply(gio::MountOperationResult::Aborted);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PasswordAnswer {
    Provide {
        username: Option<String>,
        domain: Option<String>,
        password: Option<String>,
    },
    Decline,
}

/// 只填 GVfs 要的项：需要域时用服务端给的默认域（SMB 的 WORKGROUP 等）。
fn password_answer(
    credentials: Option<&NetworkMountCredentials>,
    already_answered: bool,
    default_user: &str,
    default_domain: &str,
    flags: gio::AskPasswordFlags,
) -> PasswordAnswer {
    let Some(credentials) = credentials.filter(|credentials| !credentials.is_empty()) else {
        return PasswordAnswer::Decline;
    };
    if already_answered {
        return PasswordAnswer::Decline;
    }
    PasswordAnswer::Provide {
        username: flags
            .contains(gio::AskPasswordFlags::NEED_USERNAME)
            .then(|| {
                credentials
                    .username
                    .clone()
                    .unwrap_or_else(|| default_user.to_owned())
            }),
        domain: flags
            .contains(gio::AskPasswordFlags::NEED_DOMAIN)
            .then(|| default_domain.to_owned()),
        password: flags
            .contains(gio::AskPasswordFlags::NEED_PASSWORD)
            .then(|| credentials.password.clone()),
    }
}

/// GVfs 后端没装或没运行时，GIO 对网络 URI 报 `NotSupported` / `NotMountableFile`；
/// `FailedHandled` 表示提问被放弃，原因由 `declined` 给出。
fn classify_mount_error(
    connection: &NetworkConnection,
    source: glib::Error,
    declined: Option<DeclinedPrompt>,
) -> NetworkMountError {
    let uri = connection.uri.clone();
    match source.kind::<gio::IOErrorEnum>() {
        Some(gio::IOErrorEnum::NotSupported | gio::IOErrorEnum::NotMountableFile) => {
            NetworkMountError::BackendUnavailable {
                uri,
                backend: connection.protocol.backend_name(),
                source,
            }
        }
        Some(gio::IOErrorEnum::PermissionDenied) => {
            NetworkMountError::AuthenticationFailed { uri, source }
        }
        Some(gio::IOErrorEnum::FailedHandled) => match declined {
            Some(DeclinedPrompt::Password) => {
                NetworkMountError::AuthenticationFailed { uri, source }
            }
            Some(DeclinedPrompt::Question(question)) => NetworkMountError::QuestionDeclined {
                uri,
                question,
                source,
            },
            None => NetworkMountError::MountFailed { uri, source },
        },
        _ => NetworkMountError::MountFailed { uri, source },
    }
}

pub async fn unmount_network_connection(
    connection: NetworkConnection,
) -> Result<(), NetworkMountError> {
    validate_network_connection_uri(connection.protocol, &connection.uri)?;
    run_on_gio_thread(move |monitor| async move {
        let mount = monitor
            .mounts()
            .into_iter()
            .find(|mount| {
                mount_uris(mount)
                    .iter()
                    .any(|uri| network_connection_matches_mounted_uri(&connection, uri))
            })
            .ok_or_else(|| NetworkMountError::NotMounted {
                uri: connection.uri.clone(),
            })?;
        mount
            .unmount_with_operation_future(
                gio::MountUnmountFlags::NONE,
                None::<&gio::MountOperation>,
            )
            .await
            .map_err(|source| NetworkMountError::UnmountFailed {
                uri: connection.uri.clone(),
                source,
            })
    })
    .await?
}

fn mounted_network_uris(monitor: &gio::VolumeMonitor) -> Vec<String> {
    let mut uris = Vec::new();
    for uri in monitor.mounts().iter().flat_map(mount_uris) {
        if is_supported_network_uri(&uri) && !uris.contains(&uri) {
            uris.push(uri);
        }
    }
    uris
}

fn mount_uris(mount: &gio::Mount) -> [String; 2] {
    [
        mount.root().uri().to_string(),
        mount.default_location().uri().to_string(),
    ]
}

fn mount_uri_for_credentials(
    connection: &NetworkConnection,
    credentials: Option<&NetworkMountCredentials>,
) -> Result<String, NetworkMountError> {
    let parts = parse_network_uri(&connection.uri)?;
    let username = credentials
        .and_then(|credentials| credentials.username.as_deref())
        .map(str::to_owned)
        .or_else(|| parts.username());
    if credentials.is_some_and(|credentials| !credentials.password.is_empty()) && username.is_none()
    {
        return Err(NetworkMountError::InvalidUri {
            uri: connection.uri.clone(),
            message: "username is required when a password is provided".to_owned(),
        });
    }
    Ok(parts.to_uri_with_username(parts.canonical_scheme(), username.as_deref()))
}

pub fn validate_network_connection_uri(
    protocol: NetworkProtocol,
    uri: &str,
) -> Result<(), NetworkMountError> {
    normalize_network_connection_uri(protocol, uri).map(|_| ())
}

pub fn normalize_network_connection_uri(
    protocol: NetworkProtocol,
    uri: &str,
) -> Result<String, NetworkMountError> {
    let parts = parse_network_uri(uri)?;
    let Some(mount_scheme) = protocol.mount_scheme(&parts.scheme) else {
        return Err(NetworkMountError::InvalidUri {
            uri: uri.to_owned(),
            message: format!("scheme is not valid for {}", protocol.config_value()),
        });
    };
    if parts.host.is_empty() {
        return Err(NetworkMountError::InvalidUri {
            uri: uri.to_owned(),
            message: "host is empty".to_owned(),
        });
    }
    if parts.has_password {
        return Err(NetworkMountError::InvalidUri {
            uri: uri.to_owned(),
            message: "passwords must not be stored in network connection URIs".to_owned(),
        });
    }
    if protocol == NetworkProtocol::Smb && parts.path_segments.is_empty() {
        return Err(NetworkMountError::InvalidUri {
            uri: uri.to_owned(),
            message: "SMB URI must include a share name".to_owned(),
        });
    }
    Ok(parts.to_uri(mount_scheme))
}

fn network_connection_label_from_uri(uri: &str) -> String {
    parse_network_uri(uri)
        .map(|parts| match parts.path_segments.last() {
            Some(segment) => segment.clone(),
            None => parts.host,
        })
        .unwrap_or_else(|_| uri.to_owned())
}

fn network_uris_match(left: &str, right: &str) -> bool {
    match (parse_network_uri(left), parse_network_uri(right)) {
        (Ok(left), Ok(right)) => left.normalized() == right.normalized(),
        _ => false,
    }
}

fn network_connection_matches_mounted_uri(connection: &NetworkConnection, mount_uri: &str) -> bool {
    match connection.protocol {
        NetworkProtocol::Sftp => match (
            parse_network_uri(&connection.uri),
            parse_network_uri(mount_uri),
        ) {
            (Ok(connection_parts), Ok(mount_parts)) => {
                connection_parts.canonical_scheme() == "sftp"
                    && mount_parts.canonical_scheme() == "sftp"
                    && connection_parts.host == mount_parts.host
                    && sftp_uri_users_match(
                        connection_parts.username().as_deref(),
                        mount_parts.username().as_deref(),
                    )
            }
            _ => false,
        },
        NetworkProtocol::Smb | NetworkProtocol::WebDav => {
            network_uris_match(&connection.uri, mount_uri)
        }
    }
}

fn sftp_uri_users_match(connection_user: Option<&str>, mount_user: Option<&str>) -> bool {
    match (connection_user, mount_user) {
        (Some(connection_user), Some(mount_user)) => connection_user == mount_user,
        (None, None) => true,
        _ => false,
    }
}

fn is_supported_network_uri(uri: &str) -> bool {
    parse_network_uri(uri)
        .map(|parts| matches!(parts.canonical_scheme(), "smb" | "dav" | "davs" | "sftp"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
