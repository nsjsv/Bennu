//! picker_session 单元测试（独立文件控制主文件行数）。

use super::*;
use crate::picker_request::FilePattern;
use crate::picker_session::scan::DirectoryScanOutcome;
use std::fs;
mod address_editing;
mod keyboard_nav;
mod sidebar;

fn spec(kind: PickerKind, filters: Vec<FilterRule>) -> PickerRequestSpec {
    PickerRequestSpec {
        kind,
        accept_label: None,
        title: None,
        filters,
        active_filter: None,
        start_folder: None,
        choices: Vec::new(),
    }
}

pub(crate) fn session(kind: PickerKind) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
    let (reply, receiver) = oneshot::channel();
    let base = tempfile::tempdir().unwrap();
    let session = PickerSession::new(
        &spec(kind.clone(), Vec::new()),
        "/req/test".to_string(),
        base.keep(),
        reply,
    );
    (session, receiver)
}

fn seeded_listing(session: &mut PickerSession, names: &[(&str, FileKind)]) {
    let entries = names
        .iter()
        .map(|&(name, kind)| {
            DirectoryEntry::new(
                session.directory.join(name),
                kind,
                file_core::entry::EntryMetadata::default(),
                name.starts_with('.'),
                false,
                false,
            )
        })
        .collect();
    session.apply_scan(DirectoryScanResult {
        directory: session.directory.clone(),
        outcome: Ok(DirectoryScanOutcome { entries }),
    });
}

#[test]
fn hover_follows_pointer_and_invalid_indices_are_dropped() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[("a.txt", FileKind::File), ("b.txt", FileKind::File)],
    );

    session.update(SessionMessage::EntryHovered { index: Some(1) });
    assert_eq!(session.hovered_index(), Some(1));

    session.update(SessionMessage::EntryHovered { index: Some(9) });
    assert_eq!(session.hovered_index(), None);
    session.update(SessionMessage::EntryHovered { index: None });
    assert_eq!(session.hovered_index(), None);
}

#[test]
fn filtering_shifts_visible_indices_and_invalidates_stale_hover() {
    let (reply, _receiver) = oneshot::channel();
    let base = tempfile::tempdir().unwrap();
    let filters = vec![FilterRule {
        name: "TXT".to_string(),
        patterns: vec![FilePattern::Glob("*.txt".to_string())],
    }];
    let mut session = PickerSession::new(
        &spec(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            filters,
        ),
        "/req/test".to_string(),
        base.keep(),
        reply,
    );
    seeded_listing(
        &mut session,
        &[("a.txt", FileKind::File), ("b.log", FileKind::File)],
    );
    session.update(SessionMessage::EntryHovered { index: Some(1) });

    // 重新套用过滤规则后 b.log 被隐藏，原悬停行号失效必须清掉。
    session.update(SessionMessage::FilterSelected { rule: 0 });
    assert_eq!(session.rows().len(), 1);
    assert_eq!(session.hovered_index(), None);
}

#[test]
fn select_all_covers_selectable_entries_only_in_multi_open_mode() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: true,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[
            ("a.txt", FileKind::File),
            ("dir", FileKind::Directory),
            ("b.txt", FileKind::File),
        ],
    );
    session.update(SessionMessage::SelectAllPressed);
    assert_eq!(session.selection(), &[0, 2]);
}

#[test]
fn select_all_is_ignored_outside_multi_open_mode() {
    let (mut session, _receiver) = session(PickerKind::SaveFile { default_name: None });
    seeded_listing(&mut session, &[("a.txt", FileKind::File)]);
    session.update(SessionMessage::SelectAllPressed);
    assert!(session.selection().is_empty());
}

#[test]
fn single_click_selects_and_double_click_confirms_file() {
    let (mut session, receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[("a.txt", FileKind::File), ("dir", FileKind::Directory)],
    );
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert_eq!(session.selection(), &[0]);

    let effect = session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    assert!(matches!(effect, SessionEffect::Confirmed(_)));
    let expected = session.directory().join("a.txt");
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Confirmed(payload) if payload.paths == vec![expected]
    ));
}

#[test]
fn directory_mode_rejects_file_selection() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: true,
    });
    seeded_listing(
        &mut session,
        &[("a.txt", FileKind::File), ("dir", FileKind::Directory)],
    );
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert!(session.selection().is_empty());
    session.update(SessionMessage::EntryClicked {
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert_eq!(session.selection(), &[1]);
}

#[test]
fn ctrl_click_accumulates_selection() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: true,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[
            ("a", FileKind::File),
            ("b", FileKind::File),
            ("c", FileKind::File),
        ],
    );
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: true,
        shift: false,
    });
    session.update(SessionMessage::EntryClicked {
        index: 2,
        ctrl: true,
        shift: false,
    });
    assert_eq!(session.selection(), &[0, 2]);
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: true,
        shift: false,
    });
    assert_eq!(session.selection(), &[2]);
}

