//! 设备格子（LocalSend、发送到手机两个窗口共用）：大圆头像（设备类型图标）+ 名字 + 一行
//! 状态，点一下就把这个窗口要发的东西发过去。发送中头像外面一圈进度；收藏的排前面，右上角
//! 是实心星；按住 Ctrl 点是多选，勾上的头像带强调色描边。要输 PIN、能取消、失败要看详情的，
//! 在格子下面的“进行中”里处理。

use iced::widget::{button, column, container, tooltip, Row, Stack};
use iced::{alignment, Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::transfer::{
    AddressFormMessage, DeviceListOwner, DirectSendFailure, DirectSendPhase, DirectSendState,
    FavoriteRow, TransferMessage,
};
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::formatting::format_middle_ellipsized_text;
use crate::model::Message;
use crate::progress_ring::progress_ring;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{icon_button, tooltip_bubble_style};
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;
use local_send::DeviceInfo;

use super::local_send_send_status::direct_send_lines;
use super::settings_group::card_row_button_style;
use super::{themed_icon, IconTone};

const TILE_WIDTH: f32 = 116.0;
const TILE_SPACING: f32 = 4.0;
const AVATAR_SIZE: f32 = 56.0;
/// 进度环比头像大一圈，环和头像之间留一道缝。
const RING_SIZE: f32 = 68.0;
const DEVICE_GLYPH_SIZE: f32 = 26.0;
const STAR_SIZE: f32 = 14.0;
const NAME_MAX_CHARS: usize = 14;
const STATUS_MAX_CHARS: usize = 18;

/// 收藏（在线的、不在线的）在前，其它附近设备在后。
pub(super) fn device_tiles(
    browser: &FileBrowser,
    owner: DeviceListOwner,
) -> Vec<Element<'_, Message>> {
    let mut tiles: Vec<Element<'_, Message>> = Vec::new();
    for FavoriteRow { favorite, online } in browser.favorite_rows() {
        let unfavor = star_button(
            IconSymbol::StarSolid,
            Some(TransferMessage::UnfavoritePressed(
                favorite.fingerprint.clone(),
            )),
        );
        tiles.push(match online {
            // 名字用收藏时起的（或对方自报的）别名。
            Some(device) => online_tile(browser, owner, device, favorite.alias.clone(), unfavor),
            None => offline_tile(favorite.alias.clone(), unfavor),
        });
    }
    for device in browser.unclaimed_nearby_devices() {
        tiles.push(online_tile(
            browser,
            owner,
            device,
            device.alias.clone(),
            favor_star(device),
        ));
    }
    tiles
}

/// 窗口多宽就一行排几个。
pub(super) fn tile_grid(tiles: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    Row::with_children(tiles)
        .spacing(TILE_SPACING)
        .wrap()
        .vertical_spacing(TILE_SPACING)
        .into()
}

/// 正在发、要 PIN、发完有失败的设备，一台一行：名字 + 状态与能做的事（取消、输 PIN、
/// 看详情）。都没有时返回 `None`。
pub(super) fn active_sends(browser: &FileBrowser) -> Option<Element<'_, Message>> {
    let favorites = browser.favorite_rows();
    let online = favorites
        .iter()
        .filter_map(|row| {
            row.online
                .map(|device| (row.favorite.alias.clone(), device))
        })
        .chain(
            browser
                .unclaimed_nearby_devices()
                .into_iter()
                .map(|device| (device.alias.clone(), device)),
        );
    let mut rows = column![].spacing(10).width(Length::Fill);
    let mut any = false;
    for (name, device) in online {
        let Some(state) = browser.direct_send_to(device) else {
            continue;
        };
        if !needs_attention(&state.phase) {
            continue;
        }
        any = true;
        rows = rows.push(
            column![
                readable_text(name).size(CAPTION_SIZE),
                direct_send_lines(browser, &device.fingerprint, state),
            ]
            .spacing(2),
        );
    }
    any.then(|| rows.into())
}

/// 发完、取消了这类结果格子上看得清，不用占一行；正在发（能取消）、要 PIN、有失败的才要。
fn needs_attention(phase: &DirectSendPhase) -> bool {
    match phase {
        DirectSendPhase::Checking
        | DirectSendPhase::Sending
        | DirectSendPhase::Canceling
        | DirectSendPhase::NeedsPin { .. }
        | DirectSendPhase::Failed(_) => true,
        DirectSendPhase::Finished { failed, .. } => !failed.is_empty(),
        DirectSendPhase::Canceled => false,
    }
}

