//! 二维码：矩阵 → iced RGBA 图像句柄（用 image widget，规避 canvas 全局光标陷阱，
//! 见 iced-014-pitfalls），以及扫码下载与链接接收共用的"选地址 / 出码"视图。

use iced::widget::{button, column, container, image};
use iced::{Element, Length};

use crate::app::transfer::{QrLinks, TransferMessage};
use crate::appearance::context_menu_button_style;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;

/// 矩阵 → RGBA 像素：白底黑模块 + 4 模块静区，4x 放大配合 nearest
/// 过滤保证缩放后模块边缘锐利。
pub(crate) fn qr_image_handle(url: &str) -> iced::widget::image::Handle {
    const QUIET_ZONE: u32 = 4;
    const SCALE: u32 = 4;
    let code = local_send::qr_matrix(url);
    let size = code.size as u32;
    let edge = (size + QUIET_ZONE * 2) * SCALE;
    let white = [255u8, 255, 255, 255];
    let black = [0u8, 0, 0, 255];
    let mut rgba = Vec::with_capacity((edge * edge * 4) as usize);
    let push_pixels = |pixels: u32, color: [u8; 4], rgba: &mut Vec<u8>| {
        for _ in 0..pixels {
            rgba.extend_from_slice(&color);
        }
    };
    // 顶部静区
    for _ in 0..QUIET_ZONE * SCALE {
        push_pixels(edge, white, &mut rgba);
    }
    for y in 0..size {
        for _ in 0..SCALE {
            push_pixels(QUIET_ZONE * SCALE, white, &mut rgba);
            for x in 0..size {
                let color = if code[y as usize][x as usize].value() {
                    black
                } else {
                    white
                };
                push_pixels(SCALE, color, &mut rgba);
            }
            push_pixels(QUIET_ZONE * SCALE, white, &mut rgba);
        }
    }
    // 底部静区
    for _ in 0..QUIET_ZONE * SCALE {
        push_pixels(edge, white, &mut rgba);
    }
    iced::widget::image::Handle::from_rgba(edge, edge, rgba)
}

const QR_DISPLAY_SIZE: f32 = 220.0;

/// 先选地址再出码：选了显示二维码、链接与"重新选择地址"；没选列出本机地址。
pub(crate) fn qr_links_view(
    links: &QrLinks,
    caption: fn(&str) -> String,
    choose: fn(usize) -> TransferMessage,
    show_addresses: TransferMessage,
) -> Element<'_, Message> {
    let mut content = column![].spacing(10).width(Length::Fill);
    match &links.chosen {
        Some((index, qr)) => {
            content = content.push(
                container(
                    image(qr.clone())
                        .filter_method(image::FilterMethod::Nearest)
                        .width(Length::Fixed(QR_DISPLAY_SIZE))
                        .height(Length::Fixed(QR_DISPLAY_SIZE)),
                )
                .center_x(Length::Fill),
            );
            if let Some(url) = links.urls.get(*index) {
                content = content.push(readable_text(caption(url)).size(11));
            }
            content.push(
                button(readable_text(tr!("Choose another address")).size(12))
                    .on_press(Message::Transfer(show_addresses))
                    .padding([5, 12])
                    .style(context_menu_button_style()),
            )
        }
        None => {
            content = content.push(
                container(readable_text(tr!("Choose an address your phone can reach")).size(13))
                    .center_x(Length::Fill),
            );
            for (index, url) in links.urls.iter().enumerate() {
                content = content.push(
                    button(
                        readable_text(display_address(url).to_owned())
                            .size(12)
                            .width(Length::Fill),
                    )
                    .on_press(Message::Transfer(choose(index)))
                    .padding([6, 10])
                    .width(Length::Fill)
                    .style(context_menu_button_style()),
                );
            }
            content
        }
    }
    .into()
}

/// 地址列表只显示 host:port，不展示带随机码的完整链接。
fn display_address(url: &str) -> &str {
    url.strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
}
