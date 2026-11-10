//! LocalSend 窗口：本机与接收状态、待发列表、收藏设备、附近设备。

use iced::widget::{button, column, container, row, scrollable, Column};
use iced::{Alignment, Element, Length, Theme};

use crate::app::transfer::{
    receiver_failure_message, AddressFormMessage, ChooserTarget, StagedItem, StagedSize,
    TransferMessage,
};
use crate::app::FileBrowser;
use crate::appearance::{context_menu_button_style, context_menu_style, muted_text_color};
use crate::directory_summary::RecursiveTotalSize;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::icons::IconSymbol;
use bennu_theme::ui_colors;
use local_send::DeviceInfo;

use super::settings_group::{muted_setting_text, toggle_setting_row};
use super::{local_send_address_form, local_send_devices};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const WINDOW_PADDING: f32 = 16.0;
const CARD_SPACING: f32 = 12.0;
const STAGED_LIST_MAX_HEIGHT: f32 = 160.0;
const STAGED_ROW_HEIGHT: f32 = 26.0;
const STAGED_NAME_MAX_CHARS: usize = 48;

pub(crate) fn view_local_send_window(browser: &FileBrowser) -> Element<'_, Message> {
    let content = column![
        this_computer_card(browser),
        staged_card(browser),
        favorites_card(browser),
        nearby_card(browser),
    ]
    .spacing(CARD_SPACING)
    .padding(WINDOW_PADDING)
    .width(Length::Fill);
    scrollable(content).height(Length::Fill).into()
}

/// 本机名、接收开关（与设置页同一个状态）与接收状态。
fn this_computer_card(browser: &FileBrowser) -> Element<'_, Message> {
    let receiving = browser.user_config().transfer_receive_enabled;
    let status: Element<'_, Message> = if let Some(failure) = browser.transfer_receiver_failure() {
        error_text(receiver_failure_message(failure))
    } else if browser.transfer_runtime.is_none() {
        error_text(tr!("LocalSend is not running."))
    } else if receiving {
        muted_setting_text(tr!("Other devices can find this computer."), 11)
    } else {
        muted_setting_text(
            tr!("Receiving is off. Other devices cannot find this computer."),
            11,
        )
    };
    card(
        column![
            readable_text(tr!(
                "This computer: %1",
                browser.effective_transfer_device_alias()
            ))
            .size(13),
            toggle_setting_row(
                tr!("Receive files"),
                None,
                receiving,
                Message::Transfer(TransferMessage::ReceiveEnabledToggled),
            ),
            status,
        ]
        .spacing(8),
    )
}

fn staged_card(browser: &FileBrowser) -> Element<'_, Message> {
    let staged = &browser.local_send_staged;
    let mut content = column![].spacing(8).width(Length::Fill);
    if staged.is_empty() {
        content = content
            .push(readable_text(tr!("No files to send")).size(13))
            .push(muted_setting_text(
                tr!("Drag files onto LocalSend in the sidebar, or choose them here."),
                11,
            ));
    } else {
        let clear = button(readable_text(tr!("Clear list")).size(12))
            .on_press(Message::Transfer(TransferMessage::StagedItemsCleared))
            .padding([4, 10])
            .style(context_menu_button_style());
        content = content
            .push(
                row![
                    readable_text(tr!(
                        "%1 item(s) · %2",
                        staged.len(),
                        staged_total_text(staged)
                    ))
                    .size(13)
                    .width(Length::Fill),
                    clear,
                ]
                .align_y(Alignment::Center),
            )
            .push(
                container(scrollable(staged_rows(staged)))
                    .height(Length::Fixed(
                        STAGED_LIST_MAX_HEIGHT.min(staged.len() as f32 * STAGED_ROW_HEIGHT),
                    ))
                    .width(Length::Fill),
            );
    }
    let choose = |label: String, target: ChooserTarget| {
        button(readable_text(label).size(12))
            .on_press(Message::Transfer(TransferMessage::ChooserPressed(target)))
            .padding([5, 12])
            .style(context_menu_button_style())
    };
    content = content.push(
        row![
            choose(tr!("Choose files..."), ChooserTarget::Files),
            choose(tr!("Choose folders..."), ChooserTarget::Folders),
        ]
        .spacing(8),
    );
    card(content)
}