#[test]
fn shift_click_selects_range() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: true,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[
            ("a", FileKind::File),
            ("b", FileKind::File),
            ("c", FileKind::File),
            ("d", FileKind::File),
        ],
    );
    session.update(SessionMessage::EntryClicked {
        index: 1,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::EntryClicked {
        index: 3,
        ctrl: false,
        shift: true,
    });
    assert_eq!(session.selection(), &[1, 2, 3]);
}

#[test]
fn save_file_requires_name_and_confirms_target() {
    let (mut session, receiver) = session(PickerKind::SaveFile {
        default_name: Some("报告.txt".to_string()),
    });
    assert_eq!(session.name_input(), "报告.txt");
    assert!(session.can_confirm());

    session.update(SessionMessage::NameInputChanged("   ".to_string()));
    assert!(!session.can_confirm());

    session.update(SessionMessage::NameInputChanged("新名字.md".to_string()));
    let effect = session.update(SessionMessage::ConfirmPressed);
    assert!(matches!(effect, SessionEffect::Confirmed(_)));
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Confirmed(payload) if payload.paths.len() == 1 && payload.paths[0].ends_with("新名字.md")
    ));
}

#[test]
fn save_file_existing_target_requires_overwrite_step() {
    let existing = tempfile::tempdir().unwrap();
    let existing_file = existing.path().join("已存在.txt");
    fs::write(&existing_file, "x").unwrap();

    let (reply, receiver) = oneshot::channel();
    let mut session = PickerSession::new(
        &spec(
            PickerKind::SaveFile {
                default_name: Some("已存在.txt".to_string()),
            },
            Vec::new(),
        ),
        "/req/test".to_string(),
        existing.path().to_path_buf(),
        reply,
    );
    let effect = session.update(SessionMessage::ConfirmPressed);
    // 第一步：进入覆盖确认，不立即完成。
    assert!(matches!(effect, SessionEffect::None));
    assert_eq!(session.overwrite_targets(), [existing_file.as_path()]);
    assert_eq!(session.accept_button_label(), "覆盖");

    let effect = session.update(SessionMessage::ConfirmPressed);
    assert!(matches!(effect, SessionEffect::Confirmed(_)));
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Confirmed(payload) if payload.paths == vec![existing_file]
    ));
}

#[test]
fn save_file_entry_click_adopts_file_name() {
    let (mut session, _receiver) = session(PickerKind::SaveFile { default_name: None });
    seeded_listing(
        &mut session,
        &[("模板.txt", FileKind::File), ("dir", FileKind::Directory)],
    );
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert_eq!(session.name_input(), "模板.txt");
    // 目录点击不改名字
    session.update(SessionMessage::EntryClicked {
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert_eq!(session.name_input(), "模板.txt");
}

#[test]
fn double_click_directory_navigates() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    let base = tempfile::tempdir().unwrap();
    let child = base.path().join("child");
    fs::create_dir(&child).unwrap();
    seeded_listing(
        &mut session,
        &[("child", FileKind::Directory), ("a", FileKind::File)],
    );
    // seeded listing 的路径是假名；改为真实路径验证导航。
    session.apply_scan(DirectoryScanResult {
        directory: session.directory().to_path_buf(),
        outcome: Ok(DirectoryScanOutcome {
            entries: vec![DirectoryEntry::new(
                child.clone(),
                FileKind::Directory,
                file_core::entry::EntryMetadata::default(),
                false,
                false,
                false,
            )],
        }),
    });
    let effect = session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    assert!(matches!(effect, SessionEffect::NavigateDirectory(target) if target == child));
}

#[test]
fn filter_rule_hides_non_matching_files_but_keeps_directories() {
    let (reply, _receiver) = oneshot::channel();
    let base = tempfile::tempdir().unwrap();
    let filter = vec![FilterRule {
        name: "PNG".to_string(),
        patterns: vec![FilePattern::Glob("*.png".to_string())],
    }];
    let mut session = PickerSession::new(
        &spec(
            PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            filter,
        ),
        "/req/test".to_string(),
        base.keep(),
        reply,
    );
    seeded_listing(
        &mut session,
        &[
            ("a.png", FileKind::File),
            ("b.jpg", FileKind::File),
            ("dir", FileKind::Directory),
        ],
    );
    assert_eq!(session.rows().len(), 2);
    assert_eq!(session.rows()[0].entry.name.to_string_lossy(), "a.png");
    assert_eq!(session.rows()[1].entry.name.to_string_lossy(), "dir");
}

#[test]
fn dismiss_cancels_reply() {
    let (mut session, receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    let effect = session.update(SessionMessage::DismissPressed);
    assert!(matches!(effect, SessionEffect::Dismissed));
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Cancelled
    ));
}

