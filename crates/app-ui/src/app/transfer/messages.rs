//! 收到的文字消息：排队，主窗口一次弹一条（官方的接收页也是一次一条）。
//!
//! 服务收到就回了 204，这里只管显示：复制、链接用浏览器打开、关掉看下一条。

use iced::Task;
use local_send::DeviceInfo;

use super::super::FileBrowser;
use super::TransferMessage;
use crate::model::Message;

/// 通知里放多少字：够认出是哪段，不把长文整段塞进通知。
const NOTIFICATION_PREVIEW_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncomingMessage {
    /// 发送方的名字：收藏里起过名的用收藏名。
    pub(crate) alias: String,
    pub(crate) text: String,
    /// 复制过：按钮变"已复制"。
    pub(crate) copied: bool,
}

impl FileBrowser {
    pub(super) fn accept_incoming_message(
        &mut self,
        device: DeviceInfo,
        text: String,
    ) -> Task<Message> {
        let alias = self.transfer_device_name(&device);
        // 已经在显示别的消息：排在后面，主窗口状态已经清过了。
        let modal = if self.incoming_messages.is_empty() {
            self.prepare_transfer_modal()
        } else {
            Task::none()
        };
        // 前台不通知（与接收完成通知同一门控）：弹窗本身就看得到。
        let notification = if self.system_focused_window.is_none() {
            let preview: String = text.chars().take(NOTIFICATION_PREVIEW_CHARS).collect();
            crate::commands::publish_desktop_notification_command(
                tr!("%1 sent a message", alias),
                preview,
            )
        } else {
            Task::none()
        };
        self.incoming_messages.push_back(IncomingMessage {
            alias,
            text,
            copied: false,
        });
        Task::batch([modal, notification])
    }

    pub(super) fn copy_incoming_message(&mut self) -> Task<Message> {
        let Some(message) = self.incoming_messages.front_mut() else {
            return Task::none();
        };
        message.copied = true;
        iced::clipboard::write(message.text.clone())
    }

    pub(super) fn open_incoming_message_link(&mut self) -> Task<Message> {
        let Some(link) = self
            .incoming_messages
            .front()
            .and_then(|message| message_link(&message.text))
        else {
            return Task::none();
        };
        let link = link.to_owned();
        Task::perform(
            async move {
                desktop_linux::open_url(&link)
                    .await
                    .map_err(|error| error.to_string())
            },
            |outcome| Message::Transfer(TransferMessage::IncomingMessageLinkFailed(outcome.err())),
        )
    }

    /// 关掉这条（关闭按钮、Esc、点外面），下一条接着显示。
    pub(in crate::app) fn close_incoming_message(&mut self) {
        self.incoming_messages.pop_front();
    }
}

/// 整段（去掉首尾空白）就是一个 http/https 链接才给"打开链接"（官方：没有空白、是
/// 绝对 URI）。只认 http/https：别人发来的 `file:`、`smb:` 之类不该一点就打开。
pub(crate) fn message_link(text: &str) -> Option<&str> {
    let candidate = text.trim();
    if candidate.contains(char::is_whitespace) {
        return None;
    }
    let url = url::Url::parse(candidate).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(candidate)
}

#[cfg(test)]
mod tests;
