//! 二维码矩阵 → iced RGBA 图像句柄。
//!
//! 独立于传输状态机的纯渲染辅助：避免在 app/transfer.rs 中堆积渲染细节，
//! 同时规避 canvas 全局光标陷阱（iced-014-pitfalls，改用 image widget）。

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
