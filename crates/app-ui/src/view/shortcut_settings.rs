use iced::widget::{row, Row, Space};
use iced::{Alignment, Element, Length};

use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::{
    CaptureRejection, CaptureSlot, LetterMode, ShortcutAction, ShortcutCaptureState, ShortcutTarget,
};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::SegmentedChoice;
use bennu_theme::controls::{icon_button, normal_button, tooltip_bubble};
use bennu_theme::icons::IconSymbol;
use bennu_theme::metrics::CONTROL_HEIGHT;

use super::settings_group::{choice_row, control_row, settings_group, RowNote, SettingsRow};
use super::{themed_icon, IconTone};

const ROW_ICON_SIZE: f32 = 14.0;

/// 每个分组一张卡片，最后是动作文件夹里的动作；正在录键的那一行上面多一行提示。
pub(super) fn shortcut_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let mut groups: Vec<(String, Vec<SettingsRow<'static>>)> = Vec::new();
    let mut current_group = None;
    for action in ShortcutAction::all().iter().copied() {
        if current_group != Some(action.group()) {
            current_group = Some(action.group());
            groups.push((action.group().label(), Vec::new()));
        }
        let rows = &mut groups.last_mut().expect("group was just pushed").1;
        push_target_rows(browser, rows, action.into());
    }
    let mut action_rows = Vec::new();
    for definition in browser.action_library.definitions() {
        push_target_rows(
            browser,
            &mut action_rows,
            ShortcutTarget::Action(definition.id.clone()),
        );
    }
    if !action_rows.is_empty() {
        groups.push((tr!("Actions"), action_rows));
    }
    let mut sections = vec![letter_mode_section(browser.user_config().letter_mode)];
    sections.extend(
        groups
            .into_iter()
            .enumerate()
            .map(|(index, (label, rows))| {
                let description = (index == 0)
                    .then(|| tr!("Click a key to record a new one, or + to add another key."));
                settings_group(label, description, rows)
            }),
    );
    sections
}

fn push_target_rows(
    browser: &FileBrowser,
    rows: &mut Vec<SettingsRow<'static>>,
    target: ShortcutTarget,
) {
    let capture = browser
        .shortcut_capture
        .as_ref()
        .filter(|capture| capture.target == target);
    if let Some(capture) = capture {
        rows.push(capture_feedback_row(browser, capture));
    }
    rows.push(target_row(browser, target, capture));
}

/// 字母键：打字跳转还是 vim 命令。
fn letter_mode_section(selected: LetterMode) -> Element<'static, Message> {
    let choices = LetterMode::ALL
        .into_iter()
        .map(|mode| SegmentedChoice {
            label: mode.label(),
            selected: mode == selected,
            message: Some(Message::LetterModeSelected(mode)),
            tooltip: None,
        })
        .collect();
    settings_group(
        tr!("Letter Keys"),
        Some(tr!(
            "What plain letters do in the file list. With Vim commands, h j k l move, y x p copy, cut and paste, d moves to the trash and / jumps to a name; F1 lists them all."
        )),
        vec![choice_row(tr!("Letter Keys"), None, choices)],
    )
}

/// 名称在左，键按钮在右，后面是“加一个键”，改过的行再加“恢复默认”（动作是“删掉键”）。
/// `capture` 是正在给这一行录的键。
fn target_row(
    browser: &FileBrowser,
    target: ShortcutTarget,
    capture: Option<&ShortcutCaptureState>,
) -> SettingsRow<'static> {
    let capturing = capture.map(|capture| capture.slot);
    let mut controls = Row::new().spacing(4).align_y(Alignment::Center);
    for (index, key) in browser.keymap().keys(target.clone()).iter().enumerate() {
        let label = if capturing == Some(CaptureSlot::Replace(index)) {
            tr!("Press keys...")
        } else {
            key.display_label()
        };
        controls = controls.push(normal_button(readable_text(label)).on_press(
            Message::ShortcutCaptureStarted(target.clone(), CaptureSlot::Replace(index)),
        ));
    }
    if capturing == Some(CaptureSlot::Add) {
        controls = controls.push(normal_button(readable_text(tr!("Press keys..."))));
    }
    controls = controls.push(row_icon_button(
        IconSymbol::Plus,
        tr!("Add a Key"),
        Message::ShortcutCaptureStarted(target.clone(), CaptureSlot::Add),
    ));
    // 没改过的行留出同宽的空位，各行的按钮才上下对齐。
    controls = if browser
        .user_config()
        .shortcuts
        .is_customized(target.clone())
    {
        controls.push(row_icon_button(
            IconSymbol::RotateCcw,
            tr!("Reset"),
            Message::ShortcutBindingReset(target.clone()),
        ))
    } else {
        controls.push(Space::new().width(Length::Fixed(CONTROL_HEIGHT)))
    };
    control_row(browser.shortcut_target_label(&target), None, controls)
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

fn capture_feedback_row(
    browser: &FileBrowser,
    capture: &ShortcutCaptureState,
) -> SettingsRow<'static> {
    let note = match &capture.rejection {
        None => RowNote::Hint(tr!("Esc cancels.")),
        Some(CaptureRejection::Unsupported) => RowNote::Error(tr!(
            "This key cannot be used. Use a function key, a named key, or a letter or digit with Ctrl or Alt."
        )),
        Some(CaptureRejection::LetterKey) => RowNote::Error(tr!(
            "Single letters, digits and symbols are for typing to jump to files and for Vim commands. Add Ctrl or Alt."
        )),
        Some(CaptureRejection::Reserved) => RowNote::Error(tr!(
            "Movement keys, Esc, Tab and Alt+1…9 are fixed and cannot be reassigned."
        )),
        Some(CaptureRejection::Conflict { key, target }) => RowNote::Error(tr!(
            "%1 conflicts with %2.",
            key.display_label(),
            browser.shortcut_target_label(target)
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
        tr!(
            "Press the new keys for %1",
            browser.shortcut_target_label(&capture.target)
        ),
        Some(note),
        controls,
    )
}
