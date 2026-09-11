use iced::widget::{button, scrollable, svg};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

mod document_preview;
mod navigation_input;
mod container {
    pub use iced::widget::container::*;
    pub type Appearance = Style;
}
mod icon_grid;
mod list_header;
mod window_chrome;

pub(crate) use document_preview::document_page_style;
pub(crate) use icon_grid::icon_grid_expansion_panel_style;
pub(crate) use navigation_input::{address_bar_style, navigation_text_input_style};
pub(crate) use window_chrome::{
    floating_window_close_button_style, floating_window_control_button_style,
    preview_window_bottom_gradient_style, preview_window_top_gradient_style,
    window_close_button_style, window_control_button_style, window_title_bar_style,
    window_top_bar_style,
};

pub(crate) use list_header::{
    list_header_cell_style, list_header_reorder_indicator_style, list_header_style,
    ListHeaderCellVisualState,
};

use crate::file_entry_presentation::SelectionRunPosition;
use crate::matugen_theme::{ui_colors, AppearanceMode};
use crate::model::{ScrollbarVisibility, SCROLLBAR_HOVER_WIDTH};

pub(crate) fn app_content_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.background)),
        text_color: Some(colors.on_background),
        ..container::Appearance::default()
    }
}

pub(crate) fn selected_row_style(theme: &Theme) -> container::Appearance {
    selected_row_style_for_run(SelectionRunPosition::Single)(theme)
}

pub(crate) fn selected_row_style_for_run(
    position: SelectionRunPosition,
) -> impl Fn(&Theme) -> container::Appearance + Clone {
    move |theme| selected_row_appearance(theme, position)
}

fn selected_row_appearance(theme: &Theme, position: SelectionRunPosition) -> container::Appearance {
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

pub(crate) fn hovered_row_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_container_high)),
        text_color: Some(colors.on_surface),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn navigation_icon_button_style() -> fn(&Theme, button::Status) -> button::Style {
    surface_button_style
}

pub(crate) fn operation_queue_indicator_button_style() -> fn(&Theme, button::Status) -> button::Style
{
    transparent_icon_button_style
}

pub(crate) fn transparent_button_style() -> fn(&Theme, button::Status) -> button::Style {
    transparent_icon_button_style
}

pub(crate) fn context_menu_button_style() -> fn(&Theme, button::Status) -> button::Style {
    surface_button_style
}
pub(crate) fn context_menu_item_button_style() -> fn(&Theme, button::Status) -> button::Style {
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

pub(crate) fn enhanced_scrollbar_style(
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

pub(crate) fn enhanced_vertical_scrollbar_direction(
    visibility: ScrollbarVisibility,
    width: f32,
) -> scrollable::Direction {
    scrollable::Direction::Vertical(auto_hide_scrollbar_properties(
        visibility,
        width.max(SCROLLBAR_HOVER_WIDTH),
    ))
}

pub(crate) fn enhanced_horizontal_scrollbar_direction(
    visibility: ScrollbarVisibility,
    width: f32,
) -> scrollable::Direction {
    scrollable::Direction::Horizontal(auto_hide_scrollbar_properties(
        visibility,
        width.max(SCROLLBAR_HOVER_WIDTH),
    ))
}

pub(crate) fn enhanced_both_scrollbar_direction(
    visibility: ScrollbarVisibility,
    width: f32,
) -> scrollable::Direction {
    let bar = auto_hide_scrollbar_properties(visibility, width.max(SCROLLBAR_HOVER_WIDTH));
    scrollable::Direction::Both {
        vertical: bar.clone(),
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

pub(crate) fn path_suggestions_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_container_low)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn path_suggestion_item_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_container)),
        text_color: Some(colors.on_surface),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn selected_path_suggestion_item_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.primary_container)),
        text_color: Some(colors.on_primary_container),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
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
pub(crate) fn preview_media_style(_theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })),
        ..container::Appearance::default()
    }
}
pub(crate) fn error_notification_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.error_container)),
        text_color: Some(colors.on_error_container),
        border: Border {
            color: colors.error,
            width: 1.0,
            radius: 12.0.into(),
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

pub(crate) fn list_row_style(
    _depth: usize,
    row_index: usize,
) -> impl Fn(&Theme) -> container::Appearance + Clone {
    move |theme| {
        let colors = ui_colors(theme);
        let is_alternate_row = row_index % 2 == 1;
        let background = if is_alternate_row {
            colors.surface_container_low
        } else {
            colors.background
        };
        container::Appearance {
            background: Some(Background::Color(background)),
            text_color: Some(base_text_color(theme)),
            border: Border {
                radius: if is_alternate_row { 7.0 } else { 0.0 }.into(),
                ..Border::default()
            },
            ..container::Appearance::default()
        }
    }
}

pub(crate) fn column_resize_divider_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(subtle_border_color(theme))),
        ..container::Appearance::default()
    }
}

