//! 设置“动作”下的编辑页：名字、对哪些项、步骤、通知、快捷键，最下面一条固定的保存栏。
//! 表单的规则在 `crate::actions::editor`；这里只把草稿画出来，改动原样交回去。

use std::path::Path;

use iced::widget::{column, container, row, Row};
use iced::{Alignment, Element, Length};

use crate::actions::editor::{
    ActionEditor, DraftEdit, EditTarget, EditorStatus, NoticeChoice, TypeCategory,
};
use crate::app::actions::{ActionMessage, EditorMessage};
use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::ShortcutTarget;
use bennu_preview::panel_text::readable_text;
use bennu_preview::window_shell::HeaderBar;
use bennu_theme::controls::{
    check_box, destructive_button, icon_button, normal_button, suggested_button, text_field,
    SegmentedChoice,
};
use bennu_theme::icons::IconSymbol;
use bennu_theme::metrics::PAGE_MAX_WIDTH;

use super::action_editor_steps::{edit, step_sections};
use super::settings_group::{
    action_row, choice_row, custom_row, field_row, label_row, settings_card, settings_group,
    settings_page, value_row, RowNote, SettingsRow,
};
use super::shortcut_settings::push_target_rows;
use super::{themed_icon, IconTone};

const HEADER_ICON_SIZE: f32 = 14.0;

fn editor_message(message: EditorMessage) -> Message {
    Message::Action(ActionMessage::Editor(message))
}

/// 顶栏：返回（回动作列表，有未保存的修改时先问）和动作名。
pub(super) fn action_editor_header(editor: &ActionEditor) -> HeaderBar<'static, Message> {
    let name = editor.draft.name.trim();
    let title = if name.is_empty() {
        tr!("New Action")
    } else {
        name.to_owned()
    };
    HeaderBar::new().title(title).start(
        icon_button(themed_icon(
            IconSymbol::ArrowLeft,
            IconTone::Normal,
            HEADER_ICON_SIZE,
        ))
        .on_press(Message::SettingsSubpageClosed),
    )
}

pub(super) fn action_editor_page<'a>(
    browser: &'a FileBrowser,
    editor: &'a ActionEditor,
) -> Element<'a, Message> {
    // 预览用主窗口当前标签页的焦点项。
    let focused = browser.tab().selected.as_deref();
    let mut sections = vec![name_card(editor), types_group(editor)];
    sections.extend(step_sections(editor, focused));
    sections.push(notice_group(editor));
    sections.push(shortcut_group(browser, editor));
    if let Some(card) = file_card(editor) {
        sections.push(card);
    }
    column![settings_page(sections), save_bar(editor)]
        .height(Length::Fill)
        .into()
}

fn name_card(editor: &ActionEditor) -> Element<'_, Message> {
    let draft = &editor.draft;
    let name = field_row(
        tr!("Name"),
        text_field(&tr!("Shown in the context menu"), &draft.name)
            .on_input(|name| edit(DraftEdit::Name(name)))
            .width(Length::Fill),
        draft.name_mistake().map(RowNote::Error),
    );
    let file = match editor.file() {
        Some(path) => value_row(tr!("File"), file_name(path)),
        None => label_row(
            tr!("File"),
            Some(RowNote::Hint(tr!(
                "Made in the actions folder when you save, named after the action"
            ))),
        ),
    };
    settings_card(vec![name, file])
}

fn types_group(editor: &ActionEditor) -> Element<'_, Message> {
    let types = &editor.draft.types;
    let categories = Row::with_children(TypeCategory::ALL.map(|category| {
        check_box(types.contains(category))
            .label(category.label())
            .on_toggle(move |chosen| edit(DraftEdit::Category(category, chosen)))
            .into()
    }))
    .spacing(16)
    .wrap()
    .vertical_spacing(8);
    let other = types.other();
    let note = match other.mistake() {
        Some(mistake) => RowNote::Error(mistake.to_owned()),
        None => RowNote::Hint(tr!(
            "MIME types separated by spaces, for example image/png or text/*"
        )),
    };
    settings_group(
        tr!("Shown For"),
        Some(tr!(
            "Choose nothing to show it for all files and folders. With several items selected, each one has to fit."
        )),
        vec![
            custom_row(categories),
            field_row(
                tr!("Other Types"),
                text_field("image/png", other.text())
                    .on_input(|text| edit(DraftEdit::OtherTypes(text)))
                    .width(Length::Fill),
                Some(note),
            ),
        ],
    )
}

