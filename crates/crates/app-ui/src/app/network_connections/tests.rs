use std::path::PathBuf;

use desktop_linux::{
    MountedNetworkConnection, NetworkConnection, NetworkMountCredentials, NetworkMountState,
};

use super::*;
use crate::config;
use crate::network_connections::{
    NetworkConnectionCredentialFallback, NetworkConnectionEditorMode,
    NetworkConnectionMountCompletion,
};

fn connection() -> NetworkConnection {
    NetworkConnection::new(
        NetworkConnectionId::new("nas"),
        "NAS",
        NetworkProtocol::Smb,
        "smb://server/share",
    )
    .unwrap()
}

fn webdav_connection() -> NetworkConnection {
    NetworkConnection::new(
        NetworkConnectionId::new("webdav"),
        "WebDAV",
        NetworkProtocol::WebDav,
        "davs://webdav.123pan.cn/webdav",
    )
    .unwrap()
}

fn sftp_connection() -> NetworkConnection {
    NetworkConnection::new(
        NetworkConnectionId::new("sftp"),
        "SFTP",
        NetworkProtocol::Sftp,
        "sftp://sftp.example.test/srv/share",
    )
    .unwrap()
}

fn authenticated_smb_connection() -> NetworkConnection {
    NetworkConnection::new_with_username(
        NetworkConnectionId::new("smb-auth"),
        "SMB Auth",
        NetworkProtocol::Smb,
        "smb://server/share",
        Some("smbtest".to_owned()),
    )
    .unwrap()
}

fn browser_with_connection() -> (FileBrowser, NetworkConnectionId) {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let connection = connection();
    let id = connection.id.clone();
    browser.network_connections =
        crate::network_connections::NetworkConnectionState::from_connections(vec![connection]);
    (browser, id)
}

fn browser_with_webdav_connection() -> (FileBrowser, NetworkConnectionId) {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let connection = webdav_connection();
    let id = connection.id.clone();
    browser.network_connections =
        crate::network_connections::NetworkConnectionState::from_connections(vec![connection]);
    (browser, id)
}

fn browser_with_sftp_connection() -> (FileBrowser, NetworkConnectionId) {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let connection = sftp_connection();
    let id = connection.id.clone();
    browser.network_connections =
        crate::network_connections::NetworkConnectionState::from_connections(vec![connection]);
    (browser, id)
}

fn browser_with_authenticated_smb_connection() -> (FileBrowser, NetworkConnectionId) {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let connection = authenticated_smb_connection();
    let id = connection.id.clone();
    browser.network_connections =
        crate::network_connections::NetworkConnectionState::from_connections(vec![connection]);
    (browser, id)
}

fn first_saved_config_connection(browser: &FileBrowser) -> &NetworkConnection {
    &browser
        .user_config
        .network_connections
        .first()
        .expect("saved connection")
        .connection
}

fn mounted_connection(connection: NetworkConnection, mount_path: &str) -> MountedNetworkConnection {
    MountedNetworkConnection {
        connection,
        mount_path: PathBuf::from(mount_path),
    }
}

fn open_missing_credentials_editor(browser: &mut FileBrowser, id: &NetworkConnectionId) {
    let connection = browser.network_connections.connection(id).unwrap().clone();
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::StoredCredentialsLoaded(
            connection,
            NetworkConnectionMountCompletion::NavigateToMount,
            NetworkConnectionCredentialFallback::OpenEditor,
            Ok(None),
        ),
    ));
}

#[test]
fn add_network_connection_editor_starts_with_empty_uri() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));

    let editor = browser
        .network_connection_editor
        .as_ref()
        .expect("network connection editor");
    assert_eq!(editor.mode, NetworkConnectionEditorMode::Add);
    assert_eq!(editor.protocol, NetworkProtocol::Smb);
    assert!(editor.uri.is_empty());
}

#[test]
fn saving_bare_smb_connection_prefixes_smb_scheme() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorLabelChanged(
            "NAS".to_owned(),
        )),
    );
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "server/share".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved_connection = first_saved_config_connection(&browser);
    assert_eq!(saved_connection.protocol, NetworkProtocol::Smb);
    assert_eq!(saved_connection.uri, "smb://server/share");
    assert!(browser.network_connection_editor.is_none());
}

#[test]
fn add_editor_saves_auto_connect_preference() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorAutoConnectToggled(true),
    ));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "server/share".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved = browser
        .user_config
        .network_connections
        .first()
        .expect("saved connection");
    assert!(saved.auto_connect);
    assert!(browser
        .network_connections
        .auto_connect_for(&saved.connection.id));
}

