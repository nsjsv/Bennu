//! 界面字体与字号档的唯一来源。
//!
//! 字号只按“这段文字是什么”取档；正文是应用默认字号，调用方不写 `.size()`。
//! 终端格子、文本预览正文这类内容字号不属于这里。

use iced::font::Weight;
use iced::Font;

/// 页面、窗口标题，配 [`BOLD`]。
pub const TITLE_SIZE: f32 = 18.0;
/// 组标题，配 [`BOLD`]。
pub const HEADING_SIZE: f32 = 13.0;
/// 正文：iced `default_text_size`，所以正文不写 `.size()`。
pub const BODY_SIZE: f32 = 13.0;
/// 说明文字，一般配灰色。
pub const CAPTION_SIZE: f32 = 12.0;
/// 角标、计数。
pub const SMALL_SIZE: f32 = 11.0;
/// 行高系数：与 iced `LineHeight::default()` 一致，几何计算按它推行高。
pub const LINE_HEIGHT: f32 = 1.3;

pub const BOLD: Font = Font {
    weight: Weight::Bold,
    ..Font::DEFAULT
};

/// 设置里能选的正文字号（像素）。默认就是 [`BODY_SIZE`]。
pub const TEXT_SIZE_CHOICES: [u16; 6] = [12, 13, 14, 15, 16, 18];
pub const DEFAULT_TEXT_SIZE: u16 = 13;

/// 选了别的正文字号时整个界面的缩放：字、行高、间距、图标按同一比例变。界面里列表行、侧栏行、
/// 菜单项的高度都是写死的，只放大字会被行裁掉；整体缩放和浏览器缩放一样，不会裁。
pub fn interface_scale(text_size: u16) -> f32 {
    f32::from(text_size) / BODY_SIZE
}

/// 存下来的字号换成能选的那几档里最接近的一档（手改过、或以后换了档位）。
pub fn nearest_text_size(stored: u16) -> u16 {
    TEXT_SIZE_CHOICES
        .into_iter()
        .min_by_key(|choice| choice.abs_diff(stored))
        .unwrap_or(DEFAULT_TEXT_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_text_size_keeps_the_interface_unscaled() {
        assert_eq!(f32::from(DEFAULT_TEXT_SIZE), BODY_SIZE);
        assert!(TEXT_SIZE_CHOICES.contains(&DEFAULT_TEXT_SIZE));
        assert_eq!(interface_scale(DEFAULT_TEXT_SIZE), 1.0);
        assert_eq!(interface_scale(18), 18.0 / 13.0);
    }

    #[test]
    fn a_stored_size_snaps_to_the_nearest_choice() {
        assert_eq!(nearest_text_size(13), 13);
        assert_eq!(nearest_text_size(17), 16);
        assert_eq!(nearest_text_size(40), 18);
        assert_eq!(nearest_text_size(0), 12);
    }
}
