use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;

use tempfile::TempDir;

use super::*;

#[test]
fn file_uri_preserves_non_utf8_path_bytes() {
    let path = local_path_from_uri("file:///tmp/nonutf8-%FF.txt").expect("decode file URI");

    assert_eq!(path.as_os_str().as_bytes(), b"/tmp/nonutf8-\xff.txt");
}

#[test]
fn non_local_and_malformed_uris_are_rejected() {
    assert!(local_path_from_uri("https://example.com/file").is_err());
    assert!(local_path_from_uri("file://remote-host/share/file").is_err());
    assert!(local_path_from_uri("not a URI").is_err());
}

#[test]
fn localhost_file_uri_is_accepted_and_malformed_percent_encoding_is_rejected() {
    let localhost = local_path_from_uri("file://localhost/tmp/folder").expect("localhost file URI");

    assert_eq!(localhost, PathBuf::from("/tmp/folder"));
    assert!(local_path_from_uri("file:///tmp/broken-%ZZ").is_err());
}

#[test]
fn cli_paths_group_files_by_parent_and_preserve_order() {
    let root = TempDir::new().expect("create root");
    let first_parent = root.path().join("first");
    let second_parent = root.path().join("second");
    fs::create_dir_all(&first_parent).expect("create first parent");
    fs::create_dir_all(&second_parent).expect("create second parent");
    let first = first_parent.join("one.txt");
    let second = second_parent.join("two.txt");
    let third = first_parent.join("three.txt");
    fs::write(&first, b"one").expect("write first");
    fs::write(&second, b"two").expect("write second");
    fs::write(&third, b"three").expect("write third");

    let request =
        LocalWorkspaceRequest::from_cli_paths(vec![first.clone(), second.clone(), third.clone()])
            .expect("classify paths");

    assert_eq!(request.tabs.len(), 2);
    assert_eq!(request.tabs[0].directory, first_parent);
    assert_eq!(request.tabs[0].selected_paths, vec![first, third]);
    assert_eq!(request.tabs[1].directory, second_parent);
    assert_eq!(request.tabs[1].selected_paths, vec![second]);
}

#[test]
fn item_directories_are_revealed_in_their_parent() {
    let root = TempDir::new().expect("create root");
    let directory = root.path().join("folder");
    fs::create_dir(&directory).expect("create directory");

    let request =
        LocalWorkspaceRequest::from_items(vec![directory.clone()]).expect("classify item");

    assert_eq!(request.tabs[0].directory, root.path());
    assert_eq!(request.tabs[0].selected_paths, vec![directory]);
}

#[test]
fn mixed_valid_and_invalid_batches_emit_no_event() {
    let root = TempDir::new().expect("create root");
    let valid = root.path().join("valid.txt");
    fs::write(&valid, b"valid").expect("write valid");
    let (event_sender, mut event_receiver) = mpsc::channel(2);
    let interface = FileManager1Interface { event_sender };

    let outcome = interface.show_items(
        vec![
            Url::from_file_path(&valid).unwrap().to_string(),
            "https://example.com/invalid".to_owned(),
        ],
        String::new(),
    );

    assert!(outcome.is_err());
    assert!(event_receiver.try_recv().is_err());
}

#[test]
fn invalid_limits_missing_paths_and_special_objects_emit_no_event() {
    let root = TempDir::new().expect("create root");
    let folder = root.path().join("folder");
    fs::create_dir(&folder).expect("create folder");
    let socket_path = root.path().join("socket");
    let _socket = UnixListener::bind(&socket_path).expect("create Unix socket");
    let missing = root.path().join("missing");
    let (event_sender, mut event_receiver) = mpsc::channel(2);
    let standard = FileManager1Interface {
        event_sender: event_sender.clone(),
    };
    let branded = BrandedActivationInterface { event_sender };

    assert!(standard.show_items(Vec::new(), String::new()).is_err());
    assert!(standard
        .show_folders(
            vec![
                Url::from_directory_path(&folder).unwrap().to_string();
                MAX_ACTIVATION_TARGETS + 1
            ],
            String::new(),
        )
        .is_err());
    assert!(standard
        .show_folders(
            vec![Url::from_directory_path(&folder).unwrap().to_string()],
            "x".repeat(MAX_STARTUP_ID_BYTES + 1),
        )
        .is_err());
    assert!(standard
        .show_items(
            vec![Url::from_file_path(&socket_path).unwrap().to_string()],
            String::new(),
        )
        .is_err());
    assert!(standard
        .show_items(
            vec![Url::from_file_path(&missing).unwrap().to_string()],
            String::new(),
        )
        .is_err());
    let mut oversized_path = vec![b'a'; MAX_ACTIVATION_PATH_BYTES + 1];
    oversized_path[0] = b'/';
    assert!(branded
        .open_paths(vec![oversized_path], String::new())
        .is_err());
    assert!(event_receiver.try_recv().is_err());
}

