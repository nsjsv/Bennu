//! 主程序与 portal 共享的样式词汇：文本/边框/阴影颜色助手、surface 与
//! primary 动作按钮、行悬停/选中态、mac 式滚动条、图标着色、导航输入框。
//! 这里只允许通过 `crate::ui_colors` 读取语义角色，禁止固定 RGB 分支。

use iced::widget::{button, container, scrollable, svg, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

use crate::controls::text_field_style;
use crate::{ui_colors, AppearanceMode};

/// 滚动条悬停展开后的宽度；正常宽度由调用方传入。
pub const SCROLLBAR_HOVER_WIDTH: f32 = 14.0;

/// 滚动条可见性（透明度档位），由滚动容器按悬停/拖动状态计算。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollbarVisibility {
    Hidden,
    VisibleWithOpacity(f32),
    Visible,
}

impl ScrollbarVisibility {
    pub fn with_opacity(opacity: f32) -> Self {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= f32::EPSILON {
            Self::Hidden
        } else if (1.0 - opacity) <= f32::EPSILON {
            Self::Visible
        } else {
            Self::VisibleWithOpacity(opacity)
        }
    }

    pub fn opacity(self) -> f32 {
        match self {
            Self::Hidden => 0.0,
            Self::VisibleWithOpacity(opacity) => opacity,
            Self::Visible => 1.0,
        }
    }
}

pub fn base_text_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface
}

pub fn muted_text_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface_variant
}

pub fn elevation_shadow_color(theme: &Theme, alpha: f32) -> Color {
    let colors = ui_colors(theme);
    Color {
        a: alpha,
        ..match colors.mode {
            AppearanceMode::Light => colors.on_background,
            AppearanceMode::Dark => colors.background,
        }
    }
}

pub fn subtle_border_color(theme: &Theme) -> Color {
    ui_colors(theme).outline_variant
}

pub fn button_surface_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container
}

pub fn button_hover_surface_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_high
}

pub fn button_pressed_surface_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_highest
}

/// 列表行悬停态：浅色面 + 圆角 8。
pub fn hovered_row_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_high)),
        text_color: Some(colors.on_surface),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 列表行选中态（单行语义）：primary container 底 + 圆角 8。
pub fn selected_row_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.primary_container)),
        text_color: Some(colors.on_primary_container),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 多栏视图「已打开子栏」行态：中性高面 + 描边，与选中/悬停区分。
/// 主软件（app-ui）与 portal 消费同一份实现。
pub fn open_child_row_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_high)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: colors.outline,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// 列表行条纹底色：奇数条纹行用浅一档 surface 区分，偶数行回到主
/// 背景。主应用列表与 FileChooser portal 消费同一份实现；depth 目前
/// 不参与取色，保留参数与列表行调用点签名对齐。
pub fn list_row_style(
    _depth: usize,
    stripe_index: usize,
) -> impl Fn(&Theme) -> container::Style + Clone {
    move |theme| {
        let colors = ui_colors(theme);
        let is_alternate_row = stripe_index % 2 == 1;
        let background = if is_alternate_row {
            colors.surface_container_low
        } else {
            colors.background
        };
        container::Style {
            background: Some(Background::Color(background)),
            text_color: Some(base_text_color(theme)),
            border: Border {
                radius: if is_alternate_row { 7.0 } else { 0.0 }.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }
}

/// 侧边栏行选中态（主程序、设置分类、portal FileChooser 共用）：中性灰底 + 圆角 8，
/// 比悬停深一档。侧栏是导航，用中性灰；强调色留给选中的文件，两种“选中”不会混。
pub fn selected_sidebar_item_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_highest)),
        text_color: Some(colors.on_surface),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 侧边栏行悬停态：自 app-ui appearance.rs 下沉。浅色面 + 圆角 8。
pub fn hovered_sidebar_item_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_high)),
        text_color: Some(colors.on_surface),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 收藏夹拖放插入位指示线：primary 细条，自 app-ui appearance.rs 下沉。
