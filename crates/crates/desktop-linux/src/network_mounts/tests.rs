use super::*;
use tempfile::tempdir;

fn connection(protocol: NetworkProtocol, uri: &str) -> NetworkConnection {
    NetworkConnection::new(NetworkConnectionId::new("id"), "", protocol, uri).unwrap()
}

#[test]
fn accepts_supported_network_uris_without_passwords() {
    assert!(validate_network_connection_uri(NetworkProtocol::Smb, "smb://server/share").is_ok());
    assert!(validate_network_connection_uri(
        NetworkProtocol::WebDav,
        "davs://user@example.test/docs"
    )
    .is_ok());
    assert!(
        validate_network_connection_uri(NetworkProtocol::WebDav, "dav://example.test/docs").is_ok()
    );
    assert!(validate_network_connection_uri(
        NetworkProtocol::WebDav,
        "https://user@example.test/docs"
    )
    .is_ok());
    assert!(validate_network_connection_uri(
        NetworkProtocol::Sftp,
        "sftp://user@example.test/srv/docs"
    )
    .is_ok());
    assert!(validate_network_connection_uri(NetworkProtocol::Sftp, "sftp://example.test").is_ok());
}

#[test]
fn normalizes_webdav_http_schemes_to_gvfs_mount_schemes() {
    let connection = NetworkConnection::new(
        NetworkConnectionId::new("docs"),
        "",
        NetworkProtocol::WebDav,
        "https://user@example.test/docs/",
    )
    .unwrap();

    assert_eq!(connection.uri, "davs://user@example.test/docs");
    assert!(network_uris_match(
        "https://example.test/docs/",
        "davs://example.test/docs"
    ));
}

#[test]
fn new_with_username_saves_username_without_password() {
    let connection = NetworkConnection::new_with_username(
        NetworkConnectionId::new("docs"),
        "",
        NetworkProtocol::WebDav,
        "https://webdav.123pan.cn/webdav",
        Some("user@example.com".to_owned()),
    )
    .unwrap();

    assert_eq!(
        connection.uri,
        "davs://user%40example.com@webdav.123pan.cn/webdav"
    );
    assert_eq!(connection.username().as_deref(), Some("user@example.com"));
    assert_eq!(
        connection.uri_without_username(),
        "davs://webdav.123pan.cn/webdav"
    );
}

#[test]
fn mount_uri_for_credentials_adds_username_without_password() {
    let connection = NetworkConnection::new(
        NetworkConnectionId::new("docs"),
        "",
        NetworkProtocol::WebDav,
        "davs://webdav.123pan.cn/webdav",
    )
    .unwrap();
    let credentials =
        NetworkMountCredentials::new(Some("user@example.com".to_owned()), "secret-password");

    let mount_uri = mount_uri_for_credentials(&connection, Some(&credentials)).unwrap();

    assert_eq!(
        mount_uri,
        "davs://user%40example.com@webdav.123pan.cn/webdav"
    );
    assert!(!mount_uri.contains("secret-password"));
}

#[test]
fn mount_uri_for_credentials_requires_username_when_password_is_provided() {
    let connection = NetworkConnection::new(
        NetworkConnectionId::new("docs"),
        "",
        NetworkProtocol::WebDav,
        "davs://webdav.123pan.cn/webdav",
    )
    .unwrap();
    let credentials = NetworkMountCredentials::new(None, "secret-password");

    let error = mount_uri_for_credentials(&connection, Some(&credentials)).unwrap_err();

    assert!(matches!(
        error,
        NetworkMountError::InvalidUri { message, .. }
            if message == "username is required when a password is provided"
    ));
}

#[test]
fn network_mount_credentials_debug_redacts_password() {
    let credentials = NetworkMountCredentials::new(Some("user".to_owned()), "secret-password");
    let debug = format!("{credentials:?}");

    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("secret-password"));
}

