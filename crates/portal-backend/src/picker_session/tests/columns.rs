//! 多栏视图（columns 子模块）单元测试：栏链截断/追加、扫描按路径
//! 回填、`target_directory`/`selected_paths` 焦点栏/最右栏取值、导航
//! 重置栏链、视图互切承接、键盘 ←↑↓→/Enter/type-ahead、Ctrl+A、
//! 预览目标与逐栏可见窗口。

use std::path::{Path, PathBuf};

use super::*;
use crate::dbus_file_chooser::PickerResolution;
use crate::picker_request::{PickerKind, PickerRequestSpec};
use crate::picker_session::columns::COLUMNS_SCALE;
use crate::picker_session::scan::{DirectoryScanOutcome, DirectoryScanResult};
use crate::picker_session::scrollbar::ScrollbarViewport;
use crate::picker_session::{PickerViewMode, SessionMessage};
use bennu_theme::column_geometry::ColumnEntryGeometry;
use file_core::entry::EntryMetadata;
use tokio::sync::oneshot;

/// OpenFile 文件多选（多栏最常用组合）；回收站/SaveFile 用例单独构造。
fn columns_session(kind: PickerKind) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
    let (reply, receiver) = oneshot::channel();
    let base = tempfile::tempdir().unwrap();
    let mut session = PickerSession::new(
        &PickerRequestSpec {
            kind,
            accept_label: None,
            title: None,
            filters: Vec::new(),
            active_filter: None,
            start_folder: None,
            choices: Vec::new(),
        },
        "/req/columns".to_string(),
        base.keep(),
        PickerViewMode::List,
        reply,
    );
    // 消息管线进多栏：与真实切换路径同源。
    session.update(SessionMessage::ViewModeSelected {
        mode: PickerViewMode::Columns,
    });
    (session, receiver)
}

fn open_file_session(multiple: bool) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
    columns_session(PickerKind::OpenFile {
        multiple,
        directory: false,
    })
}

/// 列表模式会话（视图互切用例用：不预先切到多栏）。
fn list_session(
    multiple: bool,
    directory: bool,
) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
    let (reply, receiver) = oneshot::channel();
    let base = tempfile::tempdir().unwrap();
    let session = PickerSession::new(
        &PickerRequestSpec {
            kind: PickerKind::OpenFile {
                multiple,
                directory,
            },
            accept_label: None,
            title: None,
            filters: Vec::new(),
            active_filter: None,
            start_folder: None,
            choices: Vec::new(),
        },
        "/req/columns".to_string(),
        base.keep(),
        PickerViewMode::List,
        reply,
    );
    (session, receiver)
}

fn scan(session: &mut PickerSession, directory: &Path, names: &[(&str, FileKind)]) {
    let entries = names
        .iter()
        .map(|&(name, kind)| {
            DirectoryEntry::new(
                directory.join(name),
                kind,
                EntryMetadata::default(),
                false,
                false,
                false,
            )
        })
        .collect();
    session.update(SessionMessage::ScanReady(Box::new(DirectoryScanResult {
        directory: directory.to_path_buf(),
        outcome: Ok(DirectoryScanOutcome { entries }),
    })));
}

fn chain(session: &PickerSession) -> Vec<PathBuf> {
    session.columns_chain().to_vec()
}

/// 预置根目录：{adir, bdir}。子目录内容在点开栏后再扫描（未开
/// 栏前扫描会被会话丢弃——既非根也非展开目标）。
fn seeded_root(session: &mut PickerSession) -> (PathBuf, PathBuf, PathBuf) {
    let root = session.directory().to_path_buf();
    let adir = root.join("adir");
    let bdir = root.join("bdir");
    let deep = adir.join("deep");
    scan(
        session,
        &root,
        &[("adir", FileKind::Directory), ("bdir", FileKind::Directory)],
    );
    (adir, bdir, deep)
}

/// 点开 adir 栏并回填其内容（deep 目录 + note.txt）。
fn open_adir_lane(session: &mut PickerSession) -> (PathBuf, PathBuf) {
    let root = session.directory().to_path_buf();
    let adir = root.join("adir");
    let deep = adir.join("deep");
    scan(
        session,
        &root,
        &[("adir", FileKind::Directory), ("bdir", FileKind::Directory)],
    );
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    scan(
        session,
        &adir,
        &[("deep", FileKind::Directory), ("note.txt", FileKind::File)],
    );
    (adir, deep)
}

