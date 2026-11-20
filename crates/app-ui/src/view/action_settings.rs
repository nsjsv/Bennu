//! 设置「动作」页：动作文件夹里有哪些动作、各自做什么、绑了什么键；读到了哪些 Dolphin 服务菜单；
//! 用不了的文件和原因。这一页只看不改：动作和服务菜单都是普通的文本文件，直接改文件。

use iced::Element;

use crate::actions::{
    action_folder, ActionDefinition, ActionFileProblem, ActionId, ServiceMenuAction,
};
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
    let mut service_menus = browser.action_library.service_menus().to_vec();
    if !service_menus.is_empty() {
        service_menus.sort_by(|left, right| left.name.cmp(&right.name));
        sections.push(settings_group(
            tr!("Dolphin Service Menus"),
            Some(tr!(
                "Read from kio/servicemenus in the data folders, by the same rules as Dolphin. Ones turned off in Dolphin's settings are left out. They can have shortcuts too."
            )),
            service_menus
                .iter()
                .map(|action| service_menu_row(browser, action))
                .collect(),
        ));
    }
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
    let mut details = vec![
        file_name(&definition.file),
        definition.types.describe(),
        steps,
    ];
    details.extend(key_labels(browser, definition.id.clone()));
    label_row(
        definition.name.clone(),
        Some(RowNote::Hint(details.join(" · "))),
    )
}

/// 名字；下面一行是文件名、对哪些项和键。
fn service_menu_row(
    browser: &FileBrowser,
    action: &ServiceMenuAction,
) -> super::settings_group::SettingsRow<'static> {
    let mut details = vec![file_name(&action.file), action.types.describe()];
    details.extend(key_labels(browser, action.id()));
    label_row(
        action.name.clone(),
        Some(RowNote::Hint(details.join(" · "))),
    )
}

/// 绑了键时是"键 / 键"。
fn key_labels(browser: &FileBrowser, id: ActionId) -> Option<String> {
    let keys = browser
        .keymap()
        .keys(ShortcutTarget::Action(id))
        .iter()
        .map(|key| key.display_label())
        .collect::<Vec<_>>();
    (!keys.is_empty()).then(|| keys.join(" / "))
}

fn problem_row(problem: &ActionFileProblem) -> super::settings_group::SettingsRow<'static> {
    let name = file_name(&problem.file);
    let title = match problem.line {
        Some(line) => tr!("%1, line %2", name, line),
        None => name,
    };
    label_row(title, Some(RowNote::Error(problem.reason.clone())))
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
