//! bennu-portal：Bennu 文件管理器的 xdg-desktop-portal FileChooser 后端。
//! 进程由 D-Bus activation 按需拉起；每个 FileChooser 请求弹一个独立的
//! 选择窗口，全部窗口关闭且空闲超时后进程退出。

mod dbus_file_chooser;
mod filter;
mod location;
mod picker_request;
mod picker_session;
mod theme;
mod view;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use iced::futures::Stream;
use iced::{exit, keyboard, mouse, window, Element, Subscription, Task, Theme};
use std::pin::Pin;
use std::task::{Context, Poll};

use dbus_file_chooser::{BridgeEvent, FileChooserInterface, PickerInvocation};
use picker_session::{scan_listing, PickerSession, SessionEffect, SessionMessage};
use tokio::sync::mpsc;

const PORTAL_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.bennu";
const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
const IDLE_EXIT_AFTER: Duration = Duration::from_secs(30);
const IDLE_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// 地址栏编辑输入框的稳定 Id（进入编辑时聚焦并全选草稿）。
const ADDRESS_INPUT_ID: &str = "portal-address-input";

/// D-Bus → UI 的通道；进程引导早期创建，subscription 首次 poll 时取走。
static BRIDGE_SLOT: OnceLock<Mutex<Option<mpsc::Receiver<BridgeEvent>>>> = OnceLock::new();

enum Message {
    Bridge(BridgeEvent),
    Session(window::Id, SessionMessage),
    WindowClosed(window::Id),
    KeyPressed {
        window: window::Id,
        key: keyboard::Key,
    },
    ModifiersChanged(keyboard::Modifiers),
    IdleTick,
}

struct PickerDaemon {
    windows: HashMap<window::Id, PickerSession>,
    window_by_request: HashMap<String, window::Id>,
    theme: Theme,
    keyboard_modifiers: keyboard::Modifiers,
    last_activity: Instant,
}

impl PickerDaemon {
    fn new() -> Self {
        // 与主程序同一主题构造链路（存储偏好 + matugen + 系统回退）。
        let theme = theme::resolve_startup_theme();
        PickerDaemon {
            windows: HashMap::new(),
            window_by_request: HashMap::new(),
            theme,
            keyboard_modifiers: keyboard::Modifiers::default(),
            last_activity: Instant::now(),
        }
    }
}

fn boot() -> (PickerDaemon, Task<Message>) {
    (PickerDaemon::new(), Task::none())
}

fn update(daemon: &mut PickerDaemon, message: Message) -> Task<Message> {
    // 仅真实交互刷新活跃时间；IdleTick 自身不续命，否则空闲退出永不触发。
    if !matches!(message, Message::IdleTick) {
        daemon.last_activity = Instant::now();
    }
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
        Message::KeyPressed { window, key } => handle_key(daemon, window, key),
        Message::ModifiersChanged(modifiers) => {
            daemon.keyboard_modifiers = modifiers;
            Task::none()
        }
        Message::IdleTick => {
            if daemon.windows.is_empty() && daemon.last_activity.elapsed() > IDLE_EXIT_AFTER {
                tracing::info!("空闲超时，bennu-portal 退出");
                exit()
            } else {
                Task::none()
            }
        }
    }
}

fn open_picker_window(daemon: &mut PickerDaemon, invocation: PickerInvocation) -> Task<Message> {
    let PickerInvocation {
        request_path,
        spec,
        reply,
    } = invocation;
    let remembered = location::load_last_directory();
    let start =
        location::resolve_start_directory(spec.start_folder.as_deref(), remembered.as_deref());
    let mut session = PickerSession::new(&spec, request_path.clone(), start.clone(), reply);
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
    daemon.window_by_request.insert(request_path, window_id);

    let scan_task = match effect {
        SessionEffect::ScanDirectory(directory) => scan_directory_task(window_id, directory),
        _ => Task::none(),
    };
    Task::batch([
        open_task.map(|_opened| Message::IdleTick),
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
    let wants_address_focus = matches!(session_message, SessionMessage::AddressEditingStarted);
    let effect = session.update(session_message);
    let focus_task = if wants_address_focus {
        let input_id = iced::widget::Id::from(ADDRESS_INPUT_ID);
        Task::batch([
            iced::widget::operation::focus(input_id.clone()),
            iced::widget::operation::select_all(input_id),
        ])
    } else {
        Task::none()
    };
    let task = match effect {
        SessionEffect::ScanDirectory(directory) => scan_directory_task(window_id, directory),
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
    Task::batch([task, focus_task])
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

fn close_picker_window(daemon: &mut PickerDaemon, window_id: window::Id) {
    if let Some(session) = daemon.windows.remove(&window_id) {
        daemon.window_by_request.remove(session.request_path());
        session.window_closed();
    }
}

fn handle_key(daemon: &PickerDaemon, window: window::Id, key: keyboard::Key) -> Task<Message> {
    if !daemon.windows.contains_key(&window) {
        return Task::none();
    }
    // Ctrl+A 全选：仅在文本输入未捕获时到达这里（listen_with 只转发 Ignored）。
    if let keyboard::Key::Character(character) = &key {
        if character.eq_ignore_ascii_case("a") && daemon.keyboard_modifiers.control() {
            return Task::perform(async {}, move |_| {
                Message::Session(window, SessionMessage::SelectAllPressed)
            });
        }
    }
    // 编辑地址时 Esc 只退出编辑，不关闭窗口。
    let editing_address = daemon
        .windows
        .get(&window)
        .is_some_and(|session| session.address_edit().is_some());
    let escape_message = if editing_address {
        SessionMessage::AddressEditingCancelled
    } else {
        SessionMessage::DismissPressed
    };
    match key {
        keyboard::Key::Named(keyboard::key::Named::Escape) => Task::perform(async {}, move |_| {
            Message::Session(window, escape_message.clone())
        }),
        keyboard::Key::Named(keyboard::key::Named::Enter) => Task::perform(async {}, move |_| {
            Message::Session(window, SessionMessage::ConfirmPressed)
        }),
        _ => Task::none(),
    }
}

fn view_picker(daemon: &PickerDaemon, window_id: window::Id) -> Element<'_, Message> {
    let Some(session) = daemon.windows.get(&window_id) else {
        return iced::widget::text("").into();
    };
    view::picker_window_view(
        session,
        &daemon.theme,
        iced::widget::Id::from(ADDRESS_INPUT_ID),
        |message| message,
    )
    .map(move |message| Message::Session(window_id, message))
}

fn daemon_title(daemon: &PickerDaemon, window_id: window::Id) -> String {
    daemon
        .windows
        .get(&window_id)
        .map(|session| session.kind().default_title().to_string())
        .unwrap_or_else(|| "Bennu 文件选择".to_string())
}

fn subscription(_daemon: &PickerDaemon) -> Subscription<Message> {
    Subscription::batch([
        Subscription::run(bridge_events),
        window::close_events().map(Message::WindowClosed),
        iced::event::listen_with(|event, status, window_id| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. })
                if matches!(status, iced::event::Status::Ignored) =>
            {
                Some(Message::KeyPressed {
                    window: window_id,
                    key,
                })
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
        iced::time::every(IDLE_TICK_INTERVAL).map(|_| Message::IdleTick),
    ])
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
