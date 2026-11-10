//! 任务面板里 LocalSend 收发的一行：与文件任务同一种卡片，标题、状态、进度条、
//! "已传 / 总量 · 速度 · 第几个"，结束后列出没成的文件；取消、清除。

use iced::widget::{button, column, container, progress_bar, row, Svg};
use iced::{Alignment, Element, Length};

use crate::app::transfer::{
    DirectSendFailure, DirectSendPhase, DirectSendState, ReceivePhase, ReceiveTask, TransferKind,
    TransferMessage, TransferTask, TransferTasks,
};
use crate::appearance::{
    context_menu_button_style, error_notification_style, path_suggestion_item_style,
};
use crate::formatting::{format_file_size, format_middle_ellipsized_text};
use crate::model::Message;
use bennu_preview::operation_progress::active_indeterminate_track_handle;
use bennu_preview::panel_text::readable_text;

const TITLE_ITEM_MAX_CHARS: usize = 24;
const FAILURE_MAX_CHARS: usize = 80;
/// 失败的文件最多列几行：多了只说还有几个。
const FAILURES_SHOWN: usize = 5;

pub(crate) fn transfer_task_row<'a>(
    tasks: &TransferTasks,
    task: &'a TransferTask,
    animation_frame: u8,
) -> Element<'a, Message> {
    let fraction = tasks.fraction(task);
    let (title, status, detail, failures) = match &task.kind {
        TransferKind::Send(send) => send_lines(send, fraction),
        TransferKind::Receive(receive) => receive_lines(receive),
    };
    let header = row![
        readable_text(title).size(13).width(Length::Fill),
        readable_text(status).size(11),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let mut body = column![header].spacing(4).width(Length::Fill);
    if !task.is_terminal() {
        let progress: Element<'_, Message> = match fraction {
            Some(fraction) => progress_bar(0.0..=1.0, fraction)
                .girth(Length::Fixed(3.0))
                .into(),
            None => Svg::new(active_indeterminate_track_handle(animation_frame))
                .width(Length::Fill)
                .height(Length::Fixed(4.0))
                .into(),
        };
        body = body.push(progress);
    }
    if let Some(detail) = detail {
        body = body.push(readable_text(detail).size(11).width(Length::Fill));
    }
    for failure in failures {
        body = body.push(readable_text(failure).size(11).width(Length::Fill));
    }
    let content = column![body, controls(task)].spacing(6).width(Length::Fill);
    let item = container(content).padding(8).width(Length::Fill);
    if task.is_failed() {
        item.style(error_notification_style).into()
    } else {
        item.style(path_suggestion_item_style).into()
    }
}

type Lines = (String, String, Option<String>, Vec<String>);

fn send_lines(send: &DirectSendState, fraction: Option<f32>) -> Lines {
    // 设备名、文件名是用户内容，不做本地化。
    let title = with_items(
        tr!("Send to %1", send.target.alias),
        &send.first_item,
        send.item_count,
    );
    let progress = || {
        let mut parts = vec![format!(
            "{} / {}",
            format_file_size(send.bytes_done),
            format_file_size(send.bytes_total)
        )];
        if send.speed.bytes_per_second > 0 {
            parts.push(tr!("%1/s", format_file_size(send.speed.bytes_per_second)));
        }
        parts.push(tr!(
            "file %1 of %2",
            // 协议层的序号从 0 开始。
            send.file_index + 1,
            send.total_files
        ));
        parts.join(" · ")
    };
    let (status, detail, failures) = match &send.phase {
        DirectSendPhase::Checking => (
            tr!("Verifying"),
            Some(tr!(
                "Checking files %1%",
                (fraction.unwrap_or(0.0) * 100.0) as u64
            )),
            Vec::new(),
        ),
        DirectSendPhase::Sending if send.bytes_total == 0 => (
            tr!("Waiting"),
            Some(tr!("Waiting for the device to accept...")),
            Vec::new(),
        ),
        DirectSendPhase::Sending => (tr!("Sending"), Some(progress()), Vec::new()),
        DirectSendPhase::NeedsPin { .. } => (
            tr!("Needs PIN"),
            Some(tr!("Enter the PIN in the LocalSend window.")),
            Vec::new(),
        ),
        DirectSendPhase::Canceling => (tr!("Canceling"), None, Vec::new()),
        DirectSendPhase::Canceled => (tr!("Canceled"), None, Vec::new()),
        DirectSendPhase::Finished { sent, failed, .. } => (
            if failed.is_empty() {
                tr!("Completed")
            } else {
                tr!("Failed")
            },
            Some(tr!("%1 sent, %2 failed", sent, failed.len())),
            failure_lines(
                failed
                    .iter()
                    .map(|failure| (failure.file_name.as_str(), failure.reason.as_str())),
            ),
        ),
        DirectSendPhase::Failed(failure) => (
            tr!("Failed"),
            Some(match failure {
                DirectSendFailure::Declined => tr!("The device declined."),
                DirectSendFailure::TooManyPinAttempts => tr!(
                    "Too many wrong PINs. The device no longer accepts files from this computer for now."
                ),
                DirectSendFailure::NothingToSend => {
                    tr!("Nothing to send. Empty folders cannot be sent.")
                }
                DirectSendFailure::Other(reason) => reason.clone(),
            }),
            Vec::new(),
        ),
    };
    (title, status, detail, failures)
}

