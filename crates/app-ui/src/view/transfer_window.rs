//! 发送到手机窗口：上面一块是发什么 + 扫码下载（二维码、状态、进度），下面是 LocalSend
//! 设备格子，点一下直接发过去。格子和 LocalSend 窗口同一套，只是没有“按 IP 添加”。

use iced::widget::{column, container, scrollable};
use iced::{Element, Length};

use crate::app::transfer::{
    DeviceListOwner, TransferMessage, TransferSessionPhase, TransferSessionState,
};
use crate::app::FileBrowser;
use crate::formatting::{format_file_size, format_middle_ellipsized_text};
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::typography::BOLD;

use super::local_send_tiles::{self, centered_muted_text};
use super::local_send_window::{error_text, panel};
use super::transfer_qr::qr_links_view;
use super::{dialog, local_send_devices};

const WINDOW_PADDING: f32 = 16.0;
const SECTION_SPACING: f32 = 16.0;
const NAME_MAX_CHARS: usize = 36;

pub(crate) fn view_transfer_window(browser: &FileBrowser) -> Element<'_, Message> {
    // 窗口 id 存在但会话已清空：渲染占位避免空白（窗口规范要求）。
    let Some(session) = &browser.transfer_session else {
        return container(centered_muted_text(tr!("Preparing download link...")))
            .padding(WINDOW_PADDING)
            .width(Length::Fill)
            .into();
    };
    let body = column![panel(session_panel(session)), devices_section(browser)]
        .spacing(SECTION_SPACING)
        .padding(WINDOW_PADDING)
        .width(Length::Fill);
    scrollable(body).height(Length::Fill).into()
}

/// 发什么 + 扫码下载：二维码只在服务还开着时给，传完或超时后链接已经没用了。
fn session_panel(session: &TransferSessionState) -> Element<'_, Message> {
    let mut content = column![readable_text(sending_summary(&session.paths)).font(BOLD)]
        .spacing(10)
        .width(Length::Fill);
    match &session.phase {
        TransferSessionPhase::Preparing => {
            content = content.push(centered_muted_text(tr!("Preparing download link...")));
        }
        TransferSessionPhase::Failed(error) => {
            content = content
                .push(error_text(tr!("Could not start download service")))
                .push(error_text(error.clone()));
        }
        TransferSessionPhase::Ready(links) => {
            if session.finished.is_none() {
                content = content.push(qr_links_view(
                    links,
                    |url| tr!("Scan the code or open the link on your phone: %1", url),
                    TransferMessage::QrAddressSelected,
                    TransferMessage::QrAddressListRequested,
                ));
            }
            content = content.push(centered_muted_text(match session.finished {
                // finished 只会记录终态事件；Connected/Progress 不会进入该槽。
                Some(local_send::QrEvent::TimedOut) => tr!("Transfer timed out"),
                Some(_) => tr!("Transfer finished"),
                None if session.connected => tr!("Connected. Sending files..."),
                None => tr!("Waiting for a phone to scan..."),
            }));
            if session.bytes_sent > 0 {
                content = content.push(centered_muted_text(qr_progress_line(session)));
            }
        }
    }
    content.into()
}

/// “notes.txt”或“notes.txt 等 3 项”：文件名是用户内容，不做本地化。
fn sending_summary(paths: &[std::path::PathBuf]) -> String {
    let Some(first) = paths.first() else {
        return String::new();
    };
    let name = format_middle_ellipsized_text(&dialog::path_name(first), NAME_MAX_CHARS);
    match paths.len() - 1 {
        0 => name,
        more => tr!("%1 and %2 more", name, more),
    }
}

fn qr_progress_line(session: &TransferSessionState) -> String {
    let sent = format_file_size(session.bytes_sent);
    if session.finished.is_some() {
        tr!("Sent %1", sent)
    } else {
        tr!(
            "Sent %1 (%2/s)",
            sent,
            format_file_size(session.speed_bytes_per_second)
        )
    }
}

/// 设备：标题、提示、勾选条，设备格子，正在发的那几台。
fn devices_section(browser: &FileBrowser) -> Element<'_, Message> {
    let owner = DeviceListOwner::TransferWindow;
    let mut section = local_send_devices::devices_section_head(browser, owner).push(
        local_send_tiles::tile_grid(local_send_tiles::device_tiles(browser, owner)),
    );
    if let Some(active) = local_send_tiles::active_sends(browser) {
        section = section.push(panel(active));
    }
    section.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_summary_names_the_first_file_and_counts_the_rest() {
        assert_eq!(
            sending_summary(&[PathBuf::from("/home/a/notes.txt")]),
            "notes.txt"
        );
        assert_eq!(
            sending_summary(&[
                PathBuf::from("/home/a/notes.txt"),
                PathBuf::from("/home/a/b.png"),
                PathBuf::from("/home/a/c"),
            ]),
            tr!("%1 and %2 more", "notes.txt", 2)
        );
    }
}
