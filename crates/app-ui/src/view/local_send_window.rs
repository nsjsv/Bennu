//! LocalSend 窗口：打开就能发。上面选要发的（文件或文字），中间点一台设备就发过去，
//! 底下是本机能不能被找到。传输记录、从手机接收在顶栏两边。

use iced::widget::{button, column, container, row, scrollable, tooltip, Column, Space};
use iced::{Alignment, Background, Border, Element, Length, Theme};

use crate::app::transfer::{
    receiver_failure_message, ChooserTarget, DeviceListOwner, LinkReceiveMessage, LocalSendPage,
    StagedItem, StagedSize, StagedTab, TransferMessage,
};
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::directory_summary::RecursiveTotalSize;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_preview::window_shell::HeaderBar;
use bennu_theme::controls::{
    icon_button, icon_button_style, normal_button, switch, text_area, toggle_button_style,
    tooltip_bubble_style,
};
use bennu_theme::icons::IconSymbol;
use bennu_theme::metrics::CARD_RADIUS;
use bennu_theme::typography::{BOLD, CAPTION_SIZE};
use bennu_theme::ui_colors;

use super::settings_group::muted_setting_text;
use super::{
    link_receive, local_send_address_form, local_send_devices, local_send_tiles, transfer_history,
};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const WINDOW_PADDING: f32 = 16.0;
const SECTION_SPACING: f32 = 16.0;
const PANEL_PADDING: f32 = 14.0;
const HEADER_ICON_SIZE: f32 = 16.0;
const EMPTY_ICON_SIZE: f32 = 28.0;
const STAGED_LIST_MAX_HEIGHT: f32 = 132.0;
const STAGED_ROW_HEIGHT: f32 = 26.0;
const STAGED_NAME_MAX_CHARS: usize = 40;
const STAGED_TEXT_HEIGHT: f32 = 96.0;
const FOOTER_ICON_SIZE: f32 = 18.0;

/// 顶栏：设备页左边是“传输记录”，右边是“从手机接收”（开着时高亮，再点就停）；
/// 记录页左边是后退。
pub(crate) fn local_send_header(browser: &FileBrowser) -> HeaderBar<'static, Message> {
    let page = |target| Message::Transfer(TransferMessage::LocalSendPageSelected(target));
    match browser.local_send_page {
        LocalSendPage::History => HeaderBar::new()
            .title(tr!("Transfer history"))
            .start(header_button(
                IconSymbol::ArrowLeft,
                tr!("Back"),
                Some(page(LocalSendPage::Devices)),
                false,
            ))
            .end(header_button(
                IconSymbol::Trash,
                tr!("Clear history"),
                (!browser.transfer_history.records.is_empty())
                    .then_some(Message::Transfer(TransferMessage::HistoryClearPressed)),
                false,
            )),
        LocalSendPage::Devices => {
            let receiving_from_phone = browser.link_receiving.current.is_some();
            let phone = if receiving_from_phone {
                LinkReceiveMessage::StopPressed
            } else {
                LinkReceiveMessage::StartPressed
            };
            HeaderBar::new()
                .title(tr!("LocalSend"))
                .start(header_button(
                    IconSymbol::History,
                    tr!("Transfer history"),
                    Some(page(LocalSendPage::History)),
                    false,
                ))
                .end(header_button(
                    IconSymbol::QrCode,
                    tr!("Receive from phone"),
                    Some(Message::Transfer(TransferMessage::LinkReceive(phone))),
                    receiving_from_phone,
                ))
        }
    }
}

fn header_button(
    symbol: IconSymbol,
    label: String,
    message: Option<Message>,
    active: bool,
) -> Element<'static, Message> {
    let tone = if active {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let button = icon_button(themed_icon(symbol, tone, HEADER_ICON_SIZE)).on_press_maybe(message);
    let button = if active {
        button.style(toggle_button_style(true))
    } else {
        button
    };
    tooltip(
        button,
        container(readable_text(label).size(CAPTION_SIZE))
            .padding([5, 7])
            .style(tooltip_bubble_style),
        tooltip::Position::Bottom,
    )
    .into()
}

