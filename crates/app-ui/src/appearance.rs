use iced::widget::button;
use iced::{Background, Border, Color, Theme};

mod navigation_input;
mod container {
    pub use iced::widget::container::*;
    pub type Appearance = Style;
}
mod icon_grid;
mod list_header;
mod window_chrome;

pub(crate) use icon_grid::icon_grid_expansion_panel_style;
pub(crate) use navigation_input::{address_bar_style, navigation_text_input_style};
// 顶栏/标题栏容器样式只被宿主辅助窗口 chrome 消费，留在这里；四枚
// 窗口控制按钮样式（标准/浮动）已下沉 bennu-theme::window_chrome_styles
// （预览浮动 chrome 迁移需要，随 bennu-preview 的 chrome 机件消费），
// app-ui 侧不再有直接调用点。
pub(crate) use window_chrome::{window_title_bar_style, window_top_bar_style};

// 导航按钮样式已下沉 bennu-theme（与导航输入框同组词汇）；预览/文档
// 视觉词汇在 bennu-theme::preview_styles（面板本体已随 bennu-preview
// 迁移，经 bennu-theme 路径直接消费，app-ui 侧无剩余调用点）。
pub(crate) use bennu_theme::styles::navigation_icon_button_style;

pub(crate) use list_header::{
    group_header_style, list_header_cell_style, list_header_reorder_indicator_style,
    list_header_style, ListHeaderCellVisualState,
};

// 通用样式词汇已迁至共享 crate `bennu-theme::styles`（portal 复用同一份）；
// 这里重导出保持 crate 内调用点不变。
pub(crate) use bennu_theme::styles::{
    app_content_style, base_text_color, button_hover_surface_color, button_pressed_surface_color,
    button_surface_color, context_menu_item_button_style, context_menu_style,
    enhanced_horizontal_scrollbar_direction, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, error_notification_style, faded_button_style,
    faded_text_input_style, hovered_row_style, hovered_sidebar_item_style, icon_svg_style,
    list_row_style, muted_icon_svg_style, muted_text_color, path_suggestion_item_style,
    path_suggestions_style, scale_color_alpha, selected_icon_svg_style,
    selected_path_suggestion_item_style, selected_sidebar_item_style,
    sidebar_bookmark_drop_slot_style, sidebar_style, subtle_border_color, surface_button_style,
    transparent_button_style, transparent_icon_button_style, warning_icon_svg_style,
};

use crate::file_entry_presentation::SelectionRunPosition;
use crate::matugen_theme::ui_colors;

pub(crate) fn selected_row_style(theme: &Theme) -> container::Appearance {
    selected_row_style_for_run(SelectionRunPosition::Single)(theme)
}

pub(crate) fn selected_row_style_for_run(
    position: SelectionRunPosition,
) -> impl Fn(&Theme) -> container::Appearance + Clone {
    move |theme| selected_row_appearance(theme, position)
}

fn selected_row_appearance(theme: &Theme, position: SelectionRunPosition) -> container::Appearance {
    // 单行选中态直接用共享实现；连续选区只有首/中/尾圆角差异在本地处理。
    if position == SelectionRunPosition::Single {
        return bennu_theme::styles::selected_row_style(theme);
    }
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.primary_container)),
        text_color: Some(colors.on_primary_container),
        border: Border {
            radius: selected_run_radius(position),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

fn selected_run_radius(position: SelectionRunPosition) -> iced::border::Radius {
    match position {
        SelectionRunPosition::Single => iced::border::Radius::new(8.0),
        SelectionRunPosition::First => iced::border::Radius::default().top(8.0),
        SelectionRunPosition::Middle => iced::border::Radius::default(),
        SelectionRunPosition::Last => iced::border::Radius::default().bottom(8.0),
    }
}

pub(crate) fn open_child_row_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_container_high)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: colors.outline,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn dragged_row_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_dim)),
        text_color: Some(colors.on_surface_variant),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn selection_marquee_style(theme: &Theme) -> container::Appearance {
    let accent = ui_colors(theme).primary;
    container::Appearance {
        background: Some(Background::Color(Color { a: 0.16, ..accent })),
        border: Border {
            color: accent,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn tab_split_overlay_style(theme: &Theme) -> container::Appearance {
    let accent = ui_colors(theme).primary;
    container::Appearance {
        background: Some(Background::Color(Color { a: 0.18, ..accent })),
        border: Border {
            color: Color { a: 0.74, ..accent },
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn operation_queue_indicator_button_style() -> fn(&Theme, button::Status) -> button::Style
{
    transparent_icon_button_style
}

/// 分组索引栏按钮:常态透明无框文本按钮(既定 UI 偏好),悬停浮起
/// 浅色面;当前可视区所在组常亮浅色面,悬停其上再加深一档。
pub(crate) fn grouping_rail_button_style(
    is_active_group: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let colors = ui_colors(theme);
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => {
                Some(Background::Color(if is_active_group {
                    colors.surface_container_highest
                } else {
                    colors.surface_container_high
                }))
            }
            button::Status::Active if is_active_group => {
                Some(Background::Color(colors.surface_container_high))
            }
            _ => None,
        };
        button::Style {
            background,
            text_color: if is_active_group || matches!(status, button::Status::Hovered) {
                colors.on_surface
            } else {
                colors.on_surface_variant
            },
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

pub(crate) fn context_menu_button_style() -> fn(&Theme, button::Status) -> button::Style {
    surface_button_style
}

pub(crate) fn preview_panel_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_container_low)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 14.0.into(),
        },
        ..container::Appearance::default()
    }
}
pub(crate) fn column_browser_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        text_color: Some(base_text_color(theme)),
        ..container::Appearance::default()
    }
}

pub(crate) fn column_panel_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        text_color: Some(base_text_color(theme)),
        ..container::Appearance::default()
    }
}

