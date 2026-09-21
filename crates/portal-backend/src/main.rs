//! bennu-portal：Bennu 文件管理器的 xdg-desktop-portal FileChooser 后端。
//! 进程由 D-Bus activation 拉起后常驻（对齐 xdg-desktop-portal-gtk 的
//! 生命周期模型）；每个 FileChooser 请求弹一个独立的选择窗口，窗口全部
//! 关闭后继续驻留，换取后续唤出的热启动速度。

mod dbus_file_chooser;
mod filter;
mod location;
mod picker_request;
mod picker_session;
mod theme;
mod view;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use iced::advanced::widget::operation::{Focusable, Operation, Outcome};
use iced::futures::Stream;
use iced::{keyboard, mouse, window, Element, Rectangle, Subscription, Task, Theme};
use std::pin::Pin;
use std::task::{Context, Poll};

use bennu_theme::address_bar::AddressSuggestionRequest;
use dbus_file_chooser::{BridgeEvent, FileChooserInterface, PickerInvocation};
use picker_session::suggestions::{
    load_path_suggestions, PATH_SUGGESTION_INPUT_STABILIZATION_DELAY,
};
use picker_session::{scan_listing, PickerSession, SessionEffect, SessionMessage};
use picker_session::{PathSuggestionDirection, SessionScrollRegion};
use tokio::sync::mpsc;

const PORTAL_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.bennu";
const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
/// 展开动画帧时钟：60Hz 与主应用 ui_pacing::FRAME_INTERVAL_60HZ 同值；
/// portal 不依赖 app-ui，本地保持同一节奏。
const ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// D-Bus → UI 的通道；进程引导早期创建，subscription 首次 poll 时取走。
static BRIDGE_SLOT: OnceLock<Mutex<Option<mpsc::Receiver<BridgeEvent>>>> = OnceLock::new();

enum Message {
    Bridge(BridgeEvent),
    Session(window::Id, SessionMessage),
    WindowClosed(window::Id),
    KeyPressed {
        window: window::Id,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        /// 事件是否已被焦点控件捕获：补全面板的方向键/Tab 在 captured
        /// 下仍须生效（text_input 聚焦会捕获），其余全局动作只认 Ignored。
        captured: bool,
    },
    ModifiersChanged(keyboard::Modifiers),
    /// 窗口内左键按下：地址栏编辑态下用来探查输入框是否失焦。
    WindowLeftPressed {
        window: window::Id,
    },
    /// 焦点探查回信：地址输入框是否仍持焦点（失焦即取消编辑）。
    AddressInputFocusChecked {
        window: window::Id,
        is_focused: bool,
    },
    AnimationTick,
}

struct PickerDaemon {
    windows: HashMap<window::Id, PickerSession>,
    theme: Theme,
    keyboard_modifiers: keyboard::Modifiers,
}

impl PickerDaemon {
    fn new() -> Self {
        // 初值只覆盖进程启动到首次开窗之间的空窗期；权威主题在每次
        // 开窗时重解析（见 open_picker_window），构造链路与主程序一致。
        let theme = theme::resolve_startup_theme();
        PickerDaemon {
            windows: HashMap::new(),
            theme,
            keyboard_modifiers: keyboard::Modifiers::default(),
        }
    }
}

fn boot() -> (PickerDaemon, Task<Message>) {
    (PickerDaemon::new(), Task::none())
}

fn update(daemon: &mut PickerDaemon, message: Message) -> Task<Message> {
    match message {
        Message::Bridge(BridgeEvent::Invocation(invocation)) => {
            open_picker_window(daemon, invocation)
        }
        Message::Session(window_id, session_message) => {
            apply_session_message(daemon, window_id, session_message)
        }
        Message::WindowClosed(window_id) => {
            close_picker_window(daemon, window_id);
            Task::none()
        }
        Message::KeyPressed {
            window,
            key,
            modifiers,
            captured,
        } => handle_key(daemon, window, key, modifiers, captured),
        Message::WindowLeftPressed { window } => check_address_input_focus(daemon, window),
        Message::AddressInputFocusChecked { window, is_focused } => {
            handle_address_input_focus_checked(daemon, window, is_focused)
        }
        Message::ModifiersChanged(modifiers) => {
            daemon.keyboard_modifiers = modifiers;
            // 滚轮轴向换算（竖向区 shift 横滚）需要 shift：视图包装层与
            // 会话处理端同源读取。
            for session in daemon.windows.values_mut() {
                session.set_scroll_shift_pressed(modifiers.shift());
            }
            Task::none()
        }
        Message::AnimationTick => {
            // 帧时钟仅在动画活跃期间存在（见 subscription 的挂载条件），
            // 逐窗口推进动画并批量路由产出的 Task。
            let mut frame_tasks = Vec::new();
            for (window_id, session) in daemon.windows.iter_mut() {
                frame_tasks.extend(route_session_tasks(*window_id, session.advance_frame()));
            }
            Task::batch(frame_tasks)
        }
    }
}

