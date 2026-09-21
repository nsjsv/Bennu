use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length};

use crate::appearance::context_menu_style;
use crate::config::RenderingGpuPreference;
use crate::icons::IconSymbol;
use crate::model::Message;
use crate::typography::readable_text;

use super::option_controls::{primary_action_button, secondary_action_button};
use super::settings_group::toggle_setting_row;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const RENDERER_RESTART_NOTICE_WIDTH: f32 = 420.0;

pub(super) fn rendering_gpu_preference_row(
    preference: RenderingGpuPreference,
) -> Element<'static, Message> {
    toggle_setting_row(
        "Discrete GPU",
        None,
        matches!(preference, RenderingGpuPreference::HighPerformanceGpu),
        Message::RenderingGpuPreferenceSelected(next_rendering_gpu_preference(preference)),
    )
}

pub(super) fn renderer_restart_notice_panel() -> Element<'static, Message> {
    let title = row![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, MENU_ICON_SIZE),
        readable_text("Restart Required")
            .size(16)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let actions = row![
        Space::new().width(Length::Fill),
        primary_action_button("Restart", Message::RendererRestartRequested),
        secondary_action_button("OK", Message::RendererRestartNoticeDismissed),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(
        column![
            title,
            readable_text("Rendering GPU preference changes require restarting Bennu.").size(13),
            actions,
        ]
        .spacing(12)
        .width(Length::Fill),
    )
    .padding(14)
    .width(Length::Fixed(RENDERER_RESTART_NOTICE_WIDTH))
    .style(context_menu_style)
    .into()
}

fn next_rendering_gpu_preference(preference: RenderingGpuPreference) -> RenderingGpuPreference {
    match preference {
        RenderingGpuPreference::DisplayGpu => RenderingGpuPreference::HighPerformanceGpu,
        RenderingGpuPreference::HighPerformanceGpu => RenderingGpuPreference::DisplayGpu,
    }
}
