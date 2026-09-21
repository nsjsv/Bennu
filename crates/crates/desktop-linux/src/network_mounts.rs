use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output, Stdio};
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::sleep;

mod uri;
use uri::{parse_network_uri, percent_encode_gvfs_prefix};

const GIO: &str = "gio";
const GVFS_MOUNT_PATH_RETRIES: usize = 10;
const GVFS_MOUNT_PATH_RETRY_DELAY: Duration = Duration::from_millis(100);

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

#[derive(Debug, Error)]
pub enum NetworkMountError {
    #[error("invalid network URI {uri:?}: {message}")]
    InvalidUri { uri: String, message: String },
    #[error("could not run gio to list network mounts: {source}")]
    ListSpawn {
        #[source]
        source: std::io::Error,
    },
    #[error("gio failed to list network mounts with status {status}: {stderr}")]
    ListFailed { status: ExitStatus, stderr: String },
    #[error("could not run gio to mount {uri:?}: {source}")]
    MountSpawn {
        uri: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not send credentials to gio while mounting {uri:?}: {source}")]
    MountCredentialWrite {
        uri: String,
        #[source]
        source: std::io::Error,
    },
    #[error("gio failed to mount {uri:?} with status {status}: {stderr}")]
    MountFailed {
        uri: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("{backend} backend is unavailable for {uri:?}: {reason}")]
    BackendUnavailable {
        uri: String,
        backend: &'static str,
        reason: String,
    },
    #[error("could not run gio to unmount {uri:?}: {source}")]
    UnmountSpawn {
        uri: String,
        #[source]
        source: std::io::Error,
    },
    #[error("gio failed to unmount {uri:?} with status {status}: {stderr}")]
    UnmountFailed {
        uri: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("GVfs FUSE root is unavailable for {uri:?}: {root:?}")]
    FuseUnavailable { uri: String, root: PathBuf },
    #[error("mounted GVfs path was not found for {uri:?} under {root:?}")]
    MountPathUnavailable { uri: String, root: PathBuf },
}

pub async fn load_network_mount_states(
    connections: Vec<NetworkConnection>,
) -> Result<Vec<(NetworkConnectionId, NetworkMountState)>, NetworkMountError> {
    let listing = load_gio_mount_listing().await?;
    let mount_uris = parse_gio_mount_uris(&listing);
    let fuse_root = default_gvfs_fuse_root();
    let mut states = Vec::with_capacity(connections.len());

    for connection in connections {
        let state = if mount_uris
            .iter()
            .any(|mount_uri| network_connection_matches_mounted_uri(&connection, mount_uri))
        {
            NetworkMountState::Mounted(resolve_gvfs_mount_path_from_root(&connection, &fuse_root)?)
        } else {
            NetworkMountState::Disconnected
        };
        states.push((connection.id, state));
    }

    Ok(states)
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
    let output = run_gio_mount_command(&connection, &mount_uri, credentials.as_ref()).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if !mount_already_present_message(&stderr) {
            return Err(mount_command_error(&connection, output.status, stderr));
        }
    }

    let mount_path = wait_for_gvfs_mount_path(&connection).await?;
    Ok(MountedNetworkConnection {
        connection,
        mount_path,
    })
}

async fn run_gio_mount_command(
    connection: &NetworkConnection,
    mount_uri: &str,
    credentials: Option<&NetworkMountCredentials>,
) -> Result<Output, NetworkMountError> {
    let Some(credentials) = credentials.filter(|credentials| !credentials.is_empty()) else {
        return Command::new(GIO)
            .arg("mount")
            .arg(mount_uri)
            .output()
            .await
            .map_err(|source| NetworkMountError::MountSpawn {
                uri: connection.uri.clone(),
                source,
            });
    };

    let mut child = Command::new(GIO)
        .arg("mount")
        .arg(mount_uri)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| NetworkMountError::MountSpawn {
            uri: connection.uri.clone(),
            source,
        })?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(NetworkMountError::MountCredentialWrite {
            uri: connection.uri.clone(),
            source: io::Error::new(io::ErrorKind::BrokenPipe, "gio stdin was unavailable"),
        });
    };
    stdin
        .write_all(&gio_mount_credential_stdin(
            connection.protocol,
            credentials,
        ))
        .await
        .map_err(|source| NetworkMountError::MountCredentialWrite {
            uri: connection.uri.clone(),
            source,
        })?;
    drop(stdin);

    child
        .wait_with_output()
        .await
        .map_err(|source| NetworkMountError::MountSpawn {
            uri: connection.uri.clone(),
            source,
        })
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

