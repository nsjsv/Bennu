use iced::widget::{button, column, container, row, Space};
use iced::{Alignment, Element, Length};

use crate::app::FileBrowser;
use crate::appearance::context_menu_button_style;
use crate::model::Message;
use crate::shortcuts::{ShortcutBinding, ShortcutCaptureState};
use crate::typography::readable_text;

use super::settings_group::{muted_setting_text, settings_card};

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
        "Click a shortcut, then press the replacement keys.",
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
        crate::localization::translate_current("Press keys...")
    } else {
        binding.binding.config_value()
    };

    container(
        row![
            readable_text(binding.id.label())
                .size(12)
                .width(Length::Fill),
            button(
                container(readable_text(binding_label).size(12))
                    .padding([5, 8])
                    .width(Length::Fixed(SHORTCUT_BINDING_BUTTON_WIDTH)),
            )
            .on_press(Message::ShortcutCaptureStarted(binding.id))
            .style(context_menu_button_style()),
            button(readable_text("Reset").size(12))
                .on_press(Message::ShortcutBindingReset(binding.id))
                .padding([6, 10])
                .style(context_menu_button_style()),
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
        crate::localization::translate_current(
            "Unsupported shortcut. Use a letter, number, function key, arrow, or named edit key.",
        )
    } else if let Some(conflict) = capture.conflict_binding_id {
        let rejected = capture
            .rejected_binding
            .as_ref()
            .map(|binding| binding.config_value())
            .unwrap_or_else(|| crate::localization::translate_current("Shortcut"));
        if crate::localization::current_language_is_chinese() {
            format!(
                "{rejected} 与 {} 冲突。",
                crate::localization::translate_current(conflict.label())
            )
        } else {
            format!("{rejected} conflicts with {}.", conflict.label())
        }
    } else {
        if crate::localization::current_language_is_chinese() {
            format!(
                "正在监听 {}...",
                crate::localization::translate_current(capture.binding_id.label())
            )
        } else {
            format!("Listening for {}...", capture.binding_id.label())
        }
    };

    row![
        readable_text(message).size(12).width(Length::Fill),
        button(readable_text("Cancel").size(12))
            .on_press(Message::ShortcutCaptureCanceled)
            .padding([6, 10])
            .style(context_menu_button_style()),
        Space::new().width(Length::Shrink),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}