fn open_picker_window(daemon: &mut PickerDaemon, invocation: PickerInvocation) -> Task<Message> {
    // WHY：进程常驻后主题不能锁死在登录时刻——用户在主程序切换深浅
    // 色后，旧主题会跟随整个会话。每次开窗重读存储偏好（本地毫秒级
    // IO），等价于旧模型「每请求一进程、每次新鲜解析」的行为。
    daemon.theme = theme::resolve_startup_theme();
    let PickerInvocation {
        request_path,
        spec,
        reply,
    } = invocation;
    let remembered = location::load_last_directory();
    let start =
        location::resolve_start_directory(spec.start_folder.as_deref(), remembered.as_deref());
    let mut session = PickerSession::new(&spec, request_path, start.clone(), reply);
    let effect = session.begin();

    let settings = window::Settings {
        size: view::window_size(),
        min_size: Some(view::window_min_size()),
        resizable: true,
        decorations: true,
        ..window::Settings::default()
    };
    let (window_id, open_task) = window::open(settings);
    daemon.windows.insert(window_id, session);

    let scan_task = match effect {
        // begin() 返回导航类效果；此处只翻扫描任务——窗口 UI 尚未
        // 构建，面包屑滚尾挂了也无效，初始深层目录的揭示由
        // ScanReady 回填时的滚尾钩子完成。
        SessionEffect::NavigateDirectory(directory) | SessionEffect::ScanDirectory(directory) => {
            scan_directory_task(window_id, directory)
        }
        _ => Task::none(),
    };
    Task::batch([
        // discard：开窗 Task 的输出（窗口 Id）无需消费，保留开窗副作用。
        open_task.discard(),
        scan_task,
        window::gain_focus(window_id),
    ])
}

