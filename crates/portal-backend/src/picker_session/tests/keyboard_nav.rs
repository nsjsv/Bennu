//! 列表键盘导航的会话级测试：三模式移动语义、type-ahead、光标重验证、
//! 翻页步长与滚动跟随。共享构造器见 tests/mod.rs。

use super::{seeded_listing, session};
use crate::picker_request::PickerKind;
use crate::picker_session::expansion::{LIST_ROW_HEIGHT, LIST_ROW_STRIDE};
use crate::picker_session::scan::DirectoryScanOutcome;
use crate::picker_session::scrollbar::{ScrollbarViewport, SessionScrollRegion};
use crate::picker_session::PickerSession;
use crate::picker_session::{DirectoryScanResult, SessionEffect, SessionMessage};
use file_core::entry::FileKind;

/// 装入列表视口缓存：溢出与否不影响键盘逻辑，这里只关心几何。
fn install_list_viewport(session: &mut PickerSession, viewport: ScrollbarViewport) {
    session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
        region: SessionScrollRegion::List,
        viewport,
    });
}

fn list_viewport(offset_y: f32, viewport_height: f32, content_height: f32) -> ScrollbarViewport {
    ScrollbarViewport {
        offset_x: 0.0,
        offset_y,
        viewport_width: 600.0,
        viewport_height,
        content_width: 600.0,
        content_height,
    }
}

fn files(session: &mut PickerSession, count: usize) {
    let names: Vec<String> = (0..count).map(|index| format!("f{index:03}.txt")).collect();
    let refs: Vec<(&str, FileKind)> = names
        .iter()
        .map(|name| (name.as_str(), FileKind::File))
        .collect();
    seeded_listing(session, &refs);
}

#[test]
fn cursor_moves_follow_click_selection_rules_in_single_file_mode() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[
            ("alpha.txt", FileKind::File),
            ("docs", FileKind::Directory),
            ("beta.txt", FileKind::File),
        ],
    );

    // 首次 ↓ 落在第 0 行并按单击规则选中。
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.list_cursor(), Some(0));
    assert_eq!(session.selection(), &[0]);

    // ↓ 到目录行：光标前进，旧选中集熄灭（文件模式目录不可选；
    // 保留旧选中会与光标行双高亮）。
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.list_cursor(), Some(1));
    assert!(session.selection().is_empty());

    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.list_cursor(), Some(2));
    assert_eq!(session.selection(), &[2]);

    // 顶部/底部饱和，不越界。
    session.update(SessionMessage::ListCursorMoved { delta: -9 });
    assert_eq!(session.list_cursor(), Some(0));
    session.update(SessionMessage::ListCursorMoved { delta: 9 });
    assert_eq!(session.list_cursor(), Some(2));
}

#[test]
fn click_records_cursor_and_arrow_moves_from_last_click() {
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
    // 点击即落光标：最后点击行=键盘导航起点（资源管理器语义）。
    assert_eq!(session.list_cursor(), Some(2));

    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.list_cursor(), Some(3));
    // 多选模式下光标移动重置选中集为光标行（资源管理器语义；shift
    // 范围扩展不在本任务范围）。
    assert_eq!(session.selection(), &[3]);
    session.update(SessionMessage::ListCursorMoved { delta: -1 });
    assert_eq!(session.list_cursor(), Some(2));
    assert_eq!(session.selection(), &[2]);
}

#[test]
fn cursor_movement_respects_directory_mode_gating() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: true,
    });
    seeded_listing(
        &mut session,
        &[("a.txt", FileKind::File), ("dir", FileKind::Directory)],
    );

    // 选目录模式：文件行光标可落但不进选中集；目录行进入选中集。
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.list_cursor(), Some(0));
    assert!(session.selection().is_empty());
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.list_cursor(), Some(1));
    assert_eq!(session.selection(), &[1]);
}

#[test]
fn cursor_row_stays_highlighted_when_not_selectable() {
    // 文件模式下的目录行：点击与方向键都不落选中集，但高亮必须跟随
    // 光标（否则纯目录页里点击/方向键全部隐身）。
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[("dir", FileKind::Directory), ("a.txt", FileKind::File)],
    );

    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert!(session.selection().is_empty());
    assert!(session.row_highlighted(0));

    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.selection(), &[1]);
    // 光标移走后高亮跟随：第 0 行既非选中也非光标，熄灭。
    assert!(!session.row_highlighted(0));
    assert!(session.row_highlighted(1));

    // 反向：从文件移回文件夹，旧文件的选中集必须熄灭（否则双高亮）。
    session.update(SessionMessage::ListCursorMoved { delta: -1 });
    assert!(session.selection().is_empty());
    assert!(session.row_highlighted(0));
    assert!(!session.row_highlighted(1));
}

#[test]
fn save_file_cursor_movement_syncs_name_input_for_files_only() {
    let (mut session, _receiver) = session(PickerKind::SaveFile { default_name: None });
    seeded_listing(
        &mut session,
        &[("模板.txt", FileKind::File), ("dir", FileKind::Directory)],
    );

    // 光标落文件行 = 点击文件行：文件名同步进输入框。
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.name_input(), "模板.txt");
    // 目录行不改名（与点击语义一致）。
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.list_cursor(), Some(1));
    assert_eq!(session.name_input(), "模板.txt");
}

