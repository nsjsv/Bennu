use std::path::PathBuf;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _,
    CreateWindowAux, EventMask, PropMode, Property, SelectionNotifyEvent, SelectionRequestEvent,
    Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::atoms::{
    read_property, PropertyValue, ProxyLifecycleStep, X11Atoms, SETUP_ORDER, TEARDOWN_ORDER,
    XDND_VERSION,
};
use super::protocol::{
    finished_data, offered_types, source_conflicts_with_target_windows, status_data,
    unpack_signed_root_position, TargetSession,
};
use super::runtime::{selection_timeout_elapsed, SelectionRequestIdentity};
use super::selection::{
    PropertyPayload, SelectionProgress, SelectionTransfer, MAX_SELECTION_BYTES,
};
use super::{
    spawn_x11_file_dnd, X11DndController, X11DndDropPosition, X11DndEvent, X11DndWindowHandle,
};

#[test]
fn xdnd_property_contract_requires_exact_type_format_length_and_value() {
    let window = 0x1020_3040_u32;
    let valid_window = PropertyValue {
        type_: AtomEnum::WINDOW.into(),
        format: 32,
        value: window.to_ne_bytes().to_vec(),
    };
    assert_eq!(
        valid_window.single_u32(AtomEnum::WINDOW.into()),
        Some(window)
    );

    for invalid in [
        PropertyValue {
            type_: AtomEnum::ATOM.into(),
            ..valid_window.clone()
        },
        PropertyValue {
            format: 16,
            ..valid_window.clone()
        },
        PropertyValue {
            value: [window.to_ne_bytes(), window.to_ne_bytes()].concat(),
            ..valid_window.clone()
        },
    ] {
        assert_eq!(invalid.single_u32(AtomEnum::WINDOW.into()), None);
    }
}

#[test]
fn proxy_setup_and_teardown_orders_do_not_expose_a_partial_proxy() {
    assert_eq!(
        SETUP_ORDER,
        &[
            ProxyLifecycleStep::CreateProxy,
            ProxyLifecycleStep::PublishProxySelfReference,
            ProxyLifecycleStep::PublishProxyAware,
            ProxyLifecycleStep::VerifyProxy,
            ProxyLifecycleStep::PublishMainAware,
            ProxyLifecycleStep::PublishMainProxy,
        ]
    );
    assert_eq!(
        TEARDOWN_ORDER,
        &[
            ProxyLifecycleStep::DeleteMainProxy,
            ProxyLifecycleStep::RestoreMainAware,
            ProxyLifecycleStep::DestroyProxy,
        ]
    );
}

#[test]
fn signed_root_coordinates_keep_negative_virtual_desktop_positions() {
    let packed = ((-120_i16 as u16 as u32) << 16) | (-7_i16 as u16 as u32);
    assert_eq!(unpack_signed_root_position(packed), (-120, -7));
    assert_eq!(unpack_signed_root_position((42_u32 << 16) | 91), (42, 91));
}

#[test]
fn enter_replacement_has_unique_identity_and_stale_source_cannot_drop() {
    let old = TargetSession::new(10, 20, 5, true);
    let mut current = TargetSession::new(11, 20, 5, true);
    assert_ne!(old.id, current.id);

    current.acknowledge_position(position(1));
    assert!(current.freeze_drop(10, 20, 1).is_none());
    assert_eq!(current.freeze_drop(11, 20, 1), Some(position(1)));
}

#[test]
fn drop_only_freezes_last_acknowledged_position_at_current_scale() {
    let mut session = TargetSession::new(10, 20, 5, true);
    assert!(!session.drop_requested);
    assert!(session.freeze_drop(10, 20, 3).is_none());
    assert!(session.acknowledge_position(position(3)));
    session.mark_drop_requested();
    assert!(session.drop_requested);
    assert_eq!(session.freeze_drop(10, 20, 3), Some(position(3)));
    assert!(session.freeze_drop(10, 20, 4).is_none());
}

