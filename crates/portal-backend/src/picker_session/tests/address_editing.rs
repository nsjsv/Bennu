//! 地址栏编辑会话的状态机测试（进入/防抖/回填/提交/取消/补全/导航）。
//! 共享构造器见 tests/mod.rs。

use super::{seeded_listing, session};
use crate::picker_request::PickerKind;
use crate::picker_session::{
    DirectoryListing, PathSuggestionDirection, SessionEffect, SessionMessage,
};
use file_core::entry::FileKind;

#[test]
fn navigation_clears_expansions_and_address_editing() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("sub", FileKind::Directory)]);
    session.update(SessionMessage::EntryExpandToggled { index: 0 });
    session.update(SessionMessage::AddressEditingStarted);
    assert!(session.address_editing().is_some());

    let target = session.directory().join("sub");
    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    assert_eq!(session.directory(), target);
    assert!(session.address_editing().is_none());
    assert!(matches!(session.listing(), DirectoryListing::Pending));
    // 导航退出编辑必须走带快照的 exit transition（target=0）：不允许
    // target=1.0 的孤儿过渡残留——那会让面包屑层 opacity=0 永不恢复。
    assert!(session
        .address_bar_transition
        .as_ref()
        .is_none_or(|transition| transition.target_fraction() == 0.0));
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
        session
            .address_editing()
            .map(|editing| editing.draft.as_str()),
        Some(start.to_string_lossy().as_ref())
    );

    // 空草稿提交 = 取消编辑，留在原地。
    session.update(SessionMessage::AddressEditChanged("  ".to_string()));
    session.update(SessionMessage::AddressEditingSubmitted);
    assert_eq!(session.directory(), start);
    assert!(session.address_editing().is_none());

    // 相对路径拼接当前目录；成功导航后编辑态退出。
    session.update(SessionMessage::AddressEditingStarted);
    session.update(SessionMessage::AddressEditChanged("a".to_string()));
    let effect = session.update(SessionMessage::AddressEditingSubmitted);
    assert!(matches!(effect, SessionEffect::NavigateDirectory(dir) if dir == start.join("a")));
    assert!(session.address_editing().is_none());

    // 取消编辑。
    session.update(SessionMessage::AddressEditingStarted);
    session.update(SessionMessage::AddressEditingCancelled);
    assert!(session.address_editing().is_none());
}

#[test]
fn submitting_prefers_selected_suggestion_over_parsed_draft() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("a", FileKind::Directory)]);
    let start = session.directory().to_path_buf();
    session.update(SessionMessage::AddressEditingStarted);
    let editing = session.address_editing.as_mut().expect("editing session");
    editing.suggestions = vec![start.join("suggestion"), start.join("other")];
    editing.suggestion_selection = Some(1);
    // 草稿本身可解析成另一个目录：选中建议必须优先。
    editing.draft = start.join("a").to_string_lossy().into_owned();

    let effect = session.update(SessionMessage::AddressEditingSubmitted);

    assert!(matches!(
        effect,
        SessionEffect::NavigateDirectory(dir) if dir == start.join("other")
    ));
    assert!(session.address_editing.is_none());
}

#[test]
fn submitting_unparsable_draft_cancels_silently_without_navigation() {
    let (mut session, mut receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("a", FileKind::Directory)]);
    let start = session.directory().to_path_buf();
    session.update(SessionMessage::AddressEditingStarted);
    // 空白草稿解析为 None：静默取消——不导航、不关窗、不回复。
    session.update(SessionMessage::AddressEditChanged("   ".to_string()));

    let effect = session.update(SessionMessage::AddressEditingSubmitted);

    assert!(matches!(effect, SessionEffect::None));
    assert_eq!(session.directory(), start);
    assert!(session.address_editing.is_none());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn stale_generation_replies_are_rejected() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    let current_dir = session.directory().to_path_buf();
    let editing = session.address_editing.as_mut().expect("editing session");
    editing.draft = "docs".to_string();
    let stale_request = editing.next_suggestion_request(&current_dir);

    // 防抖窗口内又改了一稿：旧凭据（代数+草稿）已失效。
    session.update(SessionMessage::AddressEditChanged("docs2".to_string()));

    let stabilized = session.update(SessionMessage::AddressSuggestionInputStabilized {
        request: stale_request.clone(),
    });
    assert!(matches!(stabilized, SessionEffect::None));
    let loaded = session.update(SessionMessage::AddressSuggestionsLoaded {
        request: stale_request,
        suggestions: vec![current_dir.join("stale")],
    });
    assert!(matches!(loaded, SessionEffect::None));
    assert!(session
        .address_editing()
        .expect("editing session")
        .suggestions
        .is_empty());
}

