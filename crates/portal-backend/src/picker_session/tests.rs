//! picker_session 单元测试（独立文件控制主文件行数）。

use super::*;
use crate::picker_request::FilePattern;
use std::fs;

fn spec(kind: PickerKind, filters: Vec<FilterRule>) -> PickerRequestSpec {
    PickerRequestSpec {
        kind,
        accept_label: None,
        filters,
        active_filter: None,
        start_folder: None,
    }
}

fn session(kind: PickerKind) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
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
fn breadcrumb_chain_lists_ancestors_root_first() {
    let chain = breadcrumb_chain(Path::new("/home/u/Downloads"));
    assert_eq!(chain.len(), 4);
    assert_eq!(chain[0], Path::new("/"));
    assert_eq!(chain[3], Path::new("/home/u/Downloads"));
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
        PickerResolution::Confirmed(paths) if paths == vec![expected]
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
        PickerResolution::Confirmed(paths) if paths.len() == 1 && paths[0].ends_with("新名字.md")
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
    assert_eq!(session.overwrite_target(), Some(existing_file.as_path()));
    assert_eq!(session.accept_button_label(), "覆盖");

    let effect = session.update(SessionMessage::ConfirmPressed);
    assert!(matches!(effect, SessionEffect::Confirmed(_)));
    assert!(matches!(
        receiver.blocking_recv().unwrap(),
        PickerResolution::Confirmed(paths) if paths == vec![existing_file]
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
    assert!(matches!(effect, SessionEffect::ScanDirectory(target) if target == child));
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

    while session.advance_animations() {}
    assert!((session.rows()[0].expand_progress - 1.0).abs() < 1e-4);
    assert!((session.rows()[1].height_progress - 1.0).abs() < 1e-4);
    assert!(!session.is_animating());
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
    while session.advance_animations() {}

    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.advance_animations();
    let mid_collapse = session.rows()[0].expand_progress;
    assert!(mid_collapse < 1.0);

    // 收起中再点 = 反悔：进度折返向展开推进，而不是继续缩小。
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.advance_animations();
    assert!(session.rows()[0].expand_progress > mid_collapse);
    assert_eq!(session.rows().len(), 2);
    while session.advance_animations() {}
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
    assert!(!session.advance_animations());
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
fn navigation_clears_expansions_and_address_editing() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("sub", FileKind::Directory)]);
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.update(SessionMessage::AddressEditingStarted);
    assert!(session.address_edit().is_some());

    let target = session.directory().join("sub");
    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    assert_eq!(session.directory(), target);
    assert!(session.address_edit().is_none());
    assert!(matches!(session.listing(), DirectoryListing::Pending));
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
    assert!(matches!(forward, SessionEffect::ScanDirectory(_)));
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
    assert!(matches!(back, SessionEffect::ScanDirectory(_)));
}

#[test]
fn address_editing_prefills_navigates_and_cancels() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("a", FileKind::Directory)]);
    let start = session.directory().to_path_buf();

    session.update(SessionMessage::AddressEditingStarted);
    assert_eq!(
        session.address_edit(),
        Some(start.to_string_lossy().as_ref())
    );

    // 空草稿提交 = 取消编辑，留在原地。
    session.update(SessionMessage::AddressEditChanged("  ".to_string()));
    session.update(SessionMessage::AddressEditingSubmitted);
    assert_eq!(session.directory(), start);
    assert!(session.address_edit().is_none());

    // 相对路径拼接当前目录；成功导航后编辑态退出。
    session.update(SessionMessage::AddressEditingStarted);
    session.update(SessionMessage::AddressEditChanged("a".to_string()));
    let effect = session.update(SessionMessage::AddressEditingSubmitted);
    assert!(matches!(effect, SessionEffect::ScanDirectory(dir) if dir == start.join("a")));
    assert!(session.address_edit().is_none());

    // 取消编辑。
    session.update(SessionMessage::AddressEditingStarted);
    session.update(SessionMessage::AddressEditingCancelled);
    assert!(session.address_edit().is_none());
}