#[test]
fn external_window_close_without_reply_counts_as_cancel() {
    let (session, receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.window_closed();
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Cancelled
    ));
}

fn scan_result(
    directory: &Path,
    outcome: Result<Vec<(&str, FileKind)>, String>,
) -> DirectoryScanResult {
    DirectoryScanResult {
        directory: directory.to_path_buf(),
        outcome: outcome.map(|entries| DirectoryScanOutcome {
            entries: entries
                .into_iter()
                .map(|(name, kind)| {
                    DirectoryEntry::new(
                        directory.join(name),
                        kind,
                        file_core::entry::EntryMetadata::default(),
                        name.starts_with('.'),
                        false,
                        false,
                    )
                })
                .collect(),
        }),
    }
}

#[test]
fn expanding_a_directory_appends_indented_children() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[("sub", FileKind::Directory), ("a.txt", FileKind::File)],
    );
    let child = session.directory().join("sub");

    let effect = session.update(SessionMessage::EntryExpandToggled { index: 0 });
    assert!(matches!(effect, SessionEffect::ScanDirectory(dir) if dir == child));
    assert_eq!(session.rows().len(), 2); // 加载中：父行标记展开，无子行

    session.apply_scan(scan_result(&child, Ok(vec![("inner.txt", FileKind::File)])));
    assert_eq!(session.rows().len(), 3);
    assert_eq!(session.rows()[1].entry.name.to_string_lossy(), "inner.txt");
    assert_eq!(session.rows()[1].depth, 1);
    assert_eq!(session.rows()[2].depth, 0);

    // 再点一次进入收起动画：子行保留并随进度缩回，播完才移除。
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    assert_eq!(session.rows().len(), 3);
    assert!(session.is_animating());
    while session.is_animating() {
        session.advance_animations();
    }
    assert_eq!(session.rows().len(), 2);
}

#[test]
fn expansion_animation_advances_and_holds_at_full() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("sub", FileKind::Directory)]);
    let child = session.directory().join("sub");
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.apply_scan(scan_result(&child, Ok(vec![("inner.txt", FileKind::File)])));

    // 子行高度随父级展开进度级联；未推进前行高进度为 0。
    assert_eq!(session.rows()[0].expand_progress, 0.0);
    assert_eq!(session.rows()[1].height_progress, 0.0);
    assert!(session.is_animating());

    session.advance_animations();
    assert!((session.rows()[0].expand_progress - 0.18).abs() < 1e-4);
    assert!((session.rows()[1].height_progress - 0.18).abs() < 1e-4);

    while session.advance_animations().changed {}
    assert!((session.rows()[0].expand_progress - 1.0).abs() < 1e-4);
    assert!((session.rows()[1].height_progress - 1.0).abs() < 1e-4);
    assert!(!session.is_animating());
}

#[test]
fn collapse_completion_is_reported_for_scrollbar_recheck() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("sub", FileKind::Directory)]);
    let child = session.directory().join("sub");
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.apply_scan(scan_result(&child, Ok(vec![("inner.txt", FileKind::File)])));

    // 纯展开阶段逐帧推进：没有任何收缩播完信号（避免每帧白探滚动条）。
    let mut saw_expansion_frame = false;
    while session.is_animating() {
        assert!(!session.advance_animations().collapse_finished);
        saw_expansion_frame = true;
    }
    assert!(saw_expansion_frame);

    // 收起播完的那一帧必须上报 collapse_finished：行列表结构性收缩，
    // advance_frame 据此追加滚动条布局探针（防视口缓存陈旧）。
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    let mut collapse_finished = false;
    while session.is_animating() {
        collapse_finished |= session.advance_animations().collapse_finished;
    }
    assert!(collapse_finished);
    assert_eq!(session.rows().len(), 1);
}

