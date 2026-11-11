use iced::widget::{row, Space};
use iced::{Alignment, Element, Length};

use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::{ShortcutBinding, ShortcutCaptureState, ShortcutConfig};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{icon_button, normal_button, tooltip_bubble};
use bennu_theme::icons::IconSymbol;
use bennu_theme::metrics::CONTROL_HEIGHT;

use super::settings_group::{control_row, settings_group, SettingsRow};
use super::{themed_icon, IconTone};

const SHORTCUT_BINDING_BUTTON_WIDTH: f32 = 142.0;
const ROW_ICON_SIZE: f32 = 14.0;

pub(super) fn shortcut_settings_section(browser: &FileBrowser) -> Element<'_, Message> {
    let capture = browser.shortcut_capture.as_ref();
    let mut rows = Vec::new();
    if let Some(capture) = capture {
        rows.push(shortcut_capture_feedback(capture));
    }
    rows.extend(
        browser
            .shortcut_config()
            .bindings()
            .iter()
            .map(|binding| shortcut_binding_row(binding, capture)),
    );
    settings_group(
        tr!("Shortcuts"),
        Some(tr!("Click a shortcut, then press the replacement keys.")),
        rows,
    )
}

/// 按键按钮在右边；改过的快捷键后面才有“恢复默认”图标（没改过的行只显示按键）。
fn shortcut_binding_row(
    binding: &ShortcutBinding,
    capture: Option<&ShortcutCaptureState>,
) -> SettingsRow<'static> {
    let is_capturing = capture.is_some_and(|capture| capture.binding_id == binding.id);
    let binding_label = if is_capturing {
        tr!("Press keys...")
    } else {
        binding.binding.config_value()
    };
    let mut controls = row![normal_button(readable_text(binding_label))
        .width(Length::Fixed(SHORTCUT_BINDING_BUTTON_WIDTH))
        .on_press(Message::ShortcutCaptureStarted(binding.id))]
    .spacing(4)
    .align_y(Alignment::Center);
    // 没改过的行留出同宽的空位，各行的按键按钮才上下对齐。
    controls = if binding.binding != ShortcutConfig::default_binding(binding.id) {
        controls.push(iced::widget::tooltip(
            icon_button(themed_icon(
                IconSymbol::RotateCcw,
                IconTone::Normal,
                ROW_ICON_SIZE,
            ))
            .on_press(Message::ShortcutBindingReset(binding.id)),
            tooltip_bubble(tr!("Reset")),
            iced::widget::tooltip::Position::Left,
        ))
    } else {
        controls.push(Space::new().width(Length::Fixed(CONTROL_HEIGHT)))
    };
    control_row(binding.id.label(), None, controls)
}

fn shortcut_capture_feedback(capture: &ShortcutCaptureState) -> SettingsRow<'static> {
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
    control_row(
        message,
        None,
        normal_button(readable_text(tr!("Cancel"))).on_press(Message::ShortcutCaptureCanceled),
    )
}