fn receive_lines(receive: &ReceiveTask) -> Lines {
    let first = receive
        .files
        .first()
        .map(|file| file.file_name.clone())
        .unwrap_or_default();
    let title = with_items(
        tr!("Receive from %1", receive.sender),
        &first,
        receive.files.len(),
    );
    let (status, detail, failures) = match &receive.phase {
        ReceivePhase::Receiving => {
            let mut parts = vec![format!(
                "{} / {}",
                format_file_size(receive.bytes_done()),
                format_file_size(receive.bytes_total())
            )];
            if receive.speed.bytes_per_second > 0 {
                parts.push(tr!(
                    "%1/s",
                    format_file_size(receive.speed.bytes_per_second)
                ));
            }
            parts.push(tr!(
                "%1 of %2 files",
                receive.files_done(),
                receive.files.len()
            ));
            (tr!("Receiving"), Some(parts.join(" · ")), Vec::new())
        }
        ReceivePhase::Canceling => (tr!("Canceling"), None, Vec::new()),
        ReceivePhase::Canceled { saved } => (
            tr!("Canceled"),
            Some(tr!("%1 file(s) kept", saved)),
            Vec::new(),
        ),
        ReceivePhase::Finished { saved, failed } => (
            if failed.is_empty() {
                tr!("Completed")
            } else {
                tr!("Failed")
            },
            Some(tr!("%1 received, %2 failed", saved, failed.len())),
            failure_lines(
                failed
                    .iter()
                    .map(|(name, reason)| (name.as_str(), reason.as_str())),
            ),
        ),
    };
    (title, status, detail, failures)
}

/// "发送到 小米手机 - 照片 (+3)"：同文件任务的标题写法。
fn with_items(action: String, first: &str, count: usize) -> String {
    if first.is_empty() {
        return action;
    }
    let first = format_middle_ellipsized_text(first, TITLE_ITEM_MAX_CHARS);
    match count.saturating_sub(1) {
        0 => format!("{action} - {first}"),
        more => format!("{action} - {first} (+{more})"),
    }
}

/// 文件名 + 英文原因（与日志一致）；多了只说还有几个。
fn failure_lines<'a>(failures: impl ExactSizeIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let total = failures.len();
    let mut lines: Vec<String> = failures
        .take(FAILURES_SHOWN)
        .map(|(name, reason)| {
            format_middle_ellipsized_text(&format!("{name}  {reason}"), FAILURE_MAX_CHARS)
        })
        .collect();
    if total > FAILURES_SHOWN {
        lines.push(tr!("and %1 more", total - FAILURES_SHOWN));
    }
    lines
}

fn controls(task: &TransferTask) -> Element<'static, Message> {
    let mut controls = row![].spacing(6).align_y(Alignment::Center);
    let can_cancel = match &task.kind {
        TransferKind::Send(send) => matches!(
            send.phase,
            DirectSendPhase::Checking | DirectSendPhase::Sending | DirectSendPhase::NeedsPin { .. }
        ),
        TransferKind::Receive(receive) => receive.phase == ReceivePhase::Receiving,
    };
    if can_cancel {
        controls = controls.push(control_button(
            tr!("Cancel"),
            TransferMessage::TaskCancelRequested(task.id),
        ));
    }
    if task.is_terminal() {
        controls = controls.push(control_button(
            tr!("Clear"),
            TransferMessage::TaskClearRequested(task.id),
        ));
    }
    controls.into()
}

fn control_button(label: String, message: TransferMessage) -> Element<'static, Message> {
    button(readable_text(label).size(12))
        .on_press(Message::Transfer(message))
        .padding([4, 8])
        .style(context_menu_button_style())
        .into()
}
