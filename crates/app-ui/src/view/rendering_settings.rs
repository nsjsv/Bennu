use iced::{Element, Length};

use super::dialog::Dialog;
use super::settings_group::{switch_row, SettingsRow};
use crate::config::RenderingGpuPreference;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{normal_button, suggested_button};

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
    Dialog::new(
        tr!("Restart Required"),
        readable_text(tr!(
            "Rendering GPU preference changes require restarting Bennu."
        ))
        .width(Length::Fill),
    )
    .actions(vec![
        normal_button(readable_text(tr!("OK")))
            .on_press(Message::RendererRestartNoticeDismissed)
            .into(),
        suggested_button(readable_text(tr!("Restart")))
            .on_press(Message::RendererRestartRequested)
            .into(),
    ])
    .width(RENDERER_RESTART_NOTICE_WIDTH)
    .into()
}

fn next_rendering_gpu_preference(preference: RenderingGpuPreference) -> RenderingGpuPreference {
    match preference {
        RenderingGpuPreference::DisplayGpu => RenderingGpuPreference::HighPerformanceGpu,
        RenderingGpuPreference::HighPerformanceGpu => RenderingGpuPreference::DisplayGpu,
    }
}