pub fn sidebar_bookmark_drop_slot_style(theme: &Theme) -> container::Style {
    let accent = ui_colors(theme).primary;
    container::Style {
        background: Some(Background::Color(accent)),
        border: Border {
            radius: 1.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 错误提示条：error container 底 + error 细边 + 圆角 12。
pub fn error_notification_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.error_container)),
        text_color: Some(colors.on_error_container),
        border: Border {
            color: colors.error,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

/// 应用内容底：窗口级背景色 + 默认正文色。自 app-ui appearance.rs
/// 下沉（预览面板 surface 消费）；app-ui re-export 维持调用路径。
pub fn app_content_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.background)),
        text_color: Some(colors.on_background),
        ..container::Style::default()
    }
}

/// 右键菜单、下拉、工具提示这类浮在内容上的小层：浅浅一层的底、细边、圆角，再加一层
/// 淡阴影，压在什么背景上都看得出边。底色不用更亮的浮起层：深色下那层和悬停一样亮，
/// 菜单行悬停会看不出来。
pub fn context_menu_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_low)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: Shadow {
            color: elevation_shadow_color(theme, 0.12),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..container::Style::default()
    }
}

/// 透明无框按钮：只染文字色，禁用时转 muted。
pub fn transparent_icon_button_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: if matches!(status, button::Status::Disabled) {
            muted_text_color(theme)
        } else {
            base_text_color(theme)
        },
        ..button::Style::default()
    }
}

pub fn context_menu_item_button_style() -> fn(&Theme, button::Status) -> button::Style {
    context_menu_item_button_style_for_status
}

fn context_menu_item_button_style_for_status(
    theme: &Theme,
    status: button::Status,
) -> button::Style {
    let background = match status {
        button::Status::Hovered => Some(Background::Color(button_hover_surface_color(theme))),
        button::Status::Pressed => Some(Background::Color(button_pressed_surface_color(theme))),
        button::Status::Active | button::Status::Disabled => None,
    };

    button::Style {
        background,
        text_color: if matches!(status, button::Status::Disabled) {
            muted_text_color(theme)
        } else {
            base_text_color(theme)
        },
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub fn icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
    icon_svg_style_for_status
}

pub fn selected_icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
    selected_icon_svg_style_for_status
}

pub fn muted_icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
    muted_icon_svg_style_for_status
}

pub fn warning_icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
    warning_icon_svg_style_for_status
}

fn icon_svg_style_for_status(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(ui_colors(theme).on_surface),
    }
}

fn selected_icon_svg_style_for_status(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(ui_colors(theme).on_primary_container),
    }
}

fn muted_icon_svg_style_for_status(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(ui_colors(theme).on_surface_variant),
    }
}

fn warning_icon_svg_style_for_status(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(ui_colors(theme).tertiary),
    }
}

// ---------------------------------------------------------------------------
// 地址栏样式组：主程序地址栏与 portal FileChooser 地址栏共用同一份视觉。
// 值自主程序 app-ui 原样搬移，一个数值都不许漂移。
// ---------------------------------------------------------------------------

/// 地址栏外框：与导航输入框共用同一圆角框面，让编辑态输入框恰好覆盖
/// 面包屑层而不露出第二层边框。
pub fn address_bar_style(theme: &Theme) -> container::Style {
    let input_style = text_field_style(theme, text_input::Status::Active);
    container::Style {
        background: Some(input_style.background),
        border: input_style.border,
        ..container::Style::default()
    }
}

/// 地址栏编辑层、退出快照下面垫的一层不透明窗口底色：盖住下面的面包屑，再由上层的
/// 半透明输入框底色叠出和外框、搜索框完全相同的颜色。
///
/// 不自己算“半透明叠到底色上”的结果：wgpu 在线性色彩空间混色，按 sRGB 算出来的
/// 实色在深色主题下比渲染器叠出的明显偏暗（2026-11-10 用户发现地址栏比搜索框暗）。
pub fn address_bar_mask_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).background)),
        border: Border {
            radius: crate::metrics::CONTROL_RADIUS.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 透明无框按钮样式入口（地址栏面包屑段等使用）。
pub fn transparent_button_style() -> fn(&Theme, button::Status) -> button::Style {
    transparent_icon_button_style
}

pub fn scale_color_alpha(color: Color, opacity: f32) -> Color {
    Color {
        a: color.a * opacity.clamp(0.0, 1.0),
        ..color
    }
}

pub fn scale_background_alpha(background: Background, opacity: f32) -> Background {
    match background {
        Background::Color(color) => Background::Color(scale_color_alpha(color, opacity)),
        Background::Gradient(gradient) => Background::Gradient(gradient),
    }
}

/// 路径补全面板：浅色面 + 细边 + 圆角 8。
pub fn path_suggestions_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container_low)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// 补全建议行常态：surface_container 底 + 圆角 6。
pub fn path_suggestion_item_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container)),
        text_color: Some(colors.on_surface),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 补全建议选中行：primary container 底 + 圆角 6。