#[test]
fn compliant_and_chromium_protocol_target_identity_are_echoed() {
    let main = 100;
    let proxy = 101;
    let copy = 200;
    assert_eq!(status_data(main, true, copy)[0], main);
    assert_eq!(finished_data(main, 5, true, copy)[0], main);
    assert_eq!(status_data(proxy, true, copy)[0], proxy);
    assert_eq!(finished_data(proxy, 5, true, copy)[0], proxy);
    assert_eq!(status_data(main, false, copy)[4], 0);
    assert_eq!(finished_data(main, 5, false, copy)[2], 0);
}

#[test]
fn target_owned_windows_cannot_be_drag_sources() {
    assert!(source_conflicts_with_target_windows(10, 10, 20));
    assert!(source_conflicts_with_target_windows(20, 10, 20));
    assert!(!source_conflicts_with_target_windows(30, 10, 20));
}

#[test]
fn type_list_replaces_inline_types_and_filters_none() {
    assert_eq!(offered_types([1, 0, 2], None), vec![1, 2]);
    assert_eq!(offered_types([1, 2, 3], Some(&[4, 0, 5])), vec![4, 5]);
    assert!(offered_types([1, 2, 3], Some(&[])).is_empty());
}

#[test]
fn normal_uri_list_completes_as_one_path_batch() {
    let mut transfer = SelectionTransfer::new(10, 20);
    let progress = transfer
        .accept_initial(payload(10, 8, b"file:///tmp/a\r\nfile:///tmp/b\n"))
        .expect("normal URI list");
    assert_eq!(
        progress,
        SelectionProgress::Complete(vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")])
    );
    assert!(transfer
        .accept_initial(payload(10, 8, b"file:///tmp/late"))
        .is_err());
}

#[test]
fn incr_uri_list_waits_for_empty_chunk_and_completes_once() {
    let mut transfer = SelectionTransfer::new(10, 20);
    assert_eq!(
        transfer
            .accept_initial(PropertyPayload {
                type_: 20,
                format: 32,
                bytes_after: 0,
                value: 30_u32.to_ne_bytes().to_vec(),
            })
            .expect("INCR header"),
        SelectionProgress::ReadingIncr
    );
    assert_eq!(
        transfer
            .accept_chunk(payload(10, 8, b"file:///tmp/first\n"))
            .expect("first chunk"),
        SelectionProgress::ReadingIncr
    );
    assert_eq!(
        transfer
            .accept_chunk(payload(10, 8, b"file:///tmp/second\n"))
            .expect("second chunk"),
        SelectionProgress::ReadingIncr
    );
    assert_eq!(
        transfer
            .accept_chunk(payload(10, 8, b""))
            .expect("terminal chunk"),
        SelectionProgress::Complete(vec![
            PathBuf::from("/tmp/first"),
            PathBuf::from("/tmp/second"),
        ])
    );
    assert!(transfer.accept_chunk(payload(10, 8, b"")).is_err());
}

#[test]
fn oversize_malformed_and_incomplete_properties_fail_the_whole_batch() {
    let mut oversize = SelectionTransfer::new(10, 20);
    assert!(oversize
        .accept_initial(PropertyPayload {
            type_: 20,
            format: 32,
            bytes_after: 0,
            value: ((MAX_SELECTION_BYTES + 1) as u32).to_ne_bytes().to_vec(),
        })
        .is_err());

    let mut malformed = SelectionTransfer::new(10, 20);
    assert!(malformed
        .accept_initial(payload(10, 8, b"file:///tmp/ok\nhttps://example.com/no"))
        .is_err());

    let mut incomplete = SelectionTransfer::new(10, 20);
    let mut property = payload(10, 8, b"file:///tmp/partial");
    property.bytes_after = 1;
    assert!(incomplete.accept_initial(property).is_err());
}

#[test]
fn timeout_and_source_destroy_terminal_are_exactly_once() {
    let started_at = Instant::now();
    assert!(!selection_timeout_elapsed(
        started_at,
        started_at + Duration::from_millis(4_999),
    ));
    assert!(selection_timeout_elapsed(
        started_at,
        started_at + Duration::from_secs(5),
    ));

    for details in ["selection timed out", "source destroyed"] {
        let mut transfer = SelectionTransfer::new(10, 20);
        assert_eq!(transfer.fail_terminal(details), Some(details.to_owned()));
        assert_eq!(transfer.fail_terminal("late terminal"), None);
    }

    let mut session = TargetSession::new(10, 20, 5, true);
    assert!(session.finish_once());
    assert!(!session.finish_once());
}

#[test]
fn selection_notify_identity_uses_a_per_drop_requestor() {
    let current = SelectionRequestIdentity::new(11, 20, 30, 40, x11rb::CURRENT_TIME);
    let current_event = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: x11rb::CURRENT_TIME,
        requestor: 11,
        selection: 20,
        target: 30,
        property: AtomEnum::NONE.into(),
    };
    let stale_event = SelectionNotifyEvent {
        requestor: 10,
        ..current_event
    };

    assert!(current.matches_notify(current_event));
    assert!(!current.matches_notify(stale_event));
}

