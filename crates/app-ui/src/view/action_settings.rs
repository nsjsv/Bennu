//! 设置「动作」页：动作文件夹里有哪些动作、各自做什么、绑了什么键（点开进编辑页）；从模板新建；
//! 读到了哪些 Dolphin 服务菜单；用不了的文件和原因。服务菜单是 Dolphin 的文件，这里只列出来。

use iced::Element;

use crate::actions::editor::ActionTemplate;
use crate::actions::{
    action_folder, ActionDefinition, ActionFileProblem, ActionId, ServiceMenuAction,
};
use crate::app::actions::{ActionMessage, EditorMessage};
use crate::app::FileBrowser;
use crate::model::Message;
use crate::shortcuts::ShortcutTarget;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::normal_button;

use super::settings_group::{
    action_row, control_row, label_row, navigation_row, settings_card, settings_group, RowNote,
};

pub(super) fn action_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let folder = action_folder();
    let definitions = browser.action_library.definitions();
    let mut sections = vec![settings_group(
        tr!("Actions"),
        Some(tr!(
            "An action shows up in the context menu of the items it fits, and can have a shortcut. Each one is a .toml file in %1: edit it here or as text, changes take effect as soon as it is saved.",
            folder.display()
        )),
        if definitions.is_empty() {
            vec![label_row(
                tr!("No actions yet"),
                Some(RowNote::Hint(tr!("Start from one of the templates below."))),
            )]
        } else {
            definitions
                .iter()
                .map(|definition| action_summary_row(browser, definition))
                .collect()
        },
    )];
    sections.push(settings_group(
        tr!("New Action"),
        None,
        ActionTemplate::ALL
            .into_iter()
            .map(|template| {
                navigation_row(
                    template.label(),
                    None,
                    Message::Action(ActionMessage::Editor(EditorMessage::New(template))),
                )
            })
            .collect(),
    ));
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

/// 名字；下面一行是文件名、对哪些项、步骤和键。点开进编辑页。
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
    navigation_row(
        definition.name.clone(),
        Some(RowNote::Hint(details.join(" · "))),
        Message::Action(ActionMessage::Editor(EditorMessage::Open(
            definition.id.clone(),
        ))),
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

/// 写错的文件不能用表单打开（读不出来），给“打开文件”，用文本编辑器改。
fn problem_row(problem: &ActionFileProblem) -> super::settings_group::SettingsRow<'static> {
    let name = file_name(&problem.file);
    let title = match problem.line {
        Some(line) => tr!("%1, line %2", name, line),
        None => name,
    };
    let note = Some(RowNote::Error(problem.reason.clone()));
    let is_file = problem
        .file
        .extension()
        .is_some_and(|extension| extension == "toml" || extension == "desktop");
    if !is_file {
        return label_row(title, note);
    }
    control_row(
        title,
        note,
        normal_button(readable_text(tr!("Open the File"))).on_press(Message::Action(
            ActionMessage::FileOpenRequested(problem.file.clone()),
        )),
    )
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
