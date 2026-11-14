//! 预览面板外层 surface：窗口级背景容器。自 app-ui
//! view/preview_panel.rs 下沉。

use iced::widget::container;
use iced::{Element, Length};

use bennu_theme::styles::app_content_style;

pub(crate) fn preview_surface<'a, Message: 'a>(
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}