#[test]
fn home_and_end_jump_cursor_to_list_bounds() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
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

    session.update(SessionMessage::ListCursorJumped { to_end: true });
    assert_eq!(session.list_cursor(), Some(2));
    assert_eq!(session.selection(), &[2]);
    session.update(SessionMessage::ListCursorJumped { to_end: false });
    assert_eq!(session.list_cursor(), Some(0));
    assert_eq!(session.selection(), &[0]);
}

#[test]
fn page_moves_use_viewport_row_count_and_fall_back_without_viewport() {
    let (mut paged, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    files(&mut paged, 25);
    // 视口高 300 = 10 行步长；内容 750。
    install_list_viewport(
        &mut paged,
        list_viewport(0.0, LIST_ROW_STRIDE * 10.0, LIST_ROW_STRIDE * 25.0),
    );

    paged.update(SessionMessage::ListCursorMoved { delta: 1 }); // 首按落 0
    paged.update(SessionMessage::ListPageMoved { pages: 1 });
    assert_eq!(paged.list_cursor(), Some(10));
    paged.update(SessionMessage::ListPageMoved { pages: 1 });
    assert_eq!(paged.list_cursor(), Some(20));
    // 底部饱和。
    paged.update(SessionMessage::ListPageMoved { pages: 1 });
    assert_eq!(paged.list_cursor(), Some(24));
    paged.update(SessionMessage::ListPageMoved { pages: -1 });
    assert_eq!(paged.list_cursor(), Some(14));

    // 无视口缓存（首帧探针未回）：回落 10 行。
    let (mut fresh, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    files(&mut fresh, 25);
    fresh.update(SessionMessage::ListCursorMoved { delta: 1 }); // 落 0
    fresh.update(SessionMessage::ListPageMoved { pages: 1 });
    assert_eq!(fresh.list_cursor(), Some(10));
}

#[test]
fn cursor_revalidates_after_row_set_shrinks() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    files(&mut session, 5);
    session.update(SessionMessage::ListCursorJumped { to_end: true });
    assert_eq!(session.list_cursor(), Some(4));

    // 扫描回填只剩 2 行：光标回退到最近合法行。
    session.apply_scan(DirectoryScanResult {
        directory: session.directory().to_path_buf(),
        outcome: Ok(DirectoryScanOutcome {
            entries: vec![
                file_core::entry::DirectoryEntry::new(
                    session.directory().join("only.txt"),
                    FileKind::File,
                    file_core::entry::EntryMetadata::default(),
                    false,
                    false,
                    false,
                ),
                file_core::entry::DirectoryEntry::new(
                    session.directory().join("second.txt"),
                    FileKind::File,
                    file_core::entry::EntryMetadata::default(),
                    false,
                    false,
                    false,
                ),
            ],
        }),
    });
    assert_eq!(session.rows().len(), 2);
    assert_eq!(session.list_cursor(), Some(1));
}

#[test]
fn navigation_resets_cursor_and_type_ahead() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[("sub", FileKind::Directory), ("a.txt", FileKind::File)],
    );
    session.update(SessionMessage::ListCursorJumped { to_end: true });
    session.update(SessionMessage::TypeAheadChar { ch: 's' });
    assert!(session.type_ahead_is_active());

    // 进入目录：光标与缓冲随目录作废。
    session.update(SessionMessage::EntryDoubleClicked { index: 0 });
    assert_eq!(session.list_cursor(), None);
    assert!(!session.type_ahead_is_active());
}

#[test]
fn type_ahead_accumulates_narrows_and_keeps_buffer_without_match() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[
            ("apple.txt", FileKind::File),
            ("apricot.txt", FileKind::File),
            ("banana.txt", FileKind::File),
        ],
    );

    session.update(SessionMessage::TypeAheadChar { ch: 'a' });
    assert_eq!(session.list_cursor(), Some(0));
    // 累积收窄："ap" 仍命中 apple，"apr" 跳到 apricot。
    session.update(SessionMessage::TypeAheadChar { ch: 'p' });
    assert_eq!(session.list_cursor(), Some(0));
    session.update(SessionMessage::TypeAheadChar { ch: 'r' });
    assert_eq!(session.list_cursor(), Some(1));
    assert_eq!(session.selection(), &[1]);

    // 无匹配：保持原位且不清缓冲（大小写不敏感由匹配层小写化保证）。
    session.update(SessionMessage::TypeAheadChar { ch: 'Z' });
    assert_eq!(session.list_cursor(), Some(1));
    assert!(session.type_ahead_is_active());

    // Esc 清缓冲（TypeAheadReset）后缓冲失效。
    session.update(SessionMessage::TypeAheadReset);
    assert!(!session.type_ahead_is_active());
}

