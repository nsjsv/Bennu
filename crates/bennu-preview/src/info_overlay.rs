//! 盖在画面左上角的信息层（视频、图片共用）：深色半透明底、浅色字，不管主题深浅、
//! 画面明暗都看得清；行多（几十条字幕轨）时在框里滚动。

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{container, row, scrollable, Column};
use iced::{Element, Length};

use crate::panel_text::readable_text;

const INFO_MARGIN: f32 = 16.0;
const INFO_PADDING: f32 = 14.0;
const INFO_RADIUS: f32 = 8.0;
const INFO_MAX_WIDTH: f32 = 520.0;
const INFO_LABEL_WIDTH: f32 = 96.0;
const INFO_COLUMN_GAP: f32 = 12.0;
const INFO_ROW_SPACING: f32 = 4.0;
const INFO_BACKGROUND_ALPHA: f32 = 0.68;
const INFO_LABEL_ALPHA: f32 = 0.62;

/// 信息层的一行：名称和内容。
pub(crate) struct InfoRow {
    pub label: String,
    pub value: String,
}

pub(crate) fn info_overlay<Message>(info_rows: Vec<InfoRow>) -> Element<'static, Message>
where
    Message: 'static,
{
    let label_style = |_: &iced::Theme| iced::widget::text::Style {
        color: Some(iced::Color::from_rgba(1.0, 1.0, 1.0, INFO_LABEL_ALPHA)),
    };
    let value_style = |_: &iced::Theme| iced::widget::text::Style {
        color: Some(iced::Color::WHITE),
    };
    let mut rows = Column::new().spacing(INFO_ROW_SPACING);
    for info_row in info_rows {
        rows = rows.push(
            row![
                readable_text(info_row.label)
                    .style(label_style)
                    .width(Length::Fixed(INFO_LABEL_WIDTH)),
                readable_text(info_row.value).style(value_style),
            ]
            .spacing(INFO_COLUMN_GAP),
        );
    }
    let card = container(scrollable(rows).height(Length::Shrink))
        .padding(INFO_PADDING)
        .max_width(INFO_MAX_WIDTH)
        .style(|_| container::Style {
            background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, INFO_BACKGROUND_ALPHA).into()),
            border: iced::Border {
                radius: INFO_RADIUS.into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        });
    container(card)
        .padding(INFO_MARGIN)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Left)
        .align_y(Vertical::Top)
        .into()
}
