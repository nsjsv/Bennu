//! 工具栏分段按钮组（导航/视图切换/面板开关共用）的唯一共享实现：
//! 主软件工具栏与 portal 地址栏行尾消费同一份容器与段按钮样式，
//! 保证两个进程的按钮视觉一致。图标元素由调用方构造（各自渲染
//! 语义色调），本模块只拥有按钮外观与交互状态。

use iced::widget::{button, container, Button, Row, Svg};
use iced::{Alignment, Element, Length, Theme};

use crate::controls::{segment_style, segment_track_style, SEGMENT_INSET};
use crate::metrics::CONTROL_HEIGHT;

const SEGMENT_WIDTH: f32 = 34.0;

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

/// 分段按钮组：与 `controls::segmented_choice_row` 同一外观（浅灰底槽、选中段抬起），
/// 总高度等于控件高度。
pub fn segmented_button_group<Message: Clone + 'static>(
    content: Row<'static, Message>,
) -> Element<'static, Message> {
    container(content.spacing(0).align_y(Alignment::Center))
        .padding(SEGMENT_INSET)
        .style(segment_track_style)
        .into()
}

pub fn segmented_button<Message: Clone + 'static>(
    icon: Svg<'static, Theme>,
    tone: SegmentedButtonTone,
    message: Message,
) -> Button<'static, Message> {
    let selected = tone == SegmentedButtonTone::Selected;
    let segment = button(container(icon).center(Length::Fill))
        .width(Length::Fixed(SEGMENT_WIDTH))
        .height(Length::Fixed(CONTROL_HEIGHT - 2.0 * SEGMENT_INSET))
        .padding(0)
        .style(move |theme: &Theme, status| segment_style(theme, status, selected));
    match tone {
        SegmentedButtonTone::Disabled => segment,
        SegmentedButtonTone::Normal | SegmentedButtonTone::Selected => segment.on_press(message),
    }
}
