//! 确认回信载荷测试：choices 回传、SaveFile 扩展名补全与覆盖确认、
//! SaveFiles 批量目标、current_filter 回传。从 tests/mod.rs 拆出以守住
//! 800 行上限；共享构造器见 tests/mod.rs。

use super::*;

fn session_full(
    kind: PickerKind,
    filters: Vec<FilterRule>,
    choices: Vec<crate::picker_request::PickerChoice>,
) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
    let (reply, receiver) = oneshot::channel();
    let base = tempfile::tempdir().unwrap();
    let mut request = spec(kind, filters);
    request.choices = choices;
    let session = PickerSession::new(
        &request,
        "/req/test".to_string(),
        base.keep(),
        PickerViewMode::List,
        reply,
    );
    (session, receiver)
}

fn session_with_choices(
    kind: PickerKind,
    choices: Vec<crate::picker_request::PickerChoice>,
) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
    session_full(kind, Vec::new(), choices)
}

fn choice(id: &str, selected: &str) -> crate::picker_request::PickerChoice {
    crate::picker_request::PickerChoice {
        id: id.to_string(),
        label: id.to_string(),
        options: vec![
            ("true".to_string(), "开".to_string()),
            ("false".to_string(), "关".to_string()),
        ],
        selected: selected.to_string(),
    }
}

#[test]
fn choice_selection_updates_by_id_and_keeps_order() {
    let (mut session, _receiver) = session_with_choices(
        PickerKind::SaveFile { default_name: None },
        vec![choice("fmt", "false"), choice("tag", "false")],
    );
    session.update(SessionMessage::ChoiceSelected {
        id: "tag".to_string(),
        value: "true".to_string(),
    });
    // 回信按请求顺序携带全部 id+选中值。
    assert_eq!(
        session.selected_choices_for_reply(),
        vec![
            ("fmt".to_string(), "false".to_string()),
            ("tag".to_string(), "true".to_string())
        ]
    );
}

#[test]
fn unknown_choice_id_is_silently_ignored() {
    let (mut session, _receiver) = session_with_choices(
        PickerKind::OpenFile {
            multiple: false,
            directory: false,
        },
        vec![choice("fmt", "false")],
    );
    session.update(SessionMessage::ChoiceSelected {
        id: "ghost".to_string(),
        value: "true".to_string(),
    });
    assert_eq!(
        session.selected_choices_for_reply(),
        vec![("fmt".to_string(), "false".to_string())]
    );
}

#[test]
fn savefiles_confirm_requires_names_and_blocks_trash() {
    let (mut trash_session, mut receiver) = session(PickerKind::SaveFiles {
        default_names: vec!["a.txt".to_string(), "b.txt".to_string()],
    });
    assert!(trash_session.can_confirm());

    // 回收站视图禁止确认（与 SaveFile 同一守卫）。
    trash_session.update(SessionMessage::SidebarTrashPressed);
    trash_session.apply_scan(DirectoryScanResult {
        directory: trash_session.directory.clone(),
        outcome: Ok(DirectoryScanOutcome {
            entries: Vec::new(),
        }),
    });
    assert!(!trash_session.can_confirm());
    assert!(matches!(
        trash_session.update(SessionMessage::ConfirmPressed),
        SessionEffect::None
    ));
    assert!(receiver.try_recv().is_err());

    // 空名字列表禁用确认。
    let (empty_session, _empty_receiver) = session(PickerKind::SaveFiles {
        default_names: Vec::new(),
    });
    assert!(!empty_session.can_confirm());
}

fn glob_filter(pattern: &str) -> Vec<FilterRule> {
    vec![FilterRule {
        name: "PNG".to_string(),
        patterns: vec![FilePattern::Glob(pattern.to_string())],
    }]
}

#[test]
fn savefile_extension_completion_lands_before_overwrite_check() {
    // 目录里已有 "照片.png"：输入 "照片" 补全后应命中覆盖确认，
    // 而不是静默生成 "照片.png"（AC2）。
    let existing = tempfile::tempdir().unwrap();
    let existing_png = existing.path().join("照片.png");
    fs::write(&existing_png, "x").unwrap();

    let (reply, receiver) = oneshot::channel();
    let mut request = spec(
        PickerKind::SaveFile {
            default_name: Some("照片".to_string()),
        },
        glob_filter("*.png"),
    );
    request.active_filter = Some(0);
    let mut session = PickerSession::new(
        &request,
        "/req/test".to_string(),
        existing.path().to_path_buf(),
        PickerViewMode::List,
        reply,
    );

    assert!(matches!(
        session.update(SessionMessage::ConfirmPressed),
        SessionEffect::None
    ));
    assert_eq!(session.overwrite_targets(), [existing_png.as_path()]);

    session.update(SessionMessage::ConfirmPressed);
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Confirmed(payload) if payload.paths == vec![existing_png]
    ));
}