#[test]
fn edit_editor_updates_auto_connect_preference() {
    let (mut browser, id) = browser_with_connection();

    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::ActionSelected(
            id.clone(),
            SidebarNetworkConnectionAction::Edit,
        )),
    );
    assert!(
        !browser
            .network_connection_editor
            .as_ref()
            .expect("edit editor")
            .auto_connect
    );
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorAutoConnectToggled(true),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    assert!(browser.network_connections.auto_connect_for(&id));
    assert!(browser.user_config.network_connections[0].auto_connect);
}

#[test]
fn edit_editor_disables_auto_connect_preference() {
    let (mut browser, id) = browser_with_connection();
    browser.network_connections.entries[0].auto_connect = true;
    browser.user_config.network_connections = browser.network_connections.saved_connections();

    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::ActionSelected(
            id.clone(),
            SidebarNetworkConnectionAction::Edit,
        )),
    );
    assert!(
        browser
            .network_connection_editor
            .as_ref()
            .expect("edit editor")
            .auto_connect
    );
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorAutoConnectToggled(false),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    assert!(!browser.network_connections.auto_connect_for(&id));
    assert!(!browser.user_config.network_connections[0].auto_connect);
    assert!(browser
        .network_connections
        .auto_connect_connections()
        .is_empty());
}

#[test]
fn connect_editor_preserves_auto_connect_preference() {
    let (mut browser, id) = browser_with_authenticated_smb_connection();
    browser.network_connections.entries[0].auto_connect = true;
    browser.user_config.network_connections = browser.network_connections.saved_connections();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    assert!(browser.network_connections.auto_connect_for(&id));
    assert!(browser.user_config.network_connections[0].auto_connect);
}

#[test]
fn startup_auto_connect_uses_persisted_preference() {
    let (mut browser, id) = browser_with_connection();
    browser.network_connections.entries[0].auto_connect = true;
    browser.user_config.network_connections = browser.network_connections.saved_connections();
    browser.user_config.network_connections[0].auto_connect = false;

    drop(browser.startup_auto_connect_network_connections());

    assert!(!browser.network_connections.is_pending(&id));
    assert!(matches!(
        browser
            .network_connections
            .entry(&id)
            .map(|entry| &entry.state),
        Some(NetworkMountState::Disconnected)
    ));
}

#[test]
fn saving_bare_smb_server_without_share_keeps_platform_validation() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "server".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    assert!(browser.user_config.network_connections.is_empty());
    assert!(browser
        .network_connection_editor
        .as_ref()
        .and_then(|editor| editor.error.as_deref())
        .is_some_and(|error| error.contains("SMB URI must include a share name")));
}

#[test]
fn saving_bare_webdav_connection_prefixes_https_for_platform_normalization() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorProtocolSelected(NetworkProtocol::WebDav),
    ));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorLabelChanged(
            "Docs".to_owned(),
        )),
    );
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "example.test/docs".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved_connection = first_saved_config_connection(&browser);
    assert_eq!(saved_connection.protocol, NetworkProtocol::WebDav);
    assert_eq!(saved_connection.uri, "davs://example.test/docs");
    assert!(browser.network_connection_editor.is_none());
}

#[test]
fn saving_explicit_webdav_scheme_preserves_scheme_semantics() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorProtocolSelected(NetworkProtocol::WebDav),
    ));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "http://example.test/docs".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved_connection = first_saved_config_connection(&browser);
    assert_eq!(saved_connection.uri, "dav://example.test/docs");
}

#[test]
fn saving_explicit_smb_scheme_preserves_scheme() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "smb://server/share".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved_connection = first_saved_config_connection(&browser);
    assert_eq!(saved_connection.uri, "smb://server/share");
}

#[test]
fn saving_bare_sftp_connection_prefixes_sftp_scheme() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorProtocolSelected(NetworkProtocol::Sftp),
    ));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorLabelChanged(
            "SFTP".to_owned(),
        )),
    );
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "example.test/srv/share".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved_connection = first_saved_config_connection(&browser);
    assert_eq!(saved_connection.protocol, NetworkProtocol::Sftp);
    assert_eq!(saved_connection.uri, "sftp://example.test/srv/share");
    assert!(browser.network_connection_editor.is_none());
}

#[test]
fn saving_explicit_sftp_scheme_preserves_scheme() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorProtocolSelected(NetworkProtocol::Sftp),
    ));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "sftp://example.test/srv/share".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved_connection = first_saved_config_connection(&browser);
    assert_eq!(saved_connection.uri, "sftp://example.test/srv/share");
}