#[test]
fn clicking_directory_appends_and_truncates_right_side() {
    let (mut session, _rx) = open_file_session(true);
    let (adir, bdir, deep) = seeded_root(&mut session);
    let root = session.directory().to_path_buf();

    // 点 adir → 栏链 [root, adir]，需要扫描。
    let effect = session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert!(matches!(effect, SessionEffect::ColumnScanAndReveal(path) if path == adir));
    assert_eq!(chain(&session), vec![root.clone(), adir.clone()]);
    scan(
        &mut session,
        &adir,
        &[("deep", FileKind::Directory), ("note.txt", FileKind::File)],
    );

    // 点 adir 里的 deep → [root, adir, deep]。
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert_eq!(
        chain(&session),
        vec![root.clone(), adir.clone(), deep.clone()]
    );

    // 点中间栏另一目录 bdir：右侧栏被替换 → [root, bdir]。
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert_eq!(chain(&session), vec![root.clone(), bdir.clone()]);

    // 点文件 → 截断右侧（无右侧），焦点留在文件所在栏。
    scan(&mut session, &bdir, &[("x.txt", FileKind::File)]);
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert_eq!(chain(&session), vec![root, bdir]);
    assert_eq!(session.columns_focused(), 1);
}

#[test]
fn scan_results_route_to_their_own_lane_by_path() {
    let (mut session, _rx) = open_file_session(false);
    let (adir, _deep) = open_adir_lane(&mut session);
    assert_eq!(session.column_entry_count(1), 2);
    assert_eq!(
        session.column_entry(1, 0).map(|e| e.path.clone()),
        Some(adir.join("deep"))
    );
    assert_eq!(
        session.column_entry(1, 1).map(|e| e.path.clone()),
        Some(adir.join("note.txt"))
    );
}

#[test]
fn target_directory_and_selected_paths_come_from_columns() {
    let (mut session, _rx) = open_file_session(false);
    let (adir, _deep) = open_adir_lane(&mut session);

    // 焦点进入点开的栏：目标目录 = 焦点栏目录。
    assert_eq!(session.target_directory(), adir);

    // 点文件：选中集落文件所在栏，确认集 = 该栏选中。
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert_eq!(session.selected_paths(), vec![adir.join("note.txt")]);
}

#[test]
fn confirmation_takes_rightmost_lane_with_selection() {
    // 选目录模式：多栏逐级点击目录时各栏都留下选中，确认取最右。
    let (mut session, mut receiver) = columns_session(PickerKind::OpenFile {
        multiple: false,
        directory: true,
    });
    let (adir, _bdir, deep) = seeded_root(&mut session);
    // 点 adir（第 0 栏选中）→ 点 deep（第 1 栏选中，第 2 栏打开）。
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    scan(&mut session, &adir, &[("deep", FileKind::Directory)]);
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 0,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::ConfirmPressed);
    let crate::dbus_file_chooser::PickerResolution::Confirmed(payload) =
        receiver.try_recv().unwrap()
    else {
        panic!("期望确认回信")
    };
    // 最右有选中项的那栏 = 第 1 栏（deep），不是第 0 栏的 adir。
    assert_eq!(payload.paths, vec![deep]);
}

#[test]
fn multi_selection_in_one_lane_confirms_as_a_set() {
    let (mut session, mut receiver) = open_file_session(true);
    let (adir, _bdir, _deep) = seeded_root(&mut session);
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    // 第 1 栏 Ctrl 多选两个文件。
    let second_file = adir.join("pic.png");
    scan(
        &mut session,
        &adir,
        &[
            ("deep", FileKind::Directory),
            ("note.txt", FileKind::File),
            ("pic.png", FileKind::File),
        ],
    );
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 1,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 2,
        ctrl: true,
        shift: false,
    });
    session.update(SessionMessage::ConfirmPressed);
    let crate::dbus_file_chooser::PickerResolution::Confirmed(payload) =
        receiver.try_recv().unwrap()
    else {
        panic!("期望确认回信")
    };
    let mut expected = vec![adir.join("note.txt"), second_file];
    expected.sort_unstable();
    let mut actual = payload.paths;
    actual.sort_unstable();
    assert_eq!(actual, expected);
}