#[test]
fn matched_replies_fill_suggestions_and_enable_keyboard_routing() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    let current_dir = session.directory().to_path_buf();
    let editing = session.address_editing.as_mut().expect("editing session");
    editing.draft = "docs".to_string();
    let request = editing.next_suggestion_request(&current_dir);

    let stabilized = session.update(SessionMessage::AddressSuggestionInputStabilized {
        request: request.clone(),
    });
    assert!(matches!(
        stabilized,
        SessionEffect::LoadPathSuggestions { .. }
    ));

    let loaded = session.update(SessionMessage::AddressSuggestionsLoaded {
        request,
        suggestions: vec![current_dir.join("docs-a"), current_dir.join("docs-b")],
    });
    assert!(matches!(loaded, SessionEffect::None));
    assert!(session.address_suggestion_keyboard_is_active());
}

#[test]
fn cancelling_keeps_draft_snapshot_for_exit_transition() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    session.update(SessionMessage::AddressEditChanged("/tmp/draft".to_string()));

    session.update(SessionMessage::AddressEditingCancelled);

    assert!(session.address_editing.is_none());
    // 渐出期间视图从过渡快照里渲染草稿。
    assert_eq!(session.address_exit_snapshot(), Some("/tmp/draft"));
    assert!(session.address_transition_fraction() > 0.0);
    assert!(session.is_animating());
}

#[test]
fn completed_exit_transition_is_collected_after_duration() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    session.update(SessionMessage::AddressEditingCancelled);
    assert!(session.address_bar_transition.is_some());

    // 真实时间过渡（160ms 级）：等播完再推帧，退出过渡必须被摘除。
    std::thread::sleep(std::time::Duration::from_millis(200));
    session.advance_address_bar_transition();

    assert!(session.address_bar_transition.is_none());
    assert!(!session.is_animating());
    assert_eq!(session.address_exit_snapshot(), None);
}

#[test]
fn navigate_back_forward_bounds_follow_history_position() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("a", FileKind::Directory)]);
    let start = session.directory().to_path_buf();

    assert!(!session.can_navigate_back());
    assert!(!session.can_navigate_forward());

    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    let child = start.join("a");
    assert_eq!(session.directory(), child);
    assert!(session.can_navigate_back());
    assert!(!session.can_navigate_forward());

    session.update(SessionMessage::NavigateBack);
    assert_eq!(session.directory(), start);
    assert!(!session.can_navigate_back());
    assert!(session.can_navigate_forward());
}

#[test]
fn breadcrumb_current_segment_enters_editing_others_navigate() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(&mut session, &[("a", FileKind::Directory)]);
    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    let current = session.directory().to_path_buf();
    let parent = current.parent().unwrap().to_path_buf();

    // 当前目录段 = 进入编辑（草稿预填当前目录）。
    session.update(SessionMessage::BreadcrumbActivated {
        target: current.clone(),
    });
    assert_eq!(
        session
            .address_editing()
            .map(|editing| editing.draft.as_str()),
        Some(current.to_string_lossy().as_ref())
    );

    // 其他段 = 退出编辑并导航。
    let effect = session.update(SessionMessage::BreadcrumbActivated {
        target: parent.clone(),
    });
    assert!(matches!(effect, SessionEffect::NavigateDirectory(dir) if dir == parent));
    assert!(session.address_editing.is_none());
}