#[test]
fn saving_bare_webdav_credentials_keeps_password_out_of_config() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::AddRequested));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorProtocolSelected(NetworkProtocol::WebDav),
    ));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "example.test/docs".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorUsernameChanged("user@example.com".to_owned()),
    ));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));

    let saved_connection = first_saved_config_connection(&browser);
    assert_eq!(
        saved_connection.uri,
        "davs://user%40example.com@example.test/docs"
    );
    assert!(!saved_connection.uri.contains("secret-password"));
    assert!(!browser
        .user_config
        .network_connections
        .iter()
        .any(|saved| saved.connection.uri.contains("secret-password")));
}

#[test]
fn pressing_mounted_network_connection_navigates_to_mount_path() {
    let (mut browser, id) = browser_with_connection();
    let mount_path = PathBuf::from("/run/user/1000/gvfs/smb-share:server=server,share=share");
    browser.network_connections.accept_loaded(vec![(
        id.clone(),
        NetworkMountState::Mounted(mount_path.clone()),
    )]);

    let command = browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id));
    drop(command);

    assert_eq!(browser.current_dir, mount_path);
    assert!(browser.network_connections.pending_actions.is_empty());
}

#[test]
fn pressing_disconnected_network_connection_marks_it_connecting() {
    let (mut browser, id) = browser_with_connection();

    let command =
        browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone()));
    drop(command);

    assert!(browser.network_connections.is_pending(&id));
    assert!(matches!(
        browser
            .network_connections
            .entry(&id)
            .map(|entry| &entry.state),
        Some(NetworkMountState::Connecting)
    ));
}

#[test]
fn pressing_authenticated_smb_connection_looks_up_stored_credentials() {
    let (mut browser, id) = browser_with_authenticated_smb_connection();

    let command =
        browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone()));
    drop(command);

    assert!(browser.network_connections.is_pending(&id));
    assert!(browser.network_connection_editor.is_none());
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::StoredCredentialsLoaded(
            browser.network_connections.connection(&id).unwrap().clone(),
            NetworkConnectionMountCompletion::NavigateToMount,
            NetworkConnectionCredentialFallback::OpenEditor,
            Ok(Some(NetworkMountCredentials::new(
                Some("smbtest".to_owned()),
                "secret-password",
            ))),
        ),
    ));
    assert!(browser.network_connection_editor.is_none());
    assert!(browser.network_connections.is_pending(&id));
}

#[test]
fn submitting_smb_credentials_saves_username_without_password() {
    let (mut browser, id) = browser_with_authenticated_smb_connection();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    let command = browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved);
    drop(command);

    let saved_connection = browser.network_connections.connection(&id).unwrap();
    assert_eq!(saved_connection.uri, "smb://smbtest@server/share");
    assert!(browser.network_connections.is_pending(&id));
    assert!(browser
        .user_config
        .network_connections
        .iter()
        .all(|saved| !saved.connection.uri.contains("secret-password")));
    assert!(browser.network_connection_editor.is_none());
}

#[test]
fn disconnect_keeps_password_for_direct_reconnect() {
    let (mut browser, id) = browser_with_authenticated_smb_connection();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));
    let mounted = mounted_connection(
        browser.network_connections.connection(&id).unwrap().clone(),
        "/run/user/1000/gvfs/smb-share:server=server,share=share",
    );
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::MountFinished(
            browser.network_connections.connection(&id).unwrap().clone(),
            NetworkConnectionMountCompletion::NavigateToMount,
            Ok(mounted),
        )),
    );

    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::ActionSelected(
            id.clone(),
            SidebarNetworkConnectionAction::Disconnect,
        )),
    );
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::UnmountFinished(
            browser.network_connections.connection(&id).unwrap().clone(),
            Ok(()),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));

    assert!(browser.network_connection_editor.is_none());
    assert!(browser.network_connections.is_pending(&id));
}

#[test]
fn failed_remembered_credential_mount_clears_password() {
    let (mut browser, id) = browser_with_authenticated_smb_connection();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::MountFinished(
            browser.network_connections.connection(&id).unwrap().clone(),
            NetworkConnectionMountCompletion::NavigateToMount,
            Err("authentication failed".to_owned()),
        )),
    );

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);

    assert!(matches!(
        browser
            .network_connection_editor
            .as_ref()
            .map(|editor| editor.mode),
        Some(NetworkConnectionEditorMode::Connect)
    ));
}

#[test]
fn editing_remote_identity_clears_remembered_password() {
    let (mut browser, id) = browser_with_authenticated_smb_connection();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));
    let current_connection = browser.network_connections.connection(&id).unwrap().clone();
    browser
        .network_connections
        .accept_unmounted(&current_connection);

    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::ActionSelected(
            id.clone(),
            SidebarNetworkConnectionAction::Edit,
        )),
    );
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorUriChanged(
            "other/share".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);

    assert!(matches!(
        browser
            .network_connection_editor
            .as_ref()
            .map(|editor| editor.mode),
        Some(NetworkConnectionEditorMode::Connect)
    ));
}

