//! 主程序与 portal 共享的样式词汇：文本/边框/阴影颜色助手、surface 与
//! primary 动作按钮、行悬停/选中态、mac 式滚动条、图标着色、导航输入框。
//! 这里只允许通过 `crate::ui_colors` 读取语义角色，禁止固定 RGB 分支。

use iced::widget::{button, container, scrollable, svg, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

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

/// 右键菜单/工具提示底：surface container + 细边 + 圆角 8。自 app-ui
/// appearance.rs 下沉（窗口控制按钮工具提示消费）；app-ui re-export。
pub fn context_menu_style(theme: &Theme) -> container::Style {
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

/// surface 按钮：surface_container 底、outline_variant 细边、圆角 7，
/// hover/pressed 各加深一档。
pub fn surface_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => button_hover_surface_color(theme),
        button::Status::Pressed => button_pressed_surface_color(theme),
        button::Status::Active | button::Status::Disabled => button_surface_color(theme),
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: if matches!(status, button::Status::Disabled) {
            muted_text_color(theme)
        } else {
            base_text_color(theme)
        },
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 7.0.into(),
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
        ..button::Style::default()
    }
}

/// 主操作按钮：primary 底、on_primary 文字、圆角 7、hover/pressed 透明度档、
/// 带轻微投影；禁用时退为 surface_container_highest。
pub fn primary_action_button_style() -> fn(&Theme, button::Status) -> button::Style {
    primary_action_button_appearance
}

fn primary_action_button_appearance(theme: &Theme, status: button::Status) -> button::Style {
    let colors = ui_colors(theme);
    let background = match status {
        button::Status::Hovered => Color {
            a: 0.9,
            ..colors.primary
        },
        button::Status::Pressed => Color {
            a: 0.8,
            ..colors.primary
        },
        button::Status::Disabled => colors.surface_container_highest,
        button::Status::Active => colors.primary,
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: if matches!(status, button::Status::Disabled) {
            colors.on_surface_variant
        } else {
            colors.on_primary
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 7.0.into(),
        },
        shadow: action_button_shadow(theme, status),
        ..button::Style::default()
    }
}

/// 破坏性确认按钮：error 底的 primary 动作按钮变体。
pub fn destructive_confirmation_button_style() -> fn(&Theme, button::Status) -> button::Style {
    destructive_confirmation_button_appearance
}

fn destructive_confirmation_button_appearance(
    theme: &Theme,
    status: button::Status,
) -> button::Style {
    let colors = ui_colors(theme);
    let background = match status {
        button::Status::Hovered => Color {
            a: 0.9,
            ..colors.error
        },
        button::Status::Pressed => Color {
            a: 0.8,
            ..colors.error
        },
        button::Status::Disabled => colors.error_container,
        button::Status::Active => colors.error,
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: if matches!(status, button::Status::Disabled) {
            colors.on_error_container
        } else {
            colors.on_error
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        shadow: action_button_shadow(theme, status),
        ..button::Style::default()
    }
}

fn action_button_shadow(theme: &Theme, status: button::Status) -> Shadow {
    if matches!(status, button::Status::Disabled) {
        Shadow::default()
    } else {
        Shadow {
            color: elevation_shadow_color(theme, 0.14),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 3.0,
        }
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

/// 导航输入框（地址栏/文件名）样式：默认 text_input + 圆角 8。
pub fn navigation_text_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.border.radius = 8.0.into();
    style
}

/// 导航栏图标按钮样式入口：与 surface 按钮同一视觉（自主程序
/// app-ui 原样搬移，保持主程序与 portal 图标按钮同源）。
pub fn navigation_icon_button_style() -> fn(&Theme, button::Status) -> button::Style {
    surface_button_style
}

// ---------------------------------------------------------------------------
// 地址栏样式组：主程序地址栏与 portal FileChooser 地址栏共用同一份视觉。
// 值自主程序 app-ui 原样搬移，一个数值都不许漂移。
// ---------------------------------------------------------------------------

/// 地址栏外框：与导航输入框共用同一圆角框面，让编辑态输入框恰好覆盖
/// 面包屑层而不露出第二层边框。
pub fn address_bar_style(theme: &Theme) -> container::Style {
    let input_style = navigation_text_input_style(theme, text_input::Status::Active);
    container::Style {
        background: Some(input_style.background),
        border: input_style.border,
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

/// 编辑态地址输入框：去掉边框只留底色，随过渡 fraction 淡入淡出，
/// 与面包屑层叠时不产生第二层边框。
pub fn faded_text_input_style(
    theme: &Theme,
    status: text_input::Status,
    opacity: f32,
) -> text_input::Style {
    let mut style = navigation_text_input_style(theme, status);
    style.border.width = 0.0;
    style.border.color = Color::TRANSPARENT;
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
    fn matugen_roles_flow_into_representative_shared_styles() {
        for document in [
            include_str!("../test-data/matugen-dark.toml"),
            include_str!("../test-data/matugen-light.toml"),
        ] {
            let theme = crate::parse_matugen_theme(document).expect("fixture must be valid");
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

            let primary = primary_action_button_style()(&theme, button::Status::Active);
            assert_eq!(primary.background, Some(Background::Color(colors.primary)));
            assert_eq!(primary.text_color, colors.on_primary);

            let input = navigation_text_input_style(&theme, text_input::Status::Active);
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
