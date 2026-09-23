//! 工具栏分段按钮组（导航/视图切换/面板开关共用）的唯一共享实现：
//! 主软件工具栏与 portal 地址栏行尾消费同一份容器与段按钮样式，
//! 保证两个进程的按钮视觉一致。图标元素由调用方构造（各自渲染
//! 语义色调），本模块只拥有按钮外观与交互状态。

use iced::widget::{button, container, Button, Row, Svg};
use iced::{Alignment, Background, Border, Color, Element, Theme};

use crate::styles::{
    base_text_color, button_hover_surface_color, button_pressed_surface_color,
    button_surface_color, muted_text_color, subtle_border_color,
};

/// 段按钮的交互状态：禁用段无动作（图标淡化由调用方负责）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentedButtonTone {
    /// 可点、无高亮。
    Normal,
    /// 当前选中模式的常亮段。
    Selected,
    /// 禁用（导航到头等）：不挂 on_press。
    Disabled,
}

/// 分段按钮组容器：surface 底色 + subtle 边框圆角 7，段间无缝。
pub fn segmented_button_group<Message: Clone + 'static>(
    content: Row<'static, Message>,
) -> Element<'static, Message> {
    container(content.spacing(0).align_y(Alignment::Center))
        .clip(true)
        .style(segmented_button_group_style)
        .into()
}

/// 分段按钮：hover/pressed 各加深一档底色，禁用段无动作且文字转 muted。
pub fn segmented_button<Message: Clone + 'static>(
    icon: Svg<'static, Theme>,
    tone: SegmentedButtonTone,
    message: Message,
) -> Button<'static, Message> {
    let mut segment = button(icon).padding([8, 10]).style(segmented_button_style);
    match tone {
        SegmentedButtonTone::Disabled => {}
        SegmentedButtonTone::Normal | SegmentedButtonTone::Selected => {
            segment = segment.on_press(message);
        }
    }
    segment
}

fn segmented_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => Some(Background::Color(button_hover_surface_color(theme))),
        button::Status::Pressed => Some(Background::Color(button_pressed_surface_color(theme))),
        button::Status::Active | button::Status::Disabled => None,
    };

    button::Style {
        background,
        text_color: if matches!(status, button::Status::Disabled) {
            muted_text_color(theme)
        } else {
            base_text_color(theme)
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..button::Style::default()
    }
}

fn segmented_button_group_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(button_surface_color(theme))),
        text_color: Some(base_text_color(theme)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 7.0.into(),
        },
        ..container::Style::default()
    }
}