#[test]
#[ignore = "requires FILE_MANAGER_X11_DND_INTEGRATION=1 and an isolated X11 server"]
fn real_x11_proxy_properties_publish_and_teardown_in_protocol_order() {
    if std::env::var_os("FILE_MANAGER_X11_DND_INTEGRATION").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    let (conn, screen_num) = x11rb::connect(None).expect("X11 connection");
    let root = conn.setup().roots[screen_num].root;
    let main = conn.generate_id().expect("main window id");
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        main,
        root,
        0,
        0,
        320,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .expect("create main window")
    .check()
    .expect("create main window reply");
    conn.flush().expect("flush main window");

    let controller = X11DndController::new(1);
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (shutdown_sender, shutdown_receiver) = std::sync::mpsc::channel();
    let worker = spawn_x11_file_dnd(
        X11DndWindowHandle::new(main, screen_num),
        controller,
        event_sender,
        shutdown_receiver,
    )
    .expect("spawn X11 runtime");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match event_receiver.try_recv() {
            Ok(X11DndEvent::RuntimeReady) => break,
            Ok(X11DndEvent::RuntimeFailed(error)) => panic!("runtime failed: {error}"),
            Ok(_) | Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => panic!("runtime did not become ready: {result:?}"),
        }
    }

    let atoms = X11Atoms::intern(&conn).expect("atoms");
    let proxy = read_property(&conn, main, atoms.xdnd_proxy)
        .expect("main proxy property")
        .and_then(|value| value.single_u32(AtomEnum::WINDOW.into()))
        .expect("main proxy XID");
    assert_eq!(
        read_property(&conn, main, atoms.xdnd_aware)
            .expect("main aware property")
            .and_then(|value| value.single_u32(AtomEnum::ATOM.into())),
        Some(XDND_VERSION)
    );
    assert_eq!(
        read_property(&conn, proxy, atoms.xdnd_proxy)
            .expect("proxy self property")
            .and_then(|value| value.single_u32(AtomEnum::WINDOW.into())),
        Some(proxy)
    );
    assert_eq!(
        read_property(&conn, proxy, atoms.xdnd_aware)
            .expect("proxy aware property")
            .and_then(|value| value.single_u32(AtomEnum::ATOM.into())),
        Some(XDND_VERSION)
    );

    shutdown_sender.send(()).expect("shutdown runtime");
    worker.join().expect("join runtime");
    assert!(read_property(&conn, main, atoms.xdnd_proxy)
        .expect("main proxy removed")
        .is_none());
    assert!(read_property(&conn, main, atoms.xdnd_aware)
        .expect("main aware restored")
        .is_none());
    conn.destroy_window(main).expect("destroy main window");
    conn.flush().expect("flush teardown");
}

#[derive(Debug, Clone, Copy)]
enum RealSelectionMode {
    Normal,
    Incr,
}