fn apply_session_message(
    daemon: &mut PickerDaemon,
    window_id: window::Id,
    session_message: SessionMessage,
) -> Task<Message> {
    let Some(session) = daemon.windows.get_mut(&window_id) else {
        return Task::none();
    };
    // 滚动/滚动条消息由会话滚动子模块处理并批量产出 Task（视口回传
    // 内嵌的会话事件也在子模块内递归路由），不经会话效果路径。
    if session_message.is_scroll_message() {
        let tasks = session.handle_scroll_message(session_message);
        return Task::batch(route_session_tasks(window_id, tasks));
    }
    // 修饰键语义由这里合成：视图只报裸点击。
    let session_message = match session_message {
        SessionMessage::EntryClicked {
            index,
            ctrl: _,
            shift: _,
        } => SessionMessage::EntryClicked {
            index,
            ctrl: daemon.keyboard_modifiers.control(),
            shift: daemon.keyboard_modifiers.shift(),
        },
        other => other,
    };
    // 根目录扫描回填（初始打开或导航完成）：面包屑已按新目录布局，
    // 此时滚尾才有效——初始深层目录的揭示只能靠这个钩子（窗口构建
    // 时刻 UI 还不存在，open_picker_window 里挂 scroll_to 无效）。
    let is_root_scan_ready = matches!(
        &session_message,
        SessionMessage::ScanReady(result) if result.directory == session.directory()
    );
    // 会话消息的判别式须在 update 消费前留存；聚焦与否按 update 之后的
    // 会话状态裁决（点面包屑当前段进编辑的判定在会话内完成，pre-state
    // 还没有会话，提前算会漏聚焦）。
    let may_begin_address_editing = matches!(
        session_message,
        SessionMessage::AddressEditingStarted | SessionMessage::BreadcrumbActivated { .. }
    );
    // 键盘补全把建议写入草稿后，光标必须落到草稿末尾（主软件
    // move_cursor_to_end 同步）；仅凭 text_input 刷新不会移动光标。
    let wants_address_cursor_end =
        matches!(session_message, SessionMessage::CompleteSuggestion { .. });
    let effect = session.update(session_message);
    // 进入编辑的两条路径都要聚焦 + 全选：点击地址栏空白（恒真）、
    // 点击面包屑当前段（post-state 有会话才真）。
    let wants_address_focus = may_begin_address_editing && session.address_editing().is_some();
    let focus_task = if wants_address_focus {
        let input_id = view::address_input_id(session.request_path());
        Task::batch([
            iced::widget::operation::focus(input_id.clone()),
            iced::widget::operation::select_all(input_id),
        ])
    } else {
        Task::none()
    };
    let cursor_task = if wants_address_cursor_end {
        iced::widget::operation::move_cursor_to_end(view::address_input_id(session.request_path()))
    } else {
        Task::none()
    };
    // 根目录扫描回填的滚尾须在 session 借用结束前构造（后续
    // Confirmed/Dismissed 分支要整体借用 daemon 记住目录）。
    let breadcrumb_reveal_task = if is_root_scan_ready {
        breadcrumb_scroll_to_end_task(session)
    } else {
        Task::none()
    };
    let task = match effect {
        // 导航类扫描：除扫描与布局探针外，面包屑滚到最右以揭示当前
        // 目录段。scroll_to 可穿透 SmoothScrollArea（其 operate 转发
        // 内层），Id 已按请求路径命名空间不会串窗。
        SessionEffect::NavigateDirectory(directory) => Task::batch([
            scan_directory_task(window_id, directory),
            breadcrumb_scroll_to_end_task(session),
            scrollbar_layout_probe_task(window_id, session),
        ]),
        // 展开行扫描：只扫描 + 核实滚动条溢出，不滚面包屑（展开与
        // 当前目录段无关，强滚会打断用户正在看的面包屑位置）。
        SessionEffect::ScanDirectory(directory) => Task::batch([
            scan_directory_task(window_id, directory),
            scrollbar_layout_probe_task(window_id, session),
        ]),
        SessionEffect::StabilizeAddressInput { request } => {
            stabilize_address_input_task(window_id, request)
        }
        SessionEffect::LoadPathSuggestions { request } => {
            load_path_suggestions_task(window_id, request)
        }
        SessionEffect::VerifyScrollbarLayout => scrollbar_layout_probe_task(window_id, session),
        SessionEffect::Confirmed(paths) => {
            tracing::info!("选择完成：{} 个条目", paths.len());
            remember_directory(daemon, window_id);
            window::close(window_id)
        }
        SessionEffect::Dismissed => {
            remember_directory(daemon, window_id);
            window::close(window_id)
        }
        SessionEffect::None => Task::none(),
    };
    Task::batch([task, focus_task, cursor_task, breadcrumb_reveal_task])
}

/// 会话产出的 Task 按所属窗口路由成全局消息。
fn route_session_tasks(
    window_id: window::Id,
    tasks: impl IntoIterator<Item = Task<SessionMessage>>,
) -> impl Iterator<Item = Task<Message>> {
    tasks
        .into_iter()
        .map(move |task| task.map(move |message| Message::Session(window_id, message)))
}

/// 布局探针 Task：核实两个滚动区域的溢出并按窗口路由回信。
fn scrollbar_layout_probe_task(window_id: window::Id, session: &PickerSession) -> Task<Message> {
    Task::batch(route_session_tasks(
        window_id,
        session.scrollbar_layout_probe_tasks(),
    ))
}

fn remember_directory(daemon: &PickerDaemon, window_id: window::Id) {
    if let Some(session) = daemon.windows.get(&window_id) {
        location::store_last_directory(session.directory());
    }
}

fn scan_directory_task(window_id: window::Id, directory: std::path::PathBuf) -> Task<Message> {
    // 结果携带扫描目标路径：会话据此路由回根列表或展开节点。
    Task::perform(scan_listing(directory.clone()), move |outcome| {
        Message::Session(
            window_id,
            SessionMessage::ScanReady(Box::new(picker_session::DirectoryScanResult {
                directory: directory.clone(),
                outcome,
            })),
        )
    })
}