pub(crate) fn view_local_send_window(browser: &FileBrowser) -> Element<'_, Message> {
    match browser.local_send_page {
        LocalSendPage::History => scrollable(
            container(transfer_history::history_list(browser))
                .padding(WINDOW_PADDING)
                .width(Length::Fill),
        )
        .height(Length::Fill)
        .into(),
        LocalSendPage::Devices => {
            let mut body = column![]
                .spacing(SECTION_SPACING)
                .padding(WINDOW_PADDING)
                .width(Length::Fill);
            if browser.link_receiving.current.is_some() {
                body = body.push(panel(
                    column![
                        readable_text(tr!("Receive from phone")).font(BOLD),
                        link_receive::link_receive_section(browser),
                    ]
                    .spacing(10),
                ));
            }
            body = body
                .push(outgoing_panel(browser))
                .push(devices_section(browser));
            column![
                scrollable(body).height(Length::Fill),
                this_computer_footer(browser)
            ]
            .into()
        }
    }
}

/// 要发什么：文件（默认）或一段文字。没选文件时是一块大的“选择文件”。
fn outgoing_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let content = match browser.local_send_tab {
        StagedTab::Files if browser.local_send_staged.is_empty() => empty_files(),
        StagedTab::Files => staged_files(&browser.local_send_staged),
        StagedTab::Text => staged_text(browser),
    };
    panel(content)
}

fn empty_files() -> Element<'static, Message> {
    column![
        themed_icon(IconSymbol::Send, IconTone::Normal, EMPTY_ICON_SIZE),
        readable_text(tr!("Choose what to send")).font(BOLD),
        choose_buttons(),
        local_send_tiles::centered_muted_text(tr!(
            "Drag files onto LocalSend in the sidebar, or choose them here."
        )),
        switch_tab_link(tr!("Send text instead"), StagedTab::Text),
    ]
    .spacing(8)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn staged_files(staged: &[StagedItem]) -> Element<'_, Message> {
    let clear = normal_button(readable_text(tr!("Clear list")))
        .on_press(Message::Transfer(TransferMessage::StagedItemsCleared));
    column![
        row![
            readable_text(tr!(
                "%1 item(s) · %2",
                staged.len(),
                staged_total_text(staged)
            ))
            .font(BOLD)
            .width(Length::Fill),
            clear,
        ]
        .align_y(Alignment::Center),
        container(scrollable(staged_rows(staged)))
            .height(Length::Fixed(
                STAGED_LIST_MAX_HEIGHT.min(staged.len() as f32 * STAGED_ROW_HEIGHT),
            ))
            .width(Length::Fill),
        row![
            choose_buttons(),
            Space::new().width(Length::Fill),
            switch_tab_link(tr!("Send text instead"), StagedTab::Text),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(8)
    .into()
}

fn choose_buttons() -> Element<'static, Message> {
    let choose = |label: String, target: ChooserTarget| {
        normal_button(readable_text(label))
            .on_press(Message::Transfer(TransferMessage::ChooserPressed(target)))
    };
    row![
        choose(tr!("Choose files..."), ChooserTarget::Files),
        choose(tr!("Choose folders..."), ChooserTarget::Folders),
    ]
    .spacing(8)
    .into()
}