#[test]
#[ignore = "requires FILE_MANAGER_X11_DND_INTEGRATION=1 and an isolated X11 server"]
fn real_x11_source_covers_identity_normal_incr_and_exactly_once() {
    if std::env::var_os("FILE_MANAGER_X11_DND_INTEGRATION").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    let (main_conn, screen_num) = x11rb::connect(None).expect("main X11 connection");
    let root = main_conn.setup().roots[screen_num].root;
    let main = create_test_window(&main_conn, root);
    let (source_conn, source_screen_num) = x11rb::connect(None).expect("source X11 connection");
    assert_eq!(source_conn.setup().roots[source_screen_num].root, root);
    let source = create_test_window(&source_conn, root);
    let atoms = X11Atoms::intern(&source_conn).expect("source atoms");
    source_conn
        .set_selection_owner(source, atoms.xdnd_selection, x11rb::CURRENT_TIME)
        .expect("set selection owner")
        .check()
        .expect("set selection owner reply");
    source_conn.flush().expect("flush selection owner");

    let controller = X11DndController::new(1);
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (shutdown_sender, shutdown_receiver) = std::sync::mpsc::channel();
    let worker = spawn_x11_file_dnd(
        X11DndWindowHandle::new(main, screen_num),
        controller,
        event_sender,
        shutdown_receiver,
    )
    .expect("spawn X11 runtime");
    wait_runtime_event(&mut event_receiver, |event| {
        matches!(event, X11DndEvent::RuntimeReady)
    });
    let proxy = read_property(&main_conn, main, atoms.xdnd_proxy)
        .expect("main proxy property")
        .and_then(|value| value.single_u32(AtomEnum::WINDOW.into()))
        .expect("proxy XID");
    source_conn
        .change_window_attributes(
            proxy,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .expect("watch proxy property")
        .check()
        .expect("watch proxy property reply");

    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_enter,
        [source, (XDND_VERSION << 24) | 1, atoms.text_uri_list, 0, 0],
    );
    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_position,
        [source, 0, (20_u32 << 16) | 30, 0, atoms.xdnd_action_copy],
    );
    let missing_type_list_status = wait_client_message(&source_conn, atoms.xdnd_status);
    assert_eq!(missing_type_list_status.data.as_data32()[0], main);
    assert_eq!(missing_type_list_status.data.as_data32()[1], 0);
    assert_eq!(missing_type_list_status.data.as_data32()[4], 0);
    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_leave,
        [source, 0, 0, 0, 0],
    );

    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_enter,
        [source, XDND_VERSION << 24, AtomEnum::STRING.into(), 0, 0],
    );
    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_position,
        [source, 0, (20_u32 << 16) | 30, 0, atoms.xdnd_action_copy],
    );
    let rejected_status = wait_client_message(&source_conn, atoms.xdnd_status);
    assert_eq!(rejected_status.data.as_data32()[0], main);
    assert_eq!(rejected_status.data.as_data32()[1], 0);
    assert_eq!(rejected_status.data.as_data32()[4], 0);
    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_leave,
        [source, 0, 0, 0, 0],
    );

    let mut previous_requestor = None;
    for (case, protocol_target, mode) in [
        (0, main, RealSelectionMode::Normal),
        (1, proxy, RealSelectionMode::Normal),
        (2, main, RealSelectionMode::Incr),
        (3, proxy, RealSelectionMode::Incr),
    ] {
        send_client_message(
            &source_conn,
            proxy,
            protocol_target,
            atoms.xdnd_enter,
            [source, XDND_VERSION << 24, atoms.text_uri_list, 0, 0],
        );
        let packed_position = (20_u32 << 16) | 30;
        send_client_message(
            &source_conn,
            proxy,
            protocol_target,
            atoms.xdnd_position,
            [
                source,
                0,
                packed_position,
                x11rb::CURRENT_TIME,
                atoms.xdnd_action_copy,
            ],
        );
        let entered = wait_runtime_event(&mut event_receiver, |event| {
            matches!(
                event,
                X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Entered { .. })
            )
        });
        let target_session_id = match entered {
            X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Entered {
                target_session_id,
                position,
            }) => {
                assert_eq!((position.root_x, position.root_y), (20, 30));
                assert_eq!((position.client_x, position.client_y), (20, 30));
                target_session_id
            }
            _ => unreachable!(),
        };
        let status = wait_client_message(&source_conn, atoms.xdnd_status);
        assert_eq!(status.data.as_data32()[0], protocol_target);
        assert_eq!(status.data.as_data32()[1] & 1, 1);

        send_client_message(
            &source_conn,
            proxy,
            protocol_target,
            atoms.xdnd_drop,
            [source, 0, x11rb::CURRENT_TIME, 0, 0],
        );
        let dropped = wait_runtime_event(&mut event_receiver, |event| {
            matches!(
                event,
                X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Dropped { .. })
            )
        });
        assert!(matches!(
            dropped,
            X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Dropped {
                target_session_id: id,
                ..
            }) if id == target_session_id
        ));
        let request = wait_selection_request(&source_conn, atoms.xdnd_selection);
        assert_ne!(request.requestor, proxy);
        assert_ne!(Some(request.requestor), previous_requestor);
        previous_requestor = Some(request.requestor);
        let payload = format!("file:///tmp/x11-{case}-first\r\nfile:///tmp/x11-{case}-second\n");
        match mode {
            RealSelectionMode::Normal => {
                source_conn
                    .change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        request.target,
                        payload.as_bytes(),
                    )
                    .expect("write normal selection")
                    .check()
                    .expect("write normal selection reply");
                send_selection_notify(&source_conn, request);
            }
            RealSelectionMode::Incr => {
                source_conn
                    .change_window_attributes(
                        request.requestor,
                        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                    )
                    .expect("watch INCR requestor")
                    .check()
                    .expect("watch INCR requestor reply");
                source_conn
                    .change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        atoms.incr,
                        &[payload.len() as u32],
                    )
                    .expect("write INCR header")
                    .check()
                    .expect("write INCR header reply");
                send_selection_notify(&source_conn, request);
                wait_property_deleted(&source_conn, request.requestor, request.property);
                let split = payload.len() / 2;
                for chunk in [&payload.as_bytes()[..split], &payload.as_bytes()[split..]] {
                    source_conn
                        .change_property8(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            request.target,
                            chunk,
                        )
                        .expect("write INCR chunk")
                        .check()
                        .expect("write INCR chunk reply");
                    source_conn.flush().expect("flush INCR chunk");
                    wait_property_deleted(&source_conn, request.requestor, request.property);
                }
                source_conn
                    .change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        request.target,
                        &[],
                    )
                    .expect("write INCR terminal")
                    .check()
                    .expect("write INCR terminal reply");
                source_conn.flush().expect("flush INCR terminal");
            }
        }

        let files = wait_runtime_event(&mut event_receiver, |event| {
            matches!(event, X11DndEvent::FilesDropped(_))
        });
        assert!(matches!(
            files,
            X11DndEvent::FilesDropped(super::X11DndFileDrop {
                target_session_id: id,
                paths,
            }) if id == target_session_id
                && paths == vec![
                    PathBuf::from(format!("/tmp/x11-{case}-first")),
                    PathBuf::from(format!("/tmp/x11-{case}-second")),
                ]
        ));
        let finished = wait_client_message(&source_conn, atoms.xdnd_finished);
        assert_eq!(finished.data.as_data32()[0], protocol_target);
        assert_eq!(finished.data.as_data32()[1] & 1, 1);
        assert_eq!(finished.data.as_data32()[2], atoms.xdnd_action_copy);
        assert_no_duplicate_terminal(
            &source_conn,
            &mut event_receiver,
            atoms.xdnd_finished,
            target_session_id,
        );
    }

    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_enter,
        [source, XDND_VERSION << 24, atoms.text_uri_list, 0, 0],
    );
    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_position,
        [source, 0, (20_u32 << 16) | 30, 0, atoms.xdnd_action_copy],
    );
    let failed_id = match wait_runtime_event(&mut event_receiver, |event| {
        matches!(
            event,
            X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Entered { .. })
        )
    }) {
        X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Entered {
            target_session_id,
            ..
        }) => target_session_id,
        _ => unreachable!(),
    };
    wait_client_message(&source_conn, atoms.xdnd_status);
    send_client_message(
        &source_conn,
        proxy,
        main,
        atoms.xdnd_drop,
        [source, 0, 0, 0, 0],
    );
    wait_runtime_event(&mut event_receiver, |event| {
        matches!(
            event,
            X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Dropped { .. })
        )
    });
    let failed_request = wait_selection_request(&source_conn, atoms.xdnd_selection);
    send_selection_failure(&source_conn, failed_request);
    assert!(matches!(
        wait_runtime_event(&mut event_receiver, |event| {
            matches!(event, X11DndEvent::FileDropFailed { .. })
        }),
        X11DndEvent::FileDropFailed { target_session_id, .. } if target_session_id == failed_id
    ));
    let failed_finished = wait_client_message(&source_conn, atoms.xdnd_finished);
    assert_eq!(failed_finished.data.as_data32()[0], main);
    assert_eq!(failed_finished.data.as_data32()[1], 0);
    assert_eq!(failed_finished.data.as_data32()[2], 0);

    send_client_message(
        &source_conn,
        proxy,
        proxy,
        atoms.xdnd_enter,
        [source, XDND_VERSION << 24, atoms.text_uri_list, 0, 0],
    );
    send_client_message(
        &source_conn,
        proxy,
        proxy,
        atoms.xdnd_position,
        [source, 0, (20_u32 << 16) | 30, 0, atoms.xdnd_action_copy],
    );
    let destroyed_id = match wait_runtime_event(&mut event_receiver, |event| {
        matches!(
            event,
            X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Entered { .. })
        )
    }) {
        X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Entered {
            target_session_id,
            ..
        }) => target_session_id,
        _ => unreachable!(),
    };
    wait_client_message(&source_conn, atoms.xdnd_status);
    send_client_message(
        &source_conn,
        proxy,
        proxy,
        atoms.xdnd_drop,
        [source, 0, 0, 0, 0],
    );
    wait_runtime_event(&mut event_receiver, |event| {
        matches!(
            event,
            X11DndEvent::FileDropTarget(super::X11FileDropTargetEvent::Dropped { .. })
        )
    });
    wait_selection_request(&source_conn, atoms.xdnd_selection);
    source_conn.destroy_window(source).expect("destroy source");
    source_conn.flush().expect("flush source destroy");
    assert!(matches!(
        wait_runtime_event(&mut event_receiver, |event| {
            matches!(event, X11DndEvent::FileDropFailed { .. })
        }),
        X11DndEvent::FileDropFailed { target_session_id, .. }
            if target_session_id == destroyed_id
    ));
    assert_no_duplicate_failure(&mut event_receiver, destroyed_id);

    shutdown_sender.send(()).expect("shutdown runtime");
    worker.join().expect("join runtime");
    main_conn.destroy_window(main).expect("destroy main");
    source_conn.flush().expect("flush source teardown");
    main_conn.flush().expect("flush main teardown");
}