fn notice_group(editor: &ActionEditor) -> Element<'_, Message> {
    let notice = &editor.draft.notice;
    let choices = NoticeChoice::ALL
        .into_iter()
        .map(|choice| SegmentedChoice {
            label: choice.label(),
            selected: choice == notice.choice(),
            message: Some(edit(DraftEdit::Notice(choice))),
            tooltip: None,
        })
        .collect();
    let explanation = match notice.choice() {
        NoticeChoice::Standard => tr!(
            "Like other tasks: “… finished” only when the window is in the background. A failure always says why."
        ),
        NoticeChoice::Custom => tr!(
            "Your own title and text, shown even when the window is in front. A failure always says why."
        ),
        NoticeChoice::Silent => tr!("Nothing pops up, not even on failure; see the task panel."),
    };
    let mut rows = vec![choice_row(
        tr!("When It Finishes"),
        Some(RowNote::Hint(explanation)),
        choices,
    )];
    if notice.choice() == NoticeChoice::Custom {
        let variables = || {
            RowNote::Hint(tr!(
                "You can use %1",
                "{action} {count} {name} {dir} {failed}"
            ))
        };
        let note = |mistake: Option<&str>| {
            mistake
                .map(|mistake| RowNote::Error(mistake.to_owned()))
                .unwrap_or_else(variables)
        };
        rows.push(field_row(
            tr!("Title"),
            text_field("{action}", notice.title().text())
                .on_input(|text| edit(DraftEdit::NoticeTitle(text)))
                .width(Length::Fill),
            Some(note(notice.title().mistake())),
        ));
        rows.push(field_row(
            tr!("Body Text"),
            text_field("{count} → {dir}", notice.body().text())
                .on_input(|text| edit(DraftEdit::NoticeBody(text)))
                .width(Length::Fill),
            Some(note(notice.body().mistake())),
        ));
    }
    settings_group(tr!("Notification"), None, rows)
}

/// 快捷键按文件名记，新动作要先保存。录键和快捷键设置页是同一行。
fn shortcut_group<'a>(browser: &'a FileBrowser, editor: &'a ActionEditor) -> Element<'a, Message> {
    let rows: Vec<SettingsRow<'static>> = match editor.action_id() {
        Some(id) => {
            let mut rows = Vec::new();
            push_target_rows(browser, &mut rows, ShortcutTarget::Action(id.clone()));
            rows
        }
        None => vec![label_row(
            tr!("Save the action first, then give it a shortcut here."),
            None,
        )],
    };
    settings_group(tr!("Shortcut"), None, rows)
}

/// 已经有文件时：复制、打开文件、移到回收站。
fn file_card(editor: &ActionEditor) -> Option<Element<'static, Message>> {
    let path = editor.file()?;
    let unsaved = editor.has_unsaved_changes();
    Some(settings_card(vec![
        action_row(
            tr!("Duplicate This Action"),
            unsaved.then(|| RowNote::Hint(tr!("Save or discard the changes first."))),
            (!unsaved).then(|| editor_message(EditorMessage::Duplicate)),
        ),
        action_row(
            tr!("Open the File"),
            Some(RowNote::Hint(tr!(
                "Edit it as text in your default editor; changes are picked up as soon as it is saved."
            ))),
            Some(Message::Action(ActionMessage::FileOpenRequested(
                path.to_path_buf(),
            ))),
        ),
        action_row(
            tr!("Move to Trash"),
            Some(RowNote::Hint(tr!(
                "Ctrl+Z in the main window brings it back."
            ))),
            Some(editor_message(EditorMessage::Delete)),
        ),
    ]))
}

/// 页面最下面的一条，不随页面滚动：现在的状态、要用户选的事，和“保存”。
fn save_bar(editor: &ActionEditor) -> Element<'_, Message> {
    let mut buttons = Row::new().spacing(8).align_y(Alignment::Center);
    let (status, problem) = if editor.leaving.is_some() {
        buttons = buttons
            .push(
                normal_button(readable_text(tr!("Keep Editing")))
                    .on_press(editor_message(EditorMessage::LeaveCanceled)),
            )
            .push(
                destructive_button(readable_text(tr!("Discard Changes")))
                    .on_press(editor_message(EditorMessage::LeaveConfirmed)),
            );
        (tr!("The changes are not saved. Discard them?"), false)
    } else {
        match &editor.status {
            EditorStatus::Changed { found } => {
                if found.is_some() {
                    buttons = buttons.push(
                        normal_button(readable_text(tr!("Reload")))
                            .on_press(editor_message(EditorMessage::Reload)),
                    );
                }
                buttons = buttons.push(
                    normal_button(readable_text(tr!("Save Anyway")))
                        .on_press(editor_message(EditorMessage::SaveAnyway)),
                );
                let status = match found {
                    Some(_) => tr!("The file was changed somewhere else, so it was not saved."),
                    None => tr!("The file was deleted somewhere else, so it was not saved."),
                };
                (status, true)
            }
            EditorStatus::Failed(reason) => (tr!("Could not save: %1", reason), true),
            EditorStatus::Saving { .. } => (tr!("Saving…"), false),
            EditorStatus::Saved => (tr!("Saved. It takes effect right away."), false),
            EditorStatus::Editing if !editor.draft.is_complete() => {
                (tr!("Fix the marked fields to save."), false)
            }
            EditorStatus::Editing
                if editor.has_unsaved_changes() || matches!(editor.target, EditTarget::New) =>
            {
                (tr!("Not saved yet."), false)
            }
            EditorStatus::Editing => (String::new(), false),
        }
    };
    let status = readable_text(status).width(Length::Fill);
    let status = if problem {
        status.style(|theme: &iced::Theme| iced::widget::text::Style {
            color: Some(bennu_theme::ui_colors(theme).error),
        })
    } else {
        status
    };
    let save = suggested_button(readable_text(tr!("Save"))).on_press_maybe(
        editor
            .can_save()
            .then(|| editor_message(EditorMessage::Save)),
    );
    let bar = settings_card(vec![custom_row(
        row![status, buttons, save]
            .spacing(12)
            .align_y(Alignment::Center),
    )]);
    container(container(bar).max_width(PAGE_MAX_WIDTH))
        .center_x(Length::Fill)
        .padding([8, 16])
        .into()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