/// 要发的文字：多行，发完还在，可以接着发给别人。
fn staged_text(browser: &FileBrowser) -> Element<'_, Message> {
    let editor = text_area(&browser.local_send_text)
        .placeholder(tr!("Type the text to send"))
        .on_action(|action| Message::Transfer(TransferMessage::StagedTextEdited(action)))
        .height(Length::Fixed(STAGED_TEXT_HEIGHT));
    let clear = normal_button(readable_text(tr!("Clear"))).on_press_maybe(
        (!browser.local_send_text.text().is_empty())
            .then_some(Message::Transfer(TransferMessage::StagedTextCleared)),
    );
    column![
        editor,
        row![
            switch_tab_link(tr!("Send files instead"), StagedTab::Files),
            Space::new().width(Length::Fill),
            clear,
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(8)
    .into()
}

/// 文件和文字之间切换：一个不起眼的文字按钮，不占一整行分段选择。
fn switch_tab_link(label: String, target: StagedTab) -> Element<'static, Message> {
    button(
        readable_text(label)
            .size(CAPTION_SIZE)
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(ui_colors(theme).primary),
            }),
    )
    .padding([4, 6])
    .style(icon_button_style)
    .on_press(Message::Transfer(TransferMessage::StagedTabSelected(
        target,
    )))
    .into()
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
        let remove = icon_button(themed_icon(
            IconSymbol::Close,
            IconTone::Normal,
            MENU_ICON_SIZE,
        ))
        .on_press(Message::Transfer(TransferMessage::StagedItemRemoved(
            item.path.clone(),
        )));
        rows = rows.push(
            row![
                readable_text(format_middle_ellipsized_text(&name, STAGED_NAME_MAX_CHARS))
                    .width(Length::Fill),
                readable_text(staged_size_text(&item.size))
                    .size(CAPTION_SIZE)
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

/// 设备：标题 + 刷新，勾了几台时的“一起发”，设备格子，按 IP 添加的表单，正在发的那几台。
fn devices_section(browser: &FileBrowser) -> Element<'_, Message> {
    let refresh = icon_button(themed_icon(
        IconSymbol::Refresh,
        IconTone::Normal,
        MENU_ICON_SIZE,
    ))
    .on_press(Message::Transfer(TransferMessage::RefreshDevicesRequested));
    let mut section = column![row![
        readable_text(tr!("Devices")).font(BOLD).width(Length::Fill),
        refresh,
    ]
    .align_y(Alignment::Center)]
    .spacing(10)
    .width(Length::Fill);

    let any_device =
        !browser.favorite_rows().is_empty() || !browser.unclaimed_nearby_devices().is_empty();
    section = section.push(muted_setting_text(if !any_device {
        tr!("Looking for nearby devices. The other device needs LocalSend open on the same network.")
    } else if browser.local_send_has_outgoing() {
        tr!("Click a device to send. Hold Ctrl to pick several.")
    } else {
        tr!("Choose files or type some text, then click a device.")
    }));
    if let Some(bar) = local_send_devices::checked_bar(
        browser,
        DeviceListOwner::LocalSendWindow,
        browser.local_send_has_outgoing(),
    ) {
        section = section.push(bar);
    }
    section = section.push(local_send_tiles::device_grid(browser));
    if let Some(form) = &browser.favorite_address_form {
        section = section.push(panel(local_send_address_form::address_form(form)));
    }
    if let Some(active) = local_send_tiles::active_sends(browser) {
        section = section.push(panel(active));
    }
    section.into()
}

/// 底栏：本机名、能不能被找到（或接收服务为什么没起来），右边是接收开关（与设置页同一个
/// 状态）。
fn this_computer_footer(browser: &FileBrowser) -> Element<'_, Message> {
    let receiving = browser.user_config().transfer_receive_enabled;
    let status = if let Some(failure) = browser.transfer_receiver_failure() {
        error_text(receiver_failure_message(failure))
    } else if browser.transfer_runtime.is_none() {
        error_text(tr!("LocalSend is not running."))
    } else if receiving {
        muted_setting_text(tr!("Other devices can find this computer."))
    } else {
        muted_setting_text(tr!(
            "Receiving is off. Other devices cannot find this computer."
        ))
    };
    let toggle = button(switch(receiving))
        .padding(0)
        .style(|_: &Theme, _| button::Style::default())
        .on_press(Message::Transfer(TransferMessage::ReceiveEnabledToggled));
    let content = row![
        themed_icon(IconSymbol::Monitor, IconTone::Normal, FOOTER_ICON_SIZE),
        column![
            readable_text(tr!(
                "This computer: %1",
                browser.effective_transfer_device_alias()
            )),
            status,
        ]
        .spacing(1)
        .width(Length::Fill),
        toggle,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    container(content)
        .padding([10.0, WINDOW_PADDING])
        .width(Length::Fill)
        .style(footer_style)
        .into()
}

pub(super) fn error_text(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(CAPTION_SIZE)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_colors(theme).error),
        })
        .into()
}

/// 一块内容：卡片底色，没有边框。
fn panel<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content.into())
        .padding(PANEL_PADDING)
        .width(Length::Fill)
        .style(panel_style)
        .into()
}

fn panel_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).surface_container)),
        border: Border {
            radius: CARD_RADIUS.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 底栏比内容暗一层（和侧栏同一层），不用分隔线也和上面分得开。
fn footer_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).surface_dim)),
        ..container::Style::default()
    }
}
