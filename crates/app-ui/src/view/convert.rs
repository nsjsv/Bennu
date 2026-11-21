//! 转换对话框：最上面快捷设置，中间每类媒体一块卡片，下面保存位置。
//! 次要选项都收在每块卡片的「更多」里，默认只露格式和质量。

use iced::widget::row;
use iced::{Alignment, Element, Length};

use crate::app::convert::ConvertMessage;
use crate::model::{
    ConvertEdit, ConvertInputError, ConvertScene, ConvertState, Message, CONVERT_SCENES,
};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{
    icon_button, normal_button, segmented_choice_row, suggested_button, text_field, tooltip,
    tooltip_bubble, SegmentedChoice,
};
use bennu_theme::icons::IconSymbol;

use super::dialog::{self, Dialog};
use super::{themed_icon, IconTone};

mod media_sections;
mod video_section;

const CONVERT_PANEL_WIDTH: f32 = 600.0;
const ICON_SIZE: f32 = 14.0;

pub(super) fn convert_panel(state: &ConvertState) -> Element<'_, Message> {
    let mut body = dialog::fields(vec![scene_field(state)]);
    let encoders = state.probed_encoders();
    if let Some(section) = &state.image {
        body = body.push(media_sections::image_section(section, encoders));
    }
    if let Some(section) = &state.video {
        body = body.push(video_section::video_section(section, encoders));
    }
    if let Some(section) = &state.audio {
        body = body.push(media_sections::audio_section(section, encoders));
    }
    body = body.push(output_field(state));
    // 没探测完不提示：探测很快，先出一行再消失只会让对话框跳一下。
    if encoders.is_some_and(|encoders| encoders.is_empty()) {
        body = body.push(ffmpeg_notice());
    }
    let input_error = state.input_error();
    if let Some(text) = input_error.and_then(input_error_text) {
        body = body.push(dialog::error(text));
    }
    Dialog::new(tr!("Convert Format"), body)
        .subtitle(tr!("%1 items", state.source_count()))
        .actions(actions(input_error.is_none()))
        .width(CONVERT_PANEL_WIDTH)
        .into()
}

fn edit(edit: ConvertEdit) -> Message {
    Message::Convert(ConvertMessage::Edit(edit))
}

/// 场景要按编码器挑格式，探测完之前按钮不可点。
fn scene_field(state: &ConvertState) -> Element<'static, Message> {
    let ready = state.probed_encoders().is_some();
    let active = state.active_scene();
    let choices = CONVERT_SCENES
        .into_iter()
        .map(|scene| SegmentedChoice {
            label: scene_label(scene),
            selected: active == Some(scene),
            message: ready.then(|| edit(ConvertEdit::Scene(scene))),
            tooltip: None,
        })
        .collect();
    dialog::field(tr!("Quick settings"), segmented_choice_row(choices))
}

fn scene_label(scene: ConvertScene) -> String {
    match scene {
        ConvertScene::MostCompatible => tr!("Most compatible"),
        ConvertScene::ForWeb => tr!("For the web"),
        ConvertScene::Smallest => tr!("Smallest size"),
    }
}

/// 空着就放在原文件旁边；能手打，右边按钮打开目录选择器。
fn output_field(state: &ConvertState) -> Element<'_, Message> {
    let choose = tooltip(
        icon_button(themed_icon(
            IconSymbol::FolderOpen,
            IconTone::Normal,
            ICON_SIZE,
        ))
        .on_press(Message::Convert(ConvertMessage::OutputDirectoryBrowse)),
        tooltip_bubble(tr!("Choose directory")),
        iced::widget::tooltip::Position::Left,
    );
    dialog::field(
        tr!("Save to"),
        row![
            text_field(
                &tr!("Next to the original files"),
                &state.output_directory_text
            )
            .on_input(|text| edit(ConvertEdit::OutputDirectory(text)))
            .width(Length::Fill),
            choose,
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
}

fn ffmpeg_notice() -> Element<'static, Message> {
    row![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Normal, ICON_SIZE),
        readable_text(tr!(
            "ffmpeg is not installed, so video, audio and some image formats cannot be converted."
        ))
        .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// 缺编码器不出红字：上面的 ffmpeg 提示已经说了。
fn input_error_text(error: ConvertInputError) -> Option<String> {
    match error {
        ConvertInputError::MissingEncoder => None,
        ConvertInputError::TargetSize => Some(tr!("Enter a number for the target size.")),
        ConvertInputError::LongEdge => Some(tr!("Enter the long edge in pixels.")),
        ConvertInputError::Trim => Some(tr!(
            "Write times like 1:30 or 0:01:30.5, with the end after the start."
        )),
        ConvertInputError::OutputDirectory => Some(tr!("Enter an absolute path.")),
    }
}

fn actions(can_submit: bool) -> Vec<Element<'static, Message>> {
    vec![
        normal_button(readable_text(tr!("Cancel")))
            .on_press(Message::DismissFloating)
            .into(),
        suggested_button(readable_text(tr!("Convert")))
            .on_press_maybe(can_submit.then_some(Message::Convert(ConvertMessage::Submitted)))
            .into(),
    ]
}