fn gio_mount_credential_stdin(
    protocol: NetworkProtocol,
    credentials: &NetworkMountCredentials,
) -> Vec<u8> {
    let mut input = Vec::with_capacity(credentials.password.len() + 2);
    if protocol == NetworkProtocol::Smb {
        // gvfsd-smb 会先询问 Domain [WORKGROUP]，空行表示接受默认域。
        input.push(b'\n');
    }
    input.extend_from_slice(credentials.password.as_bytes());
    input.push(b'\n');
    input
}

pub async fn unmount_network_connection(
    connection: NetworkConnection,
) -> Result<(), NetworkMountError> {
    validate_network_connection_uri(connection.protocol, &connection.uri)?;
    let output = Command::new(GIO)
        .arg("mount")
        .arg("-u")
        .arg(&connection.uri)
        .output()
        .await
        .map_err(|source| NetworkMountError::UnmountSpawn {
            uri: connection.uri.clone(),
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(NetworkMountError::UnmountFailed {
            uri: connection.uri,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
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

pub fn parse_gio_mount_uris(output: &str) -> Vec<String> {
    let mut uris = Vec::new();
    for line in output.lines().map(str::trim) {
        if let Some(uri) = line
            .strip_prefix("default_location=")
            .or_else(|| line.rsplit_once("->").map(|(_, uri)| uri.trim()))
        {
            if is_supported_network_uri(uri) && !uris.iter().any(|existing| existing == uri) {
                uris.push(uri.to_owned());
            }
        }
    }
    uris
}

pub fn resolve_gvfs_mount_path_from_root(
    connection: &NetworkConnection,
    root: &Path,
) -> Result<PathBuf, NetworkMountError> {
    if !root.is_dir() {
        return Err(NetworkMountError::FuseUnavailable {
            uri: connection.uri.clone(),
            root: root.to_path_buf(),
        });
    }
    let mut entries = std::fs::read_dir(root).map_err(|_| NetworkMountError::FuseUnavailable {
        uri: connection.uri.clone(),
        root: root.to_path_buf(),
    })?;

    while let Some(entry) =
        entries
            .next()
            .transpose()
            .map_err(|_| NetworkMountError::MountPathUnavailable {
                uri: connection.uri.clone(),
                root: root.to_path_buf(),
            })?
    {
        if gvfs_mount_directory_matches(&entry.file_name().to_string_lossy(), connection)? {
            return gvfs_mount_path_for_connection(entry.path(), connection);
        }
    }

    Err(NetworkMountError::MountPathUnavailable {
        uri: connection.uri.clone(),
        root: root.to_path_buf(),
    })
}

fn mount_command_error(
    connection: &NetworkConnection,
    status: ExitStatus,
    stderr: String,
) -> NetworkMountError {
    if backend_error_message(&stderr).is_some() {
        NetworkMountError::BackendUnavailable {
            uri: connection.uri.clone(),
            backend: connection.protocol.backend_name(),
            reason: stderr,
        }
    } else {
        NetworkMountError::MountFailed {
            uri: connection.uri.clone(),
            status,
            stderr,
        }
    }
}

fn gio_unavailable_message(stderr: &str) -> Option<&str> {
    let lower = stderr.to_lowercase();
    if lower.contains("error creating proxy")
        || lower.contains("no backend")
        || lower.contains("no such interface")
        || lower.contains("not mountable")
    {
        return Some("gvfs proxy/backend unavailable");
    }
    ["not supported", "unsupported", "volume doesn"]
        .into_iter()
        .find(|marker| lower.contains(marker))
}

fn backend_error_message(stderr: &str) -> Option<&str> {
    gio_unavailable_message(stderr)
}

fn mount_already_present_message(stderr: &str) -> bool {
    stderr.to_lowercase().contains("already mounted")
}

async fn load_gio_mount_listing() -> Result<String, NetworkMountError> {
    let output = Command::new(GIO)
        .arg("mount")
        .arg("-l")
        .output()
        .await
        .map_err(|source| NetworkMountError::ListSpawn { source })?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if gio_unavailable_message(&stderr).is_some() {
        return Err(NetworkMountError::BackendUnavailable {
            uri: "gio mount -l".to_owned(),
            backend: "GVfs/GIO",
            reason: stderr,
        });
    }
    if !output.status.success() {
        return Err(NetworkMountError::ListFailed {
            status: output.status,
            stderr,
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn wait_for_gvfs_mount_path(
    connection: &NetworkConnection,
) -> Result<PathBuf, NetworkMountError> {
    let fuse_root = default_gvfs_fuse_root();
    let mut last_error = None;
    for _ in 0..GVFS_MOUNT_PATH_RETRIES {
        match resolve_gvfs_mount_path_from_root(connection, &fuse_root) {
            Ok(path) => return Ok(path),
            Err(NetworkMountError::MountPathUnavailable { .. }) => {
                last_error = Some(NetworkMountError::MountPathUnavailable {
                    uri: connection.uri.clone(),
                    root: fuse_root.clone(),
                });
                sleep(GVFS_MOUNT_PATH_RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(
        last_error.unwrap_or_else(|| NetworkMountError::MountPathUnavailable {
            uri: connection.uri.clone(),
            root: fuse_root,
        }),
    )
}

fn default_gvfs_fuse_root() -> PathBuf {
    crate::gvfs_paths::default_gvfs_fuse_root()
}

fn gvfs_mount_directory_matches(
    directory_name: &str,
    connection: &NetworkConnection,
) -> Result<bool, NetworkMountError> {
    let parts = parse_network_uri(&connection.uri)?;
    match connection.protocol {
        NetworkProtocol::Smb => {
            let Some(values) = directory_name
                .strip_prefix("smb-share:")
                .map(parse_gvfs_mount_values)
            else {
                return Ok(false);
            };
            Ok(values
                .get("server")
                .is_some_and(|server| server == &parts.host)
                && values
                    .get("share")
                    .is_some_and(|share| Some(share) == parts.path_segments.first()))
        }
        NetworkProtocol::WebDav => {
            let Some(values) = directory_name
                .strip_prefix("dav:")
                .map(parse_gvfs_mount_values)
            else {
                return Ok(false);
            };
            let ssl = if parts.canonical_scheme() == "davs" {
                "true"
            } else {
                "false"
            };
            Ok(values.get("host").is_some_and(|host| host == &parts.host)
                && values.get("ssl").is_some_and(|value| value == ssl)
                && values
                    .get("prefix")
                    .is_some_and(|prefix| prefix == &percent_encode_gvfs_prefix(&parts.path)))
        }
        NetworkProtocol::Sftp => {
            let Some(values) = directory_name
                .strip_prefix("sftp:")
                .map(parse_gvfs_mount_values)
            else {
                return Ok(false);
            };
            Ok(values.get("host").is_some_and(|host| host == &parts.host)
                && sftp_gvfs_user_matches(parts.username().as_deref(), values.get("user")))
        }
    }
}

fn gvfs_mount_path_for_connection(
    mount_root: PathBuf,
    connection: &NetworkConnection,
) -> Result<PathBuf, NetworkMountError> {
    if connection.protocol != NetworkProtocol::Sftp {
        return Ok(mount_root);
    }
    let parts = parse_network_uri(&connection.uri)?;
    Ok(parts
        .path_segments
        .iter()
        .fold(mount_root, |path, segment| path.join(segment)))
}

fn sftp_gvfs_user_matches(connection_user: Option<&str>, mount_user: Option<&String>) -> bool {
    match (connection_user, mount_user) {
        (Some(connection_user), Some(mount_user)) => {
            mount_user == connection_user
                || mount_user == &percent_encode_gvfs_prefix(connection_user)
        }
        (None, None) => true,
        _ => false,
    }
}

fn parse_gvfs_mount_values(value: &str) -> std::collections::HashMap<String, String> {
    value
        .split(',')
        .filter_map(|item| item.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
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