#[test]
fn toggle_during_collapse_resumes_expansion() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("sub", FileKind::Directory)]);
    let child = session.directory().join("sub");
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.apply_scan(scan_result(&child, Ok(vec![("inner.txt", FileKind::File)])));
    while session.advance_animations().changed {}

    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.advance_animations();
    let mid_collapse = session.rows()[0].expand_progress;
    assert!(mid_collapse < 1.0);

    // 收起中再点 = 反悔：进度折返向展开推进，而不是继续缩小。
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.advance_animations();
    assert!(session.rows()[0].expand_progress > mid_collapse);
    assert_eq!(session.rows().len(), 2);
    while session.advance_animations().changed {}
    assert_eq!(session.rows().len(), 2);
}

#[test]
fn no_expansion_means_not_animating() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("a.txt", FileKind::File)]);
    assert!(!session.is_animating());
    assert!(!session.advance_animations().changed);
}

#[test]
fn failed_expansion_scan_collapses_the_node() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("sub", FileKind::Directory)]);
    let child = session.directory().join("sub");
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    assert_eq!(session.rows().len(), 1);

    session.apply_scan(scan_result(&child, Err("permission denied".to_string())));
    assert_eq!(session.rows().len(), 1);
}

#[test]
fn loading_expansion_ignores_duplicate_toggles() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("sub", FileKind::Directory)]);

    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    let again = session.update(SessionMessage::EntryExpandToggled { index: 0 });
    assert!(matches!(again, SessionEffect::None));
    assert_eq!(session.rows().len(), 1);
    // 展开标记如今由动画状态表达：Pending 节点进度仍在推进。
    assert!(session.is_animating());
}

#[test]
fn history_back_and_forward_follow_the_visit_order() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("a", FileKind::Directory)]);
    let start = session.directory().to_path_buf();
    let a = start.join("a");
    let b = a.join("b");

    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    assert_eq!(session.directory(), a);
    session.apply_scan(scan_result(&a, Ok(vec![("b", FileKind::Directory)])));
    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    assert_eq!(session.directory(), b);

    // 后退两次回到起点，前进按原路径回来；边界外再走无效。
    session.update(SessionMessage::NavigateBack);
    assert_eq!(session.directory(), a);
    session.update(SessionMessage::NavigateBack);
    assert_eq!(session.directory(), start);
    let stuck = session.update(SessionMessage::NavigateBack);
    assert!(matches!(stuck, SessionEffect::None));

    session.update(SessionMessage::NavigateForward);
    assert_eq!(session.directory(), a);
    session.update(SessionMessage::NavigateForward);
    assert_eq!(session.directory(), b);
    let stuck = session.update(SessionMessage::NavigateForward);
    assert!(matches!(stuck, SessionEffect::None));
}

#[test]
fn new_navigation_after_back_truncates_the_forward_branch() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[("a", FileKind::Directory), ("c", FileKind::Directory)],
    );
    let start = session.directory().to_path_buf();

    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    session.update(SessionMessage::NavigateBack);
    let forward = session.update(SessionMessage::NavigateForward);
    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    session.update(SessionMessage::NavigateBack);
    let forward = session.update(SessionMessage::NavigateForward);
    assert!(matches!(forward, SessionEffect::NavigateDirectory(_)));
    session.update(SessionMessage::NavigateBack);

    // 回退后走新分支：前进历史被截断。
    seeded_listing(
        &mut session,
        &[("a", FileKind::Directory), ("c", FileKind::Directory)],
    );
    session.update(SessionMessage::EntryDoubleClicked { index: 1 });
    assert_eq!(session.directory(), start.join("c"));
    // 前进分支已截断：再前进无效，但仍可后退。
    let stuck = session.update(SessionMessage::NavigateForward);
    assert!(matches!(stuck, SessionEffect::None));
    let back = session.update(SessionMessage::NavigateBack);
    assert!(matches!(back, SessionEffect::NavigateDirectory(_)));
}

#[test]
fn save_file_click_selects_file_and_directory_rows() {
    let (mut session, _receiver) = session(PickerKind::SaveFile { default_name: None });
    seeded_listing(
        &mut session,
        &[("模板.txt", FileKind::File), ("dir", FileKind::Directory)],
    );

    // 文件点击：选中集 + anchor + 名字同步填入，预览目标可得。
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert_eq!(session.selection(), &[0]);
    assert_eq!(session.primary_selected_row(), Some(0));
    assert_eq!(session.name_input(), "模板.txt");

    // 目录点击：同样选中（单击选中、双击进入），预览目标可得。
    session.update(SessionMessage::EntryClicked {
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert_eq!(session.selection(), &[1]);
    assert_eq!(session.primary_selected_row(), Some(1));
    // 目录不改名字（既有行为）。
    assert_eq!(session.name_input(), "模板.txt");
}

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
        outcome: Ok(DirectoryScanOutcome { entries: Vec::new() }),
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
