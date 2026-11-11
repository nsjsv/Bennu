//! 设备行里这台设备最近一次直推的状态：校验/发送进度、要 PIN 的输入、结果与失败详情。

use iced::widget::{column, row, Column};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{DirectSendFailure, DirectSendPhase, DirectSendState, TransferMessage};
use crate::app::FileBrowser;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::ui_colors;

use super::settings_group::muted_setting_text;
use bennu_theme::controls::normal_button;
use bennu_theme::controls::text_field;
use bennu_theme::typography::CAPTION_SIZE;

const STATUS_MAX_CHARS: usize = 72;
const DETAIL_MAX_CHARS: usize = 64;

/// 状态行，外加要 PIN 时的输入行、展开了的失败详情。
pub(super) fn direct_send_lines<'a>(
    browser: &FileBrowser,
    fingerprint: &str,
    state: &'a DirectSendState,
) -> Column<'a, Message> {
    let mut lines = column![status_line(browser, fingerprint, state)].spacing(2);
    match &state.phase {
        DirectSendPhase::NeedsPin { input, .. } => {
            lines = lines.push(pin_prompt(fingerprint, input));
        }
        DirectSendPhase::Finished {
            failed,
            details_open: true,
            ..
        } => {
            for failure in failed {
                // 文件名是用户内容；原因是 local-send 的英文原因，与日志一致。
                let line = format!("{}  {}", failure.file_name, failure.reason);
                lines = lines.push(error_text(format_middle_ellipsized_text(
                    &line,
                    DETAIL_MAX_CHARS,
                )));
            }
        }
        _ => {}
    }
    lines
}

fn status_line(
    browser: &FileBrowser,
    fingerprint: &str,
    state: &DirectSendState,
) -> Element<'static, Message> {
    match &state.phase {
        DirectSendPhase::Checking => with_cancel_button(
            muted_setting_text(
                tr!("Checking files %1%", browser.checking_percent(state)),
                11,
            ),
            fingerprint,
        ),
        DirectSendPhase::Sending if state.total_files > 0 => with_cancel_button(
            muted_setting_text(
                tr!(
                    "Sending %1/%2 · %3%",
                    // 协议层的序号从 0 开始。
                    state.file_index + 1,
                    state.total_files,
                    file_percent(state)
                ),
                11,
            ),
            fingerprint,
        ),
        // 还没报过进度：对方在决定要不要收。
        DirectSendPhase::Sending => with_cancel_button(
            muted_setting_text(tr!("Waiting for the device to accept..."), 11),
            fingerprint,
        ),
        DirectSendPhase::Canceling => muted_setting_text(tr!("Canceling..."), 11),
        DirectSendPhase::NeedsPin { wrong: false, .. } => {
            readable_text(tr!("This device requires a PIN"))
                .size(CAPTION_SIZE)
                .into()
        }
        DirectSendPhase::NeedsPin { wrong: true, .. } => error_text(tr!("Wrong PIN. Try again.")),
        DirectSendPhase::Finished { failed, .. } if failed.is_empty() => {
            muted_setting_text(tr!("Sent"), 11)
        }
        DirectSendPhase::Finished {
            sent: 0,
            details_open,
            ..
        } => with_details_button(error_text(tr!("Send failed")), fingerprint, *details_open),
        DirectSendPhase::Finished {
            failed,
            details_open,
            ..
        } => with_details_button(
            muted_setting_text(tr!("Sent, %1 failed", failed.len()), 11),
            fingerprint,
            *details_open,
        ),
        DirectSendPhase::Canceled => muted_setting_text(tr!("Canceled"), 11),
        DirectSendPhase::Failed(DirectSendFailure::Declined) => {
            error_text(tr!("The device declined."))
        }
        DirectSendPhase::Failed(DirectSendFailure::TooManyPinAttempts) => error_text(tr!(
            "Too many wrong PINs. The device no longer accepts files from this computer for now."
        )),
        DirectSendPhase::Failed(DirectSendFailure::NothingToSend) => {
            error_text(tr!("Nothing to send. Empty folders cannot be sent."))
        }
        DirectSendPhase::Failed(DirectSendFailure::Other(error)) => error_text(
            format_middle_ellipsized_text(&tr!("Send failed: %1", error), STATUS_MAX_CHARS),
        ),
    }
}

/// 进行中的发送旁边的"取消"：与任务面板同一个动作。
fn with_cancel_button(
    status: Element<'static, Message>,
    fingerprint: &str,
) -> Element<'static, Message> {
    row![
        status,
        normal_button(readable_text(tr!("Cancel")).size(CAPTION_SIZE)).on_press(Message::Transfer(
            TransferMessage::DirectSendCancelPressed(fingerprint.to_owned(),)
        )),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn with_details_button(
    status: Element<'static, Message>,
    fingerprint: &str,
    open: bool,
) -> Element<'static, Message> {
    let label = if open {
        tr!("Hide details")
    } else {
        tr!("Details")
    };
    row![
        status,
        normal_button(readable_text(label).size(CAPTION_SIZE)).on_press(Message::Transfer(
            TransferMessage::DirectSendDetailsToggled(fingerprint.to_owned(),)
        )),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn error_text(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(CAPTION_SIZE)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_colors(theme).error),
        })
        .into()
}

/// 对方要 PIN：圆点遮住的输入框（回车即发）+ 取消/发送；用同一份准备结果重发。
fn pin_prompt<'a>(fingerprint: &str, input: &'a str) -> Element<'a, Message> {
    let changed = fingerprint.to_owned();
    let submit = Message::Transfer(TransferMessage::DirectSendPinSubmitted(
        fingerprint.to_owned(),
    ));
    let action = |label: String, message: Option<Message>| {
        normal_button(readable_text(label)).on_press_maybe(message)
    };
    row![
        text_field(&tr!("PIN"), input)
            .secure(true)
            .on_input(move |value| {
                Message::Transfer(TransferMessage::DirectSendPinChanged {
                    fingerprint: changed.clone(),
                    value,
                })
            })
            .on_submit_maybe((!input.is_empty()).then(|| submit.clone()))
            .width(Length::Fill),
        action(
            tr!("Cancel"),
            Some(Message::Transfer(TransferMessage::DirectSendCancelPressed(
                fingerprint.to_owned(),
            ))),
        ),
        action(tr!("Send"), (!input.is_empty()).then_some(submit)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// 当前文件的进度百分比；大小为 0 的文件直接算完成。
fn file_percent(state: &DirectSendState) -> u64 {
    if state.file_size == 0 {
        return 100;
    }
    (state.bytes_sent.min(state.file_size) * 100) / state.file_size
}
