//! 包内成员密码弹窗:成员提取(拖拽/粘贴入队失败补救)与双击打开
//! (物化失败补救)共用。复用整包解压弹窗的密码输入组件,仅面板内容
//! 按动作类型裁剪,不复制输入框。

use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length};

use crate::app::archive_member_password::{
    ArchiveMemberPasswordAction, ArchiveMemberPasswordMessage, ArchiveMemberPasswordState,
};
use crate::appearance::context_menu_style;
use crate::icons::IconSymbol;
use crate::model::Message;
use crate::typography::{localized_text, readable_text};

use super::option_controls::{
    inactive_primary_action_button, primary_action_button, secondary_action_button,
};
use super::{
    archive_extraction::archive_password_input_field, themed_icon, IconTone, MENU_ICON_SIZE,
};

const ARCHIVE_MEMBER_PASSWORD_PANEL_WIDTH: f32 = 500.0;

pub(super) fn archive_member_password_panel(
    state: &ArchiveMemberPasswordState,
) -> Element<'_, Message> {
    let title = row![
        themed_icon(IconSymbol::FileArchive, IconTone::Normal, MENU_ICON_SIZE),
        readable_text(summary_title(state))
            .size(16)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let validation_error = state
        .validation_error()
        .map(str::to_owned)
        .unwrap_or_default();

    let content = column![
        title,
        readable_text("Enter the archive password to continue.").size(13),
        archive_password_input_field(
            state.password(),
            state.can_submit_password(),
            |password| Message::ArchiveMemberPassword(
                ArchiveMemberPasswordMessage::PasswordChanged(password)
            ),
            Message::ArchiveMemberPassword(ArchiveMemberPasswordMessage::Submitted),
        ),
        localized_text(validation_error).size(12),
        archive_member_password_actions(state),
    ]
    .spacing(10)
    .width(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fixed(ARCHIVE_MEMBER_PASSWORD_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

/// 标题按动作区分:提取动作复用「解压归档」,打开动作显示「打开」;
/// 都是既有静态词条,不新增文案 key。
fn summary_title(state: &ArchiveMemberPasswordState) -> &'static str {
    match state.action() {
        ArchiveMemberPasswordAction::Extract { .. } => "Extract Archive",
        ArchiveMemberPasswordAction::Open { .. } => "Open",
    }
}

fn archive_member_password_actions(state: &ArchiveMemberPasswordState) -> Element<'_, Message> {
    let (submit_label, submitting_label) = match state.action() {
        ArchiveMemberPasswordAction::Extract { .. } => ("Extract", "Extract"),
        // 打开动作在重试在途时显示进行中;提取提交即入队,无在途态。
        ArchiveMemberPasswordAction::Open { .. } => ("Open", "Opening..."),
    };
    let submit_button = if state.can_submit_password() {
        primary_action_button(
            submit_label,
            Message::ArchiveMemberPassword(ArchiveMemberPasswordMessage::Submitted),
        )
    } else {
        inactive_primary_action_button(submitting_label)
    };

    row![
        Space::new().width(Length::Fill),
        secondary_action_button("Cancel", Message::DismissFloating),
        submit_button,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}
