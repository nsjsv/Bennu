//! 窗口按钮机件：最小化 / 最大化 / 关闭按钮组与预览“固定”按钮。主程序与 portal 的
//! 所有窗口经 `window_shell` 共用这一份实现；按钮消息属于宿主，经闭包注入。

use bennu_localization::tr;
use iced::widget::{button, tooltip, Row};
use iced::{window, Background, Element, Length};

use bennu_theme::controls::tooltip_bubble;
use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::window_chrome_styles::{window_control_button_style, WINDOW_CONTROL_SIZE};
use bennu_theme::window_controls::{
    WindowControlKind, WindowControlSide, WindowControlsConfig, WindowFrameState,
};

use crate::preview_message::PreviewMessage;

const WINDOW_CONTROL_ICON_SIZE: f32 = 12.0;
pub(crate) const WINDOW_CONTROL_SPACING: f32 = 8.0;

/// 一侧（左 / 右）的窗口按钮组，只含配置为可见的按钮。
pub fn window_control_group<Message>(
    config: &WindowControlsConfig,
    side: WindowControlSide,
    window: window::Id,
    frame_state: WindowFrameState,
    on_control: &dyn Fn(WindowControlKind, window::Id) -> Message,
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    let mut controls = Row::new()
        .spacing(WINDOW_CONTROL_SPACING)
        .align_y(iced::Alignment::Center);
    for placement in config
        .placements_on(side)
        .filter(|placement| placement.visibility().is_visible())
    {
        controls = controls.push(window_control_button(
            placement.kind(),
            frame_state,
            on_control(placement.kind(), window),
        ));
    }
    controls.into()
}

fn window_control_button<Message>(
    kind: WindowControlKind,
    frame_state: WindowFrameState,
    message: Message,
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    let (icon, label) = match kind {
        WindowControlKind::Minimize => (IconSymbol::Minus, tr!("Minimize")),
        WindowControlKind::MaximizeRestore => match frame_state {
            WindowFrameState::Restored => (IconSymbol::Square, tr!("Maximize")),
            WindowFrameState::Maximized => (IconSymbol::RestoreWindow, tr!("Restore")),
        },
        WindowControlKind::Close => (IconSymbol::Close, tr!("Close")),
    };
    with_tooltip(
        round_button(icon)
            .on_press(message)
            .style(window_control_button_style),
        label,
    )
}

/// 预览“固定”按钮：外观同窗口按钮，固定时用强调色。
pub fn preview_pin_button<Message>(pinned: bool) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let control = round_button(IconSymbol::Pin)
        .on_press(Message::from(PreviewMessage::PreviewWindowPinToggled))
        .style(move |theme, status| {
            let mut style = window_control_button_style(theme, status);
            if pinned {
                let colors = bennu_theme::ui_colors(theme);
                style.background = Some(Background::Color(colors.primary));
                style.text_color = colors.on_primary;
            }
            style
        });
    with_tooltip(control, if pinned { tr!("Unpin") } else { tr!("Pin") })
}

fn round_button<Message: 'static>(icon: IconSymbol) -> iced::widget::Button<'static, Message> {
    button(
        iced::widget::container(themed_icon(
            icon,
            IconTone::Normal,
            WINDOW_CONTROL_ICON_SIZE,
        ))
        .center(Length::Fill),
    )
    .padding(0)
    .width(Length::Fixed(WINDOW_CONTROL_SIZE))
    .height(Length::Fixed(WINDOW_CONTROL_SIZE))
}

fn with_tooltip<Message: Clone + 'static>(
    control: iced::widget::Button<'static, Message>,
    label: String,
) -> Element<'static, Message> {
    tooltip(control, tooltip_bubble(label), tooltip::Position::Bottom).into()
}