/// 防抖回信：停笔 120ms 后原样带回凭据；会话按凭据决定是否升级为
/// 真正的读取请求（陈旧回信在会话侧拒收）。
fn stabilize_address_input_task(
    window_id: window::Id,
    request: AddressSuggestionRequest,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(PATH_SUGGESTION_INPUT_STABILIZATION_DELAY).await;
            request
        },
        move |request| {
            Message::Session(
                window_id,
                SessionMessage::AddressSuggestionInputStabilized { request },
            )
        },
    )
}

fn load_path_suggestions_task(
    window_id: window::Id,
    request: AddressSuggestionRequest,
) -> Task<Message> {
    Task::perform(
        load_path_suggestions(request.draft.clone(), request.current_dir.clone()),
        move |suggestions| {
            Message::Session(
                window_id,
                SessionMessage::AddressSuggestionsLoaded {
                    request,
                    suggestions,
                },
            )
        },
    )
}

fn breadcrumb_scroll_to_end_task(session: &PickerSession) -> Task<Message> {
    iced::widget::operation::scroll_to(
        picker_session::scrollbar::scroll_id(
            session.request_path(),
            SessionScrollRegion::Breadcrumb,
        ),
        iced::widget::scrollable::AbsoluteOffset {
            x: f32::MAX,
            y: 0.0,
        },
    )
}

fn close_picker_window(daemon: &mut PickerDaemon, window_id: window::Id) {
    if let Some(session) = daemon.windows.remove(&window_id) {
        session.window_closed();
    }
}

fn handle_key(
    daemon: &PickerDaemon,
    window: window::Id,
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    captured: bool,
) -> Task<Message> {
    let Some(session) = daemon.windows.get(&window) else {
        return Task::none();
    };

    // 补全面板的方向键/Tab 优先于一切：text_input 聚焦会捕获这些键
    //（captured=true），必须在此消费而不是等 Ignored——与主软件在
    // 全局 handler 先查 address_suggestion_keyboard_is_active 同构。
    if session.address_suggestion_keyboard_is_active() {
        if let Some(message) = address_suggestion_key_message(&key, modifiers) {
            return dispatch_session_message(window, message);
        }
    }

    // 编辑态 Esc 即使被 text_input 捕获也要能退出编辑；非编辑态仅
    // Ignored 的 Esc 才关闭窗口。
    if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
        if session.address_editing().is_some() {
            return dispatch_session_message(window, SessionMessage::AddressEditingCancelled);
        }
        if !captured {
            return dispatch_session_message(window, SessionMessage::DismissPressed);
        }
        return Task::none();
    }

    // 其余全局动作（Enter 确认、Ctrl+A 全选）只认 Ignored：text_input
    // 已用 Enter 提交草稿、用 Ctrl+A 选中文本，重复触发会互相打架。
    if captured {
        return Task::none();
    }
    if let keyboard::Key::Character(character) = &key {
        if character.eq_ignore_ascii_case("a") && modifiers.control() {
            return dispatch_session_message(window, SessionMessage::SelectAllPressed);
        }
    }
    match key {
        keyboard::Key::Named(keyboard::key::Named::Enter) => {
            dispatch_session_message(window, SessionMessage::ConfirmPressed)
        }
        _ => Task::none(),
    }
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

fn dispatch_session_message(window: window::Id, message: SessionMessage) -> Task<Message> {
    Task::perform(async {}, move |_| Message::Session(window, message))
}

/// 左键按下后探查地址输入框焦点：仅窗口处于编辑态时发起（主软件
/// handle_window_pointer_pressed 里 AddressInputFocusChecked 的机制照搬
/// ——text_input 在点击落到自身 bounds 之外时会自行失焦，控件消息先于
/// 本 subscription 消息入队，探查时焦点已就位，结果可信）。
fn check_address_input_focus(daemon: &PickerDaemon, window_id: window::Id) -> Task<Message> {
    let Some(session) = daemon.windows.get(&window_id) else {
        return Task::none();
    };
    if session.address_editing().is_none() {
        return Task::none();
    }
    iced::advanced::widget::operate(AddressInputFocusCheck::new(
        window_id,
        view::address_input_id(session.request_path()),
    ))
}