fn online_tile<'a>(
    browser: &'a FileBrowser,
    owner: DeviceListOwner,
    device: &'a DeviceInfo,
    name: String,
    star: Element<'a, Message>,
) -> Element<'a, Message> {
    let state = browser.direct_send_to(device);
    let checked = browser
        .checked_devices
        .of(owner)
        .contains(&device.fingerprint);
    let avatar = avatar(
        device_icon(device),
        AvatarLook::Online { checked },
        state.and_then(|state| ring_progress(browser, state)),
    );
    let status = match state {
        Some(state) => tile_status(browser, state),
        None => TileStatus::Plain(device.device_model.clone().unwrap_or_default()),
    };
    tile(
        avatar,
        name,
        status,
        Some(Message::Transfer(TransferMessage::DevicePressed {
            owner,
            device: device.clone(),
        })),
        Some(star),
    )
}

/// 不在线的收藏：灰着、点不了；后台按记住的地址一直在找，找到了就变成在线。
fn offline_tile(name: String, star: Element<'static, Message>) -> Element<'static, Message> {
    tile(
        avatar(IconSymbol::Monitor, AvatarLook::Offline, None),
        name,
        TileStatus::Plain(tr!("Offline")),
        None,
        Some(star),
    )
}

/// 按 IP 添加只在 LocalSend 窗口里：表单跟着那个窗口开关。
pub(super) fn add_by_ip_tile() -> Element<'static, Message> {
    tile(
        avatar(IconSymbol::Plus, AvatarLook::Offline, None),
        tr!("Add by IP"),
        TileStatus::Plain(String::new()),
        Some(Message::Transfer(TransferMessage::AddressForm(
            AddressFormMessage::Opened,
        ))),
        None,
    )
}

enum TileStatus {
    Plain(String),
    Error(String),
}

fn tile<'a>(
    avatar: Element<'a, Message>,
    name: String,
    status: TileStatus,
    press: Option<Message>,
    corner: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    // 名字、型号是对方自报或用户起的，不做本地化。
    let status = match status {
        TileStatus::Plain(text) => {
            centered_muted_text(format_middle_ellipsized_text(&text, STATUS_MAX_CHARS))
        }
        TileStatus::Error(text) => {
            readable_text(format_middle_ellipsized_text(&text, STATUS_MAX_CHARS))
                .size(CAPTION_SIZE)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(ui_colors(theme).error),
                })
                .into()
        }
    };
    let body = column![
        avatar,
        readable_text(format_middle_ellipsized_text(&name, NAME_MAX_CHARS)),
        status,
    ]
    .spacing(2)
    .align_x(Alignment::Center)
    .width(Length::Fill);
    let body = button(container(body).padding([8, 4]))
        .on_press_maybe(press)
        .width(Length::Fixed(TILE_WIDTH))
        .padding(0)
        .style(card_row_button_style());
    let mut layers = vec![body.into()];
    if let Some(corner) = corner {
        layers.push(
            container(corner)
                .width(Length::Fill)
                .align_x(alignment::Horizontal::Right)
                .into(),
        );
    }
    Stack::with_children(layers)
        .width(Length::Fixed(TILE_WIDTH))
        .into()
}

#[derive(Clone, Copy)]
enum AvatarLook {
    Online { checked: bool },
    Offline,
}

fn avatar(
    symbol: IconSymbol,
    look: AvatarLook,
    progress: Option<f32>,
) -> Element<'static, Message> {
    let tone = match look {
        AvatarLook::Online { .. } => IconTone::Selected,
        AvatarLook::Offline => IconTone::Normal,
    };
    let circle = container(themed_icon(symbol, tone, DEVICE_GLYPH_SIZE))
        .center(Length::Fixed(AVATAR_SIZE))
        .style(move |theme: &Theme| avatar_style(theme, look));
    let mut layers: Vec<Element<'static, Message>> =
        vec![container(circle).center(Length::Fixed(RING_SIZE)).into()];
    if let Some(progress) = progress {
        layers.push(progress_ring(progress));
    }
    Stack::with_children(layers)
        .width(Length::Fixed(RING_SIZE))
        .height(Length::Fixed(RING_SIZE))
        .into()
}

fn avatar_style(theme: &Theme, look: AvatarLook) -> container::Style {
    let colors = ui_colors(theme);
    let (background, border) = match look {
        AvatarLook::Online { checked } => (
            colors.primary_container,
            if checked {
                Border {
                    color: colors.primary,
                    width: 2.0,
                    radius: (AVATAR_SIZE / 2.0).into(),
                }
            } else {
                round()
            },
        ),
        AvatarLook::Offline => (colors.surface_container_high, round()),
    };
    container::Style {
        background: Some(Background::Color(background)),
        border,
        ..container::Style::default()
    }
}

fn round() -> Border {
    Border {
        color: Color::TRANSPARENT,
        width: 0.0,
        radius: (AVATAR_SIZE / 2.0).into(),
    }
}

