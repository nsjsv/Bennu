//! daemon 订阅构造：全局窗口/键盘/指针事件转发、D-Bus 桥流、帧时钟
//! 与媒体播放订阅的挂载判定（gating 读引擎活跃态，对齐主软件 app.rs）。

use std::time::Duration;

use iced::futures::Stream;
use iced::{keyboard, mouse, Subscription};
use std::pin::Pin;
use std::task::{Context, Poll};

use bennu_preview::animated_image_preview::animated_image_preview_subscription;
use bennu_preview::engine::{audio_preview_tick_subscription, video_preview_tick_subscription};
use bennu_preview::video_preview::video_preview_subscription;
use tokio::sync::mpsc;

use crate::dbus_file_chooser::BridgeEvent;
use crate::picker_session::{PickerSession, SessionMessage};
use crate::{Message, PickerDaemon};

/// 展开动画帧时钟：60Hz 与主应用 ui_pacing::FRAME_INTERVAL_60HZ 同值；
/// portal 不依赖 app-ui，本地保持同一节奏。
const ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub(crate) fn subscription(daemon: &PickerDaemon) -> Subscription<Message> {
    let mut subscriptions = vec![
        Subscription::run(bridge_events),
        iced::window::close_events().map(Message::WindowClosed),
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
            // 预览窗指针事件（chrome 淡入淡出/拖拽收尾/列宽拖拽）：全窗
            // 转发、update 按预览窗过滤（主软件 events.rs 同构）。
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                Some(Message::PreviewPointerMoved {
                    window: window_id,
                    position,
                })
            }
            iced::Event::Mouse(mouse::Event::CursorLeft) => {
                Some(Message::PreviewPointerLeft { window: window_id })
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                Some(Message::PreviewPointerReleased { window: window_id })
            }
            // 窗口 resize：选择窗刷新网格列数，预览窗跟进 pending resize。
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Message::WindowResized {
                    window: window_id,
                    width: size.width,
                    height: size.height,
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
            // 窗口焦点跟踪：预览窗的 Esc 分层与失焦自动关闭（步骤 3）
            // 依赖这份簿记，本步先落事件转发。
            iced::Event::Window(iced::window::Event::Focused) => {
                Some(Message::WindowFocused(window_id))
            }
            iced::Event::Window(iced::window::Event::Unfocused) => {
                Some(Message::WindowUnfocused(window_id))
            }
            _ => None,
        }),
    ];
    // 仅在有窗口播放动画（选择窗：展开/收起、地址栏渐变、惯性滚动、
    // 滚动条淡入淡出；预览窗：chrome/底部控件淡入淡出、预览树展开、
    // 惯性滚动、滚动条淡入淡出）时订阅帧时钟，动画结束自然摘除。
    if daemon.windows.values().any(PickerSession::is_animating) || daemon.preview.is_animating() {
        subscriptions
            .push(iced::time::every(ANIMATION_FRAME_INTERVAL).map(|_| Message::AnimationTick));
    }
    // 媒体播放/帧流订阅（主软件 app.rs 的 gating 模式：读引擎活跃态，
    // 播放结束/会话切换自然摘除）：音频/视频进度 tick（250ms）与动图/
    // 视频帧流（当前播放路径 + 代数 + 起播位置）。挂载判定在
    // active_media_streams（纯查询，单测锚点）。
    let media = active_media_streams(&daemon.preview.engine);
    if media.audio_tick {
        subscriptions.push(audio_preview_tick_subscription().map(Message::Preview));
    }
    if media.video_tick {
        subscriptions.push(video_preview_tick_subscription().map(Message::Preview));
    }
    if let Some((path, generation, position)) = media.animated_image {
        subscriptions.push(
            animated_image_preview_subscription(path, generation, position).map(Message::Preview),
        );
    }
    if let Some((path, generation, position)) = media.video {
        subscriptions
            .push(video_preview_subscription(path, generation, position).map(Message::Preview));
    }
    Subscription::batch(subscriptions)
}

/// 媒体订阅挂载判定（纯查询，单测锚点）：四路订阅的活跃快照，
/// subscription() 据此挂载（gating 语义对齐主软件 app.rs）。
pub(crate) struct ActiveMediaStreams {
    pub(crate) audio_tick: bool,
    pub(crate) video_tick: bool,
    pub(crate) animated_image: Option<(std::path::PathBuf, u64, Duration)>,
    pub(crate) video: Option<(std::path::PathBuf, u64, Duration)>,
}

pub(crate) fn active_media_streams(
    engine: &bennu_preview::engine::PreviewEngine,
) -> ActiveMediaStreams {
    ActiveMediaStreams {
        audio_tick: engine.audio_preview_is_active(),
        video_tick: engine.video_preview_is_active(),
        animated_image: engine.active_animated_image_preview_stream(),
        video: engine.active_video_preview_stream(),
    }
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
                    return receiver.poll_recv(context).map(|event| {
                        event.map(|event| Message::Bridge(std::sync::Arc::new(event)))
                    })
                }
                BridgeSource::Ended => return Poll::Ready(None),
            }
        }
    }
}

fn take_bridge_receiver() -> Option<mpsc::Receiver<BridgeEvent>> {
    crate::BRIDGE_SLOT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}
