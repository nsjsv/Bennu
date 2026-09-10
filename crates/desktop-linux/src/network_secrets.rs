use std::io;
use std::process::{ExitStatus, Output, Stdio};

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::network_mounts::{NetworkConnection, NetworkMountCredentials};

const SECRET_TOOL: &str = "secret-tool";
const APP_ATTRIBUTE: &str = "bennu";
/// 改名前的 keyring application 属性；仅用于读取回退与一次性迁移，
/// 新写入一律使用 APP_ATTRIBUTE。
const LEGACY_APP_ATTRIBUTE: &str = "file-manager";
const KIND_ATTRIBUTE: &str = "network-connection-password";

#[derive(Debug, Error)]
pub enum NetworkSecretError {
    #[error("could not run secret-tool to {action} network password for {uri:?}: {source}")]
    CommandSpawn {
        action: &'static str,
        uri: String,
        #[source]
        source: io::Error,
    },
    #[error("could not send network password to secret-tool for {uri:?}: {source}")]
    SecretWrite {
        uri: String,
        #[source]
        source: io::Error,
    },
    #[error("secret-tool failed to {action} network password for {uri:?} with status {status}: {stderr}")]
    CommandFailed {
        action: &'static str,
        uri: String,
        status: ExitStatus,
        stderr: String,
    },
}

pub async fn lookup_network_connection_credentials(
    connection: NetworkConnection,
) -> Result<Option<NetworkMountCredentials>, NetworkSecretError> {
    let key = NetworkSecretKey::from_connection(&connection);
    if let Some(credentials) = lookup_by_attribute(&connection, &key, APP_ATTRIBUTE).await? {
        return Ok(Some(credentials));
    }

    // 新属性未命中：回退读改名前遗留的条目，命中则一次性搬运到新属性，
    // 避免旧条目永久孤儿化。
    let Some(credentials) = lookup_by_attribute(&connection, &key, LEGACY_APP_ATTRIBUTE).await?
    else {
        return Ok(None);
    };
    store_network_connection_credentials(connection.clone(), credentials.clone()).await?;
    let legacy_output =
        run_secret_tool_command("clear", &connection, key.legacy_clear_args()).await?;
    if !legacy_output.status.success() && !stderr_text(&legacy_output).is_empty() {
        return Err(secret_tool_failed(
            "clear",
            &connection,
            legacy_output.status,
            stderr_text(&legacy_output),
        ));
    }
    Ok(Some(credentials))
}

async fn lookup_by_attribute(
    connection: &NetworkConnection,
    key: &NetworkSecretKey,
    app_attribute: &str,
) -> Result<Option<NetworkMountCredentials>, NetworkSecretError> {
    let output =
        run_secret_tool_command("lookup", connection, key.lookup_args_for(app_attribute)).await?;
    if !output.status.success() {
        let stderr = stderr_text(&output);
        if stderr.is_empty() {
            return Ok(None);
        }
        return Err(secret_tool_failed(
            "lookup",
            connection,
            output.status,
            stderr,
        ));
    }
    let Some(password) = password_from_lookup_stdout(&output.stdout) else {
        return Ok(None);
    };
    Ok(Some(NetworkMountCredentials::new(
        connection.username(),
        password,
    )))
}

pub async fn store_network_connection_credentials(
    connection: NetworkConnection,
    credentials: NetworkMountCredentials,
) -> Result<(), NetworkSecretError> {
    if credentials.is_empty() {
        return Ok(());
    }
    let key = NetworkSecretKey::from_connection(&connection);
    let mut child = Command::new(SECRET_TOOL)
        .args(key.store_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| NetworkSecretError::CommandSpawn {
            action: "store",
            uri: connection.uri.clone(),
            source,
        })?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(NetworkSecretError::SecretWrite {
            uri: connection.uri.clone(),
            source: io::Error::new(
                io::ErrorKind::BrokenPipe,
                "secret-tool stdin was unavailable",
            ),
        });
    };
    stdin
        .write_all(credentials.password.as_bytes())
        .await
        .map_err(|source| NetworkSecretError::SecretWrite {
            uri: connection.uri.clone(),
            source,
        })?;
    drop(stdin);

    let output =
        child
            .wait_with_output()
            .await
            .map_err(|source| NetworkSecretError::CommandSpawn {
                action: "store",
                uri: connection.uri.clone(),
                source,
            })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(secret_tool_failed(
            "store",
            &connection,
            output.status,
            stderr_text(&output),
        ))
    }
}

