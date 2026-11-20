//! 设置「动作」页：动作文件夹里有哪些动作、各自做什么、绑了什么键；用不了的文件和原因。
//! 这一页只看不改：动作就是普通的文本文件，在文件夹里直接改。

use iced::Element;

use crate::actions::{action_folder, ActionDefinition, ActionFileProblem};
use crate::app::actions::ActionMessage;
use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::ShortcutTarget;

use super::settings_group::{action_row, label_row, settings_card, settings_group, RowNote};

pub(super) fn action_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let folder = action_folder();
    let definitions = browser.action_library.definitions();
    let mut sections = vec![settings_group(
        tr!("Actions"),
        Some(tr!(
            "Each .toml file in %1 is an action: it shows up in the context menu of the items it fits, and can have a shortcut on the Shortcuts page. Changes take effect as soon as a file is saved.",
            folder.display()
        )),
        if definitions.is_empty() {
            vec![label_row(
                tr!("No actions yet"),
                Some(RowNote::Hint(tr!(
                    "Open the folder and add a file; the documentation has examples."
                ))),
            )]
        } else {
            definitions
                .iter()
                .map(|definition| action_summary_row(browser, definition))
                .collect()
        },
    )];
    let problems = browser.action_library.problems();
    if !problems.is_empty() {
        sections.push(settings_group(
            tr!("Not Used"),
            Some(tr!("These files are skipped until they are fixed.")),
            problems.iter().map(problem_row).collect(),
        ));
    }
    sections.push(settings_card(vec![action_row(
        tr!("Open the Actions Folder"),
        None,
        Some(Message::Action(ActionMessage::FolderOpenRequested)),
    )]));
    sections
}

/// 名字；下面一行是文件名、对哪些项、步骤和键。
fn action_summary_row(
    browser: &FileBrowser,
    definition: &ActionDefinition,
) -> super::settings_group::SettingsRow<'static> {
    let steps = definition
        .steps
        .iter()
        .map(|step| step.label())
        .collect::<Vec<_>>()
        .join(" → ");
    let keys = browser
        .keymap()
        .keys(ShortcutTarget::Action(definition.id.clone()))
        .iter()
        .map(|key| key.display_label())
        .collect::<Vec<_>>();
    let mut details = vec![
        format!("{}.toml", definition.id.file_stem()),
        definition.types.describe(),
        steps,
    ];
    if !keys.is_empty() {
        details.push(keys.join(" / "));
    }
    label_row(
        definition.name.clone(),
        Some(RowNote::Hint(details.join(" · "))),
    )
}

fn problem_row(problem: &ActionFileProblem) -> super::settings_group::SettingsRow<'static> {
    let name = problem
        .file
        .file_name()
        .unwrap_or(problem.file.as_os_str())
        .to_string_lossy()
        .into_owned();
    let title = match problem.line {
        Some(line) => tr!("%1, line %2", name, line),
        None => name,
    };
    label_row(title, Some(RowNote::Error(problem.reason.clone())))
}
