//! 控件与容器的尺寸常量：控件高度、圆角、内边距的唯一来源。

/// 按钮、输入框、下拉框、分段选择的统一高度。
pub const CONTROL_HEIGHT: f32 = 32.0;
pub const CONTROL_RADIUS: f32 = 8.0;
pub const CARD_RADIUS: f32 = 12.0;
pub const CHECKBOX_RADIUS: f32 = 4.0;
pub const BUTTON_PADDING_X: f32 = 12.0;
pub const FIELD_PADDING_X: f32 = 10.0;

/// 输入框和下拉框用绝对行高 + 上下内边距凑出 [`CONTROL_HEIGHT`]：
/// 默认行高是字号的 1.3 倍，13px 时是 16.9，凑不出整数高度。
pub const CONTROL_LINE_HEIGHT: f32 = 18.0;
pub const CONTROL_PADDING_Y: f32 = (CONTROL_HEIGHT - CONTROL_LINE_HEIGHT) / 2.0;
