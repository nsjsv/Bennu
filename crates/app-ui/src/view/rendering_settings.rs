use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length};

use super::settings_group::{switch_row, SettingsRow};
use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use crate::appearance::context_menu_style;
use crate::config::RenderingGpuPreference;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{normal_button, suggested_button};
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::{BOLD, TITLE_SIZE};

const RENDERER_RESTART_NOTICE_WIDTH: f32 = 420.0;

pub(super) fn rendering_gpu_preference_row(
    preference: RenderingGpuPreference,
) -> SettingsRow<'static> {
    switch_row(
        tr!("Discrete GPU"),
        None,
        matches!(preference, RenderingGpuPreference::HighPerformanceGpu),
        Message::RenderingGpuPreferenceSelected(next_rendering_gpu_preference(preference)),
    )
}

pub(super) fn renderer_restart_notice_panel() -> Element<'static, Message> {
    let title = row![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, MENU_ICON_SIZE),
        readable_text(tr!("Restart Required"))
            .size(TITLE_SIZE)
            .font(BOLD)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let actions = row![
        Space::new().width(Length::Fill),
        suggested_button(readable_text(tr!("Restart"))).on_press(Message::RendererRestartRequested),
        normal_button(readable_text(tr!("OK"))).on_press(Message::RendererRestartNoticeDismissed),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(
        column![
            title,
            readable_text(tr!(
                "Rendering GPU preference changes require restarting Bennu."
            )),
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