#[test]
fn editing_label_only_keeps_remembered_password() {
    let (mut browser, id) = browser_with_authenticated_smb_connection();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));
    let current_connection = browser.network_connections.connection(&id).unwrap().clone();
    browser
        .network_connections
        .accept_unmounted(&current_connection);

    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::ActionSelected(
            id.clone(),
            SidebarNetworkConnectionAction::Edit,
        )),
    );
    drop(
        browser.handle_network_connection_message(NetworkConnectionMessage::EditorLabelChanged(
            "Renamed".to_owned(),
        )),
    );
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved));
    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));

    assert!(browser.network_connection_editor.is_none());
    assert!(browser.network_connections.is_pending(&id));
}

#[test]
fn pressing_disconnected_webdav_connection_opens_connect_editor() {
    let (mut browser, id) = browser_with_webdav_connection();

    let command =
        browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone()));
    drop(command);
    open_missing_credentials_editor(&mut browser, &id);

    assert!(browser.network_connections.pending_actions.is_empty());
    assert!(matches!(
        browser
            .network_connection_editor
            .as_ref()
            .map(|editor| editor.mode),
        Some(NetworkConnectionEditorMode::Connect)
    ));
}

#[test]
fn submitting_webdav_credentials_saves_username_without_password() {
    let (mut browser, id) = browser_with_webdav_connection();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorUsernameChanged("user@example.com".to_owned()),
    ));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    let command = browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved);
    drop(command);

    let saved_connection = browser.network_connections.connection(&id).unwrap();
    assert_eq!(
        saved_connection.uri,
        "davs://user%40example.com@webdav.123pan.cn/webdav"
    );
    assert!(browser.network_connections.is_pending(&id));
    assert!(browser
        .user_config
        .network_connections
        .iter()
        .all(|saved| !saved.connection.uri.contains("secret-password")));
    assert!(browser.network_connection_editor.is_none());
}

#[test]
fn submitting_sftp_credentials_saves_username_without_password() {
    let (mut browser, id) = browser_with_sftp_connection();

    drop(browser.handle_network_connection_message(NetworkConnectionMessage::Pressed(id.clone())));
    open_missing_credentials_editor(&mut browser, &id);
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorUsernameChanged("sftp-user".to_owned()),
    ));
    drop(browser.handle_network_connection_message(
        NetworkConnectionMessage::EditorPasswordChanged("secret-password".to_owned()),
    ));
    let command = browser.handle_network_connection_message(NetworkConnectionMessage::EditorSaved);
    drop(command);

    let saved_connection = browser.network_connections.connection(&id).unwrap();
    assert_eq!(
        saved_connection.uri,
        "sftp://sftp-user@sftp.example.test/srv/share"
    );
    assert!(browser.network_connections.is_pending(&id));
    assert!(browser
        .user_config
        .network_connections
        .iter()
        .all(|saved| !saved.connection.uri.contains("secret-password")));
    assert!(browser.network_connection_editor.is_none());
}

#[test]
fn mount_failure_sets_global_error_and_entry_error() {
    let (mut browser, id) = browser_with_connection();

    let command =
        browser.handle_network_connection_message(NetworkConnectionMessage::MountFinished(
            browser.network_connections.connection(&id).unwrap().clone(),
            NetworkConnectionMountCompletion::NavigateToMount,
            Err("authentication failed".to_owned()),
        ));
    drop(command);

    assert_eq!(
        browser.current_error(),
        Some("Could not connect network location: authentication failed")
    );
    assert!(matches!(
        browser.network_connections.entry(&id).map(|entry| &entry.state),
        Some(NetworkMountState::Error(error)) if error == "authentication failed"
    ));
}

#[test]
fn refresh_only_mount_success_does_not_navigate() {
    let (mut browser, id) = browser_with_connection();
    browser.current_dir = PathBuf::from("/home/user");
    let mounted = mounted_connection(
        connection(),
        "/run/user/1000/gvfs/smb-share:server=server,share=share",
    );

    let command =
        browser.handle_network_connection_message(NetworkConnectionMessage::MountFinished(
            mounted.connection.clone(),
            NetworkConnectionMountCompletion::RefreshOnly,
            Ok(mounted),
        ));
    drop(command);

    assert_eq!(browser.current_dir, PathBuf::from("/home/user"));
    assert!(matches!(
        browser.network_connections.entry(&id).map(|entry| &entry.state),
        Some(NetworkMountState::Mounted(path))
            if path == &PathBuf::from("/run/user/1000/gvfs/smb-share:server=server,share=share")
    ));
}