#[test]
fn list_keys_clear_type_ahead_buffer() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut session,
        &[("a", FileKind::File), ("b", FileKind::File)],
    );
    session.update(SessionMessage::TypeAheadChar { ch: 'b' });
    assert!(session.type_ahead_is_active());

    // ↑↓Home/End/Page 等列表键打断 type-ahead：缓冲清空。
    session.update(SessionMessage::ListCursorMoved { delta: -1 });
    assert!(!session.type_ahead_is_active());
    session.update(SessionMessage::TypeAheadChar { ch: 'b' });
    session.update(SessionMessage::ListCursorJumped { to_end: true });
    assert!(!session.type_ahead_is_active());
    session.update(SessionMessage::TypeAheadChar { ch: 'b' });
    session.update(SessionMessage::ListPageMoved { pages: -1 });
    assert!(!session.type_ahead_is_active());
}

#[test]
fn enter_and_arrow_targets_depend_on_cursor_row_kind() {
    // 文件模式：光标在目录行 → Enter 激活（进入目录）、←/→ 折叠开关。
    let (mut file_session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    seeded_listing(
        &mut file_session,
        &[("a.txt", FileKind::File), ("dir", FileKind::Directory)],
    );
    file_session.update(SessionMessage::ListCursorMoved { delta: 1 });
    file_session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert!(matches!(
        file_session.keyboard_enter_activation(),
        Some(SessionMessage::EntryDoubleClicked { index: 1 })
    ));
    assert_eq!(file_session.cursor_directory_row(), Some(1));
    // 文件行：Enter 走确认、←/→ 不劫持。
    file_session.update(SessionMessage::ListCursorMoved { delta: -1 });
    assert!(file_session.keyboard_enter_activation().is_none());
    assert_eq!(file_session.cursor_directory_row(), None);

    // 选目录模式：目录行的 Enter 也走确认（选中目录而非进入）。
    let (mut dir_session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: true,
    });
    seeded_listing(&mut dir_session, &[("dir", FileKind::Directory)]);
    dir_session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert!(dir_session.keyboard_enter_activation().is_none());
    assert_eq!(dir_session.cursor_directory_row(), Some(0));
}

#[test]
fn cursor_below_viewport_scrolls_to_bottom_edge() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    files(&mut session, 30);
    // 视口在第 0-9 行；内容 900。
    install_list_viewport(
        &mut session,
        list_viewport(0.0, LIST_ROW_STRIDE * 10.0, LIST_ROW_STRIDE * 30.0),
    );

    let effect = session.update(SessionMessage::ListCursorJumped { to_end: true });
    // 第 29 行底缘 29×30+28=898，贴底 = 898-300 = 598。
    let expected = 29.0 * LIST_ROW_STRIDE + LIST_ROW_HEIGHT - LIST_ROW_STRIDE * 10.0;
    assert!(
        matches!(effect, SessionEffect::ScrollListTo { offset_y } if (offset_y - expected).abs() < f32::EPSILON)
    );
    // 缓存同步：scroll_to 不回发 on_scroll，连续按键靠它判断可见性。
    assert_eq!(
        session
            .scrollbar_viewport_for(&SessionScrollRegion::List)
            .unwrap()
            .offset_y,
        expected
    );
}

#[test]
fn cursor_above_viewport_scrolls_to_top_edge() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    files(&mut session, 30);
    install_list_viewport(
        &mut session,
        list_viewport(0.0, LIST_ROW_STRIDE * 10.0, LIST_ROW_STRIDE * 30.0),
    );
    session.update(SessionMessage::ListCursorJumped { to_end: true });

    // Home：第 0 行顶缘 0 < 当前偏移 → 贴顶归零。
    let effect = session.update(SessionMessage::ListCursorJumped { to_end: false });
    assert!(matches!(effect, SessionEffect::ScrollListTo { offset_y } if offset_y == 0.0));
    assert_eq!(
        session
            .scrollbar_viewport_for(&SessionScrollRegion::List)
            .unwrap()
            .offset_y,
        0.0
    );
}

#[test]
fn visible_cursor_does_not_scroll() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    files(&mut session, 30);
    install_list_viewport(
        &mut session,
        list_viewport(0.0, LIST_ROW_STRIDE * 10.0, LIST_ROW_STRIDE * 30.0),
    );

    // 行矩形已在视口内：不产生滚动。
    let effect = session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert!(matches!(effect, SessionEffect::None));
}

#[test]
fn keyboard_scroll_interrupts_wheel_inertia() {
    let (mut session, _receiver) = session(PickerKind::OpenFile {
        multiple: false,
        directory: false,
    });
    files(&mut session, 30);
    install_list_viewport(
        &mut session,
        list_viewport(0.0, LIST_ROW_STRIDE * 10.0, LIST_ROW_STRIDE * 30.0),
    );
    session.handle_scroll_message(SessionMessage::WheelScrolled {
        region: SessionScrollRegion::List,
        delta: iced::mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
    });
    assert!(session.smooth_scroll.is_active());

    // 键盘滚动即时到位：进行中的滚轮惯性必须被打断，否则惯性会把
    // 偏移再拉走。
    session.update(SessionMessage::ListCursorJumped { to_end: true });
    assert!(!session.smooth_scroll.is_active());
}