#[test]
fn multi_selection_is_confined_within_one_lane() {
    let (mut session, _rx) = open_file_session(true);
    let root = session.directory().to_path_buf();
    scan(
        &mut session,
        &root,
        &[
            ("a.txt", FileKind::File),
            ("b.txt", FileKind::File),
            ("sub", FileKind::Directory),
        ],
    );
    // 点 sub（打开子栏）→ 点 c.txt（第 1 栏选中）→ 回第 0 栏点 a.txt。
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 2,
        ctrl: false,
        shift: false,
    });
    scan(
        &mut session,
        &root.join("sub"),
        &[("c.txt", FileKind::File)],
    );
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 0,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    // 点击第 0 栏文件截断了第 1 栏，选中集 = 第 0 栏的 a.txt。
    assert_eq!(session.selected_paths(), vec![root.join("a.txt")]);
}

#[test]
fn save_file_saves_into_focused_lane_directory() {
    let (mut session, mut receiver) = columns_session(PickerKind::SaveFile {
        default_name: Some("out.png".to_string()),
    });
    let root = session.directory().to_path_buf();
    let adir = root.join("adir");
    scan(&mut session, &root, &[("adir", FileKind::Directory)]);
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    // 焦点在第 1 栏（点击目录后进入）：保存路径 = 第 1 栏目录 + 名字。
    assert_eq!(session.columns_focused(), 1);
    session.update(SessionMessage::ConfirmPressed);
    let crate::dbus_file_chooser::PickerResolution::Confirmed(payload) =
        receiver.try_recv().unwrap()
    else {
        panic!("期望确认回信")
    };
    assert_eq!(payload.paths, vec![adir.join("out.png")]);
}

#[test]
fn navigation_resets_chain_to_single_lane() {
    let (mut session, _rx) = open_file_session(false);
    let (adir, bdir, _deep) = seeded_root(&mut session);
    let root = session.directory().to_path_buf();
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    scan(&mut session, &adir, &[("deep", FileKind::Directory)]);
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 0,
        ctrl: false,
        shift: false,
    });
    assert_eq!(chain(&session).len(), 3);

    // 面包屑导航（历史导航/侧边栏/地址栏同一 enter_directory 收口）。
    session.update(SessionMessage::BreadcrumbActivated {
        target: bdir.clone(),
    });
    assert_eq!(chain(&session), vec![bdir.clone()]);
    assert_eq!(session.columns_focused(), 0);
    // 上级导航同样重置。
    session.update(SessionMessage::NavigateUp);
    assert_eq!(chain(&session), vec![root]);
}

#[test]
fn view_switch_hands_over_selection_and_directory_both_ways() {
    let (mut session, _rx) = list_session(true, false);
    let root = session.directory().to_path_buf();
    scan(
        &mut session,
        &root,
        &[
            ("a.txt", FileKind::File),
            ("b.txt", FileKind::File),
            ("sub", FileKind::Directory),
        ],
    );

    // 列表多选根级条目 → 进多栏：选中承接为第 0 栏选中。
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::EntryClicked {
        index: 1,
        ctrl: true,
        shift: false,
    });
    session.update(SessionMessage::ViewModeSelected {
        mode: PickerViewMode::Columns,
    });
    assert_eq!(session.selected_paths().len(), 2);

    // 多栏里点进 sub 选中 c.txt → 回列表：目录 = 焦点栏目录，选中承接。
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 2,
        ctrl: false,
        shift: false,
    });
    scan(
        &mut session,
        &root.join("sub"),
        &[("c.txt", FileKind::File)],
    );
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 0,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::ViewModeSelected {
        mode: PickerViewMode::List,
    });
    assert_eq!(session.directory(), root.join("sub"));
    assert_eq!(
        session.selected_paths(),
        vec![root.join("sub").join("c.txt")]
    );
    // 行集合 = 焦点栏条目（子内容上移为根列表）。
    assert_eq!(session.rows().len(), 1);
}