#[test]
fn suggestion_row_click_requires_membership_in_current_list() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    let start = session.directory().to_path_buf();

    // 陈旧浮层的点击：目标不在当前建议列表中，必须被拒。
    let effect = session.update(SessionMessage::AddressSuggestionSelected {
        path: start.join("ghost"),
    });
    assert!(matches!(effect, SessionEffect::None));
    assert!(session.address_editing.is_some());

    let editing = session.address_editing.as_mut().expect("editing session");
    editing.suggestions = vec![start.join("real")];
    let effect = session.update(SessionMessage::AddressSuggestionSelected {
        path: start.join("real"),
    });
    assert!(matches!(
        effect,
        SessionEffect::NavigateDirectory(dir) if dir == start.join("real")
    ));
}

#[test]
fn empty_address_draft_does_not_request_stabilization() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);

    // 空白草稿无处可补全：不发起防抖（Stabilize）效果，避免空转回信。
    let effect = session.update(SessionMessage::AddressEditChanged("   ".to_string()));
    assert!(matches!(effect, SessionEffect::None));
}

#[test]
fn repeated_address_editing_start_keeps_existing_draft() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    session.update(SessionMessage::AddressEditChanged(
        "/tmp/edited".to_string(),
    ));

    // 编辑态重复触发进入（如点地址栏空白/面包屑当前段）不重置草稿。
    session.update(SessionMessage::AddressEditingStarted);
    assert_eq!(
        session
            .address_editing()
            .map(|editing| editing.draft.as_str()),
        Some("/tmp/edited")
    );
}

#[test]
fn completed_enter_transition_holds_at_full_and_is_not_collected() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);

    // 进入方向播完停在 1.0：advance 只回收 exit 侧，保留它是为了下次
    // 退出能从中点连续起步（锁住 C1 修复依赖的不变量）。
    std::thread::sleep(std::time::Duration::from_millis(200));
    session.advance_address_bar_transition();

    let transition = session
        .address_bar_transition
        .as_ref()
        .expect("enter transition is kept after completion");
    assert_eq!(transition.target_fraction(), 1.0);
    assert!(transition.is_complete());
    assert!(!session.is_animating());
}

#[test]
fn move_selection_cycles_through_suggestions() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    let start = session.directory().to_path_buf();
    let editing = session.address_editing.as_mut().expect("editing session");
    editing.suggestions = vec![start.join("a"), start.join("b"), start.join("c")];

    session.update(SessionMessage::MoveSuggestionSelection {
        direction: PathSuggestionDirection::Next,
    });
    assert_eq!(
        session
            .address_editing()
            .and_then(|e| e.suggestion_selection),
        Some(0)
    );

    // 从末尾 Next 回到 0；从 0 Previous 绕到末尾（循环）。
    session.update(SessionMessage::MoveSuggestionSelection {
        direction: PathSuggestionDirection::Previous,
    });
    assert_eq!(
        session
            .address_editing()
            .and_then(|e| e.suggestion_selection),
        Some(2)
    );
    session.update(SessionMessage::MoveSuggestionSelection {
        direction: PathSuggestionDirection::Next,
    });
    assert_eq!(
        session
            .address_editing()
            .and_then(|e| e.suggestion_selection),
        Some(0)
    );
}

#[test]
fn completing_suggestion_writes_draft_and_requests_next_level() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    session.update(SessionMessage::AddressEditingStarted);
    let start = session.directory().to_path_buf();
    let editing = session.address_editing.as_mut().expect("editing session");
    editing.suggestions = vec![start.join("a"), start.join("b")];

    // 首次 Complete 未选中时选第一项；草稿补尾分隔符并立即请求下一级。
    let effect = session.update(SessionMessage::CompleteSuggestion {
        direction: PathSuggestionDirection::Next,
    });
    assert!(matches!(effect, SessionEffect::LoadPathSuggestions { .. }));
    let editing = session.address_editing().expect("editing session");
    let expected = format!(
        "{}{}",
        start.join("a").to_string_lossy(),
        std::path::MAIN_SEPARATOR
    );
    assert_eq!(editing.draft, expected);
    assert_eq!(editing.suggestion_selection, Some(0));

    // 无建议时 Complete 是 no-op（不产生请求）。
    let editing = session.address_editing.as_mut().expect("editing session");
    editing.suggestions.clear();
    let effect = session.update(SessionMessage::CompleteSuggestion {
        direction: PathSuggestionDirection::Next,
    });
    assert!(matches!(effect, SessionEffect::None));
}
