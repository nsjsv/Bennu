use iced::widget::{column, container, row, Space};
use iced::{Alignment, Element, Length};

use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::{ShortcutBinding, ShortcutCaptureState};
use bennu_preview::panel_text::readable_text;

use super::settings_group::{muted_setting_text, settings_card};
use bennu_theme::controls::normal_button;

const SHORTCUT_BINDING_BUTTON_WIDTH: f32 = 142.0;

pub(super) fn shortcut_settings_section(browser: &FileBrowser) -> Element<'_, Message> {
    let mut shortcut_rows: Vec<Element<'static, Message>> = Vec::new();
    for binding in browser.shortcut_config().bindings() {
        shortcut_rows.push(shortcut_binding_row(
            binding,
            browser.shortcut_capture.as_ref(),
        ));
    }

    let mut section = column![muted_setting_text(
        tr!("Click a shortcut, then press the replacement keys."),
        12,
    )]
    .spacing(6)
    .width(Length::Fill);

    if let Some(capture) = &browser.shortcut_capture {
        section = section.push(shortcut_capture_feedback(capture));
    }

    section.push(settings_card(shortcut_rows)).into()
}

fn shortcut_binding_row(
    binding: &ShortcutBinding,
    capture: Option<&ShortcutCaptureState>,
) -> Element<'static, Message> {
    let is_capturing = capture.is_some_and(|capture| capture.binding_id == binding.id);
    let binding_label = if is_capturing {
        tr!("Press keys...")
    } else {
        binding.binding.config_value()
    };

    container(
        row![
            readable_text(binding.id.label()).width(Length::Fill),
            normal_button(readable_text(binding_label))
                .width(Length::Fixed(SHORTCUT_BINDING_BUTTON_WIDTH))
                .on_press(Message::ShortcutCaptureStarted(binding.id)),
            normal_button(readable_text(tr!("Reset")))
                .on_press(Message::ShortcutBindingReset(binding.id)),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .padding([4, 8])
    .width(Length::Fill)
    .into()
}

fn shortcut_capture_feedback(capture: &ShortcutCaptureState) -> Element<'static, Message> {
    let message = if capture.unsupported_key {
        tr!("Unsupported shortcut. Use a letter, number, function key, arrow, or named edit key.")
    } else if let Some(conflict) = capture.conflict_binding_id {
        let rejected = capture
            .rejected_binding
            .as_ref()
            .map(|binding| binding.config_value())
            .unwrap_or_else(|| tr!("Shortcut"));
        tr!("%1 conflicts with %2.", rejected, conflict.label())
    } else {
        tr!("Listening for %1...", capture.binding_id.label())
    };

    row![
        readable_text(message).width(Length::Fill),
        normal_button(readable_text(tr!("Cancel"))).on_press(Message::ShortcutCaptureCanceled),
        Space::new().width(Length::Shrink),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}