/// 探查回信：失焦且编辑会话仍在（未被同批的提交/取消消费，如点击
/// 建议行先提交导航）才取消编辑——与主软件 AddressInputFocusChecked
/// 处理里的 checked_session_is_current 复核同构。
fn handle_address_input_focus_checked(
    daemon: &mut PickerDaemon,
    window_id: window::Id,
    is_focused: bool,
) -> Task<Message> {
    if is_focused {
        return Task::none();
    }
    let session_still_editing = daemon
        .windows
        .get(&window_id)
        .is_some_and(|session| session.address_editing().is_some());
    if session_still_editing {
        apply_session_message(daemon, window_id, SessionMessage::AddressEditingCancelled)
    } else {
        Task::none()
    }
}

/// 焦点探查 operation：遍历控件树读取目标输入框的焦点态（主软件
/// windows.rs 的 TextInputFocusCheck 同构；Id 按请求路径命名空间保证
/// 只命中本窗口的输入框）。
struct AddressInputFocusCheck {
    window: window::Id,
    target: iced::widget::Id,
    is_focused: bool,
}

impl AddressInputFocusCheck {
    fn new(window: window::Id, target: iced::widget::Id) -> Self {
        Self {
            window,
            target,
            is_focused: false,
        }
    }
}

impl Operation<Message> for AddressInputFocusCheck {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if id == Some(&self.target) {
            self.is_focused = state.is_focused();
        }
    }

    fn finish(&self) -> Outcome<Message> {
        Outcome::Some(Message::AddressInputFocusChecked {
            window: self.window,
            is_focused: self.is_focused,
        })
    }
}

fn view_picker(daemon: &PickerDaemon, window_id: window::Id) -> Element<'_, Message> {
    let Some(session) = daemon.windows.get(&window_id) else {
        return iced::widget::text("").into();
    };
    view::picker_window_view(session, &daemon.theme, |message| message)
        .map(move |message| Message::Session(window_id, message))
}

fn daemon_title(daemon: &PickerDaemon, window_id: window::Id) -> String {
    daemon
        .windows
        .get(&window_id)
        .map(session_window_title)
        .unwrap_or_else(|| "Bennu 文件选择".to_string())
}

/// 窗口标题：调用方显式传入 title 则跟随，空缺时回落模式默认标题。
/// 协议规定 title 仅作显示，不参与任何选择语义。
fn session_window_title(session: &PickerSession) -> String {
    session
        .title()
        .map(str::to_string)
        .unwrap_or_else(|| session.kind().default_title().to_string())
}

fn subscription(daemon: &PickerDaemon) -> Subscription<Message> {
    let mut subscriptions = vec![
        Subscription::run(bridge_events),
        window::close_events().map(Message::WindowClosed),
        iced::event::listen_with(|event, status, window_id| match event {
            // 键盘事件无差别转发（Captured 也收）：补全面板的 ↑/↓/Tab
            // 与编辑态 Esc 都要抢在焦点控件的捕获语义之前生效；是否
            // 尊重捕获由 handle_key 按动作分类裁决。
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                Some(Message::KeyPressed {
                    window: window_id,
                    key,
                    modifiers,
                    captured: matches!(status, iced::event::Status::Captured),
                })
            }
            // 左键按下（无论是否被控件捕获）都可能把焦点从地址输入框
            // 移走：text_input 对 bounds 外的点击自行失焦。探查在
            // update 侧按「窗口处于编辑态」过滤，非编辑窗口零开销。
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                Some(Message::WindowLeftPressed { window: window_id })
            }
            // 鼠标侧键 = 后退/前进（与主程序一致，被捕获时不触发）。
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Back))
                if matches!(status, iced::event::Status::Ignored) =>
            {
                Some(Message::Session(window_id, SessionMessage::NavigateBack))
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Forward))
                if matches!(status, iced::event::Status::Ignored) =>
            {
                Some(Message::Session(window_id, SessionMessage::NavigateForward))
            }
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::ModifiersChanged(modifiers))
            }
            _ => None,
        }),
    ];
    // 仅在有窗口播放动画（展开/收起、地址栏渐变、惯性滚动、滚动条
    // 淡入淡出）时订阅帧时钟，动画结束自然摘除——与主应用按动画
    // 活跃度挂载 time::every 同模式。
    if daemon.windows.values().any(PickerSession::is_animating) {
        subscriptions
            .push(iced::time::every(ANIMATION_FRAME_INTERVAL).map(|_| Message::AnimationTick));
    }
    Subscription::batch(subscriptions)
}