pub(crate) fn sidebar_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(Color {
            a: 0.92,
            ..colors.surface
        })),
        text_color: Some(colors.on_surface),
        border: Border {
            color: Color {
                a: 0.55,
                ..colors.outline_variant
            },
            width: 1.0,
            radius: 18.0.into(),
        },
        shadow: Shadow {
            color: elevation_shadow_color(theme, 0.22),
            offset: Vector::new(0.0, 10.0),
            blur_radius: 22.0,
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn selected_sidebar_item_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.primary_container)),
        text_color: Some(colors.on_primary_container),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
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

pub(crate) fn hovered_sidebar_item_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_container_high)),
        text_color: Some(colors.on_surface),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn sidebar_bookmark_drop_slot_style(theme: &Theme) -> container::Appearance {
    let accent = ui_colors(theme).primary;
    container::Appearance {
        background: Some(Background::Color(accent)),
        border: Border {
            radius: 1.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

pub(crate) fn context_menu_style(theme: &Theme) -> container::Appearance {
    let colors = ui_colors(theme);
    container::Appearance {
        background: Some(Background::Color(colors.surface_container_low)),
        text_color: Some(colors.on_surface),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 8.0.into(),
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
        text_color: Some(mix_color(
            colors.on_surface,
            colors.background,
            fade,
        )),
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

pub(crate) fn icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
    icon_svg_style_for_status
}

pub(crate) fn selected_icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
    selected_icon_svg_style_for_status
}

pub(crate) fn muted_icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
    muted_icon_svg_style_for_status
}

pub(crate) fn warning_icon_svg_style() -> fn(&Theme, svg::Status) -> svg::Style {
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

fn transparent_icon_button_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: if matches!(status, button::Status::Disabled) {
            muted_text_color(theme)
        } else {
            base_text_color(theme)
        },
        ..button::Style::default()
    }
}

fn surface_button_style(theme: &Theme, status: button::Status) -> button::Style {
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

pub(crate) fn button_surface_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container
}

pub(crate) fn button_hover_surface_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_high
}

pub(crate) fn button_pressed_surface_color(theme: &Theme) -> Color {
    ui_colors(theme).surface_container_highest
}

fn mac_scrollbar_style(
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

pub(crate) fn base_text_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface
}

pub(crate) fn muted_text_color(theme: &Theme) -> Color {
    ui_colors(theme).on_surface_variant
}

pub(crate) fn elevation_shadow_color(theme: &Theme, alpha: f32) -> Color {
    let colors = ui_colors(theme);
    Color {
        a: alpha,
        ..match colors.mode {
            AppearanceMode::Light => colors.on_background,
            AppearanceMode::Dark => colors.background,
        }
    }
}

pub(crate) fn subtle_border_color(theme: &Theme) -> Color {
    ui_colors(theme).outline_variant
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matugen_theme::{parse_matugen_theme, ui_colors};

    #[test]
    fn generated_light_and_dark_roles_drive_representative_surfaces() {
        for document in [
            include_str!("../test-data/matugen-dark.toml"),
            include_str!("../test-data/matugen-light.toml"),
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

            let close = window_close_button_style(&theme, button::Status::Hovered);
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

    #[test]
    fn preview_media_surface_is_opaque_black_without_border() {
        for theme in [Theme::Light, Theme::Dark] {
            let style = preview_media_style(&theme);
            assert_eq!(
                style.background,
                Some(Background::Color(Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                }))
            );
            assert_eq!(style.border, Border::default());
        }
    }
}