#[test]
fn entering_columns_with_single_directory_selection_opens_child_lane() {
    // 选目录模式：列表里单选目录 → 进多栏追加其子栏。
    let (mut session, _rx) = list_session(false, true);
    let root = session.directory().to_path_buf();
    scan(
        &mut session,
        &root,
        &[("sub", FileKind::Directory), ("f.txt", FileKind::File)],
    );
    session.update(SessionMessage::EntryClicked {
        index: 0,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::ViewModeSelected {
        mode: PickerViewMode::Columns,
    });
    assert_eq!(chain(&session), vec![root.clone(), root.join("sub")]);
    // 确认集仍指向被进入的目录（新栏无选中）。
    assert_eq!(session.selected_paths(), vec![root.join("sub")]);
}

#[test]
fn entering_columns_with_file_selection_keeps_single_lane() {
    // 文件模式：单选文件 → 进多栏不开子栏。
    let (mut session, _rx) = list_session(false, false);
    let root = session.directory().to_path_buf();
    scan(
        &mut session,
        &root,
        &[("sub", FileKind::Directory), ("f.txt", FileKind::File)],
    );
    session.update(SessionMessage::EntryClicked {
        index: 1,
        ctrl: false,
        shift: false,
    });
    session.update(SessionMessage::ViewModeSelected {
        mode: PickerViewMode::Columns,
    });
    assert_eq!(chain(&session), vec![root.clone()]);
    assert_eq!(session.selected_paths(), vec![root.join("f.txt")]);
}

#[test]
fn arrow_keys_move_within_lane_and_cross_lanes_without_crossing_bounds() {
    let (mut session, _rx) = open_file_session(false);
    let (adir, _bdir, _deep) = seeded_root(&mut session);
    let root = session.directory().to_path_buf();

    // ↓：首次键盘移动落在第 0 行（adir，文件模式不可选，仅光标）；
    // 边界：继续 ↓ 不越界。
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    assert_eq!(session.columns_cursor(0), Some(0));
    session.update(SessionMessage::ListCursorMoved { delta: -5 });
    assert_eq!(session.columns_cursor(0), Some(0));

    // →：进入光标目录（新栏首项光标）。
    session.update(SessionMessage::ColumnsForwardRequested);
    assert_eq!(chain(&session), vec![root.clone(), adir.clone()]);
    assert_eq!(session.columns_focused(), 1);
    assert_eq!(session.columns_cursor(1), Some(0));

    // 第 1 栏 ↓ 后 ←：回到父栏光标落在打开本栏的 adir 上。
    session.update(SessionMessage::ListCursorMoved { delta: 1 });
    session.update(SessionMessage::ColumnsBackwardRequested);
    assert_eq!(session.columns_focused(), 0);
    assert_eq!(session.columns_cursor(0), Some(0));

    // 第 0 栏 ←：边界不越界。
    session.update(SessionMessage::ColumnsBackwardRequested);
    assert_eq!(session.columns_focused(), 0);
}

#[test]
fn enter_key_activates_directory_cursor_and_otherwise_confirms() {
    let (mut session, mut receiver) = open_file_session(false);
    let (adir, _deep) = open_adir_lane(&mut session);
    // 焦点栏光标在文件行 → Enter 确认该选中。
    session.update(SessionMessage::ListCursorMoved { delta: -1 });
    session.update(SessionMessage::ListCursorJumped { to_end: true });
    session.update(SessionMessage::ConfirmPressed);
    match receiver.try_recv().unwrap() {
        crate::dbus_file_chooser::PickerResolution::Confirmed(payload) => {
            assert_eq!(payload.paths, vec![adir.join("note.txt")]);
        }
        _ => panic!("期望确认回信"),
    }
}

#[test]
fn type_ahead_matches_within_focused_lane() {
    let (mut session, _rx) = open_file_session(false);
    let (adir, _deep) = open_adir_lane(&mut session);
    // 焦点在第 1 栏：type-ahead 匹配本栏的 note（不匹配第 0 栏）。
    session.update(SessionMessage::TypeAheadChar { ch: 'n' });
    assert_eq!(session.columns_focused(), 1);
    assert_eq!(session.columns_cursor(1), Some(1));
    // 回父栏：type-ahead 重新在焦点栏匹配（adir 首字符）。
    session.update(SessionMessage::ColumnsBackwardRequested);
    session.update(SessionMessage::TypeAheadChar { ch: 'a' });
    assert_eq!(session.columns_focused(), 0);
    assert_eq!(session.columns_cursor(0), Some(0));
    let _ = adir;
}

#[test]
fn select_all_covers_focused_lane_only() {
    let (mut session, _rx) = open_file_session(true);
    let (adir, _deep) = open_adir_lane(&mut session);
    // 焦点在第 1 栏（adir）：Ctrl+A 全选本栏可选文件（note.txt）。
    session.update(SessionMessage::SelectAllPressed);
    assert_eq!(session.selected_paths(), vec![adir.join("note.txt")]);
}

#[test]
fn preview_target_is_focused_lane_anchor_entry() {
    let (mut session, _rx) = open_file_session(false);
    let (adir, _deep) = open_adir_lane(&mut session);
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert_eq!(
        session.preview_target_entry().map(|e| e.path.clone()),
        Some(adir.join("note.txt"))
    );
}

#[test]
fn columns_storage_value_round_trips() {
    assert_eq!(
        PickerViewMode::from_storage_value(Some("columns")),
        PickerViewMode::Columns
    );
    assert_eq!(PickerViewMode::Columns.storage_value(), "columns");
}

#[test]
fn lane_visible_window_maps_offsets_with_lane_geometry() {
    let (mut session, _rx) = open_file_session(false);
    let files: Vec<String> = (0..100).map(|index| format!("img{index:03}.png")).collect();
    let names: Vec<(&str, FileKind)> = files
        .iter()
        .map(|name| (name.as_str(), FileKind::File))
        .collect();
    let lane_root = session.directory().to_path_buf();
    scan(&mut session, &lane_root, &names);
    let lane = 0;
    let geometry = ColumnEntryGeometry::for_scale(COLUMNS_SCALE);
    let stride = geometry.entry_scroll_height;
    let viewport = ScrollbarViewport {
        offset_x: 0.0,
        offset_y: 40.0 * stride,
        viewport_width: 300.0,
        viewport_height: 10.0 * stride,
        content_width: 300.0,
        content_height: 100.0 * stride,
    };
    let (first, last) = session.columns_visible_window(lane, &viewport).unwrap();
    // 视口盖住第 40-49 行（首行起点 = padding 顶 5px），±8 行余量。
    assert_eq!(first, 31);
    assert_eq!(last, 57);
}

#[test]
fn emphasis_only_lives_in_focused_lane() {
    // 栏链上父层级行不亮强调色：高亮只归焦点栏，父层级的被点开
    // 目录行走 open_child 态（视觉层职责）。
    let (mut session, _rx) = open_file_session(false);
    let (adir, _bdir, _deep) = seeded_root(&mut session);

    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    scan(&mut session, &adir, &[("deep", FileKind::Directory)]);

    // 点击把焦点推进到子栏（lane1）；cursor 留在 lane0 但 lane0 已非
    // 焦点栏：父层级不亮（open_child 灰态由视觉层画），新栏无选中
    // 无光标也不亮。绿只归焦点栏里的真选中。
    assert_eq!(session.columns.focused(), 1);
    assert!(!session.columns_row_highlighted(0, 0));
    assert!(!session.columns_row_highlighted(1, 0));
}

#[test]
fn lane_click_clears_other_lanes_selection() {
    // 跨栏点击重置：选中集只允许存在于一栏（design 决策）。
    let (mut session, _rx) = open_file_session(true);
    let (adir, _bdir, deep) = seeded_root(&mut session);

    // lane0 点 adir 开子栏；回填后 lane1 选中 note.txt。
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 0,
        ctrl: false,
        shift: false,
    });
    scan(
        &mut session,
        &adir,
        &[("deep", FileKind::Directory), ("note.txt", FileKind::File)],
    );
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 1,
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert_eq!(
        session.columns_rightmost_selection_paths(),
        &[adir.join("note.txt")]
    );

    // 点回 lane0 的 bdir：lane1 选中熄灭；文件模式下 bdir 不可选，
    // 选中集整体为空（目录导航不等于选中）。
    let root = session.directory().to_path_buf();
    scan(
        &mut session,
        &root,
        &[("adir", FileKind::Directory), ("bdir", FileKind::Directory)],
    );
    session.update(SessionMessage::ColumnEntryClicked {
        lane: 0,
        index: 1,
        ctrl: false,
        shift: false,
    });
    assert!(session.columns_rightmost_selection_paths().is_empty());
    let _ = deep;
}
