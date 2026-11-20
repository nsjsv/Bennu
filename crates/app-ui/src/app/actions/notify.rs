//! 动作结束时的通知：每个动作三选一（默认 / 自己写 / 不通知）。动作里各步自己的
//! "X 完成"通知一律不发，由任务结束收口拦下。

use iced::Task;

use super::run::{FinishedRun, RunStop};
use crate::actions::{ActionNotice, NoticeValues};
use crate::app::file_operation_notifications::{
    bounded_notification_failure_reason, NOTIFICATION_FILE_NAME_MAX_CHARS,
};
use crate::app::FileBrowser;
use crate::commands::publish_desktop_notification_command;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;

/// 通知的标题和正文；`None` 是不发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActionNotification {
    pub(super) summary: String,
    pub(super) body: String,
}

impl FileBrowser {
    pub(super) fn action_notification_command(&self, run: &FinishedRun) -> Task<Message> {
        let window_in_front = self.system_focused_window.is_some();
        match action_notification(run, window_in_front) {
            Some(notification) => {
                publish_desktop_notification_command(notification.summary, notification.body)
            }
            None => Task::none(),
        }
    }
}

/// 默认的和其他通知一样只在窗口不在前台时弹；自己写的和失败的总是弹；用户取消的不弹。
pub(super) fn action_notification(
    run: &FinishedRun,
    window_in_front: bool,
) -> Option<ActionNotification> {
    let name = &run.definition.name;
    match (&run.definition.notice, &run.stop) {
        (ActionNotice::Silent, _) | (_, Some(RunStop::Canceled)) => None,
        (_, Some(RunStop::Failed(reason))) => Some(ActionNotification {
            summary: tr!("%1 failed", name),
            body: bounded_notification_failure_reason(reason),
        }),
        (ActionNotice::Standard, None) if window_in_front => None,
        (ActionNotice::Standard, None) => Some(ActionNotification {
            summary: tr!("%1 completed", name),
            body: standard_body(run),
        }),
        (ActionNotice::Custom { title, body }, None) => {
            let values = NoticeValues {
                action: name,
                results: &run.results,
                failed: run.failures.len(),
            };
            Some(ActionNotification {
                summary: title.render(values),
                body: body
                    .as_ref()
                    .map(|body| body.render(values))
                    .unwrap_or_default(),
            })
        }
    }
}

/// 第一个结果的名字（多个时"等 N 项"），有没做成的项再加一行。
fn standard_body(run: &FinishedRun) -> String {
    let results = match run.results.as_slice() {
        [] => String::new(),
        [only] => file_name(only),
        [first, rest @ ..] => tr!("%1 and %2 more", file_name(first), rest.len()),
    };
    let failures = match run.failures.as_slice() {
        [] => None,
        [only] => Some(bounded_notification_failure_reason(only)),
        many => Some(tr!(
            "%1 items could not be processed, for example %2",
            many.len(),
            bounded_notification_failure_reason(&many[0])
        )),
    };
    [Some(results), failures]
        .into_iter()
        .flatten()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn file_name(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    format_middle_ellipsized_text(&name, NOTIFICATION_FILE_NAME_MAX_CHARS)
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
