//! daemon 顶层消息体系：D-Bus 桥、选择窗会话、键盘路由、预览子系统
//! （包裹变体 + 预览窗窗口生命周期/滚动管线）与帧时钟。预览面板泛型
//! 要求 `Message: From<PreviewMessage> + Clone`（bennu-preview 边界
//! 契约）；桥事件本体不可 Clone（一次性 reply sender），Arc 包裹后
//! 从不进控件树的事件同样满足 bound。

use std::sync::Arc;

use iced::{keyboard, mouse, window};

use bennu_preview::preview_message::PreviewMessage;
use bennu_theme::scrollbar::ScrollbarViewport;
use bennu_theme::window_controls::WindowControlKind;

use crate::dbus_file_chooser::BridgeEvent;
use crate::picker_session::SessionMessage;
use crate::preview_scroll::PreviewScrollRegion;

impl From<PreviewMessage> for Message {
    fn from(message: PreviewMessage) -> Self {
        Message::Preview(message)
    }
}

#[derive(Clone)]
pub(crate) enum Message {
    Bridge(Arc<BridgeEvent>),
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
    /// 空格预览 toggle（keyboard_route 裁决后的延迟消息；source = 选中项
    /// 所属选择窗，预览窗内按下时为发起会话的请求窗，可能已亡）。
    PreviewSpacePressed {
        source: Option<window::Id>,
    },
    /// 预览子系统包裹变体：引擎直连变体在 PreviewEngine 内处理，
    /// 回退变体由 preview_host 路由（对齐 app-ui update 入口模式）。
    Preview(PreviewMessage),
    /// 预览窗 chrome 控制按钮（最小化/最大化切换/关闭）。
    PreviewWindowControl {
        window: window::Id,
        kind: WindowControlKind,
    },
    /// 预览窗标题栏按下：单击拖动，双击最大化（拖动面 widget 发布）。
    PreviewWindowTitlePressed {
        window: window::Id,
        double_click: bool,
    },
    /// 预览窗 resize 边缘按下 → 原生 drag_resize。
    PreviewWindowResizeEdgePressed {
        window: window::Id,
        direction: window::Direction,
    },
    /// 最大化观测回信（toggle 后 is_maximized 查询）。
    PreviewWindowMaximizeObserved {
        window: window::Id,
        maximized: bool,
    },
    /// 指针事件（所有窗口转发，update 侧按预览窗过滤；主软件 events.rs
    /// 同构）。高频消息，非预览窗直接 no-op。
    PreviewPointerMoved {
        window: window::Id,
        position: iced::Point,
    },
    PreviewPointerLeft {
        window: window::Id,
    },
    /// 预览窗内左键释放：拖拽收尾（chrome 拖动态/图片平移/SQLite 列宽）。
    PreviewPointerReleased {
        window: window::Id,
    },
    /// 预览窗尺寸变化（pending resize 匹配 + 文档重排）。
    PreviewWindowResized {
        window: window::Id,
        width: f32,
        height: f32,
    },
    /// 预览窗滚动管线（机制同选择窗滚动消息，区域枚举/id 换预览命名
    /// 空间；视口回传内层事件是 PreviewMessage，缓存后回流引擎）。
    PreviewWheelScrolled {
        region: PreviewScrollRegion,
        delta: mouse::ScrollDelta,
    },
    PreviewScrollbarLayoutVerified {
        region: PreviewScrollRegion,
        viewport: ScrollbarViewport,
    },
    PreviewScrollbarViewportChanged {
        region: PreviewScrollRegion,
        viewport: ScrollbarViewport,
        event: Box<PreviewMessage>,
    },
    PreviewScrollbarAutoHideElapsed {
        generation: u64,
    },
    /// SQLite 表格列宽拖拽结束（面板闭包注入）。
    PreviewSqliteDragFinished,
    /// 预览窗 Esc（keyboard_route 裁决后的延迟消息）：只关预览。
    PreviewCloseFocused,
    WindowFocused(window::Id),
    WindowUnfocused(window::Id),
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