pub fn selected_path_suggestion_item_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.primary_container)),
        text_color: Some(colors.on_primary_container),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 面包屑段按钮：透明底随编辑渐变淡出；拖放悬停目标复用补全选中行
/// 的底色/描边，保证“可放置”反馈与选中语义同色。
pub fn faded_button_style(
    theme: &Theme,
    status: button::Status,
    opacity: f32,
    is_drop_target: bool,
) -> button::Style {
    let mut style = transparent_icon_button_style(theme, status);
    if is_drop_target {
        let target_style = selected_path_suggestion_item_style(theme);
        style.background = target_style.background;
        if let Some(text_color) = target_style.text_color {
            style.text_color = text_color;
        }
        style.border = target_style.border;
    }
    style.text_color = scale_color_alpha(style.text_color, opacity);
    style.border.color = scale_color_alpha(style.border.color, opacity);
    style.background = style
        .background
        .map(|background| scale_background_alpha(background, opacity));
    style
}

/// 编辑态地址输入框：外观同普通输入框（含聚焦边框），文字随过渡 fraction 淡入淡出。
/// 它放在 `address_bar_overlay` 的不透明垫层上，盖住外框的边框，所以只看得到一层边框。
pub fn faded_text_input_style(
    theme: &Theme,
    status: text_input::Status,
    opacity: f32,
) -> text_input::Style {
    let mut style = text_field_style(theme, status);
    style.icon = scale_color_alpha(style.icon, opacity);
    style.placeholder = scale_color_alpha(style.placeholder, opacity);
    style.value = scale_color_alpha(style.value, opacity);
    style.selection = scale_color_alpha(style.selection, opacity);
    style
}

pub fn enhanced_scrollbar_style(
    visibility: ScrollbarVisibility,
) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style + Clone {
    move |theme, status| {
        let mut style = mac_scrollbar_style(theme, status, visibility);
        // 保留 Iced 的命中区域和拖动状态机，避免与 Canvas 滑块绘制两套视觉反馈。
        style.vertical_rail.scroller.background = Background::Color(Color::TRANSPARENT);
        style.vertical_rail.scroller.border = Border::default();
        style.horizontal_rail.scroller.background = Background::Color(Color::TRANSPARENT);
        style.horizontal_rail.scroller.border = Border::default();
        style
    }
}

pub fn enhanced_vertical_scrollbar_direction(
    visibility: ScrollbarVisibility,
    width: f32,
) -> scrollable::Direction {
    scrollable::Direction::Vertical(auto_hide_scrollbar_properties(
        visibility,
        width.max(SCROLLBAR_HOVER_WIDTH),
    ))
}

pub fn enhanced_horizontal_scrollbar_direction(
    visibility: ScrollbarVisibility,
    width: f32,
) -> scrollable::Direction {
    scrollable::Direction::Horizontal(auto_hide_scrollbar_properties(
        visibility,
        width.max(SCROLLBAR_HOVER_WIDTH),
    ))
}

pub fn enhanced_both_scrollbar_direction(
    visibility: ScrollbarVisibility,
    width: f32,
) -> scrollable::Direction {
    let bar = auto_hide_scrollbar_properties(visibility, width.max(SCROLLBAR_HOVER_WIDTH));
    scrollable::Direction::Both {
        vertical: bar,
        horizontal: bar,
    }
}

fn auto_hide_scrollbar_properties(
    visibility: ScrollbarVisibility,
    width: f32,
) -> scrollable::Scrollbar {
    let width = if visibility.opacity() <= f32::EPSILON {
        0.0
    } else {
        width
    };

    scrollable::Scrollbar::new()
        .width(width)
        .scroller_width(width)
}

