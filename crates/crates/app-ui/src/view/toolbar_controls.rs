use iced::widget::{button, container, row, Button, Row};
use iced::{Alignment, Background, Border, Color, Element, Theme};

use crate::app::panes::BrowserPaneView;
use crate::appearance::{
    base_text_color, button_hover_surface_color, button_pressed_surface_color,
    button_surface_color, muted_text_color, subtle_border_color,
};
use crate::icons::IconSymbol;
use crate::model::{BrowserPaneId, BrowserViewMode, Message};

use super::{themed_icon, IconTone, TOOLBAR_ICON_SIZE, VIEW_MODE_ICON_SIZE};

pub(super) fn navigation_button_group(pane_id: BrowserPaneId) -> Element<'static, Message> {
    toolbar_button_group(row![
        toolbar_segment_button(
            IconSymbol::ArrowLeft,
            IconTone::Normal,
            Message::PaneBack(pane_id),
            TOOLBAR_ICON_SIZE,
        ),
        toolbar_segment_button(
            IconSymbol::ArrowRight,
            IconTone::Normal,
            Message::PaneForward(pane_id),
            TOOLBAR_ICON_SIZE,
        ),
        toolbar_segment_button(
            IconSymbol::ArrowUp,
            IconTone::Normal,
            Message::PaneUp(pane_id),
            TOOLBAR_ICON_SIZE,
        ),
    ])
}

pub(super) fn view_mode_button_group(pane: BrowserPaneView<'_>) -> Element<'static, Message> {
    toolbar_button_group(row![
        view_mode_button(
            pane.id,
            pane.view_mode,
            BrowserViewMode::Columns,
            IconSymbol::Columns,
        ),
        view_mode_button(
            pane.id,
            pane.view_mode,
            BrowserViewMode::List,
            IconSymbol::List,
        ),
        view_mode_button(
            pane.id,
            pane.view_mode,
            BrowserViewMode::Icons,
            IconSymbol::Grid,
        ),
    ])
}

/// 右侧预览面板开关:状态全局唯一,分栏时两个窗格的按钮自然同步亮灭。
pub(super) fn right_preview_panel_toggle_button(is_open: bool) -> Element<'static, Message> {
    let tone = if is_open {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    toolbar_button_group(row![toolbar_segment_button(
        IconSymbol::PanelRight,
        tone,
        Message::ToggleRightPreviewPanel,
        VIEW_MODE_ICON_SIZE,
    )])
}

fn toolbar_button_group(content: Row<'static, Message>) -> Element<'static, Message> {
    container(content.spacing(0).align_y(Alignment::Center))
        .clip(true)
        .style(toolbar_button_group_style)
        .into()
}

fn view_mode_button(
    pane_id: BrowserPaneId,
    current_mode: BrowserViewMode,
    target_mode: BrowserViewMode,
    icon: IconSymbol,
) -> Button<'static, Message> {
    let tone = if current_mode == target_mode {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    toolbar_segment_button(
        icon,
        tone,
        Message::BrowserViewModeSelected(pane_id, target_mode),
        VIEW_MODE_ICON_SIZE,
    )
}

fn toolbar_segment_button(
    icon: IconSymbol,
    tone: IconTone,
    message: Message,
    icon_size: f32,
) -> Button<'static, Message> {
    button(themed_icon(icon, tone, icon_size))
        .on_press(message)
        .padding([8, 10])
        .style(toolbar_segment_button_style())
}

fn toolbar_segment_button_style() -> fn(&Theme, button::Status) -> button::Style {
    toolbar_segment_button_appearance
}

fn toolbar_segment_button_appearance(theme: &Theme, status: button::Status) -> button::Style {
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
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..button::Style::default()
    }
}

fn toolbar_button_group_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(button_surface_color(theme))),
        text_color: Some(base_text_color(theme)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 7.0.into(),
        },
        ..container::Style::default()
    }
}
