//! 预览面板外层 surface：窗口级背景容器与滚动区高度下限。自 app-ui
//! view/preview_panel.rs 下沉，逐字节保真。

use iced::widget::container;
use iced::{Element, Length};

use bennu_theme::styles::app_content_style;

use crate::preview::PreviewSize;

const PREVIEW_MIN_SCROLL_HEIGHT: f32 = 160.0;

pub(crate) fn preview_surface<'a, Message: 'a>(
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}

pub(crate) fn preview_scroll_height(size: PreviewSize) -> f32 {
    size.height.max(PREVIEW_MIN_SCROLL_HEIGHT)
}