fn staged_rows(staged: &[StagedItem]) -> Column<'_, Message> {
    let mut rows = column![].spacing(2).width(Length::Fill);
    for item in staged {
        // 文件名是用户内容，不做本地化；根目录没有文件名时显示完整路径。
        let name = item
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| item.path.to_string_lossy().into_owned());
        let remove = button(themed_icon(
            IconSymbol::Close,
            IconTone::Normal,
            MENU_ICON_SIZE,
        ))
        .on_press(Message::Transfer(TransferMessage::StagedItemRemoved(
            item.path.clone(),
        )))
        .padding([2, 6])
        .style(context_menu_button_style());
        rows = rows.push(
            row![
                readable_text(format_middle_ellipsized_text(&name, STAGED_NAME_MAX_CHARS))
                    .size(12)
                    .width(Length::Fill),
                readable_text(staged_size_text(&item.size))
                    .size(11)
                    .style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(muted_text_color(theme)),
                    }),
                remove,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    rows
}

fn staged_size_text(size: &StagedSize) -> String {
    match size {
        StagedSize::Measuring => tr!("Calculating..."),
        StagedSize::Measured(size) => size.display_text(),
        StagedSize::Unreadable(_) => tr!("Unreadable"),
    }
}

/// 总大小：还有在算的显示"正在计算…"；有读不全或读不了的项时显示"≥"。
fn staged_total_text(staged: &[StagedItem]) -> String {
    let mut total = RecursiveTotalSize {
        bytes: 0,
        complete: true,
    };
    for item in staged {
        match &item.size {
            StagedSize::Measuring => return tr!("Calculating..."),
            StagedSize::Measured(size) => {
                total.bytes = total.bytes.saturating_add(size.bytes);
                total.complete &= size.complete;
            }
            StagedSize::Unreadable(_) => total.complete = false,
        }
    }
    total.display_text()
}

/// 有待发文件时点设备就发这批；没有时发送不可点。
fn send_staged(browser: &FileBrowser) -> impl Fn(&DeviceInfo) -> Option<Message> {
    let has_files = !browser.local_send_staged.is_empty();
    move |device| {
        has_files
            .then(|| Message::Transfer(TransferMessage::SendStagedToDevicePressed(device.clone())))
    }
}

/// 收藏：标题 + "按 IP 添加"（展开的表单在标题下面）+ 全部收藏设备。
fn favorites_card(browser: &FileBrowser) -> Element<'_, Message> {
    let add = button(
        row![
            themed_icon(IconSymbol::Plus, IconTone::Normal, MENU_ICON_SIZE),
            readable_text(tr!("Add by IP")).size(12),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .on_press(Message::Transfer(TransferMessage::AddressForm(
        AddressFormMessage::Opened,
    )))
    .padding([4, 10])
    .style(context_menu_button_style());
    let mut content = column![row![
        local_send_devices::favorites_title().width(Length::Fill),
        add
    ]
    .align_y(Alignment::Center)]
    .spacing(8);
    if let Some(form) = &browser.favorite_address_form {
        content = content.push(local_send_address_form::address_form(form));
    }
    content = if browser.user_config().transfer_favorite_devices.is_empty() {
        content
            .push(readable_text(tr!("No favorite devices yet")).size(12))
            .push(muted_setting_text(
                tr!("Click the star next to a device to add it here."),
                11,
            ))
    } else {
        content.push(local_send_devices::favorite_rows(
            browser,
            send_staged(browser),
        ))
    };
    card(content)
}

fn nearby_card(browser: &FileBrowser) -> Element<'_, Message> {
    card(
        column![
            local_send_devices::nearby_devices_header(),
            local_send_devices::nearby_rows(browser, send_staged(browser)),
        ]
        .spacing(8),
    )
}

fn error_text(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(11)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_colors(theme).error),
        })
        .into()
}

/// 卡片外壳：与"发送到手机"窗口同样式（surface 背景 + 边框）。
fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content.into())
        .padding(12)
        .width(Length::Fill)
        .style(context_menu_style)
        .into()
}
