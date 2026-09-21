use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::sync::{Arc, Barrier};

use desktop_linux::{
    DesktopActivationEvent, DesktopActivationRuntime, DesktopLaunchWindowPolicy,
    FileManagerActivationClaim, StandardFileManagerServiceStatus, FILE_MANAGER1_BUS_NAME,
    FILE_MANAGER1_OBJECT_PATH, FILE_MANAGER_ACTIVATION_BUS_NAME,
    FILE_MANAGER_ACTIVATION_OBJECT_PATH,
};
use tempfile::TempDir;
use url::Url;
use zbus::fdo::RequestNameFlags;

#[test]
#[ignore = "run under an isolated dbus-run-session"]
fn isolated_bus_claims_standard_name_when_available() {
    let runtime = match DesktopActivationRuntime::claim_or_forward(
        &[],
        DesktopLaunchWindowPolicy::MergeIntoExisting,
    )
    .expect("claim branded activation")
    {
        FileManagerActivationClaim::Primary(runtime) => runtime,
        FileManagerActivationClaim::Forwarded => {
            panic!("isolated bus unexpectedly had a brand owner")
        }
        FileManagerActivationClaim::Detached => {
            panic!("unexpected detached claim")
        }
    };

    assert_eq!(
        runtime.standard_service_status(),
        &StandardFileManagerServiceStatus::Owned
    );

    let client = zbus::blocking::Connection::session().expect("connect standard client");
    let introspection = zbus::blocking::Proxy::new(
        &client,
        FILE_MANAGER1_BUS_NAME,
        FILE_MANAGER1_OBJECT_PATH,
        "org.freedesktop.DBus.Introspectable",
    )
    .expect("create standard introspection proxy")
    .call::<_, _, String>("Introspect", &())
    .expect("introspect standard FileManager1 name");
    assert!(introspection.contains("interface name=\"org.freedesktop.FileManager1\""));
    for method in ["ShowFolders", "ShowItems", "ShowItemProperties"] {
        assert!(introspection.contains(&format!("method name=\"{method}\"")));
    }
}

#[test]
#[ignore = "run under an isolated dbus-run-session"]
fn concurrent_claims_choose_one_primary_and_forward_the_loser() {
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                DesktopActivationRuntime::claim_or_forward(
                    &[],
                    DesktopLaunchWindowPolicy::MergeIntoExisting,
                )
                .expect("claim or forward")
            })
        })
        .collect::<Vec<_>>();
    let claims = threads
        .into_iter()
        .map(|thread| thread.join().expect("claim thread"))
        .collect::<Vec<_>>();

    let primary_count = claims
        .iter()
        .filter(|claim| matches!(claim, FileManagerActivationClaim::Primary(_)))
        .count();
    let forwarded_count = claims
        .iter()
        .filter(|claim| matches!(claim, FileManagerActivationClaim::Forwarded))
        .count();
    assert_eq!(primary_count, 1);
    assert_eq!(forwarded_count, 1);

    let primary = claims
        .into_iter()
        .find_map(|claim| match claim {
            FileManagerActivationClaim::Primary(runtime) => Some(runtime),
            FileManagerActivationClaim::Forwarded => None,
            FileManagerActivationClaim::Detached => None,
        })
        .expect("primary runtime");
    let mut events = primary
        .take_event_receiver()
        .expect("take primary receiver");
    assert!(matches!(
        events.blocking_recv(),
        Some(DesktopActivationEvent::FocusMainWindow(_))
    ));
}

