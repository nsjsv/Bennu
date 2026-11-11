//! 包内成员密码弹窗:成员提取(拖拽/粘贴入队失败补救)与双击打开
//! (物化失败补救)共用。复用整包解压弹窗的密码输入组件,仅面板内容
//! 按动作类型裁剪,不复制输入框。

use iced::Element;

use super::archive_extraction::archive_password_input_field;
use super::dialog::{self, Dialog};
use crate::app::archive_member_password::{
    ArchiveMemberPasswordAction, ArchiveMemberPasswordMessage, ArchiveMemberPasswordState,
};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{normal_button, suggested_button};

pub(super) fn archive_member_password_panel(
    state: &ArchiveMemberPasswordState,
) -> Element<'_, Message> {
    let mut body = dialog::fields(vec![
        dialog::hint(tr!("Enter the archive password to continue.")),
        archive_password_input_field(
            state.password(),
            state.can_submit_password(),
            |password| {
                Message::ArchiveMemberPassword(ArchiveMemberPasswordMessage::PasswordChanged(
                    password,
                ))
            },
            Message::ArchiveMemberPassword(ArchiveMemberPasswordMessage::Submitted),
        ),
    ]);
    if let Some(problem) = state.validation_error() {
        body = body.push(dialog::error(problem.to_owned()));
    }
    Dialog::new(summary_title(state), body)
        .actions(archive_member_password_actions(state))
        .into()
}

/// 标题按动作区分:提取动作复用「解压归档」,打开动作显示「打开」。
fn summary_title(state: &ArchiveMemberPasswordState) -> String {
    match state.action() {
        ArchiveMemberPasswordAction::Extract { .. } => tr!("Extract Archive"),
        ArchiveMemberPasswordAction::Open { .. } => tr!("Open"),
    }
}

fn archive_member_password_actions(
    state: &ArchiveMemberPasswordState,
) -> Vec<Element<'_, Message>> {
    let (submit_label, submitting_label) = match state.action() {
        ArchiveMemberPasswordAction::Extract { .. } => (tr!("Extract"), tr!("Extract")),
        // 打开动作在重试在途时显示进行中;提取提交即入队,无在途态。
        ArchiveMemberPasswordAction::Open { .. } => (tr!("Open"), tr!("Opening...")),
    };
    let submit = if state.can_submit_password() {
        suggested_button(readable_text(submit_label)).on_press(Message::ArchiveMemberPassword(
            ArchiveMemberPasswordMessage::Submitted,
        ))
    } else {
        suggested_button(readable_text(submitting_label))
    };
    vec![
        normal_button(readable_text(tr!("Cancel")))
            .on_press(Message::DismissFloating)
            .into(),
        submit.into(),
    ]
}
