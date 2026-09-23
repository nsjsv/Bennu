use iced::widget::{row, Button, Row};
use iced::Element;

use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::segmented_buttons::{
    segmented_button, segmented_button_group, SegmentedButtonTone,
};

use crate::app::panes::BrowserPaneView;
use crate::model::{BrowserPaneId, BrowserViewMode, Message};

use super::{TOOLBAR_ICON_SIZE, VIEW_MODE_ICON_SIZE};

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
    segmented_button_group(content)
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

/// 语义色调（图标着色）映射为按钮交互状态：选中段常亮、其余可点。
fn toolbar_segment_button(
    icon: IconSymbol,
    tone: IconTone,
    message: Message,
    icon_size: f32,
) -> Button<'static, Message> {
    let interaction = match tone {
        IconTone::Selected => SegmentedButtonTone::Selected,
        IconTone::Normal | IconTone::Warning => SegmentedButtonTone::Normal,
    };
    segmented_button(themed_icon(icon, tone, icon_size), interaction, message)
}