/// 进度环：正在检查、正在发时有；在等对方接受、正在取消时只有一圈底环（0）。其它状态不画环。
fn ring_progress(browser: &FileBrowser, state: &DirectSendState) -> Option<f32> {
    match state.phase {
        DirectSendPhase::Checking => Some(browser.checking_percent(state) as f32 / 100.0),
        DirectSendPhase::Sending | DirectSendPhase::Canceling => {
            Some(sent_fraction(state.bytes_done, state.bytes_total))
        }
        _ => None,
    }
}

/// 整批已传的比例；发送端第一次报进度前总量是 0（对方还在决定要不要收）。
fn sent_fraction(bytes_done: u64, bytes_total: u64) -> f32 {
    if bytes_total == 0 {
        return 0.0;
    }
    (bytes_done.min(bytes_total) as f64 / bytes_total as f64) as f32
}

/// 格子上的一行字，要短：详细的说明和操作在“进行中”那一行。
fn tile_status(browser: &FileBrowser, state: &DirectSendState) -> TileStatus {
    match &state.phase {
        DirectSendPhase::Checking => {
            TileStatus::Plain(tr!("Checking files %1%", browser.checking_percent(state)))
        }
        DirectSendPhase::Sending if state.bytes_total > 0 => TileStatus::Plain(tr!(
            "Sending %1%",
            (sent_fraction(state.bytes_done, state.bytes_total) * 100.0) as u64
        )),
        DirectSendPhase::Sending => TileStatus::Plain(tr!("Waiting...")),
        DirectSendPhase::Canceling => TileStatus::Plain(tr!("Canceling...")),
        DirectSendPhase::NeedsPin { .. } => TileStatus::Plain(tr!("Needs a PIN")),
        DirectSendPhase::Finished { failed, .. } if failed.is_empty() => {
            TileStatus::Plain(tr!("Sent"))
        }
        DirectSendPhase::Finished { sent: 0, .. } => TileStatus::Error(tr!("Send failed")),
        DirectSendPhase::Finished { failed, .. } => {
            TileStatus::Error(tr!("%1 file(s) failed", failed.len()))
        }
        DirectSendPhase::Canceled => TileStatus::Plain(tr!("Canceled")),
        DirectSendPhase::Failed(DirectSendFailure::Declined) => TileStatus::Error(tr!("Declined")),
        DirectSendPhase::Failed(_) => TileStatus::Error(tr!("Send failed")),
    }
}

/// 居中的小号灰字：格子里的状态、空状态的说明。
pub(super) fn centered_muted_text(label: String) -> Element<'static, Message> {
    readable_text(label)
        .size(CAPTION_SIZE)
        .width(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

/// 附近设备的 ☆：明文设备收藏不了，☆ 点不了并说明原因。
fn favor_star(device: &DeviceInfo) -> Element<'static, Message> {
    if device.is_certified() {
        return star_button(
            IconSymbol::Star,
            Some(TransferMessage::FavoritePressed(device.clone())),
        );
    }
    tooltip(
        star_button(IconSymbol::Star, None),
        container(
            readable_text(tr!(
                "This device has encryption turned off, so it cannot be a favorite."
            ))
            .size(CAPTION_SIZE),
        )
        .padding([5, 7])
        .style(tooltip_bubble_style),
        tooltip::Position::Bottom,
    )
    .into()
}

fn star_button(symbol: IconSymbol, message: Option<TransferMessage>) -> Element<'static, Message> {
    icon_button(themed_icon(symbol, IconTone::Normal, STAR_SIZE))
        .on_press_maybe(message.map(Message::Transfer))
        .into()
}

/// 按设备类型取图标；不认识的类型按桌面电脑处理。
fn device_icon(device: &DeviceInfo) -> IconSymbol {
    match device.device_type.as_deref() {
        Some("mobile") => IconSymbol::Smartphone,
        Some("headless" | "server") => IconSymbol::HardDrive,
        _ => IconSymbol::Monitor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_shows_the_whole_batch_and_an_empty_track_while_waiting() {
        assert_eq!(sent_fraction(25, 100), 0.25);
        assert_eq!(sent_fraction(100, 100), 1.0);
        assert_eq!(sent_fraction(120, 100), 1.0);
        // 发送端还没报进度：对方在决定要不要收，只画底环。
        assert_eq!(sent_fraction(0, 0), 0.0);
    }

    #[test]
    fn only_sends_with_something_to_do_take_a_row_under_the_tiles() {
        assert!(needs_attention(&DirectSendPhase::Checking));
        assert!(needs_attention(&DirectSendPhase::Failed(
            DirectSendFailure::Declined
        )));
        assert!(!needs_attention(&DirectSendPhase::Canceled));
        assert!(!needs_attention(&DirectSendPhase::Finished {
            sent: 2,
            failed: Vec::new(),
            details_open: false,
        }));
    }
}