fn create_test_window(conn: &RustConnection, root: Window) -> Window {
    let window = conn.generate_id().expect("test window id");
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        320,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .expect("create test window")
    .check()
    .expect("create test window reply");
    conn.flush().expect("flush test window");
    window
}

fn send_client_message(
    conn: &RustConnection,
    destination: Window,
    protocol_target: Window,
    type_: Atom,
    data: [u32; 5],
) {
    conn.send_event(
        false,
        destination,
        EventMask::NO_EVENT,
        ClientMessageEvent::new(32, protocol_target, type_, data),
    )
    .expect("send client message")
    .check()
    .expect("send client message reply");
    conn.flush().expect("flush client message");
}

fn send_selection_notify(conn: &RustConnection, request: SelectionRequestEvent) {
    conn.send_event(
        false,
        request.requestor,
        EventMask::NO_EVENT,
        SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: request.property,
        },
    )
    .expect("send SelectionNotify")
    .check()
    .expect("send SelectionNotify reply");
    conn.flush().expect("flush SelectionNotify");
}

fn send_selection_failure(conn: &RustConnection, request: SelectionRequestEvent) {
    conn.send_event(
        false,
        request.requestor,
        EventMask::NO_EVENT,
        SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: AtomEnum::NONE.into(),
        },
    )
    .expect("send failed SelectionNotify")
    .check()
    .expect("send failed SelectionNotify reply");
    conn.flush().expect("flush failed SelectionNotify");
}

