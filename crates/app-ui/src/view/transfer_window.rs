//! 发送到手机窗口：二维码、下载状态/进度、LocalSend 设备直推列表。

use iced::widget::{column, container, row, scrollable, Space};
use iced::{Element, Length};

use crate::app::transfer::{
    DeviceListOwner, TransferMessage, TransferSessionPhase, TransferSessionState,
};
use crate::app::FileBrowser;
use crate::appearance::context_menu_style;
use crate::formatting::format_file_size;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use local_send::DeviceInfo;

use super::local_send_devices;
use super::transfer_qr::qr_links_view;

const TRANSFER_CARD_SPACING: f32 = 12.0;
const TRANSFER_WINDOW_PADDING: f32 = 16.0;
const DEVICE_LIST_MAX_HEIGHT: f32 = 220.0;
/// 设备行含名字、型号 · IP、状态三行文字。
const DEVICE_ROW_HEIGHT: f32 = 56.0;

pub(crate) fn view_transfer_window(browser: &FileBrowser) -> Element<'_, Message> {
    let mut content = column![].spacing(TRANSFER_CARD_SPACING).width(Length::Fill);
    match &browser.transfer_session {
        Some(session) => {
            content = content.push(session_card(session));
            content = content.push(device_list_card(browser));
        }
        // 窗口 id 存在但会话已清空：渲染占位避免空白（窗口规范要求）。
        None => {
            content = content.push(status_line(tr!("Preparing download link...")));
        }
    }
    container(content)
        .padding(TRANSFER_WINDOW_PADDING)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// 二维码/状态卡片。
fn session_card(session: &TransferSessionState) -> Element<'_, Message> {
    let mut card = column![].spacing(10).width(Length::Fill);
    match &session.phase {
        TransferSessionPhase::Preparing => {
            card = card.push(status_line(tr!("Preparing download link...")));
        }
        TransferSessionPhase::Failed(error) => {
            card = card
                .push(status_line(tr!("Could not start download service")))
                .push(readable_text(error.clone()));
        }
        TransferSessionPhase::Ready(links) => {
            // 传完或超时后服务已停，链接没用了。
            if session.finished.is_none() {
                card = card.push(qr_links_view(
                    links,
                    |url| tr!("Scan the code or open the link on your phone: %1", url),
                    TransferMessage::QrAddressSelected,
                    TransferMessage::QrAddressListRequested,
                ));
            }
            card = card.push(match session.finished {
                // finished 只会记录终态事件；Connected/Progress 不会进入该槽。
                Some(local_send::QrEvent::TimedOut) => status_line(tr!("Transfer timed out")),
                Some(_) => status_line(tr!("Transfer finished")),
                None if session.connected => status_line(tr!("Connected. Sending files...")),
                None => status_line(tr!("Waiting for a phone to scan...")),
            });
            if session.bytes_sent > 0 {
                card = card.push(readable_text(qr_progress_line(session)));
            }
        }
    }
    card_container(card)
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

/// LocalSend 设备直推列表卡片：发的是打开窗口时那批文件（会话非空才会打开窗口）。
/// 有收藏时先列收藏；"按 IP 添加"只在 LocalSend 窗口里。
fn device_list_card(browser: &FileBrowser) -> Element<'_, Message> {
    let rows = local_send_devices::DeviceRows {
        browser,
        owner: DeviceListOwner::TransferWindow,
        send: |device: &DeviceInfo| {
            Some(Message::Transfer(TransferMessage::SendToDevicePressed(
                device.clone(),
            )))
        },
    };
    let favorite_count = browser.user_config().transfer_favorite_devices.len();
    let mut sections = column![].spacing(8).width(Length::Fill);
    let mut row_count = browser.unclaimed_nearby_devices().len().max(1) + 1;
    if favorite_count > 0 {
        sections = sections
            .push(local_send_devices::favorites_title())
            .push(local_send_devices::favorite_rows(&rows));
        row_count += favorite_count + 1;
    }
    let sections = sections
        .push(local_send_devices::nearby_devices_header())
        .push(local_send_devices::nearby_rows(&rows));
    // 设备少时不占满固定高度，避免滚动区空白。
    let content = container(scrollable(sections))
        .height(Length::Fixed(
            DEVICE_LIST_MAX_HEIGHT.min(row_count as f32 * DEVICE_ROW_HEIGHT),
        ))
        .width(Length::Fill);
    // 会话非空才会打开窗口：总有文件可发。
    let checked_bar =
        local_send_devices::checked_bar(browser, DeviceListOwner::TransferWindow, true);
    card_container(column![].push(checked_bar).push(content).spacing(8))
}

fn status_line(label: String) -> Element<'static, Message> {
    row![
        Space::new().width(Length::Fill),
        readable_text(label),
        Space::new().width(Length::Fill),
    ]
    .width(Length::Fill)
    .into()
}

/// 卡片外壳：面板样式与右键菜单一致（surface 背景 + 边框）。
fn card_container<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content.into())
        .padding(12)
        .width(Length::Fill)
        .style(context_menu_style)
        .into()
}
