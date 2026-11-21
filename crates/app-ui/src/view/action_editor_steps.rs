//! 编辑页的步骤：一步一张卡片。第一行是种类和上移、下移、删除，第二行是这种步骤的值；
//! 文件夹和命令下面显示用主窗口焦点项展开后的样子。

use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use iced::widget::Row;
use iced::{Alignment, Element, Length};

use crate::actions::editor::{ActionEditor, DraftEdit, StepDraft, StepKind, StepOption};
use crate::actions::{ConvertFormat, TrashScope};
use crate::app::actions::{ActionMessage, EditorMessage};
use crate::model::{ConvertibleMediaKind, Message};
use bennu_theme::controls::{dropdown, icon_button, text_field, tooltip_bubble};
use bennu_theme::icons::IconSymbol;
use file_core::{ArchiveExtractionPlacement, ArchiveFormat};

use super::settings_group::{
    action_row, control_row, field_row, notice_row, settings_card, settings_group, RowNote,
    SettingsRow,
};
use super::settings_window::SETTINGS_DROPDOWN_WIDTH;
use super::{themed_icon, IconTone};

const ROW_ICON_SIZE: f32 = 14.0;

pub(super) fn edit(edit: DraftEdit) -> Message {
    Message::Action(ActionMessage::Editor(EditorMessage::Edit(edit)))
}

/// “步骤”一组：每一步一张卡片，最后是“添加一步”。
pub(super) fn step_sections<'a>(
    editor: &'a ActionEditor,
    focused: Option<&'a Path>,
) -> Vec<Element<'a, Message>> {
    let draft = &editor.draft;
    let mut cards = draft
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| step_rows(editor, index, step, focused))
        .collect::<Vec<_>>();
    let first = match draft.steps_mistake() {
        Some(mistake) => vec![notice_row(RowNote::Error(mistake))],
        None => cards.remove(0),
    };
    let mut sections = vec![settings_group(
        tr!("Steps"),
        Some(tr!(
            "They run from top to bottom; each step works on what the step before made."
        )),
        first,
    )];
    sections.extend(cards.into_iter().map(settings_card));
    sections.push(settings_card(vec![action_row(
        tr!("Add a Step"),
        None,
        Some(edit(DraftEdit::StepAdded)),
    )]));
    sections
}

fn step_rows<'a>(
    editor: &'a ActionEditor,
    index: usize,
    step: &'a StepDraft,
    focused: Option<&'a Path>,
) -> Vec<SettingsRow<'a>> {
    let count = editor.draft.steps.len();
    let mut controls = Row::new().spacing(4).align_y(Alignment::Center).push(
        dropdown(
            StepKind::ALL.map(KindChoice),
            Some(KindChoice(step.kind())),
            move |choice: KindChoice| edit(DraftEdit::StepKind(index, choice.0)),
        )
        .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH)),
    );
    controls = controls
        .push(step_button(
            IconSymbol::ArrowUp,
            tr!("Move Up"),
            (index > 0).then_some(DraftEdit::StepMovedUp(index)),
        ))
        .push(step_button(
            IconSymbol::ArrowDown,
            tr!("Move Down"),
            (index + 1 < count).then_some(DraftEdit::StepMovedDown(index)),
        ))
        .push(step_button(
            IconSymbol::Trash,
            tr!("Remove This Step"),
            Some(DraftEdit::StepRemoved(index)),
        ));
    // 输入框自己的错写在框下面；这里只写“移到回收站要在最后”这种整步的错。
    let step_note = step
        .text_field_mistake()
        .is_none()
        .then(|| editor.draft.step_mistake(index))
        .flatten()
        .map(RowNote::Error);
    let mut rows = vec![control_row(tr!("Step %1", index + 1), step_note, controls)];
    if let Some(value_row) = value_row(editor, index, step, focused) {
        rows.push(value_row);
    }
    rows
}

fn step_button(
    icon: IconSymbol,
    tooltip: String,
    edit_on_press: Option<DraftEdit>,
) -> Element<'static, Message> {
    let button = icon_button(themed_icon(icon, IconTone::Normal, ROW_ICON_SIZE))
        .on_press_maybe(edit_on_press.map(edit));
    bennu_theme::controls::tooltip(
        button,
        tooltip_bubble(tooltip),
        iced::widget::tooltip::Position::Top,
    )
    .into()
}

