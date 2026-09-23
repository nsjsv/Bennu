//! 键盘路由：全局按键 → 会话消息的裁决层。优先级（对齐主软件
//! keyboard-routing 规范的 captured 语义）：
//! 1. 补全面板活跃：↑/↓/Tab/Shift+Tab 无视 captured 优先归补全；
//! 2. Esc：编辑态退编辑 > type-ahead 清缓冲 > 未捕获时关窗；
//! 3. 仅 Ignored 的动作：Ctrl+A 全选、Enter、Backspace 上级、列表
//!    导航与 type-ahead 字符（编辑态的 Enter/字符已被 text_input 捕获，
//!    天然不进这里）。
//! 从 main.rs 拆出以守住其行数上限；路由判定独立成纯函数便于单测。

use iced::{keyboard, window, Task};

use super::{Message, PickerDaemon};
use crate::picker_session::{PathSuggestionDirection, PickerSession, SessionMessage};

pub(crate) fn handle_key(
    daemon: &PickerDaemon,
    window_id: window::Id,
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    captured: bool,
) -> Task<Message> {
    if matches!(key, keyboard::Key::Named(keyboard::key::Named::Space)) {
        // Space 预览（主软件 ShortcutAction::Preview / 预览窗内
        // CapturedPreviewShortcutPressed 的合流）。
        if let Some(source) = space_toggle_source(daemon, window_id, captured) {
            return dispatch_space_toggle(source);
        }
        return Task::none();
    }
    if daemon.preview.is_preview_window(window_id) {
        // Esc 只关预览且无视 captured（主软件 Escape 恒走 Application
        // 路由的 handle_focused_window_escape_pressed → Preview 分支）；
        // 其余键归预览窗控件（滚动/媒体控件由 iced widget 自理）。
        if preview_escape_requested(daemon, window_id, &key) {
            return dispatch_preview_close();
        }
        return Task::none();
    }
    let Some(session) = daemon.windows.get(&window_id) else {
        return Task::none();
    };
    match key_message(session, &key, modifiers, captured) {
        Some(message) => dispatch_session_message(window_id, message),
        None => Task::none(),
    }
}

/// Space 预览裁决（纯函数便于单测）：Some(source) = 触发 toggle，
/// source = 选中项所属选择窗（预览窗内按下时为发起会话的请求窗，
/// 可能已亡 → None → toggle 退化为关闭）；外层 None = 不触发。
/// 选择窗内仅未捕获且非编辑态触发（Space 是 Named 键，不与仅认
/// Character 的 type-ahead 竞争）；预览窗内 captured 与否同语义。
fn space_toggle_source(
    daemon: &PickerDaemon,
    window_id: window::Id,
    captured: bool,
) -> Option<Option<window::Id>> {
    if daemon.preview.is_preview_window(window_id) {
        return Some(daemon.preview.selection_source_window(window_id));
    }
    let editing = daemon
        .windows
        .get(&window_id)
        .is_some_and(|session| session.address_editing().is_some());
    (!captured && !editing).then_some(Some(window_id))
}

fn dispatch_space_toggle(source: Option<window::Id>) -> Task<Message> {
    Task::perform(async {}, move |_| Message::PreviewSpacePressed { source })
}

/// Esc 分层门禁（纯函数便于单测）：预览窗的 Esc 只关预览（无视
/// captured）；其余键与选择窗的 Esc 均不走此路（选择窗 Esc 走既有
/// 会话裁决：编辑退编辑 > type-ahead > 关窗），互不穿透。
fn preview_escape_requested(
    daemon: &PickerDaemon,
    window_id: window::Id,
    key: &keyboard::Key,
) -> bool {
    daemon.preview.is_preview_window(window_id)
        && matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape))
}

/// 预览窗 Esc：只关预览（焦点回请求窗在关闭路径内）。
fn dispatch_preview_close() -> Task<Message> {
    Task::perform(async {}, |_| Message::PreviewCloseFocused)
}