fn wait_runtime_event(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<X11DndEvent>,
    predicate: impl Fn(&X11DndEvent) -> bool,
) -> X11DndEvent {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match receiver.try_recv() {
            Ok(event) if predicate(&event) => return event,
            Ok(X11DndEvent::RuntimeFailed(error)) => panic!("runtime failed: {error}"),
            Ok(_) | Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            result => panic!("timed out waiting for runtime event: {result:?}"),
        }
    }
}

fn wait_client_message(conn: &RustConnection, type_: Atom) -> ClientMessageEvent {
    match wait_source_event(
        conn,
        |event| matches!(event, Event::ClientMessage(message) if message.type_ == type_),
    ) {
        Event::ClientMessage(message) => message,
        _ => unreachable!(),
    }
}

fn wait_selection_request(conn: &RustConnection, selection: Atom) -> SelectionRequestEvent {
    match wait_source_event(
        conn,
        |event| matches!(event, Event::SelectionRequest(request) if request.selection == selection),
    ) {
        Event::SelectionRequest(request) => request,
        _ => unreachable!(),
    }
}

fn wait_property_deleted(conn: &RustConnection, window: Window, atom: Atom) {
    wait_source_event(conn, |event| {
        matches!(event, Event::PropertyNotify(property)
            if property.window == window
                && property.atom == atom
                && property.state == Property::DELETE)
    });
}