#[test]
fn branded_activation_preserves_path_bytes() {
    let root = TempDir::new().expect("create root");
    let path = root
        .path()
        .join(OsString::from_vec(b"nonutf8-\xff".to_vec()));
    fs::write(&path, b"bytes").expect("write non-UTF-8 file");
    let (event_sender, mut event_receiver) = mpsc::channel(2);
    let interface = BrandedActivationInterface { event_sender };

    interface
        .open_paths(vec![path.as_os_str().as_bytes().to_vec()], String::new())
        .expect("accept encoded path");

    let DesktopActivationEvent::MergeWorkspace(workspace, _) =
        event_receiver.try_recv().expect("activation event")
    else {
        panic!("expected workspace activation");
    };
    assert_eq!(
        workspace.tabs[0].selected_paths[0].as_os_str().as_bytes(),
        path.as_os_str().as_bytes()
    );
}

#[test]
fn closed_activation_channel_reports_failure() {
    let (event_sender, event_receiver) = mpsc::channel(1);
    drop(event_receiver);
    let interface = BrandedActivationInterface { event_sender };

    let error = interface
        .activate(String::new())
        .expect_err("closed channel must reject");

    assert!(matches!(error, zbus::fdo::Error::Failed(_)));
}

#[test]
fn full_activation_channel_rejects_without_waiting() {
    let (event_sender, _event_receiver) = mpsc::channel(1);
    let interface = BrandedActivationInterface { event_sender };
    interface.activate(String::new()).expect("fill channel");

    let error = interface
        .activate(String::new())
        .expect_err("full channel must reject");

    assert!(matches!(error, zbus::fdo::Error::LimitsExceeded(_)));
}

#[test]
fn name_taken_with_open_new_window_policy_detaches_without_forwarding() {
    let mut forward_calls = 0;
    let claim = super::resolve_name_taken_claim(
        &[PathBuf::from("/tmp")],
        super::DesktopLaunchWindowPolicy::OpenNewWindow,
        |_| {
            forward_calls += 1;
            Ok(())
        },
    )
    .expect("detached claim");

    assert!(matches!(claim, super::FileManagerActivationClaim::Detached));
    assert_eq!(forward_calls, 0);
}

#[test]
fn name_taken_with_merge_policy_forwards_and_reports_forwarded() {
    let mut forwarded_paths = None;
    let claim = super::resolve_name_taken_claim(
        &[PathBuf::from("/tmp")],
        super::DesktopLaunchWindowPolicy::MergeIntoExisting,
        |paths| {
            forwarded_paths = Some(paths.to_vec());
            Ok(())
        },
    )
    .expect("forwarded claim");

    assert!(matches!(
        claim,
        super::FileManagerActivationClaim::Forwarded
    ));
    assert_eq!(forwarded_paths, Some(vec![PathBuf::from("/tmp")]));
}

#[test]
fn name_taken_with_merge_policy_propagates_forward_failure() {
    let claim = super::resolve_name_taken_claim(
        &[PathBuf::from("/tmp")],
        super::DesktopLaunchWindowPolicy::MergeIntoExisting,
        |_| Err(super::FileManagerActivationError::ReceiverClosed),
    );

    assert!(matches!(
        claim,
        Err(super::FileManagerActivationError::ReceiverClosed)
    ));
}