/// 键位 → 会话消息的完整裁决（纯函数，优先级见模块文档）。
fn key_message(
    session: &PickerSession,
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    captured: bool,
) -> Option<SessionMessage> {
    // 补全面板的方向键/Tab 优先于一切：text_input 聚焦会捕获这些键
    //（captured=true），必须在此消费而不是等 Ignored——与主软件在
    // 全局 handler 先查 address_suggestion_keyboard_is_active 同构。
    if session.address_suggestion_keyboard_is_active() {
        if let Some(message) = address_suggestion_key_message(key, modifiers) {
            return Some(message);
        }
    }
    if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
        return escape_message(session, captured);
    }
    // 其余全局动作（Enter 确认、Ctrl+A 全选、列表导航、type-ahead）只认
    // Ignored：text_input 已用 Enter 提交草稿、用 Ctrl+A 选中文本、用
    // 字符键打字，重复触发会互相打架。
    if captured {
        return None;
    }
    if let keyboard::Key::Character(character) = key {
        if character.eq_ignore_ascii_case("a") && modifiers.control() {
            return Some(SessionMessage::SelectAllPressed);
        }
    }
    list_navigation_message(session, key, modifiers)
}

/// Esc 优先级：编辑态退编辑（无视 captured）> type-ahead 清缓冲 > 未
/// 捕获时关窗。缓冲对用户不可见，直接关窗会丢掉"取消跳转"的意图。
fn escape_message(session: &PickerSession, captured: bool) -> Option<SessionMessage> {
    if session.address_editing().is_some() {
        return Some(SessionMessage::AddressEditingCancelled);
    }
    if session.type_ahead_is_active() {
        return Some(SessionMessage::TypeAheadReset);
    }
    if !captured {
        return Some(SessionMessage::DismissPressed);
    }
    None
}