/// D-Bus 桥流：首次 poll 取走全局通道，此后转发事件直到对端关闭。
struct BridgeEvents {
    source: BridgeSource,
}

enum BridgeSource {
    NotTaken,
    Live(mpsc::Receiver<BridgeEvent>),
    Ended,
}

fn bridge_events() -> BridgeEvents {
    BridgeEvents {
        source: BridgeSource::NotTaken,
    }
}

impl Stream for BridgeEvents {
    type Item = Message;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = &mut *self;
        loop {
            match &mut stream.source {
                BridgeSource::NotTaken => match take_bridge_receiver() {
                    Some(receiver) => stream.source = BridgeSource::Live(receiver),
                    // 引导顺序保证订阅开始前 receiver 已放入；兜底直接结束。
                    None => stream.source = BridgeSource::Ended,
                },
                BridgeSource::Live(receiver) => {
                    return receiver
                        .poll_recv(context)
                        .map(|event| event.map(Message::Bridge))
                }
                BridgeSource::Ended => return Poll::Ready(None),
            }
        }
    }
}

fn take_bridge_receiver() -> Option<mpsc::Receiver<BridgeEvent>> {
    BRIDGE_SLOT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}

async fn build_portal_connection(
    bridge_sender: mpsc::Sender<BridgeEvent>,
) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .name(PORTAL_BUS_NAME)?
        .serve_at(PORTAL_OBJECT_PATH, FileChooserInterface::new(bridge_sender))?
        .build()
        .await
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let (bridge_sender, bridge_receiver) = mpsc::channel(16);
    let _ = BRIDGE_SLOT.set(Mutex::new(Some(bridge_receiver)));

    // zbus 与其 tokio runtime 驻留独立线程：iced 的 daemon 必须在
    // main 线程同步驱动（其内部自建 executor），二者互不嵌套。
    std::thread::Builder::new()
        .name("portal-dbus".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!("tokio runtime 创建失败: {error}");
                    std::process::exit(1);
                }
            };
            runtime.block_on(async move {
                let connection = match build_portal_connection(bridge_sender).await {
                    Ok(connection) => connection,
                    Err(error) => {
                        tracing::error!("D-Bus 服务启动失败: {error}");
                        std::process::exit(1);
                    }
                };
                // 连接对象与本线程同寿：pending 永不完成，保持 bus 名与
                // 对象注册直到进程退出。
                let _connection_guard = connection;
                iced::futures::future::pending::<()>().await;
            });
        })
        .expect("D-Bus 线程创建失败");

    let result = iced::daemon(boot, update, view_picker)
        .title(daemon_title)
        .subscription(subscription)
        .theme(|daemon: &PickerDaemon, _| daemon.theme.clone())
        .run();
    if let Err(error) = result {
        tracing::error!("选择窗口运行失败: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker_request::{PickerKind, PickerRequestSpec};

    fn session_with_title(title: Option<&str>) -> PickerSession {
        let (reply, _receiver) = tokio::sync::oneshot::channel();
        let base = tempfile::tempdir().unwrap();
        PickerSession::new(
            &PickerRequestSpec {
                kind: PickerKind::SaveFile { default_name: None },
                accept_label: None,
                title: title.map(str::to_string),
                filters: Vec::new(),
                active_filter: None,
                start_folder: None,
            },
            "/req/test".to_string(),
            base.keep(),
            reply,
        )
    }

    #[test]
    fn window_title_prefers_caller_title() {
        let session = session_with_title(Some("导出报告"));
        assert_eq!(session_window_title(&session), "导出报告");
    }

    #[test]
    fn window_title_falls_back_to_kind_default() {
        let session = session_with_title(None);
        assert_eq!(session_window_title(&session), "另存为");
    }
}