/// mac 式滚动条：透明轨道、半透明圆角滑块，悬停/拖动加深。
pub fn mac_scrollbar_style(
    theme: &Theme,
    status: scrollable::Status,
    visibility: ScrollbarVisibility,
) -> scrollable::Style {
    let mut opacity = visibility.opacity();

    match status {
        scrollable::Status::Hovered {
            is_horizontal_scrollbar_hovered,
            is_vertical_scrollbar_hovered,
            ..
        } if opacity > 0.0
            && (is_horizontal_scrollbar_hovered || is_vertical_scrollbar_hovered) =>
        {
            opacity = (opacity + 0.18).min(1.0);
        }
        scrollable::Status::Dragged {
            is_horizontal_scrollbar_dragged,
            is_vertical_scrollbar_dragged,
            ..
        } if opacity > 0.0
            && (is_horizontal_scrollbar_dragged || is_vertical_scrollbar_dragged) =>
        {
            opacity = opacity.max(0.86);
        }
        _ => {}
    }

    let mut style = scrollable::default(theme, status);
    let scroller_background = mac_scrollbar_scroller_color(theme, opacity).into();
    let rail_border = Border {
        radius: 999.0.into(),
        ..Border::default()
    };
    let scroller_border = Border {
        radius: 999.0.into(),
        ..Border::default()
    };

    style.vertical_rail.background = None;
    style.vertical_rail.border = rail_border;
    style.vertical_rail.scroller.background = scroller_background;
    style.vertical_rail.scroller.border = scroller_border;
    style.horizontal_rail.background = None;
    style.horizontal_rail.border = rail_border;
    style.horizontal_rail.scroller.background = scroller_background;
    style.horizontal_rail.scroller.border = scroller_border;
    style.gap = None;
    style
}

fn mac_scrollbar_scroller_color(theme: &Theme, opacity: f32) -> Color {
    Color {
        a: 0.42 * opacity.clamp(0.0, 1.0),
        ..ui_colors(theme).on_surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_visibility_opacity_ladder_round_trips() {
        assert_eq!(
            ScrollbarVisibility::with_opacity(0.0),
            ScrollbarVisibility::Hidden
        );
        assert_eq!(
            ScrollbarVisibility::with_opacity(-1.0),
            ScrollbarVisibility::Hidden
        );
        assert_eq!(
            ScrollbarVisibility::with_opacity(1.0),
            ScrollbarVisibility::Visible
        );
        match ScrollbarVisibility::with_opacity(0.4) {
            ScrollbarVisibility::VisibleWithOpacity(opacity) => {
                assert!((opacity - 0.4).abs() < 1e-3)
            }
            other => panic!("expected explicit opacity, got {other:?}"),
        }
    }

    #[test]
    fn scheme_roles_flow_into_representative_shared_styles() {
        let schemes = crate::ApplicationTheme::new(crate::AppearanceMode::Light);
        for theme_mode in [crate::ThemeMode::Light, crate::ThemeMode::Dark] {
            let theme = schemes.active(theme_mode, crate::ColorSchemePreset::Catppuccin);
            let colors = crate::ui_colors(&theme);

            let selected = selected_row_style(&theme);
            assert_eq!(
                selected.background,
                Some(Background::Color(colors.primary_container))
            );
            assert_eq!(selected.text_color, Some(colors.on_primary_container));

            let hovered = hovered_row_style(&theme);
            assert_eq!(
                hovered.background,
                Some(Background::Color(colors.surface_container_high))
            );

            let error = error_notification_style(&theme);
            assert_eq!(
                error.background,
                Some(Background::Color(colors.error_container))
            );

            let input = text_field_style(&theme, text_input::Status::Active);
            assert_eq!(input.border.radius, 8.0.into());
        }
    }

    #[test]
    fn list_row_stripes_alternate_surface_tint_and_radius() {
        for theme in [Theme::Light, Theme::Dark] {
            let colors = ui_colors(&theme);
            let even = list_row_style(0, 0)(&theme);
            assert_eq!(even.background, Some(Background::Color(colors.background)));
            assert_eq!(even.border.radius, 0.0.into());

            let odd = list_row_style(0, 1)(&theme);
            assert_eq!(
                odd.background,
                Some(Background::Color(colors.surface_container_low))
            );
            assert_eq!(odd.border.radius, 7.0.into());
        }
    }
}