/// 非编辑态列表键映射：Enter/Backspace/Home/End/PageUp/PageDown/
/// ↑↓←→ 与无 ctrl/alt/command 修饰的字符键（type-ahead）。←/→ 与
/// Enter 需要光标行的种类与模式判定，查询方法在 keyboard_nav 子模块。
fn list_navigation_message(
    session: &PickerSession,
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<SessionMessage> {
    use keyboard::key::Named;
    if !list_key_modifiers_clear(modifiers) {
        return None;
    }
    match key {
        keyboard::Key::Named(Named::Enter) => {
            // 光标在目录行（非选目录模式）→ 复用双击激活（进入目录）；
            // 其余 → 确认（多选保持选中集）。
            Some(match session.keyboard_enter_directory_row() {
                Some(index) => SessionMessage::EntryDoubleClicked { index },
                None => SessionMessage::ConfirmPressed,
            })
        }
        keyboard::Key::Named(Named::Backspace) => Some(SessionMessage::NavigateUp),
        keyboard::Key::Named(Named::ArrowUp) => Some(SessionMessage::ListCursorMoved { delta: -1 }),
        keyboard::Key::Named(Named::ArrowDown) => {
            Some(SessionMessage::ListCursorMoved { delta: 1 })
        }
        // 仅目录行劫持折叠/展开开关；文件行透传（不产生消息）。
        keyboard::Key::Named(Named::ArrowLeft | Named::ArrowRight) => session
            .cursor_directory_row()
            .map(|index| SessionMessage::EntryExpandToggled { index }),
        keyboard::Key::Named(Named::Home) => {
            Some(SessionMessage::ListCursorJumped { to_end: false })
        }
        keyboard::Key::Named(Named::End) => Some(SessionMessage::ListCursorJumped { to_end: true }),
        keyboard::Key::Named(Named::PageUp) => Some(SessionMessage::ListPageMoved { pages: -1 }),
        keyboard::Key::Named(Named::PageDown) => Some(SessionMessage::ListPageMoved { pages: 1 }),
        // type-ahead 只认单字符产出（组合输入取首字符）。
        keyboard::Key::Character(character) => character
            .chars()
            .next()
            .map(|ch| SessionMessage::TypeAheadChar { ch }),
        _ => None,
    }
}

/// 列表键与 type-ahead 的修饰键门：ctrl/alt/command 组合留给控件与
/// 未来快捷键；shift 放行（大小写在匹配层忽略；方向键的 shift 范围
/// 扩展不在本任务范围，shift+方向按普通移动处理）。
///
/// Enter/Backspace 不清 type-ahead：确认/返回是缓冲匹配结果的消费者
/// 而非竞争者，清了反而打断"跳转后回车"的连贯动作。
fn list_key_modifiers_clear(modifiers: keyboard::Modifiers) -> bool {
    !modifiers.control() && !modifiers.alt() && !modifiers.command()
}

/// 补全面板活跃时的按键映射（主软件 handle_path_suggestion_keyboard_
/// key 同构）：↓/无修饰 = Next，↑/无修饰 = Previous，Tab/无修饰 =
/// 补全 Next，Shift+Tab = 补全 Previous；带其他修饰键不拦截。
fn address_suggestion_key_message(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<SessionMessage> {
    let no_shortcut_modifiers =
        !modifiers.alt() && !modifiers.control() && !modifiers.command() && !modifiers.shift();
    let only_shift_modifier =
        modifiers.shift() && !modifiers.alt() && !modifiers.control() && !modifiers.command();
    match key.as_ref() {
        keyboard::Key::Named(keyboard::key::Named::ArrowDown) if no_shortcut_modifiers => {
            Some(SessionMessage::MoveSuggestionSelection {
                direction: PathSuggestionDirection::Next,
            })
        }
        keyboard::Key::Named(keyboard::key::Named::ArrowUp) if no_shortcut_modifiers => {
            Some(SessionMessage::MoveSuggestionSelection {
                direction: PathSuggestionDirection::Previous,
            })
        }
        keyboard::Key::Named(keyboard::key::Named::Tab) if only_shift_modifier => {
            Some(SessionMessage::CompleteSuggestion {
                direction: PathSuggestionDirection::Previous,
            })
        }
        keyboard::Key::Named(keyboard::key::Named::Tab) if no_shortcut_modifiers => {
            Some(SessionMessage::CompleteSuggestion {
                direction: PathSuggestionDirection::Next,
            })
        }
        _ => None,
    }
}

fn dispatch_session_message(window_id: window::Id, message: SessionMessage) -> Task<Message> {
    Task::perform(async {}, move |_| Message::Session(window_id, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker_request::{PickerKind, PickerRequestSpec};
    use crate::picker_session::scan::{DirectoryScanOutcome, DirectoryScanResult};
    use crate::picker_session::SessionEffect;
    use crate::PickerDaemon;
    use bennu_preview::preview::PreviewWindowProfile;
    use file_core::entry::{DirectoryEntry, EntryMetadata, FileKind};
    use std::path::PathBuf;

    fn picker_session(kind: PickerKind) -> PickerSession {
        let (reply, _receiver) = tokio::sync::oneshot::channel();
        let base = tempfile::tempdir().unwrap();
        PickerSession::new(
            &PickerRequestSpec {
                kind,
                accept_label: None,
                title: None,
                filters: Vec::new(),
                active_filter: None,
                start_folder: None,
            },
            "/req/route".to_string(),
            base.keep(),
            reply,
        )
    }

    fn seeded(session: &mut PickerSession, names: &[(&str, FileKind)]) {
        let entries = names
            .iter()
            .map(|&(name, kind)| {
                DirectoryEntry::new(
                    session.directory().join(name),
                    kind,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                )
            })
            .collect();
        session.update(SessionMessage::ScanReady(Box::new(DirectoryScanResult {
            directory: session.directory().to_path_buf(),
            outcome: Ok(DirectoryScanOutcome { entries }),
        })));
    }

    fn named(name: keyboard::key::Named) -> keyboard::Key {
        keyboard::Key::Named(name)
    }

    fn character(text: &str) -> keyboard::Key {
        keyboard::Key::Character(text.into())
    }

    fn no_modifiers() -> keyboard::Modifiers {
        keyboard::Modifiers::default()
    }

    #[test]
    fn escape_prefers_editing_then_type_ahead_then_close() {
        let mut session = picker_session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        // 普通态：未捕获关窗、捕获不响应。
        assert!(matches!(
            escape_message(&session, false),
            Some(SessionMessage::DismissPressed)
        ));
        assert!(escape_message(&session, true).is_none());

        // type-ahead 活跃：Esc 先清缓冲而不是关窗。
        session.update(SessionMessage::TypeAheadChar { ch: 'a' });
        assert!(matches!(
            escape_message(&session, false),
            Some(SessionMessage::TypeAheadReset)
        ));

        // 编辑态优先级最高：退编辑，即使同时有缓冲。
        session.update(SessionMessage::AddressEditingStarted);
        assert!(matches!(
            escape_message(&session, true),
            Some(SessionMessage::AddressEditingCancelled)
        ));
    }

    #[test]
    fn list_keys_map_to_navigation_messages() {
        let mut session = picker_session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        seeded(&mut session, &[("a.txt", FileKind::File)]);
        let plain = |key: keyboard::Key| key_message(&session, &key, no_modifiers(), false);

        assert!(matches!(
            plain(named(keyboard::key::Named::ArrowDown)),
            Some(SessionMessage::ListCursorMoved { delta: 1 })
        ));
        assert!(matches!(
            plain(named(keyboard::key::Named::ArrowUp)),
            Some(SessionMessage::ListCursorMoved { delta: -1 })
        ));
        assert!(matches!(
            plain(named(keyboard::key::Named::Home)),
            Some(SessionMessage::ListCursorJumped { to_end: false })
        ));
        assert!(matches!(
            plain(named(keyboard::key::Named::End)),
            Some(SessionMessage::ListCursorJumped { to_end: true })
        ));
        assert!(matches!(
            plain(named(keyboard::key::Named::PageUp)),
            Some(SessionMessage::ListPageMoved { pages: -1 })
        ));
        assert!(matches!(
            plain(named(keyboard::key::Named::PageDown)),
            Some(SessionMessage::ListPageMoved { pages: 1 })
        ));
        assert!(matches!(
            plain(named(keyboard::key::Named::Backspace)),
            Some(SessionMessage::NavigateUp)
        ));
        assert!(matches!(
            plain(character("k")),
            Some(SessionMessage::TypeAheadChar { ch: 'k' })
        ));
    }

    #[test]
    fn captured_events_stop_at_ctrl_a_and_never_reach_list_navigation() {
        let mut session = picker_session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        seeded(&mut session, &[("a.txt", FileKind::File)]);
        let ctrl = keyboard::Modifiers::CTRL;

        // Ctrl+A 仅 Ignored 生效（捕获时归 text_input 选中文本）。
        assert!(matches!(
            key_message(&session, &character("a"), ctrl, false),
            Some(SessionMessage::SelectAllPressed)
        ));
        assert!(key_message(&session, &character("a"), ctrl, true).is_none());
        // 捕获的方向键/字符不进列表导航（编辑态打字不被劫持）。
        assert!(key_message(
            &session,
            &named(keyboard::key::Named::ArrowDown),
            no_modifiers(),
            true
        )
        .is_none());
        assert!(key_message(&session, &character("x"), no_modifiers(), true).is_none());
        // ctrl/alt/command 组合不触发列表键与 type-ahead。
        assert!(key_message(
            &session,
            &named(keyboard::key::Named::ArrowDown),
            ctrl,
            false
        )
        .is_none());
        assert!(key_message(&session, &character("x"), ctrl, false).is_none());
    }

    #[test]
    fn enter_activates_directory_cursor_and_confirms_otherwise() {
        let mut session = picker_session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        seeded(
            &mut session,
            &[("a.txt", FileKind::File), ("dir", FileKind::Directory)],
        );
        // 光标未在目录行：Enter 走确认。
        assert!(matches!(
            key_message(
                &session,
                &named(keyboard::key::Named::Enter),
                no_modifiers(),
                false
            ),
            Some(SessionMessage::ConfirmPressed)
        ));
        // 光标移到目录行：Enter 复用双击激活（进入目录）。
        session.update(SessionMessage::ListCursorMoved { delta: 1 });
        session.update(SessionMessage::ListCursorMoved { delta: 1 });
        assert!(matches!(
            key_message(
                &session,
                &named(keyboard::key::Named::Enter),
                no_modifiers(),
                false
            ),
            Some(SessionMessage::EntryDoubleClicked { index: 1 })
        ));

        // 选目录模式：目录行的 Enter 也走确认（选中目录而非进入）。
        let mut dir_session = picker_session(PickerKind::OpenFile {
            multiple: false,
            directory: true,
        });
        seeded(&mut dir_session, &[("dir", FileKind::Directory)]);
        dir_session.update(SessionMessage::ListCursorMoved { delta: 1 });
        assert!(matches!(
            key_message(
                &dir_session,
                &named(keyboard::key::Named::Enter),
                no_modifiers(),
                false
            ),
            Some(SessionMessage::ConfirmPressed)
        ));
    }

    #[test]
    fn arrows_toggle_expansion_only_on_directory_rows() {
        let mut session = picker_session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        seeded(
            &mut session,
            &[("dir", FileKind::Directory), ("a.txt", FileKind::File)],
        );
        // 文件行（首按落第 0 行是目录……先移到文件行验证不劫持）。
        session.update(SessionMessage::ListCursorJumped { to_end: true });
        assert!(key_message(
            &session,
            &named(keyboard::key::Named::ArrowRight),
            no_modifiers(),
            false
        )
        .is_none());
        // 目录行：←/→ 都映射折叠开关。
        session.update(SessionMessage::ListCursorJumped { to_end: false });
        assert!(matches!(
            key_message(
                &session,
                &named(keyboard::key::Named::ArrowRight),
                no_modifiers(),
                false
            ),
            Some(SessionMessage::EntryExpandToggled { index: 0 })
        ));
        assert!(matches!(
            key_message(
                &session,
                &named(keyboard::key::Named::ArrowLeft),
                no_modifiers(),
                false
            ),
            Some(SessionMessage::EntryExpandToggled { index: 0 })
        ));
    }

    #[test]
    fn suggestion_panel_takes_priority_over_list_navigation() {
        let mut session = picker_session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        seeded(&mut session, &[("a.txt", FileKind::File)]);
        session.update(SessionMessage::AddressEditingStarted);
        // 走消息管线装填建议：改稿 → 防抖回信 → 读取回信。
        let stabilize = session.update(SessionMessage::AddressEditChanged("/tm".to_string()));
        let SessionEffect::StabilizeAddressInput { request } = stabilize else {
            panic!("改稿必须发起防抖");
        };
        let load = session.update(SessionMessage::AddressSuggestionInputStabilized { request });
        let SessionEffect::LoadPathSuggestions { request } = load else {
            panic!("防抖回信必须升级为读取");
        };
        session.update(SessionMessage::AddressSuggestionsLoaded {
            request,
            suggestions: vec![PathBuf::from("/tmp/pics")],
        });
        assert!(session.address_suggestion_keyboard_is_active());

        // 补全面板活跃：↓ 归补全（即使被 text_input 捕获），列表导航
        // 不抢；Tab/Shift+Tab 同理。
        assert!(matches!(
            key_message(
                &session,
                &named(keyboard::key::Named::ArrowDown),
                no_modifiers(),
                true
            ),
            Some(SessionMessage::MoveSuggestionSelection { .. })
        ));
        assert!(matches!(
            key_message(
                &session,
                &named(keyboard::key::Named::Tab),
                no_modifiers(),
                false
            ),
            Some(SessionMessage::CompleteSuggestion { .. })
        ));
    }

    fn daemon_with_open_file_session() -> (PickerDaemon, window::Id) {
        // 直接构造结构体：PickerDaemon::new() 的主题解析链含 dark-light
        // 检测（zbus 需要 reactor），测试线程里不可用；路由裁决只读
        // windows/preview 字段，主题无关。
        let mut daemon = PickerDaemon {
            windows: std::collections::HashMap::new(),
            theme: bennu_theme::fallback_theme(bennu_theme::AppearanceMode::Light),
            keyboard_modifiers: keyboard::Modifiers::default(),
            cursor_position: None,
            sidebar_resize: std::collections::HashMap::new(),
            preview: crate::preview_host::PreviewHost::new(
                tempfile::tempdir().unwrap().keep().join("state.sqlite"),
            ),
        };
        let (reply, _receiver) = tokio::sync::oneshot::channel();
        let base = tempfile::tempdir().unwrap();
        let session = PickerSession::new(
            &PickerRequestSpec {
                kind: PickerKind::OpenFile {
                    multiple: false,
                    directory: false,
                },
                accept_label: None,
                title: None,
                filters: Vec::new(),
                active_filter: None,
                start_folder: None,
            },
            "/req/space".to_string(),
            base.keep(),
            reply,
        );
        let window_id = window::Id::unique();
        daemon.windows.insert(window_id, session);
        (daemon, window_id)
    }

    #[test]
    fn space_triggers_preview_toggle_only_when_ignored_and_not_editing() {
        let (mut daemon, window_id) = daemon_with_open_file_session();
        // 干净状态：未捕获空格触发 toggle，来源为按下窗。
        assert_eq!(
            space_toggle_source(&daemon, window_id, false),
            Some(Some(window_id))
        );
        // 编辑态：空格归地址输入框（主软件 captured 语义对齐）。
        daemon
            .windows
            .get_mut(&window_id)
            .unwrap()
            .update(SessionMessage::AddressEditingStarted);
        assert_eq!(space_toggle_source(&daemon, window_id, false), None);
        // 非编辑态但被控件捕获：同样不触发。
        let (daemon, window_id) = daemon_with_open_file_session();
        assert_eq!(space_toggle_source(&daemon, window_id, true), None);
    }

    #[test]
    fn space_inside_preview_window_toggles_from_request_window() {
        let (mut daemon, window_id) = daemon_with_open_file_session();
        let (preview_window, _task) = daemon
            .preview
            .engine
            .ensure_preview_window(PreviewWindowProfile::Regular);
        daemon.preview.request_window = Some(window_id);
        // 预览窗内空格：无论 captured 与否都作用于请求窗选中项。
        assert_eq!(
            space_toggle_source(&daemon, preview_window, true),
            Some(Some(window_id))
        );
        assert_eq!(
            space_toggle_source(&daemon, preview_window, false),
            Some(Some(window_id))
        );
        // 请求窗已亡：toggle 退化为关闭（选中项元 None）。
        daemon.preview.request_window = None;
        assert_eq!(
            space_toggle_source(&daemon, preview_window, false),
            Some(None)
        );
    }

    #[test]
    fn escape_layers_between_preview_and_picker_windows() {
        // Esc 分层：预览窗的 Esc 只关预览，不穿透到选择窗的关窗/退编辑
        // 裁决；选择窗的 Esc 维持既有会话路由；预览窗的其余键不触发。
        let (mut daemon, window_id) = daemon_with_open_file_session();
        let (preview_window, _task) = daemon
            .preview
            .engine
            .ensure_preview_window(PreviewWindowProfile::Regular);
        let escape = named(keyboard::key::Named::Escape);
        let arrow = named(keyboard::key::Named::ArrowDown);
        assert!(preview_escape_requested(&daemon, preview_window, &escape));
        // 预览窗内非 Esc 键不关预览（归 iced 控件）。
        assert!(!preview_escape_requested(&daemon, preview_window, &arrow));
        // 选择窗的 Esc 走会话裁决，不进预览分支。
        assert!(!preview_escape_requested(&daemon, window_id, &escape));
        // 未知窗口（预览窗已关）：不落入预览分支。
        daemon.preview.engine.preview_window = None;
        assert!(!preview_escape_requested(&daemon, preview_window, &escape));
    }
}