#[test]
fn savefile_without_filter_or_with_user_extension_stays_put() {
    let base = tempfile::tempdir().unwrap();

    // 无过滤：不补。
    let (reply, receiver) = oneshot::channel();
    let mut session = PickerSession::new(
        &spec(
            PickerKind::SaveFile {
                default_name: Some("照片".to_string()),
            },
            Vec::new(),
        ),
        "/req/test".to_string(),
        base.path().to_path_buf(),
        PickerViewMode::List,
        reply,
    );
    session.update(SessionMessage::ConfirmPressed);
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Confirmed(payload) if payload.paths == vec![base.path().join("照片")]
    ));

    // 用户已带扩展名：不改写（无同名文件，直接确认）。
    let (reply, receiver) = oneshot::channel();
    let mut request = spec(
        PickerKind::SaveFile {
            default_name: Some("照片.bmp".to_string()),
        },
        glob_filter("*.png"),
    );
    request.active_filter = Some(0);
    let mut session = PickerSession::new(
        &request,
        "/req/test".to_string(),
        base.path().to_path_buf(),
        PickerViewMode::List,
        reply,
    );
    session.update(SessionMessage::ConfirmPressed);
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Confirmed(payload) if payload.paths == vec![base.path().join("照片.bmp")]
    ));
}

#[test]
fn savefiles_confirm_returns_all_targets_and_conflict_gates_overwrite() {
    let base = tempfile::tempdir().unwrap();
    fs::write(base.path().join("b.txt"), "x").unwrap();

    let names = vec![
        "a.txt".to_string(),
        "b.txt".to_string(),
        "c.txt".to_string(),
    ];
    let mut request = spec(
        PickerKind::SaveFiles {
            default_names: names.clone(),
        },
        Vec::new(),
    );
    request.choices = vec![choice("fmt", "pdf")];
    let (reply, mut receiver) = oneshot::channel();
    let mut session = PickerSession::new(
        &request,
        "/req/test".to_string(),
        base.path().to_path_buf(),
        PickerViewMode::List,
        reply,
    );

    // 第一次确认：b.txt 冲突 → 只把冲突子集送进覆盖确认，不完成。
    assert!(matches!(
        session.update(SessionMessage::ConfirmPressed),
        SessionEffect::None
    ));
    assert_eq!(
        session.overwrite_targets(),
        [base.path().join("b.txt").as_path()]
    );
    // 确认未完成前不回信。
    assert!(receiver.try_recv().is_err());

    // 二次确认：返回全部目标（含未冲突项）+ choices 回传。
    session.update(SessionMessage::ConfirmPressed);
    let PickerResolution::Confirmed(payload) = receiver.blocking_recv().unwrap() else {
        panic!("应为 Confirmed 载荷");
    };
    assert_eq!(
        payload.paths,
        names
            .iter()
            .map(|name| base.path().join(name))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        payload.choices,
        vec![("fmt".to_string(), "pdf".to_string())]
    );
    // 无过滤规则：不带 current_filter。
    assert_eq!(payload.current_filter, None);
}

#[test]
fn confirmed_reply_carries_active_filter_rule() {
    let (mut session, mut receiver) = session_full(
        PickerKind::OpenFile {
            multiple: false,
            directory: false,
        },
        glob_filter("*.png"),
        Vec::new(),
    );
    let entries = vec![DirectoryEntry::new(
        session.directory.join("a.png"),
        FileKind::File,
        file_core::entry::EntryMetadata::default(),
        false,
        false,
        false,
    )];
    session.apply_scan(DirectoryScanResult {
        directory: session.directory.clone(),
        outcome: Ok(DirectoryScanOutcome { entries }),
    });
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::ConfirmPressed);
    let PickerResolution::Confirmed(payload) = receiver.blocking_recv().unwrap() else {
        panic!("应为 Confirmed 载荷");
    };
    // 结构与传入规则一致（R4）。
    assert_eq!(
        payload.current_filter,
        Some(FilterRule {
            name: "PNG".to_string(),
            patterns: vec![FilePattern::Glob("*.png".to_string())],
        })
    );
}
