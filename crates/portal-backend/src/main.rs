//! bennu-portal：Bennu 文件管理器的 xdg-desktop-portal FileChooser 后端。
//! 进程由 D-Bus activation 拉起后常驻（对齐 xdg-desktop-portal-gtk 的
//! 生命周期模型）；每个 FileChooser 请求弹一个独立的选择窗口，窗口全部
//! 关闭后继续驻留，换取后续唤出的热启动速度。

mod address_focus_probe;
mod dbus_file_chooser;
mod filter;
mod keyboard_route;
mod location;
mod message;
mod picker_request;
mod picker_session;
mod preview_host;
mod preview_scroll;
mod scrollbar_state;
mod sidebar_bridge;
mod subscriptions;
mod theme;
mod thumbnail_dispatch;
mod view;

pub(crate) use message::Message;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::{keyboard, window, Element, Task, Theme};

use bennu_theme::address_bar::AddressSuggestionRequest;
use dbus_file_chooser::{BridgeEvent, FileChooserInterface, PickerInvocation};
use picker_session::suggestions::{
    load_path_suggestions, PATH_SUGGESTION_INPUT_STABILIZATION_DELAY,
};
use picker_session::SessionScrollRegion;
use picker_session::{PickerSession, SessionEffect, SessionMessage};
use thumbnail_dispatch::spawn_thumbnail_tasks;
use tokio::sync::mpsc;

const PORTAL_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.bennu";
const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
/// iced fallback 候选链：GPU（wgpu）→ 软件渲染。GPU 首选由
/// [`apply_renderer_environment`] 钉在驱动显示器的 GPU（通常为核显）。
const PORTAL_ICED_BACKEND_CANDIDATES: &str = "wgpu,tiny-skia";

/// D-Bus → UI 的通道；进程引导早期创建，subscription 首次 poll 时取走。
static BRIDGE_SLOT: OnceLock<Mutex<Option<mpsc::Receiver<BridgeEvent>>>> = OnceLock::new();

struct PickerDaemon {
    windows: HashMap<window::Id, PickerSession>,
    theme: Theme,
    keyboard_modifiers: keyboard::Modifiers,
    /// 最新指针位置（拖宽增量基准；与主程序 FileBrowser.cursor_position
    /// 同一角色）。全部窗口的 CursorMoved 都更新，按下时读取。
    cursor_position: Option<iced::Point>,
    /// 进行中的侧栏拖宽：窗口 → 起点（指针 X + 起始宽度）。
    sidebar_resize: HashMap<window::Id, sidebar_bridge::SidebarResizeDrag>,
    /// 全进程唯一的预览会话（多选择窗并发时新请求顶掉旧会话，
    /// design 决策 #3）；含预览窗焦点/请求窗簿记。
    preview: preview_host::PreviewHost,
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
            cursor_position: None,
            sidebar_resize: HashMap::new(),
            preview: preview_host::PreviewHost::new(theme::state_database_path()),
        }
    }
}

fn boot() -> (PickerDaemon, Task<Message>) {
    (PickerDaemon::new(), Task::none())
}