/// 这种步骤的值：下拉框，或者文件夹、命令的输入框。复制路径没有值。
fn value_row<'a>(
    editor: &'a ActionEditor,
    index: usize,
    step: &'a StepDraft,
    focused: Option<&'a Path>,
) -> Option<SettingsRow<'a>> {
    let option_row =
        |title: String, element: Element<'a, Message>| Some(control_row(title, None, element));
    let choose = move |option: StepOption| edit(DraftEdit::StepOption(index, option));
    match step {
        StepDraft::Convert(format) => option_row(
            tr!("Format"),
            dropdown(
                crate::actions::CONVERT_FORMATS
                    .iter()
                    .copied()
                    .map(FormatChoice)
                    .collect::<Vec<_>>(),
                Some(FormatChoice(*format)),
                move |choice: FormatChoice| choose(StepOption::Format(choice.0)),
            )
            .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
            .into(),
        ),
        StepDraft::Compress(format) => option_row(
            tr!("Format"),
            dropdown(
                crate::actions::COMPRESS_FORMATS.map(ArchiveChoice),
                Some(ArchiveChoice(*format)),
                move |choice: ArchiveChoice| choose(StepOption::Archive(choice.0)),
            )
            .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
            .into(),
        ),
        StepDraft::Extract(placement) => option_row(
            tr!("Where"),
            dropdown(
                [
                    ArchiveExtractionPlacement::Smart,
                    ArchiveExtractionPlacement::ArchiveFolder,
                ]
                .map(PlacementChoice),
                Some(PlacementChoice(*placement)),
                move |choice: PlacementChoice| choose(StepOption::Placement(choice.0)),
            )
            .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
            .into(),
        ),
        StepDraft::Trash(scope) => option_row(
            tr!("What"),
            dropdown(
                [TrashScope::Current, TrashScope::Selected].map(ScopeChoice),
                Some(ScopeChoice(*scope)),
                move |choice: ScopeChoice| choose(StepOption::Scope(choice.0)),
            )
            .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
            .into(),
        ),
        StepDraft::CopyTo(folder) | StepDraft::MoveTo(folder) => {
            let preview = folder.value().zip(focused).map(|(template, item)| {
                tr!(
                    "For %1: %2",
                    file_name(item),
                    template.resolve(item, &editor.run_values).display()
                )
            });
            let note = field_note(
                folder.mistake(),
                preview,
                "{dir} {name} {stem} {ext} {date}",
            );
            Some(text_row(
                tr!("Folder"),
                index,
                folder.text(),
                "{dir}/…",
                note,
            ))
        }
        StepDraft::Run(command) => {
            let preview = command.value().zip(focused).map(|(template, item)| {
                match template.invocations(&[item.to_path_buf()], &editor.run_values) {
                    Ok(invocations) => Ok(tr!(
                        "For %1: %2",
                        file_name(item),
                        invocations
                            .first()
                            .map(|words| shell_line(words))
                            .unwrap_or_default()
                    )),
                    Err(refused) => Err(refused.reason()),
                }
            });
            let (preview, refused) = match preview {
                Some(Ok(line)) => (Some(line), None),
                Some(Err(reason)) => (None, Some(reason)),
                None => (None, None),
            };
            let mistake = command.mistake().map(str::to_owned).or(refused);
            let note = field_note(
                mistake.as_deref(),
                preview,
                "{file} {files} {name} {stem} {ext} {dir} {date}",
            );
            Some(text_row(
                tr!("Command"),
                index,
                command.text(),
                "ffmpeg -i {file} …",
                note,
            ))
        }
        StepDraft::CopyPaths => None,
    }
}

fn text_row<'a>(
    title: String,
    index: usize,
    text: &'a str,
    placeholder: &str,
    note: RowNote,
) -> SettingsRow<'a> {
    let field = text_field(placeholder, text)
        .on_input(move |text| edit(DraftEdit::StepText(index, text)))
        .width(Length::Fill);
    field_row(title, field, Some(note))
}

/// 输入框下面一行：有错时是错；没错时是展开后的样子（有焦点项时）和能用的变量。
fn field_note(mistake: Option<&str>, preview: Option<String>, variables: &str) -> RowNote {
    if let Some(mistake) = mistake {
        return RowNote::Error(mistake.to_owned());
    }
    let variables = tr!("You can use %1", variables);
    RowNote::Hint(match preview {
        Some(preview) => format!("{preview}\n{variables}"),
        None => variables,
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// 预览里的一行命令：按 shell 的写法给需要的参数加上单引号。只为显示，执行时不经过 shell。
fn shell_line(words: &[OsString]) -> String {
    words
        .iter()
        .map(|word| {
            let word = word.to_string_lossy();
            let plain = !word.is_empty()
                && word.chars().all(|character| {
                    character.is_alphanumeric() || "-_./=:,+@%~".contains(character)
                });
            if plain {
                word.into_owned()
            } else {
                format!("'{}'", word.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KindChoice(StepKind);

impl fmt::Display for KindChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0.label())
    }
}

/// 格式名后面写上它能转哪类文件：`m4a（视频、音频）`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FormatChoice(ConvertFormat);

impl fmt::Display for FormatChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sources = [
            (ConvertibleMediaKind::Image, tr!("Images")),
            (ConvertibleMediaKind::Video, tr!("Video")),
            (ConvertibleMediaKind::Audio, tr!("Audio")),
        ]
        .into_iter()
        .filter(|(kind, _)| self.0.target_for(*kind).is_some())
        .map(|(_, label)| label)
        .reduce(|joined, label| tr!("%1, %2", joined, label))
        .unwrap_or_default();
        formatter.write_str(&tr!("%1 (%2)", self.0.name(), sources))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArchiveChoice(ArchiveFormat);

impl fmt::Display for ArchiveChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.extension())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlacementChoice(ArchiveExtractionPlacement);

impl fmt::Display for PlacementChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&match self.0 {
            ArchiveExtractionPlacement::Smart => tr!("Smart Extract Here"),
            ArchiveExtractionPlacement::ArchiveFolder => tr!("Extract to Archive Folder"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScopeChoice(TrashScope);

impl fmt::Display for ScopeChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&match self.0 {
            TrashScope::Current => tr!("What the step before made"),
            TrashScope::Selected => tr!("The items selected at the start"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_quotes_only_what_needs_it() {
        let words = ["ffmpeg", "-i", "/home/me/会议 1.mp4", "it's", ""].map(OsString::from);
        assert_eq!(
            shell_line(&words),
            "ffmpeg -i '/home/me/会议 1.mp4' 'it'\\''s' ''"
        );
    }
}
