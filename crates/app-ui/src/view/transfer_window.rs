//! 发送到手机窗口：二维码、下载状态/进度、LocalSend 设备直推列表。

use iced::widget::{button, column, container, image, row, scrollable, Space};
use iced::{Alignment, Element, Length};

use crate::app::transfer::{
    DirectSendState, TransferMessage, TransferSessionPhase, TransferSessionState,
};
use crate::app::FileBrowser;
use crate::appearance::{context_menu_button_style, context_menu_style};
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::localization::translate_current;
use crate::model::Message;
use crate::typography::readable_text;
use local_send::DeviceInfo;

use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const QR_DISPLAY_SIZE: f32 = 220.0;
const TRANSFER_CARD_SPACING: f32 = 12.0;
const TRANSFER_WINDOW_PADDING: f32 = 16.0;
const DIRECT_SEND_STATUS_MAX_CHARS: usize = 72;
const DEVICE_LIST_MAX_HEIGHT: f32 = 180.0;

/// 从下载链接提取显示用地址段（host:port），列表项不展示完整 token 链接。
fn display_address(url: &str) -> &str {
    url.strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
}

pub(crate) fn view_transfer_window(browser: &FileBrowser) -> Element<'_, Message> {
    let mut content = column![].spacing(TRANSFER_CARD_SPACING).width(Length::Fill);
    match &browser.transfer_session {
        Some(session) => {
            content = content.push(session_card(session));
            content = content.push(device_list_card(browser));
            if let Some(direct_send) = &session.direct_send {
                content = content.push(direct_send_card(direct_send));
            }
        }
        // 窗口 id 存在但会话已清空：渲染占位避免空白（窗口规范要求）。
        None => {
            content = content.push(status_line("Preparing download link..."));
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
            card = card.push(status_line("Preparing download link..."));
        }
        TransferSessionPhase::Failed(error) => {
            card = card
                .push(status_line("Could not start download service"))
                .push(readable_text(error.clone()).size(12));
        }
        TransferSessionPhase::Ready { urls, selected, qr } => {
            match (selected.as_ref(), qr.as_ref()) {
                (Some(index), Some(qr)) => {
                    // 已选地址：显示对应二维码；finished 后二维码无意义。
                    if session.finished.is_none() {
                        card = card.push(
                            container(
                                image(qr.clone())
                                    .filter_method(image::FilterMethod::Nearest)
                                    .width(Length::Fixed(QR_DISPLAY_SIZE))
                                    .height(Length::Fixed(QR_DISPLAY_SIZE)),
                            )
                            .width(Length::Fill)
                            .center_x(Length::Fill),
                        );
                    }
                    if let Some(url) = urls.get(*index) {
                        card = card.push(
                            readable_text(translate_current(&format!(
                                "Scan the code or open the link on your phone: {url}"
                            )))
                            .size(11),
                        );
                    }
                    card = card.push(
                        button(readable_text("Choose another address").size(12))
                            .on_press(Message::Transfer(TransferMessage::QrAddressListRequested))
                            .padding([5, 12])
                            .style(context_menu_button_style()),
                    );
                }
                _ => {
                    // 未选地址：展示本机全部候选地址，点击后才生成二维码。
                    card = card.push(status_line("Choose an address your phone can reach"));
                    for (index, url) in urls.iter().enumerate() {
                        let address = display_address(url).to_owned();
                        card = card.push(
                            button(readable_text(address.clone()).size(12).width(Length::Fill))
                                .on_press(Message::Transfer(TransferMessage::QrAddressSelected(
                                    index,
                                )))
                                .padding([6, 10])
                                .width(Length::Fill)
                                .style(context_menu_button_style()),
                        );
                    }
                }
            }
            card = card.push(match session.finished {
                // finished 只会记录终态事件；Connected/Progress 不会进入该槽。
                Some(local_send::QrEvent::TimedOut) => status_line("Transfer timed out"),
                Some(_) => status_line("Transfer finished"),
                None if session.connected => status_line("Connected. Sending files..."),
                None => status_line("Waiting for a phone to scan..."),
            });
            if session.bytes_sent > 0 {
                card = card.push(
                    readable_text(crate::localization::transfer_progress_line(
                        session.bytes_sent,
                        session.speed_bytes_per_second,
                        session.finished.is_some(),
                    ))
                    .size(12),
                );
            }
        }
    }
    card_container(card)
}