fn wait_source_event(conn: &RustConnection, predicate: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match conn.poll_for_event() {
            Ok(Some(Event::Error(error))) => panic!("X11 source error: {error:?}"),
            Ok(Some(event)) if predicate(&event) => return event,
            Ok(Some(_)) | Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            result => panic!("timed out waiting for source event: {result:?}"),
        }
    }
}

fn assert_no_duplicate_terminal(
    source_conn: &RustConnection,
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<X11DndEvent>,
    finished_atom: Atom,
    target_session_id: super::X11FileDropTargetSessionId,
) {
    let deadline = Instant::now() + Duration::from_millis(30);
    while Instant::now() < deadline {
        while let Ok(event) = receiver.try_recv() {
            assert!(!matches!(
                event,
                X11DndEvent::FilesDropped(super::X11DndFileDrop {
                    target_session_id: id,
                    ..
                }) if id == target_session_id
            ));
        }
        while let Some(event) = source_conn.poll_for_event().expect("poll source event") {
            assert!(!matches!(
                event,
                Event::ClientMessage(message) if message.type_ == finished_atom
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn assert_no_duplicate_failure(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<X11DndEvent>,
    target_session_id: super::X11FileDropTargetSessionId,
) {
    let deadline = Instant::now() + Duration::from_millis(30);
    while Instant::now() < deadline {
        while let Ok(event) = receiver.try_recv() {
            assert!(!matches!(
                event,
                X11DndEvent::FileDropFailed {
                    target_session_id: id,
                    ..
                } if id == target_session_id
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn payload(type_: u32, format: u8, value: &[u8]) -> PropertyPayload {
    PropertyPayload {
        type_,
        format,
        bytes_after: 0,
        value: value.to_vec(),
    }
}

fn position(scale_generation: u64) -> X11DndDropPosition {
    X11DndDropPosition {
        root_x: -10,
        root_y: 20,
        client_x: 5,
        client_y: 6,
        timestamp: 7,
        scale_generation,
    }
}
