use iced::widget::{row, Row, Space};
use iced::{Alignment, Element, Length};

use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::{
    CaptureRejection, CaptureSlot, ShortcutAction, ShortcutCaptureState, ShortcutConfig,
    ShortcutGroup,
};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{icon_button, normal_button, tooltip_bubble};
use bennu_theme::icons::IconSymbol;
use bennu_theme::metrics::CONTROL_HEIGHT;

use super::settings_group::{control_row, settings_group, RowNote, SettingsRow};
use super::{themed_icon, IconTone};

const ROW_ICON_SIZE: f32 = 14.0;

/// 每个分组一张卡片；正在录键的动作上面多一行提示。
pub(super) fn shortcut_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let config = browser.shortcut_config();
    let capture = browser.shortcut_capture.as_ref();
    let mut groups: Vec<(ShortcutGroup, Vec<SettingsRow<'static>>)> = Vec::new();
    for action in ShortcutAction::all().iter().copied() {
        let rows = match groups.last_mut() {
            Some((group, rows)) if *group == action.group() => rows,
            _ => {
                groups.push((action.group(), Vec::new()));
                &mut groups.last_mut().expect("group was just pushed").1
            }
        };
        if let Some(capture) = capture.filter(|capture| capture.action == action) {
            rows.push(capture_feedback_row(capture));
        }
        rows.push(action_row(config, action, capture));
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(index, (group, rows))| {
            let description = (index == 0)
                .then(|| tr!("Click a key to record a new one, or + to add another key."));
            settings_group(group.label(), description, rows)
        })
        .collect()
}

/// 名称在左，键按钮在右，后面是“加一个键”，改过的行再加“恢复默认”。
fn action_row(
    config: &ShortcutConfig,
    action: ShortcutAction,
    capture: Option<&ShortcutCaptureState>,
) -> SettingsRow<'static> {
    let capturing = capture
        .filter(|capture| capture.action == action)
        .map(|capture| capture.slot);
    let mut controls = Row::new().spacing(4).align_y(Alignment::Center);
    for (index, key) in config.keys(action).iter().enumerate() {
        let label = if capturing == Some(CaptureSlot::Replace(index)) {
            tr!("Press keys...")
        } else {
            key.display_label()
        };
        controls = controls.push(normal_button(readable_text(label)).on_press(
            Message::ShortcutCaptureStarted(action, CaptureSlot::Replace(index)),
        ));
    }
    if capturing == Some(CaptureSlot::Add) {
        controls = controls.push(normal_button(readable_text(tr!("Press keys..."))));
    }
    controls = controls.push(row_icon_button(
        IconSymbol::Plus,
        tr!("Add a Key"),
        Message::ShortcutCaptureStarted(action, CaptureSlot::Add),
    ));
    // 没改过的行留出同宽的空位，各行的按钮才上下对齐。
    controls = if config.is_customized(action) {
        controls.push(row_icon_button(
            IconSymbol::RotateCcw,
            tr!("Reset"),
            Message::ShortcutBindingReset(action),
        ))
    } else {
        controls.push(Space::new().width(Length::Fixed(CONTROL_HEIGHT)))
    };
    control_row(action.label(), None, controls)
}

fn row_icon_button(
    icon: IconSymbol,
    tooltip: String,
    message: Message,
) -> Element<'static, Message> {
    iced::widget::tooltip(
        icon_button(themed_icon(icon, IconTone::Normal, ROW_ICON_SIZE)).on_press(message),
        tooltip_bubble(tooltip),
        iced::widget::tooltip::Position::Left,
    )
    .into()
}

fn capture_feedback_row(capture: &ShortcutCaptureState) -> SettingsRow<'static> {
    let note = match capture.rejection {
        None => RowNote::Hint(tr!("Esc cancels.")),
        Some(CaptureRejection::Unsupported) => RowNote::Error(tr!(
            "This key cannot be used. Use a function key, a named key, or a letter or digit with Ctrl or Alt."
        )),
        Some(CaptureRejection::LetterKey) => RowNote::Error(tr!(
            "Single letters, digits and symbols are for typing to jump to files. Add Ctrl or Alt."
        )),
        Some(CaptureRejection::Reserved) => RowNote::Error(tr!(
            "Movement keys, Esc, Tab and Alt+1…9 are fixed and cannot be reassigned."
        )),
        Some(CaptureRejection::Conflict { key, action }) => RowNote::Error(tr!(
            "%1 conflicts with %2.",
            key.display_label(),
            action.label()
        )),
    };
    let mut controls = row![
        normal_button(readable_text(tr!("Cancel"))).on_press(Message::ShortcutCaptureCanceled)
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if matches!(capture.slot, CaptureSlot::Replace(_)) {
        controls = controls.push(
            normal_button(readable_text(tr!("Remove This Key")))
                .on_press(Message::ShortcutCapturedKeyRemoved),
        );
    }
    control_row(
        tr!("Press the new keys for %1", capture.action.label()),
        Some(note),
        controls,
    )
}
