use iced::widget::{container, Row, Space};
use iced::{Element, Length};

use crate::appearance::{
    switch_thumb_off_style, switch_thumb_on_style, switch_track_off_style, switch_track_on_style,
};
use crate::model::Message;

pub(super) fn switch_control(is_on: bool) -> Element<'static, Message> {
    let content = if is_on {
        Row::new()
            .push(Space::new().width(Length::Fill))
            .push(switch_thumb(switch_thumb_on_style))
    } else {
        Row::new()
            .push(switch_thumb(switch_thumb_off_style))
            .push(Space::new().width(Length::Fill))
    };

    container(content)
        .padding(3)
        .width(Length::Fixed(38.0))
        .height(Length::Fixed(22.0))
        .style(if is_on {
            switch_track_on_style
        } else {
            switch_track_off_style
        })
        .into()
}

fn switch_thumb(style: fn(&iced::Theme) -> container::Style) -> Element<'static, Message> {
    container(Space::new().width(Length::Fixed(1.0)))
        .width(Length::Fixed(14.0))
        .height(Length::Fixed(14.0))
        .style(style)
        .into()
}
