use super::*;
use gio::glib::prelude::ToValue;
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

fn credentials() -> NetworkMountCredentials {
    NetworkMountCredentials::new(Some("ym".to_owned()), "secret-password")
}

#[test]
fn password_answer_fills_only_what_gvfs_asks_for() {
    let all = gio::AskPasswordFlags::NEED_USERNAME
        | gio::AskPasswordFlags::NEED_DOMAIN
        | gio::AskPasswordFlags::NEED_PASSWORD;
    // SMB 问域时接受服务端给的默认域（原先往 gio 标准输入写空行）。
    assert_eq!(
        password_answer(Some(&credentials()), false, "guest", "WORKGROUP", all),
        PasswordAnswer::Provide {
            username: Some("ym".to_owned()),
            domain: Some("WORKGROUP".to_owned()),
            password: Some("secret-password".to_owned()),
        }
    );
    assert_eq!(
        password_answer(
            Some(&credentials()),
            false,
            "guest",
            "WORKGROUP",
            gio::AskPasswordFlags::NEED_PASSWORD
        ),
        PasswordAnswer::Provide {
            username: None,
            domain: None,
            password: Some("secret-password".to_owned()),
        }
    );
    let without_username = NetworkMountCredentials::new(None, "secret-password");
    assert!(matches!(
        password_answer(Some(&without_username), false, "guest", "", all),
        PasswordAnswer::Provide { username: Some(username), .. } if username == "guest"
    ));
}

#[test]
fn password_is_answered_once_and_never_without_credentials() {
    let flags = gio::AskPasswordFlags::NEED_PASSWORD;
    assert_eq!(
        password_answer(Some(&credentials()), true, "", "", flags),
        PasswordAnswer::Decline
    );
    assert_eq!(
        password_answer(None, false, "", "", flags),
        PasswordAnswer::Decline
    );
    assert_eq!(
        password_answer(
            Some(&NetworkMountCredentials::new(Some("ym".to_owned()), "")),
            false,
            "",
            "",
            flags
        ),
        PasswordAnswer::Decline
    );
}

#[test]
fn mount_operation_answers_the_first_password_request_and_declines_the_rest() {
    let declined = Rc::new(RefCell::new(None));
    let operation = mount_operation(Some(credentials()), declined.clone());
    let replies = Rc::new(RefCell::new(Vec::new()));
    let recorded = replies.clone();
    operation.connect_reply(move |_, result| recorded.borrow_mut().push(result));
    let ask = || {
        operation.emit_by_name::<()>(
            "ask-password",
            &[
                &"Password required",
                &"guest",
                &"WORKGROUP",
                &gio::AskPasswordFlags::NEED_PASSWORD,
            ],
        )
    };

    ask();
    assert_eq!(operation.password().as_deref(), Some("secret-password"));
    assert_eq!(*declined.borrow(), None);
    ask();

    assert_eq!(
        *replies.borrow(),
        [
            gio::MountOperationResult::Handled,
            gio::MountOperationResult::Aborted
        ]
    );
    assert_eq!(*declined.borrow(), Some(DeclinedPrompt::Password));
}

#[test]
fn mount_operation_declines_questions_instead_of_answering_for_the_user() {
    let declined = Rc::new(RefCell::new(None));
    let operation = mount_operation(Some(credentials()), declined.clone());
    let replies = Rc::new(RefCell::new(Vec::new()));
    let recorded = replies.clone();
    operation.connect_reply(move |_, result| recorded.borrow_mut().push(result));

    operation.emit_by_name::<()>(
        "ask-question",
        &[
            &"The identity of the remote computer is unknown.",
            &glib::StrV::from(["Log In Anyway", "Cancel Login"]).to_value(),
        ],
    );

    assert_eq!(*replies.borrow(), [gio::MountOperationResult::Aborted]);
    assert_eq!(
        *declined.borrow(),
        Some(DeclinedPrompt::Question(
            "The identity of the remote computer is unknown.".to_owned()
        ))
    );
}

/// 同一个错误码配英文、中文两种消息，分类必须相同：分类只看错误码。
fn in_both_languages(code: gio::IOErrorEnum, english: &str, chinese: &str) -> [glib::Error; 2] {
    [
        glib::Error::new(code, english),
        glib::Error::new(code, chinese),
    ]
}

#[test]
fn missing_backends_are_classified_by_error_code_in_any_language() {
    let smb = connection(NetworkProtocol::Smb, "smb://server/photos");
    for error in in_both_languages(
        gio::IOErrorEnum::NotSupported,
        "Operation not supported",
        "不支持该操作",
    )
    .into_iter()
    .chain(in_both_languages(
        gio::IOErrorEnum::NotMountableFile,
        "Location is not mountable",
        "位置不可挂载",
    )) {
        assert!(matches!(
            classify_mount_error(&smb, error, None),
            NetworkMountError::BackendUnavailable {
                backend: "gvfs-smb",
                ..
            }
        ));
    }
}

#[test]
fn rejected_sign_ins_are_classified_by_error_code_in_any_language() {
    let dav = connection(NetworkProtocol::WebDav, "davs://example.test/docs");
    for error in in_both_languages(
        gio::IOErrorEnum::PermissionDenied,
        "Permission denied",
        "权限不够",
    ) {
        assert!(matches!(
            classify_mount_error(&dav, error, None),
            NetworkMountError::AuthenticationFailed { .. }
        ));
    }
    for error in in_both_languages(
        gio::IOErrorEnum::FailedHandled,
        "Password dialog cancelled",
        "密码对话框已取消",
    ) {
        assert!(matches!(
            classify_mount_error(&dav, error, Some(DeclinedPrompt::Password)),
            NetworkMountError::AuthenticationFailed { .. }
        ));
    }
}

#[test]
fn declined_questions_and_other_failures_keep_their_own_kind() {
    let sftp = connection(NetworkProtocol::Sftp, "sftp://ym@server/srv");
    let declined = classify_mount_error(
        &sftp,
        glib::Error::new(gio::IOErrorEnum::FailedHandled, "已取消"),
        Some(DeclinedPrompt::Question("Unknown host key".to_owned())),
    );
    assert!(matches!(
        declined,
        NetworkMountError::QuestionDeclined { question, .. } if question == "Unknown host key"
    ));
    for error in in_both_languages(
        gio::IOErrorEnum::HostNotFound,
        "Host not found",
        "找不到主机",
    ) {
        assert!(matches!(
            classify_mount_error(&sftp, error, None),
            NetworkMountError::MountFailed { uri, .. } if uri == "sftp://ym@server/srv"
        ));
    }
}

#[test]
fn an_existing_mount_counts_as_mounted_in_any_language() {
    for error in in_both_languages(
        gio::IOErrorEnum::AlreadyMounted,
        "Location is already mounted",
        "位置已经挂载",
    ) {
        assert!(already_mounted_is_success(error).is_ok());
    }
    assert!(already_mounted_is_success(glib::Error::new(gio::IOErrorEnum::Failed, "x")).is_err());
}

#[test]
fn the_mounted_location_gives_its_local_path_directly() {
    let directory = tempdir().unwrap();
    let smb = connection(NetworkProtocol::Smb, "smb://server/photos");

    assert_eq!(
        local_path_of_location(&smb, &gio::File::for_path(directory.path())).unwrap(),
        directory.path()
    );
    assert!(matches!(
        local_path_of_location(
            &smb,
            &gio::File::for_uri("smb://server.invalid/not-mounted")
        ),
        Err(NetworkMountError::MountPathUnavailable { .. })
    ));
}