#[test]
#[ignore = "run under an isolated dbus-run-session"]
fn isolated_bus_exposes_standard_contract_and_keeps_brand_activation_when_standard_is_owned() {
    let standard_owner = zbus::blocking::Connection::session().expect("connect standard owner");
    standard_owner
        .request_name_with_flags(FILE_MANAGER1_BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .expect("claim standard name");

    let runtime = match DesktopActivationRuntime::claim_or_forward(
        &[],
        DesktopLaunchWindowPolicy::MergeIntoExisting,
    )
    .expect("claim branded activation")
    {
        FileManagerActivationClaim::Primary(runtime) => runtime,
        FileManagerActivationClaim::Forwarded => {
            panic!("isolated bus unexpectedly had a brand owner")
        }
        FileManagerActivationClaim::Detached => {
            panic!("unexpected detached claim")
        }
    };
    assert!(matches!(
        runtime.standard_service_status(),
        StandardFileManagerServiceStatus::Occupied(_)
    ));
    let mut events = runtime
        .take_event_receiver()
        .expect("take activation events");

    let client = zbus::blocking::Connection::session().expect("connect client");
    let introspection = zbus::blocking::Proxy::new(
        &client,
        FILE_MANAGER_ACTIVATION_BUS_NAME,
        FILE_MANAGER1_OBJECT_PATH,
        "org.freedesktop.DBus.Introspectable",
    )
    .expect("create introspection proxy")
    .call::<_, _, String>("Introspect", &())
    .expect("introspect FileManager1 object");
    for method in ["ShowFolders", "ShowItems", "ShowItemProperties"] {
        let method_start = introspection
            .find(&format!("<method name=\"{method}\">"))
            .expect("standard method in introspection");
        let method_xml = &introspection[method_start..];
        let method_xml = &method_xml[..method_xml.find("</method>").expect("standard method end")];
        assert!(method_xml.contains("type=\"as\" direction=\"in\""));
        assert!(method_xml.contains("type=\"s\" direction=\"in\""));
    }

    let root = TempDir::new().expect("create root");
    let folder = root.path().join("folder");
    fs::create_dir(&folder).expect("create folder");
    let first = folder.join("first.txt");
    let second = folder.join("second.txt");
    fs::write(&first, b"first").expect("write first");
    fs::write(&second, b"second").expect("write second");
    let standard_proxy = zbus::blocking::Proxy::new(
        &client,
        FILE_MANAGER_ACTIVATION_BUS_NAME,
        FILE_MANAGER1_OBJECT_PATH,
        "org.freedesktop.FileManager1",
    )
    .expect("create standard proxy");

    standard_proxy
        .call::<_, _, ()>(
            "ShowFolders",
            &(
                vec![Url::from_directory_path(&folder).unwrap().to_string()],
                String::new(),
            ),
        )
        .expect("show folder");
    let DesktopActivationEvent::MergeWorkspace(folder_workspace, _) =
        events.blocking_recv().expect("folder event")
    else {
        panic!("expected folder workspace");
    };
    assert_eq!(folder_workspace.tabs()[0].directory(), folder);

    standard_proxy
        .call::<_, _, ()>(
            "ShowItems",
            &(
                vec![
                    Url::from_file_path(&first).unwrap().to_string(),
                    Url::from_file_path(&second).unwrap().to_string(),
                ],
                String::new(),
            ),
        )
        .expect("show items");
    let DesktopActivationEvent::MergeWorkspace(item_workspace, _) =
        events.blocking_recv().expect("item event")
    else {
        panic!("expected item workspace");
    };
    assert_eq!(
        item_workspace.tabs()[0].selected_paths(),
        &[first.clone(), second.clone()]
    );

    standard_proxy
        .call::<_, _, ()>(
            "ShowItemProperties",
            &(
                vec![
                    Url::from_file_path(&first).unwrap().to_string(),
                    Url::from_file_path(&second).unwrap().to_string(),
                ],
                String::new(),
            ),
        )
        .expect("show properties");
    let DesktopActivationEvent::OpenProperties(targets, _) =
        events.blocking_recv().expect("properties event")
    else {
        panic!("expected properties event");
    };
    assert_eq!(targets.paths(), &[first, second]);

    let non_utf8 = folder.join(std::ffi::OsString::from_vec(b"nonutf8-\xff".to_vec()));
    fs::write(&non_utf8, b"bytes").expect("write non-UTF-8 file");
    assert!(matches!(
        DesktopActivationRuntime::claim_or_forward(
            std::slice::from_ref(&non_utf8),
            DesktopLaunchWindowPolicy::MergeIntoExisting,
        )
        .expect("forward private activation"),
        FileManagerActivationClaim::Forwarded
    ));
    let DesktopActivationEvent::MergeWorkspace(private_workspace, _) =
        events.blocking_recv().expect("private event")
    else {
        panic!("expected private workspace event");
    };
    assert_eq!(
        private_workspace.tabs()[0].selected_paths()[0].as_os_str(),
        non_utf8.as_os_str()
    );

    let branded_introspection = zbus::blocking::Proxy::new(
        &client,
        FILE_MANAGER_ACTIVATION_BUS_NAME,
        FILE_MANAGER_ACTIVATION_OBJECT_PATH,
        "org.freedesktop.DBus.Introspectable",
    )
    .expect("create branded introspection proxy")
    .call::<_, _, String>("Introspect", &())
    .expect("introspect branded object");
    assert!(branded_introspection.contains("<method name=\"Activate\">"));
    assert!(branded_introspection.contains("<method name=\"OpenPaths\">"));
}

#[test]
#[ignore = "run under an isolated dbus-run-session"]
fn isolated_bus_open_new_window_policy_detaches_instead_of_forwarding() {
    let primary = match DesktopActivationRuntime::claim_or_forward(
        &[],
        DesktopLaunchWindowPolicy::MergeIntoExisting,
    )
    .expect("claim branded activation")
    {
        FileManagerActivationClaim::Primary(runtime) => runtime,
        FileManagerActivationClaim::Forwarded => {
            panic!("isolated bus unexpectedly had a brand owner")
        }
        FileManagerActivationClaim::Detached => {
            panic!("unexpected detached claim")
        }
    };
    drop(primary);

    assert!(matches!(
        DesktopActivationRuntime::claim_or_forward(&[], DesktopLaunchWindowPolicy::OpenNewWindow,)
            .expect("detached claim"),
        FileManagerActivationClaim::Detached
    ));
    assert!(matches!(
        DesktopActivationRuntime::claim_or_forward(
            &[],
            DesktopLaunchWindowPolicy::MergeIntoExisting,
        )
        .expect("forwarded claim"),
        FileManagerActivationClaim::Forwarded
    ));
}
