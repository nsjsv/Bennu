//! 预览浮动窗口 chrome：全客户区悬浮控制层（顶部渐变 + 浮动窗口控制
//! 组 + 固定按钮 + 拖动面）。自 app-ui view/window_chrome.rs 下沉。
//! 窗口管理语义（最小化/最大化/关闭消息、标题拖动面）属宿主 Message
//! 体系，经闭包注入；固定按钮事件是预览域消息（PreviewMessage）。
//! 窗口控制按钮机件（window_control_group 等）与宿主辅助窗口 chrome
//! 共用，此处是唯一实现，宿主经包装函数映射自己的窗口消息。

use iced::widget::{button, container, tooltip, Row, Space, Stack};
use iced::window;
use iced::{Background, Element, Length};

use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::styles::context_menu_style;
use bennu_theme::window_chrome_styles::{
    floating_window_close_button_style, floating_window_control_button_style,
    window_close_button_style, window_control_button_style,
};
use bennu_theme::window_controls::{
    WindowControlKind, WindowControlSide, WindowControlsConfig, WindowFrameState,
};

use crate::panel_text::readable_text;
use crate::preview::PreviewWindowChromeState;
use crate::preview_message::PreviewMessage;

use iced::Theme;

const WINDOW_CONTROL_WIDTH: f32 = 36.0;
const WINDOW_CONTROL_HEIGHT: f32 = 32.0;
const WINDOW_CONTROL_ICON_SIZE: f32 = 12.0;
const WINDOW_CONTROL_VERTICAL_PADDING: f32 =
    (WINDOW_CONTROL_HEIGHT - WINDOW_CONTROL_ICON_SIZE) / 2.0;
const WINDOW_CONTROL_HORIZONTAL_PADDING: f32 =
    (WINDOW_CONTROL_WIDTH - WINDOW_CONTROL_ICON_SIZE) / 2.0;
const WINDOW_CONTROL_SPACING: f32 = 2.0;

/// 窗口控制按钮的呈现形态：标准（辅助窗口标题栏）或带透明度的浮动
/// （预览/设置窗口悬浮层）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum WindowControlPresentation {
    Standard,
    Floating { opacity: f32 },
}

