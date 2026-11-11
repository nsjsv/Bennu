//! 统一控件：全应用（含 portal）的按钮、输入框、下拉框、复选框、开关、
//! 分段选择、滑块、进度条和提示气泡只在这里构造。
//!
//! 构造函数返回已设好样式、尺寸和内边距的 iced widget，调用方只挂行为
//! （`on_press`、`on_input`……）。iced 原始的输入类控件构造函数被仓库根
//! `clippy.toml` 禁用，绕过这里会在 clippy 报错。
//! 颜色只从 `ui_colors` 角色派生，配色重算后控件自动跟随。

mod buttons;
mod escape;
mod feedback;
mod fields;
mod toggles;

pub use buttons::{
    destructive_button, destructive_button_style, icon_button, icon_button_style, normal_button,
    normal_button_style, suggested_button, suggested_button_style, toggle_button_style,
};
pub use escape::on_escape;
pub use feedback::{
    progress_bar, progress_bar_style, slider, slider_style, tooltip_bubble, tooltip_bubble_style,
};
pub use fields::{
    dropdown, dropdown_menu_style, dropdown_style, inline_text_field, text_area, text_field,
    text_field_style,
};
pub use toggles::{
    check_box, segmented_choice_row, switch, SegmentedChoice, SWITCH_HEIGHT, SWITCH_WIDTH,
};
pub(crate) use toggles::{segment_style, segment_track_style, SEGMENT_INSET};

use iced::{Color, Theme};

use crate::{ui_colors, AppearanceMode};

/// 前景色按透明度叠在任何底色上：按钮、输入框底色不依赖它所在的卡片或窗口底色。
pub(crate) fn foreground_tint(theme: &Theme, alpha: f32) -> Color {
    Color {
        a: alpha,
        ..ui_colors(theme).on_surface
    }
}

/// 比底槽更“抬起”的面（分段选择的选中块）：浅色主题用最白的面；深色主题里
/// 越亮越靠前，所以叠一层更浓的前景色，而不是取某个固定的面色（可能比底槽还暗）。
fn raised_surface(theme: &Theme) -> Color {
    let colors = ui_colors(theme);
    match colors.mode {
        AppearanceMode::Light => colors.surface_container_lowest,
        AppearanceMode::Dark => foreground_tint(theme, 0.16),
    }
}

/// 开关关闭时的圆点、滑块手柄：要和灰色轨道拉开对比，深色主题用接近白的文字色。
fn knob_color(theme: &Theme) -> Color {
    let colors = ui_colors(theme);
    match colors.mode {
        AppearanceMode::Light => colors.surface_container_lowest,
        AppearanceMode::Dark => colors.on_surface,
    }
}

/// 强调色按钮的悬停/按下：向文字色混一点，深浅主题都能看出变化。
fn mix(base: Color, toward: Color, amount: f32) -> Color {
    Color {
        r: base.r + (toward.r - base.r) * amount,
        g: base.g + (toward.g - base.g) * amount,
        b: base.b + (toward.b - base.b) * amount,
        a: base.a,
    }
}
