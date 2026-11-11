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

/// 列表卡片（设置、弹窗里的分组行）：卡片四周内缩，行悬停底色与卡片圆角同心。
pub const CARD_INSET: f32 = 4.0;
pub const ROW_RADIUS: f32 = CARD_RADIUS - CARD_INSET;
pub const ROW_PADDING_X: f32 = 12.0;
/// 上下内边距要够两行（标题 + 说明）的行不贴分隔线。
pub const ROW_PADDING_Y: f32 = 6.0;
/// 单行的行最矮 40；行里放一根 `ROW_MIN_HEIGHT - 2 × ROW_PADDING_Y` 高的撑子。
pub const ROW_MIN_HEIGHT: f32 = 40.0;
/// 卡片内容（组标题、卡片）的最大宽度，窗口更宽时居中。
pub const PAGE_MAX_WIDTH: f32 = 640.0;