#[test]
fn smb_credential_stdin_accepts_default_domain_before_password() {
    let credentials = NetworkMountCredentials::new(Some("user".to_owned()), "secret-password");

    let input = gio_mount_credential_stdin(NetworkProtocol::Smb, &credentials);

    assert_eq!(input, b"\nsecret-password\n");
}

#[test]
fn webdav_credential_stdin_writes_only_password() {
    let credentials = NetworkMountCredentials::new(Some("user".to_owned()), "secret-password");

    let input = gio_mount_credential_stdin(NetworkProtocol::WebDav, &credentials);

    assert_eq!(input, b"secret-password\n");
}

#[test]
fn sftp_credential_stdin_writes_only_password() {
    let credentials = NetworkMountCredentials::new(Some("user".to_owned()), "secret-password");

    let input = gio_mount_credential_stdin(NetworkProtocol::Sftp, &credentials);

    assert_eq!(input, b"secret-password\n");
}

#[test]
fn rejects_unsupported_or_password_network_uris() {
    assert!(validate_network_connection_uri(NetworkProtocol::Smb, "ftp://server/share").is_err());
    assert!(validate_network_connection_uri(NetworkProtocol::Smb, "smb:///share").is_err());
    assert!(validate_network_connection_uri(NetworkProtocol::Smb, "smb://server").is_err());
    assert!(validate_network_connection_uri(
        NetworkProtocol::WebDav,
        "davs://user:secret@host/docs"
    )
    .is_err());
    assert!(validate_network_connection_uri(
        NetworkProtocol::Sftp,
        "sftp://user:secret@host/srv/docs"
    )
    .is_err());
    assert!(validate_network_connection_uri(NetworkProtocol::Sftp, "ssh://host/srv/docs").is_err());
}

#[test]
fn parses_gio_mount_listing_uris() {
    let output = r#"
Mount(0): Photos -> smb://server/photos/
  Type: GDaemonMount
  default_location=smb://server/photos/
Mount(1): Docs -> davs://example.test/docs
  default_location=davs://example.test/docs
Mount(2): SFTP -> sftp://smbtest@172.31.240.10/
  Type: GDaemonMount
"#;

    let uris = parse_gio_mount_uris(output);

    assert_eq!(
        uris,
        vec![
            "smb://server/photos/".to_owned(),
            "davs://example.test/docs".to_owned(),
            "sftp://smbtest@172.31.240.10/".to_owned()
        ]
    );
}

#[test]
fn resolves_smb_gvfs_mount_path_from_fixture_root() {
    let root = tempdir().unwrap();
    let mount_dir = root.path().join("smb-share:server=server,share=photos");
    std::fs::create_dir(&mount_dir).unwrap();
    let connection = connection(NetworkProtocol::Smb, "smb://server/photos");

    let resolved = resolve_gvfs_mount_path_from_root(&connection, root.path()).unwrap();

    assert_eq!(resolved, mount_dir);
}

#[test]
fn resolves_smb_gvfs_mount_path_with_extra_keys() {
    let root = tempdir().unwrap();
    let mount_dir = root
        .path()
        .join("smb-share:server=server,share=photos,user=ym");
    std::fs::create_dir(&mount_dir).unwrap();
    let connection = connection(NetworkProtocol::Smb, "smb://ym@server/photos");

    let resolved = resolve_gvfs_mount_path_from_root(&connection, root.path()).unwrap();

    assert_eq!(resolved, mount_dir);
}

#[test]
fn resolves_webdav_gvfs_mount_path_from_fixture_root() {
    let root = tempdir().unwrap();
    let mount_dir = root
        .path()
        .join("dav:host=example.test,ssl=true,prefix=%2Fdocs");
    std::fs::create_dir(&mount_dir).unwrap();
    let connection = connection(NetworkProtocol::WebDav, "davs://example.test/docs");

    let resolved = resolve_gvfs_mount_path_from_root(&connection, root.path()).unwrap();

    assert_eq!(resolved, mount_dir);
}