fn update(daemon: &mut PickerDaemon, message: Message) -> Task<Message> {
    match message {
        Message::Bridge(bridge_event) => {
            // 桥事件携带一次性 reply sender，本体不可 Clone；Message 的
            // Clone bound 是 iced 面板/控件的静态要求，桥事件从不进控件
            // 树，Arc 包裹即可满足。
            // Arc 拆包：桥事件在消息队列里独占所有权，try_unwrap 必成。
            let bridge_event = match std::sync::Arc::try_unwrap(bridge_event) {
                Ok(event) => event,
                Err(_) => unreachable!("桥事件独占所有权"),
            };
            let BridgeEvent::Invocation(invocation) = bridge_event;
            open_picker_window(daemon, invocation)
        }
        Message::Session(window_id, session_message) => {
            apply_session_message(daemon, window_id, session_message)
        }
        Message::WindowClosed(window_id) => {
            close_picker_window(daemon, window_id);
            // 预览窗被外部关闭或请求窗先亡：预览会话一并收尾。
            daemon.preview.handle_window_closed(window_id)
        }
        Message::KeyPressed {
            window,
            key,
            modifiers,
            captured,
        } => keyboard_route::handle_key(daemon, window, key, modifiers, captured),
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
        Message::PreviewSpacePressed { source } => {
            preview_host::handle_space_pressed(daemon, source)
        }
        Message::Preview(preview_message) => {
            preview_host::handle_preview_message(daemon, preview_message)
        }
        Message::PreviewWindowControl { window, kind } => {
            daemon.preview.handle_window_control(window, kind)
        }
        Message::PreviewWindowTitlePressed {
            window,
            double_click,
        } => daemon
            .preview
            .handle_title_bar_pressed(window, double_click),
        Message::PreviewWindowResizeEdgePressed { window, direction } => {
            daemon.preview.handle_resize_edge_pressed(window, direction)
        }
        Message::PreviewWindowMaximizeObserved { window, maximized } => {
            daemon.preview.accept_maximize_observed(window, maximized);
            Task::none()
        }
        Message::PreviewPointerMoved { window, position } => {
            sidebar_bridge::handle_pointer_moved(daemon, window, position)
        }
        Message::PreviewPointerLeft { window } => {
            // 拖宽中指针离窗：拖宽终止（增量无从继续，恢复代价只是重按）。
            daemon.sidebar_resize.remove(&window);
            daemon.preview.handle_pointer_left(window)
        }
        Message::PreviewPointerReleased { window } => {
            if daemon.preview.is_preview_window(window) {
                daemon.preview.finish_window_drags()
            } else {
                daemon.sidebar_resize.remove(&window);
                Task::none()
            }
        }
        Message::PreviewSqliteDragFinished => daemon.preview.finish_window_drags(),
        Message::PreviewCloseFocused => daemon.preview.close_focused_preview(),
        Message::PreviewWindowResized {
            window,
            width,
            height,
        } => preview_host::handle_window_resized(daemon, window, width, height),
        Message::PreviewWheelScrolled { region, delta } => daemon
            .preview
            .scroll
            .handle_wheel_scrolled(region, daemon.keyboard_modifiers.shift(), delta),
        Message::PreviewScrollbarLayoutVerified { region, viewport } => daemon
            .preview
            .scroll
            .handle_layout_verified(region, viewport),
        Message::PreviewScrollbarViewportChanged {
            region,
            viewport,
            event,
        } => {
            // 视口快照先写缓存（thumb 位置跟随），内层事件回流引擎/
            // 宿主回退路由（选择窗 ScrollbarViewportChanged 同构）。
            daemon.preview.scroll.remember_viewport(region, viewport);
            preview_host::handle_preview_message(daemon, *event)
        }
        Message::PreviewScrollbarAutoHideElapsed { generation } => {
            daemon.preview.scroll.handle_auto_hide_elapsed(generation);
            Task::none()
        }
        Message::WindowFocused(window) => {
            // 预览窗 Esc 分层/失焦关闭（步骤 3）读这份焦点簿记。
            daemon.preview.focused_window = Some(window);
            Task::none()
        }
        Message::WindowUnfocused(window) => {
            // 只清匹配来源的焦点：迟到的离开事件不得清掉新窗口焦点。
            if daemon.preview.focused_window == Some(window) {
                daemon.preview.focused_window = None;
            }
            // 预览窗失焦且未钉住 → 自动关闭（主软件 handle_window_
            // unfocused 同语义；关闭内部带焦点回请求窗）。
            daemon.preview.handle_window_unfocused(window)
        }
        Message::AnimationTick => {
            // 帧时钟仅在动画活跃期间存在（见 subscription 的挂载条件），
            // 逐窗口推进动画并批量路由产出的 Task；预览窗（chrome 淡入
            // 淡出/预览树/惯性滚动/滚动条）一并推进。
            let mut frame_tasks = Vec::new();
            for (window_id, session) in daemon.windows.iter_mut() {
                frame_tasks.extend(route_session_tasks(*window_id, session.advance_frame()));
            }
            frame_tasks.push(daemon.preview.advance_frame());
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
    let effects = session.begin();

    let settings = window::Settings {
        size: view::window_size(),
        min_size: Some(view::window_min_size()),
        resizable: true,
        decorations: true,
        ..window::Settings::default()
    };
    // 独立 app-id 供 niri window-rule 悬浮匹配（docs/niri.md），与 app-ui
    // 的 bennu-* 窗口家族保持同一命名约定。
    let mut settings = settings;
    settings.platform_specific.application_id = "bennu-filechooser".to_owned();
    let (window_id, open_task) = window::open(settings);
    daemon.windows.insert(window_id, session);

    // begin() 声明开窗所需效果（侧栏数据加载 + 初始导航），此处统一
    // 翻译，与 apply_session_message 同一翻译责任；开窗路径不经会话
    // update。扫描只翻任务本身——窗口 UI 尚未构建，面包屑滚尾挂了也
    // 无效，初始深层目录的揭示由 ScanReady 回填时的滚尾钩子完成。
    let mut startup_tasks = vec![open_task.discard()];
    for effect in effects {
        match effect {
            SessionEffect::NavigateDirectory(directory)
            | SessionEffect::ScanDirectory(directory) => {
                startup_tasks.push(scan_directory_task(window_id, directory));
            }
            SessionEffect::LoadSidebarData => {
                startup_tasks.push(sidebar_bridge::sidebar_data_task(window_id));
            }
            _ => {}
        }
    }
    startup_tasks.push(window::gain_focus(window_id));
    Task::batch(startup_tasks)
}

fn apply_session_message(
    daemon: &mut PickerDaemon,
    window_id: window::Id,
    session_message: SessionMessage,
) -> Task<Message> {
    // 侧栏拖宽按下：在取会话借用前拦截（需读 daemon 的指针簿记定
    // 起点，iced::Point 不进会话层），不进会话 update。
    if matches!(session_message, SessionMessage::SidebarResizeStarted)
        && daemon.windows.contains_key(&window_id)
    {
        sidebar_bridge::start_sidebar_resize(daemon, window_id);
        return Task::none();
    }
    let Some(session) = daemon.windows.get_mut(&window_id) else {
        return Task::none();
    };
    // 预览首帧/档位升级回流：缩略图回信先抄送预览宿主（引擎按等待
    // 标记与当前展示判定收货，迟到小图不回退），与会话行内显示互不
    // 影响（主软件 accept_preview_thumbnail_ready 的 Purpose::Preview
    // 分支对齐；预览属全进程，不按未窗过濾）。
    let preview_thumbnail_task = match &session_message {
        SessionMessage::ThumbnailReady { request, outcome } => daemon
            .preview
            .accept_session_thumbnail_outcome(request, outcome),
        _ => Task::none(),
    };
    // 滚动/滚动条消息由会话滚动子模块处理并批量产出 Task（视口回传
    // 内嵌的会话事件也在子模块内递归路由），不经会话效果路径。
    if session_message.is_scroll_message() {
        let tasks = session.handle_scroll_message(session_message);
        // 滚动路径不经 SessionEffect：缩略图请求同样在这里 drain 发起
        //（滚动正是视口更新 → 新行进入可见区间的触发源）。
        let thumbnail_tasks = spawn_thumbnail_tasks(window_id, session);
        return Task::batch(route_session_tasks(window_id, tasks).chain(thumbnail_tasks));
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
    // 会话消息统一收口处的缩略图泵：ThumbnailReady 回信处理完也走到这
    // 里再 drain，腾出的并发额度自然补位，无需定时器。
    let thumbnail_tasks = spawn_thumbnail_tasks(window_id, session);
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
        // 侧栏数据一次性加载（打开时读一次）。
        SessionEffect::LoadSidebarData => sidebar_bridge::sidebar_data_task(window_id),
        // 未挂载设备：挂载回信携带目标路径/错误。
        SessionEffect::MountDevice(id) => sidebar_bridge::mount_device_task(window_id, id),
        // 网络连接：先查主程序已存凭证（密钥环），查不到按无 app 凭证
        // 挂载——选择器没有凭据表单，失败只在侧栏提示（app-ui 手动连接
        // 的 lookup 流程裁剪版）。
        SessionEffect::MountConnection(id) => {
            sidebar_bridge::mount_connection_task(window_id, session, id)
        }
        // 键盘导航滚动跟随：scroll_to 可穿透 SmoothScrollArea 直达内层
        // scrollable（Id 按请求路径命名空间不会串窗）；只对 List 发布局
        // 探针——面包屑布局未变，避免无谓的面包屑滚动条淡入，探针回信
        // 同时核实溢出联动滚动条显隐并纠偏视口缓存。
        SessionEffect::ScrollListTo { offset_y } => Task::batch([
            iced::widget::operation::scroll_to(
                picker_session::scrollbar::scroll_id(
                    session.request_path(),
                    SessionScrollRegion::List,
                ),
                iced::widget::scrollable::AbsoluteOffset {
                    x: 0.0,
                    y: offset_y,
                },
            ),
            Task::batch(route_session_tasks(
                window_id,
                [session.scrollbar_layout_probe_task(SessionScrollRegion::List)],
            )),
        ]),
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
    Task::batch([
        task,
        focus_task,
        cursor_task,
        breadcrumb_reveal_task,
        Task::batch(thumbnail_tasks),
        preview_thumbnail_task,
    ])
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
        // 回收站是虚拟视图：不作为下次开窗的起始目录（SaveFile 落在
        // trash 下确认破禁，且真实路径记忆才有导航价值）。
        if !session.is_trash_view() {
            location::store_last_directory(session.directory());
        }
    }
}

fn scan_directory_task(window_id: window::Id, directory: std::path::PathBuf) -> Task<Message> {
    // 回收站虚拟视图：扫描走 file-core trash（行数据复用 PickerRow，
    // 路径为 files/ 下真实载荷）。其余目录常规扫描。结果统一携带扫描
    // 目标路径：会话据此路由回根列表或展开节点。
    let directory_for_scan = directory.clone();
    let is_trash_scan = directory.as_os_str() == picker_session::TRASH_DIRECTORY;
    let scan_future = async move {
        if is_trash_scan {
            picker_session::scan_trash_listing().await
        } else {
            picker_session::scan_listing(directory_for_scan).await
        }
    };
    Task::perform(scan_future, move |outcome| {
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
    iced::advanced::widget::operate(address_focus_probe::AddressInputFocusCheck::new(
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

fn view_picker(daemon: &PickerDaemon, window_id: window::Id) -> Element<'_, Message> {
    if daemon.preview.is_preview_window(window_id) {
        // 预览窗视图适配层（面板 + 滚动接线 + chrome + 拖动面 + resize）。
        return view::preview_window_view(daemon, window_id);
    }
    let Some(session) = daemon.windows.get(&window_id) else {
        // 拾取窗之外的未知窗口安全空白。
        return iced::widget::text("").into();
    };
    view::picker_window_view(session, &daemon.theme, |message| message)
        .map(move |message| Message::Session(window_id, message))
}

fn daemon_title(daemon: &PickerDaemon, window_id: window::Id) -> String {
    if daemon.preview.is_preview_window(window_id) {
        return "预览".to_owned();
    }
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

async fn build_portal_connection(
    bridge_sender: mpsc::Sender<BridgeEvent>,
) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .name(PORTAL_BUS_NAME)?
        .serve_at(PORTAL_OBJECT_PATH, FileChooserInterface::new(bridge_sender))?
        .build()
        .await
}

/// 在任何 iced/wgpu 初始化前定下渲染链：核显 → 独显 → 软件渲染。
///
/// - `ICED_BACKEND=wgpu,tiny-skia`：iced fallback 依次尝试 GPU、软件渲染。
/// - GPU 首选钉住驱动显示器的 GPU（与主软件 DisplayGpu 偏好同配方：
///   power pref + MESA 设备选择 + loader ICD 过滤）。笔记本上即核显，
///   且必然已上电；独显 NVIDIA 冷初始化实测 ~2.2s，必须排除在首选外。
/// - 检测失败时仅设 `WGPU_POWER_PREF=low`，让 wgpu 自行排序（有核显选核显）。
/// - 已存在的环境变量不覆盖：保留运维/实验入口（如强制软渲染排障）。
fn apply_renderer_environment() {
    if std::env::var_os("ICED_BACKEND").is_none() {
        std::env::set_var("ICED_BACKEND", PORTAL_ICED_BACKEND_CANDIDATES);
    }
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        match display_renderer::detect_display_renderer_gpu() {
            Some(gpu) => {
                std::env::set_var("WGPU_POWER_PREF", gpu.class().wgpu_power_preference());
                std::env::set_var("MESA_VK_DEVICE_SELECT", gpu.mesa_vulkan_device_select());
                if let Some(loader_select) = gpu.vulkan_loader_driver_select() {
                    std::env::set_var("VK_LOADER_DRIVERS_SELECT", loader_select);
                }
            }
            None => std::env::set_var("WGPU_POWER_PREF", "low"),
        }
    }
}

fn main() {
    apply_renderer_environment();
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
        .subscription(subscriptions::subscription)
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
    use crate::subscriptions::active_media_streams;

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

    #[test]
    fn media_streams_gated_by_engine_activity() {
        // 空闲引擎：四路媒体订阅全部不挂载。
        let daemon = test_daemon();
        let media = active_media_streams(&daemon.preview.engine);
        assert!(!media.audio_tick);
        assert!(!media.video_tick);
        assert!(media.animated_image.is_none());
        assert!(media.video.is_none());
    }

    #[test]
    fn media_streams_active_while_video_preview_is_playing() {
        // 视频播放中：进度 tick 与帧流两路同时挂载（携带路径+代数+
        // 起播位置，帧流订阅身份的三大要素）。
        let mut daemon = test_daemon();
        let path = std::path::PathBuf::from("/tmp/movie.mp4");
        daemon.preview.engine.preview = Some(bennu_preview::preview::PreviewState::Ready(
            bennu_preview::preview::PreviewContent::Video {
                path: path.clone(),
                frame: None,
                width: 640,
                height: 360,
                duration: None,
            },
        ));
        daemon.preview.engine.video_preview = Some(
            bennu_preview::preview::VideoPreviewPlayback::playing(path.clone(), None),
        );

        let media = active_media_streams(&daemon.preview.engine);
        assert!(!media.audio_tick);
        assert!(media.video_tick);
        assert!(media
            .video
            .is_some_and(|(stream_path, _, _)| stream_path == path));
        assert!(media.animated_image.is_none());
    }

    /// 测试专用 daemon：绕过 new() 的主题解析（需 tokio reactor），
    /// 预览库路径指向空（配置回落默认，隔离测试机配置）。
    fn test_daemon() -> PickerDaemon {
        PickerDaemon {
            windows: HashMap::new(),
            theme: Theme::Light,
            keyboard_modifiers: keyboard::Modifiers::default(),
            cursor_position: None,
            sidebar_resize: HashMap::new(),
            preview: preview_host::PreviewHost::new(std::path::PathBuf::new()),
        }
    }
}