pub async fn clear_network_connection_credentials(
    connection: NetworkConnection,
) -> Result<(), NetworkSecretError> {
    let key = NetworkSecretKey::from_connection(&connection);
    let output = run_secret_tool_command("clear", &connection, key.clear_args()).await?;
    if output.status.success() || stderr_text(&output).is_empty() {
        return Ok(());
    }
    // 新属性未命中：清理改名前遗留的条目。
    let legacy_output =
        run_secret_tool_command("clear", &connection, key.legacy_clear_args()).await?;
    if legacy_output.status.success() || stderr_text(&legacy_output).is_empty() {
        Ok(())
    } else {
        Err(secret_tool_failed(
            "clear",
            &connection,
            legacy_output.status,
            stderr_text(&legacy_output),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkSecretKey {
    id: String,
    protocol: String,
    uri: String,
    username: String,
}

impl NetworkSecretKey {
    fn from_connection(connection: &NetworkConnection) -> Self {
        Self {
            id: connection.id.as_str().to_owned(),
            protocol: connection.protocol.config_value().to_owned(),
            uri: connection.uri.clone(),
            username: connection.username().unwrap_or_default(),
        }
    }

    fn clear_args(&self) -> Vec<String> {
        self.command_args("clear")
    }

    fn legacy_clear_args(&self) -> Vec<String> {
        let mut args = vec!["clear".to_owned()];
        args.extend(self.attribute_args_for(LEGACY_APP_ATTRIBUTE));
        args
    }

    fn lookup_args_for(&self, app_attribute: &str) -> Vec<String> {
        let mut args = vec!["lookup".to_owned()];
        args.extend(self.attribute_args_for(app_attribute));
        args
    }

    fn store_args(&self) -> Vec<String> {
        let mut args = vec!["store".to_owned(), format!("--label={}", self.label())];
        args.extend(self.attribute_args());
        args
    }

    fn command_args(&self, command: &str) -> Vec<String> {
        let mut args = vec![command.to_owned()];
        args.extend(self.attribute_args());
        args
    }

    fn attribute_args(&self) -> Vec<String> {
        self.attribute_args_for(APP_ATTRIBUTE)
    }

    fn attribute_args_for(&self, app_attribute: &str) -> Vec<String> {
        vec![
            "application".to_owned(),
            app_attribute.to_owned(),
            "kind".to_owned(),
            KIND_ATTRIBUTE.to_owned(),
            "id".to_owned(),
            self.id.clone(),
            "protocol".to_owned(),
            self.protocol.clone(),
            "uri".to_owned(),
            self.uri.clone(),
            "username".to_owned(),
            self.username.clone(),
        ]
    }

    fn label(&self) -> String {
        format!("Bennu network password ({})", self.id)
    }
}

async fn run_secret_tool_command(
    action: &'static str,
    connection: &NetworkConnection,
    args: Vec<String>,
) -> Result<Output, NetworkSecretError> {
    Command::new(SECRET_TOOL)
        .args(args)
        .output()
        .await
        .map_err(|source| NetworkSecretError::CommandSpawn {
            action,
            uri: connection.uri.clone(),
            source,
        })
}

fn secret_tool_failed(
    action: &'static str,
    connection: &NetworkConnection,
    status: ExitStatus,
    stderr: String,
) -> NetworkSecretError {
    NetworkSecretError::CommandFailed {
        action,
        uri: connection.uri.clone(),
        status,
        stderr,
    }
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

fn password_from_lookup_stdout(stdout: &[u8]) -> Option<String> {
    let mut bytes = stdout;
    if let Some(stripped) = bytes.strip_suffix(b"\n") {
        bytes = stripped;
    }
    if let Some(stripped) = bytes.strip_suffix(b"\r") {
        bytes = stripped;
    }
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_mounts::{NetworkConnectionId, NetworkProtocol};

    fn connection() -> NetworkConnection {
        NetworkConnection::new_with_username(
            NetworkConnectionId::new("nas"),
            "NAS",
            NetworkProtocol::Smb,
            "smb://server/share",
            Some("user".to_owned()),
        )
        .unwrap()
    }

    #[test]
    fn secret_key_includes_remote_identity() {
        let key = NetworkSecretKey::from_connection(&connection());

        assert_eq!(key.id, "nas");
        assert_eq!(key.protocol, "smb");
        assert_eq!(key.uri, "smb://user@server/share");
        assert_eq!(key.username, "user");
        assert!(key
            .attribute_args()
            .windows(2)
            .any(|pair| pair == ["kind", KIND_ATTRIBUTE]));
    }

    #[test]
    fn store_args_do_not_include_password() {
        let key = NetworkSecretKey::from_connection(&connection());
        let args = key.store_args();

        assert!(args.contains(&"store".to_owned()));
        assert!(args.iter().any(|arg| arg.starts_with("--label=")));
        assert!(!args.iter().any(|arg| arg.contains("secret-password")));
    }

    #[test]
    fn lookup_stdout_strips_line_ending_without_trimming_spaces() {
        assert_eq!(
            password_from_lookup_stdout(b" secret-password \n").as_deref(),
            Some(" secret-password ")
        );
        assert_eq!(
            password_from_lookup_stdout(b"secret-password\r\n").as_deref(),
            Some("secret-password")
        );
    }

    #[test]
    fn empty_lookup_stdout_is_missing_secret() {
        assert!(password_from_lookup_stdout(b"").is_none());
        assert!(password_from_lookup_stdout(b"\n").is_none());
    }
}