pub(crate) fn list_panel_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        text_color: Some(base_text_color(theme)),
        ..container::Appearance::default()
    }
}

pub(crate) fn column_resize_divider_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(subtle_border_color(theme))),
        ..container::Appearance::default()
    }
}

pub(crate) fn tab_strip_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        text_color: Some(colors.on_surface),
        ..container::Appearance::default()
    }
}

pub(crate) fn tab_item_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(Color {
            a: 0.78,
            ..colors.surface_container_high
        })),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 12.0.into(),
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn selected_tab_item_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.primary_container)),
        text_color: Some(colors.on_primary_container),
        border: Border {
            color: colors.primary,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..container::Appearance::default()
    }
}

/// 拖拽预览条目(无底板)的文字颜色:淡出程度越高越向背景色收敛,
/// 与窗口内图标淡出(faded_themed_icon)同一收敛方向。
pub(crate) fn faded_drag_preview_label_style(theme: &Theme, fade: f32) -> container::Appearance {
    fn mix_color(from: iced::Color, to: iced::Color, t: f32) -> iced::Color {
        iced::Color {
            r: from.r + (to.r - from.r) * t,
            g: from.g + (to.g - from.g) * t,
            b: from.b + (to.b - from.b) * t,
            a: from.a,
        }
    }
    let colors = ui_colors(theme);
    container::Appearance {
        text_color: Some(mix_color(colors.on_surface, colors.background, fade)),
        ..container::Appearance::default()
    }
}

/// 拖拽预览胶囊底板:淡出程度越高,底板/文字/描边越向背景色收敛,
/// 与窗口内图标淡出(faded_themed_icon)同一收敛方向。逐个条目与
/// 聚合总数行共用。
pub(crate) fn faded_drag_preview_pill_style(theme: &Theme, fade: f32) -> container::Appearance {
    fn mix_color(from: iced::Color, to: iced::Color, t: f32) -> iced::Color {
        iced::Color {
            r: from.r + (to.r - from.r) * t,
            g: from.g + (to.g - from.g) * t,
            b: from.b + (to.b - from.b) * t,
            a: from.a,
        }
    }
    let colors = ui_colors(theme);
    let mix = |color: iced::Color| mix_color(color, colors.background, fade);
    container::Appearance {
        background: Some(Background::Color(mix(colors.surface_bright))),
        text_color: Some(mix(colors.on_surface)),
        border: Border {
            color: mix(subtle_border_color(theme)),
            width: 1.0,
            radius: 9.0.into(),
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn switch_track_on_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(ui_colors(theme).primary)),
        border: Border {
            radius: 11.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn switch_track_off_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(ui_colors(theme).outline_variant)),
        border: Border {
            radius: 11.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn switch_thumb_on_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(ui_colors(theme).on_primary)),
        border: Border {
            radius: 7.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn switch_thumb_off_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(ui_colors(theme).on_surface)),
        border: Border {
            radius: 7.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matugen_theme::{parse_matugen_theme, ui_colors};

    #[test]
    fn generated_light_and_dark_roles_drive_representative_surfaces() {
        for document in [
            include_str!("../../bennu-theme/test-data/matugen-dark.toml"),
            include_str!("../../bennu-theme/test-data/matugen-light.toml"),
        ] {
            let theme = parse_matugen_theme(document).expect("fixture must be valid");
            let colors = ui_colors(&theme);

            let app = app_content_style(&theme);
            assert_eq!(app.background, Some(Background::Color(colors.background)));
            assert_eq!(app.text_color, Some(colors.on_background));

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
            let context_menu_active =
                context_menu_item_button_style()(&theme, button::Status::Active);
            assert!(context_menu_active.background.is_none());
            assert_eq!(context_menu_active.border.width, 0.0);

            let context_menu_hovered =
                context_menu_item_button_style()(&theme, button::Status::Hovered);
            assert_eq!(
                context_menu_hovered.background,
                Some(Background::Color(colors.surface_container_high))
            );
            assert_eq!(context_menu_hovered.border.width, 0.0);

            let error = error_notification_style(&theme);
            assert_eq!(
                error.background,
                Some(Background::Color(colors.error_container))
            );
            assert_eq!(error.text_color, Some(colors.on_error_container));
            assert_eq!(error.border.color, colors.error);

            let close = bennu_theme::window_chrome_styles::window_close_button_style(
                &theme,
                button::Status::Hovered,
            );
            assert_eq!(close.background, Some(Background::Color(colors.error)));
            assert_eq!(close.text_color, colors.on_error);

            let switch_on = switch_thumb_on_style(&theme);
            assert_eq!(
                switch_on.background,
                Some(Background::Color(colors.on_primary))
            );
            let switch_off = switch_thumb_off_style(&theme);
            assert_eq!(
                switch_off.background,
                Some(Background::Color(colors.on_surface))
            );
        }
    }
}
