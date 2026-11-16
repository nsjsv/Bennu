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
