use iced::{Background, Border, Color, Theme};

use super::{base_text_color, button_hover_surface_color, container};
use crate::matugen_theme::ui_colors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListHeaderCellVisualState {
    Idle,
    Hovered,
    Dragged,
    DropTarget,
}

pub(crate) fn list_header_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        text_color: Some(base_text_color(theme)),
        ..container::Appearance::default()
    }
}

/// 分组标题条:低调全宽标题,无背景无描边,仅用次要文字色与内容区
/// 分(表头同族的"面板 chrome"视觉)。列表组头行与大图组头条共用。
pub(crate) fn group_header_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        text_color: Some(ui_colors(theme).on_surface_variant),
        ..container::Appearance::default()
    }
}

pub(crate) fn list_header_cell_style(
    state: ListHeaderCellVisualState,
) -> impl Fn(&Theme) -> container::Appearance + Clone {
    move |theme| match state {
        ListHeaderCellVisualState::Idle => container::Appearance::default(),
        ListHeaderCellVisualState::Hovered => container::Appearance {
            background: Some(Background::Color(Color {
                a: 0.65,
                ..button_hover_surface_color(theme)
            })),
            text_color: Some(base_text_color(theme)),
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..container::Appearance::default()
        },
        ListHeaderCellVisualState::Dragged | ListHeaderCellVisualState::DropTarget => {
            active_list_header_cell_style(theme, state)
        }
    }
}

fn active_list_header_cell_style(
    theme: &Theme,
    state: ListHeaderCellVisualState,
) -> container::Appearance {
    let colors = ui_colors(theme);
    let accent = colors.primary;
    let is_drop_target = state == ListHeaderCellVisualState::DropTarget;
    let background = if is_drop_target {
        Background::Color(Color { a: 0.18, ..accent })
    } else {
        Background::Color(Color {
            a: 0.72,
            ..colors.primary_container
        })
    };
    let border = Border {
        color: if is_drop_target {
            Color { a: 0.92, ..accent }
        } else {
            Color { a: 0.58, ..accent }
        },
        width: 1.0,
        radius: 6.0.into(),
    };

    container::Appearance {
        background: Some(background),
        text_color: Some(base_text_color(theme)),
        border,
        ..container::Appearance::default()
    }
}

pub(crate) fn list_header_reorder_indicator_style(theme: &Theme) -> container::Appearance {
    container::Appearance {
        background: Some(Background::Color(list_header_reorder_accent_color(theme))),
        border: Border {
            radius: 999.0.into(),
            ..Border::default()
        },
        ..container::Appearance::default()
    }
}

fn list_header_reorder_accent_color(theme: &Theme) -> Color {
    ui_colors(theme).primary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_header_is_clear_and_pointer_states_remain_visible() {
        for theme in [Theme::Light, Theme::Dark] {
            let header = list_header_style(&theme);
            assert!(header.background.is_none());
            assert_eq!(header.border.width, 0.0);
            assert!(header.text_color.is_some());

            let idle = list_header_cell_style(ListHeaderCellVisualState::Idle)(&theme);
            assert!(idle.background.is_none());
            let hovered = list_header_cell_style(ListHeaderCellVisualState::Hovered)(&theme);
            assert!(hovered.background.is_some());
            assert_eq!(hovered.border.width, 0.0);

            for state in [
                ListHeaderCellVisualState::Dragged,
                ListHeaderCellVisualState::DropTarget,
            ] {
                let active = list_header_cell_style(state)(&theme);
                assert!(active.background.is_some());
                assert_eq!(active.border.width, 1.0);
            }
        }
    }
}