/// LocalSend 设备直推列表卡片。
fn device_list_card(browser: &FileBrowser) -> Element<'_, Message> {
    let mut list = column![].spacing(4).width(Length::Fill);
    let devices = browser
        .transfer_runtime
        .as_ref()
        .map(|runtime| runtime.devices.as_slice())
        .unwrap_or(&[]);
    if devices.is_empty() {
        list = list.push(status_line("No nearby devices found"));
    } else {
        for device in devices {
            list = list.push(device_row(device));
        }
    }
    // 标题 + 手动刷新：立即清空快照并触发一轮子网扫描（不等 10s 周期）。
    let header = row![
        readable_text("Nearby LocalSend devices")
            .size(13)
            .width(Length::Fill),
        button(themed_icon(
            IconSymbol::Refresh,
            IconTone::Normal,
            MENU_ICON_SIZE
        ))
        .on_press(Message::Transfer(TransferMessage::RefreshDevicesRequested,))
        .padding([4, 8])
        .style(context_menu_button_style()),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let content = column![
        header,
        container(scrollable(list))
            .height(Length::Fixed(DEVICE_LIST_MAX_HEIGHT.min(
                // 设备少时不占满固定高度，避免滚动区空白。
                (devices.len() as f32 + 1.0) * 32.0
            )))
            .width(Length::Fill),
    ]
    .spacing(8)
    .width(Length::Fill);
    card_container(content)
}

fn device_row(device: &DeviceInfo) -> Element<'_, Message> {
    let mut subtitle = String::new();
    if let Some(model) = &device.device_model {
        subtitle.push_str(model);
    }
    if let Some(kind) = &device.device_type {
        if !subtitle.is_empty() {
            subtitle.push_str(" · ");
        }
        subtitle.push_str(kind);
    }
    // 设备名是用户内容，不做本地化；副行仅展示协议自报型号/类型。
    let label = if subtitle.is_empty() {
        device.alias.clone()
    } else {
        format!("{alias} ({subtitle})", alias = device.alias)
    };
    let send_button = button(readable_text("Send").size(12))
        .on_press(Message::Transfer(TransferMessage::SendToDevicePressed(
            device.clone(),
        )))
        .padding([5, 12])
        .style(context_menu_button_style());
    row![
        readable_text(label).size(12).width(Length::Fill),
        send_button,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn direct_send_card(state: &DirectSendState) -> Element<'_, Message> {
    let detail: String = match &state.outcome {
        Some(Ok(())) => translate_current(&format!("Sent to {}", state.alias)),
        Some(Err(error)) => translate_current(&format!("Send to {} failed: {error}", state.alias)),
        None if state.total_files > 0 => crate::localization::direct_send_progress_line(
            state.alias.clone(),
            state.file_name.clone(),
            state.file_index,
            state.total_files,
            state.bytes_sent,
            state.file_size,
        ),
        None => translate_current(&format!("Sending to {}...", state.alias)),
    };
    card_container(
        row![
            themed_icon(IconSymbol::Download, IconTone::Normal, MENU_ICON_SIZE),
            readable_text(format_middle_ellipsized_text(
                &detail,
                DIRECT_SEND_STATUS_MAX_CHARS
            ))
            .size(12)
            .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
}

fn status_line(label: &'static str) -> Element<'static, Message> {
    row![
        Space::new().width(Length::Fill),
        readable_text(label).size(13),
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