/// 标准呈现的控制组（宿主辅助窗口 chrome 经包装函数消费）。控制动作
/// 闭包按引用传入（预览浮动 chrome 会复用同一闭包构造左右两组）。
pub fn window_control_group<Message>(
    config: &WindowControlsConfig,
    side: WindowControlSide,
    window: window::Id,
    frame_state: WindowFrameState,
    on_control: &(impl Fn(WindowControlKind, window::Id) -> Message + 'static),
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    control_group(
        config,
        side,
        window,
        frame_state,
        on_control,
        WindowControlPresentation::Standard,
    )
}

/// 浮动呈现的控制组（宿主设置窗口悬浮控制行与预览浮动 chrome 消费）。
pub fn floating_window_control_group<Message>(
    config: &WindowControlsConfig,
    side: WindowControlSide,
    window: window::Id,
    frame_state: WindowFrameState,
    opacity: f32,
    on_control: &(impl Fn(WindowControlKind, window::Id) -> Message + 'static),
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    control_group(
        config,
        side,
        window,
        frame_state,
        on_control,
        WindowControlPresentation::Floating {
            opacity: opacity.clamp(0.0, 1.0),
        },
    )
}

fn control_group<Message>(
    config: &WindowControlsConfig,
    side: WindowControlSide,
    window: window::Id,
    frame_state: WindowFrameState,
    on_control: &(impl Fn(WindowControlKind, window::Id) -> Message + 'static),
    presentation: WindowControlPresentation,
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    let mut controls = Row::new()
        .spacing(WINDOW_CONTROL_SPACING)
        .height(Length::Fixed(WINDOW_CONTROL_HEIGHT));
    for placement in config
        .placements_on(side)
        .filter(|placement| placement.visibility().is_visible())
    {
        controls = controls.push(window_control_button(
            placement.kind(),
            window,
            frame_state,
            presentation,
            on_control,
        ));
    }
    controls.into()
}

fn window_control_button<Message>(
    kind: WindowControlKind,
    window: window::Id,
    frame_state: WindowFrameState,
    presentation: WindowControlPresentation,
    on_control: &impl Fn(WindowControlKind, window::Id) -> Message,
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    let (icon, label) = match kind {
        WindowControlKind::Minimize => (IconSymbol::Minus, "Minimize"),
        WindowControlKind::MaximizeRestore => match frame_state {
            WindowFrameState::Restored => (IconSymbol::Square, "Maximize"),
            WindowFrameState::Maximized => (IconSymbol::RestoreWindow, "Restore"),
        },
        WindowControlKind::Close => (IconSymbol::Close, "Close"),
    };
    let message = on_control(kind, window);
    let (style, opacity): (fn(&Theme, button::Status) -> button::Style, f32) =
        match (presentation, kind) {
            (WindowControlPresentation::Floating { opacity }, WindowControlKind::Close) => {
                (floating_window_close_button_style, opacity)
            }
            (WindowControlPresentation::Floating { opacity }, _) => {
                (floating_window_control_button_style, opacity)
            }
            (WindowControlPresentation::Standard, WindowControlKind::Close) => {
                (window_close_button_style, 1.0)
            }
            (WindowControlPresentation::Standard, _) => (window_control_button_style, 1.0),
        };
    let control =
        button(themed_icon(icon, IconTone::Normal, WINDOW_CONTROL_ICON_SIZE).opacity(opacity))
            .on_press(message)
            .padding([
                WINDOW_CONTROL_VERTICAL_PADDING,
                WINDOW_CONTROL_HORIZONTAL_PADDING,
            ])
            .width(Length::Fixed(WINDOW_CONTROL_WIDTH))
            .height(Length::Fixed(WINDOW_CONTROL_HEIGHT))
            .style(move |theme, status| {
                let mut style = style(theme, status);
                style.background = style
                    .background
                    .map(|background| background.scale_alpha(opacity));
                style.text_color = style.text_color.scale_alpha(opacity);
                style
            });

    tooltip(
        control,
        container(readable_text(label).size(11))
            .padding([5, 7])
            .style(context_menu_style),
        tooltip::Position::Bottom,
    )
    .into()
}

/// 标准呈现的预览固定按钮（宿主集成顶栏/独立标题栏经包装消费）。
pub fn preview_pin_button<Message>(pinned: bool) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    pin_button(pinned, WindowControlPresentation::Standard)
}

fn floating_preview_pin_button<Message>(pinned: bool, opacity: f32) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    pin_button(
        pinned,
        WindowControlPresentation::Floating {
            opacity: opacity.clamp(0.0, 1.0),
        },
    )
}

// 预览固定按钮：固定时用主题 primary 高亮，点击发送 PreviewWindowPinToggled。
fn pin_button<Message>(
    pinned: bool,
    presentation: WindowControlPresentation,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let (base_style, opacity): (fn(&Theme, button::Status) -> button::Style, f32) =
        match presentation {
            WindowControlPresentation::Floating { opacity } => (
                floating_window_control_button_style,
                opacity.clamp(0.0, 1.0),
            ),
            WindowControlPresentation::Standard => (window_control_button_style, 1.0),
        };
    let control = button(
        themed_icon(IconSymbol::Pin, IconTone::Normal, WINDOW_CONTROL_ICON_SIZE).opacity(opacity),
    )
    .on_press(Message::from(PreviewMessage::PreviewWindowPinToggled))
    .padding([
        WINDOW_CONTROL_VERTICAL_PADDING,
        WINDOW_CONTROL_HORIZONTAL_PADDING,
    ])
    .width(Length::Fixed(WINDOW_CONTROL_WIDTH))
    .height(Length::Fixed(WINDOW_CONTROL_HEIGHT))
    .style(move |theme, status| {
        let mut style = base_style(theme, status);
        if pinned {
            let colors = bennu_theme::ui_colors(theme);
            style.background = Some(Background::Color(colors.primary));
            style.text_color = colors.on_primary;
        }
        style.background = style
            .background
            .map(|background| background.scale_alpha(opacity));
        style.text_color = style.text_color.scale_alpha(opacity);
        style
    });

    tooltip(
        control,
        container(readable_text(if pinned { "Unpin" } else { "Pin" }).size(11))
            .padding([5, 7])
            .style(context_menu_style),
        tooltip::Position::Bottom,
    )
    .into()
}

/// 预览浮动窗口内容：媒体内容之上叠加顶部控制层（渐变 + 左控制组 +
/// 固定按钮 + 右控制组），整层包宿主注入的拖动面包装（标题拖动/双击
/// 最大化）。窗口控制动作消息与拖动面是宿主窗口管理语义，经闭包注入。
// 参数集合是窗口 chrome 的完整配置（内容 + 控制配置 + 消息闭包对），镜像宿主调用点
#[allow(clippy::too_many_arguments)]
pub fn floating_preview_window_content<'a, Message>(
    content: Element<'a, Message>,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    chrome_opacity: f32,
    pinned: bool,
    on_window_control: &(impl Fn(WindowControlKind, window::Id) -> Message + 'static),
    drag_region_wrap: impl Fn(Element<'a, Message>) -> Element<'a, Message> + 'a,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let chrome_opacity = chrome_opacity.clamp(0.0, 1.0);
    let top_bar: Element<'a, Message> = if chrome_opacity > f32::EPSILON {
        let controls = Row::new()
            .spacing(10)
            .padding(iced::Padding {
                top: 6.0,
                right: 8.0,
                bottom: 10.0,
                left: 8.0,
            })
            .align_y(iced::Alignment::Center)
            .push(floating_window_control_group(
                config,
                WindowControlSide::Left,
                window,
                frame_state,
                chrome_opacity,
                on_window_control,
            ))
            .push(floating_preview_pin_button(pinned, chrome_opacity))
            .push(Space::new().width(Length::Fill))
            .push(floating_window_control_group(
                config,
                WindowControlSide::Right,
                window,
                frame_state,
                chrome_opacity,
                on_window_control,
            ))
            .width(Length::Fill)
            .height(Length::Fixed(PreviewWindowChromeState::REVEAL_HEIGHT));
        let gradient = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme| {
                bennu_theme::preview_styles::preview_window_top_gradient_style(
                    theme,
                    chrome_opacity,
                )
            });

        Stack::with_children([gradient.into(), controls.into()])
            .width(Length::Fill)
            .height(Length::Fixed(PreviewWindowChromeState::REVEAL_HEIGHT))
            .into()
    } else {
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(PreviewWindowChromeState::REVEAL_HEIGHT))
            .into()
    };
    let drag_surface = drag_region_wrap(top_bar);

    Stack::with_children([content, drag_surface])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