#[test]
fn resolves_sftp_gvfs_mount_path_from_fixture_root() {
    let root = tempdir().unwrap();
    let mount_dir = root.path().join("sftp:host=server,user=ym");
    std::fs::create_dir(&mount_dir).unwrap();
    let connection = connection(NetworkProtocol::Sftp, "sftp://ym@server/srv/share");

    let resolved = resolve_gvfs_mount_path_from_root(&connection, root.path()).unwrap();

    assert_eq!(resolved, mount_dir.join("srv").join("share"));
}

#[test]
fn sftp_mounted_uri_matches_host_and_user_without_remote_path() {
    let connection = connection(NetworkProtocol::Sftp, "sftp://ym@server/srv/share");

    assert!(network_connection_matches_mounted_uri(
        &connection,
        "sftp://ym@server/"
    ));
    assert!(!network_connection_matches_mounted_uri(
        &connection,
        "sftp://other@server/"
    ));
}

#[test]
fn missing_fuse_root_is_structured_error() {
    let root = tempdir().unwrap().path().join("missing");
    let connection = connection(NetworkProtocol::Smb, "smb://server/photos");

    let error = resolve_gvfs_mount_path_from_root(&connection, &root).unwrap_err();

    assert!(matches!(error, NetworkMountError::FuseUnavailable { .. }));
}

#[cfg(unix)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    ExitStatus::from_raw(code << 8)
}

#[cfg(unix)]
#[test]
fn mount_backend_failure_keeps_backend_and_uri() {
    let connection = connection(NetworkProtocol::WebDav, "davs://example.test/docs");

    let error = mount_command_error(
        &connection,
        exit_status(1),
        "volume doesn't implement mount".to_owned(),
    );

    assert!(matches!(
        error,
        NetworkMountError::BackendUnavailable {
            uri,
            backend: "gvfs-dav",
            reason,
        } if uri == "davs://example.test/docs" && reason == "volume doesn't implement mount"
    ));
}

#[cfg(unix)]
#[test]
fn gio_proxy_failure_is_backend_unavailable() {
    let connection = connection(NetworkProtocol::Smb, "smb://server/photos");

    let error = mount_command_error(
        &connection,
        exit_status(1),
        "Error creating proxy: Could not connect: Operation not permitted".to_owned(),
    );

    assert!(matches!(
        error,
        NetworkMountError::BackendUnavailable {
            uri,
            backend: "gvfs-smb",
            reason,
        } if uri == "smb://server/photos"
            && reason.contains("Operation not permitted")
    ));
}

#[cfg(unix)]
#[test]
fn not_mountable_failure_is_backend_unavailable() {
    let connection = connection(NetworkProtocol::WebDav, "davs://example.test/docs");

    let error = mount_command_error(
        &connection,
        exit_status(1),
        "Location is not mountable".to_owned(),
    );

    assert!(matches!(
        error,
        NetworkMountError::BackendUnavailable {
            uri,
            backend: "gvfs-dav",
            reason,
        } if uri == "davs://example.test/docs" && reason == "Location is not mountable"
    ));
}

#[test]
fn already_mounted_message_is_mount_success_boundary() {
    assert!(mount_already_present_message("Location is already mounted"));
    assert!(mount_already_present_message(
        "gio: davs://example.test/docs/: Location is already mounted"
    ));
    assert!(!mount_already_present_message("authentication failed"));
}

#[cfg(unix)]
#[test]
fn mount_failure_keeps_status_stderr_and_uri() {
    let connection = connection(NetworkProtocol::Smb, "smb://server/photos");

    let error = mount_command_error(
        &connection,
        exit_status(2),
        "authentication failed".to_owned(),
    );

    assert!(matches!(
        error,
        NetworkMountError::MountFailed {
            uri,
            stderr,
            ..
        } if uri == "smb://server/photos" && stderr == "authentication failed"
    ));
}
