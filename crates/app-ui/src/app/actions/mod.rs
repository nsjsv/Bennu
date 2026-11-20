//! 自定义动作接到文件管理器上：读动作文件夹、从菜单和快捷键开始一次运行、一步一步跑、
//! 结束时通知。动作文件本身的模型在 `crate::actions`。

mod library_loading;
mod notify;
mod run;
mod steps;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::Task;

pub(crate) use library_loading::action_library_subscription;
pub(crate) use run::ActionRuns;
pub(crate) use steps::PreparedStep;

use super::FileBrowser;
use crate::actions::{ActionDefinition, ActionId, ActionLibrary};
use crate::model::{BrowserLocation, Message};

/// 动作作用在什么上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActionSubject {
    /// 选中的项。
    Selection,
    /// 一个文件夹：在空白处右键时的当前文件夹。
    Folder(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) enum ActionMessage {
    LibraryLoaded(ActionLibrary),
    Invoked {
        action: ActionId,
        subject: ActionSubject,
    },
    StepPrepared {
        run: u64,
        prepared: PreparedStep,
    },
    HintExpired,
    /// 设置页的“打开动作文件夹”：不存在时先建好（只有自己能进），再在新标签页里打开。
    FolderOpenRequested,
    FolderReady(Result<PathBuf, String>),
}

/// 按了动作的快捷键、它却不适用时，底栏显示一会儿的说明。不弹窗：不打断正在做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionHint {
    pub(crate) text: String,
    until: Instant,
}

const HINT_DURATION: Duration = Duration::from_secs(3);

impl FileBrowser {
    pub(super) fn handle_action_message(&mut self, message: ActionMessage) -> Task<Message> {
        match message {
            ActionMessage::LibraryLoaded(library) => {
                self.action_library = library;
                Task::none()
            }
            ActionMessage::Invoked { action, subject } => self.invoke_action(&action, subject),
            ActionMessage::StepPrepared { run, prepared } => self.accept_action_step(run, prepared),
            ActionMessage::FolderOpenRequested => Task::perform(
                async {
                    let folder = crate::actions::action_folder();
                    let created = folder.clone();
                    tokio::task::spawn_blocking(move || {
                        use std::os::unix::fs::DirBuilderExt;
                        std::fs::DirBuilder::new()
                            .recursive(true)
                            .mode(0o700)
                            .create(&created)
                    })
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| tr!("%1: %2", folder.display(), error))?;
                    Ok(folder)
                },
                |ready| Message::Action(ActionMessage::FolderReady(ready)),
            ),
            ActionMessage::FolderReady(Ok(folder)) => self.open_directory_in_new_tab(folder),
            ActionMessage::FolderReady(Err(error)) => {
                self.show_global_error(error);
                Task::none()
            }
            ActionMessage::HintExpired => {
                if self
                    .action_hint
                    .as_ref()
                    .is_some_and(|hint| hint.until <= Instant::now())
                {
                    self.action_hint = None;
                }
                Task::none()
            }
        }
    }

    /// 设置页、提示里一个快捷键目标的名字。动作文件不在时用它的文件名。
    pub(crate) fn shortcut_target_label(
        &self,
        target: &crate::shortcuts::ShortcutTarget,
    ) -> String {
        match target {
            crate::shortcuts::ShortcutTarget::Builtin(action) => action.label(),
            crate::shortcuts::ShortcutTarget::Action(id) => self
                .action_library
                .find(id)
                .map(|definition| definition.name.clone())
                .unwrap_or_else(|| id.file_stem().to_owned()),
        }
    }

    /// 动作的快捷键：有选中项时对选中项，没有时对当前文件夹。
    pub(super) fn invoke_action_from_keyboard(&mut self, action: &ActionId) -> Task<Message> {
        let subject = if self.selected_paths_for_operation().is_empty() {
            match self.tab().current_directory() {
                Some(directory) => ActionSubject::Folder(directory.to_path_buf()),
                None => return Task::none(),
            }
        } else {
            ActionSubject::Selection
        };
        if let Some(definition) = self
            .applicable_actions(&subject)
            .into_iter()
            .find(|definition| definition.id == *action)
        {
            let items = self
                .action_items(&subject)
                .map(|items| items.into_iter().map(|(path, _)| path).collect())
                .unwrap_or_default();
            return self.start_action_run(definition, items);
        }
        let Some(definition) = self.action_library.find(action) else {
            return Task::none();
        };
        let until = Instant::now() + HINT_DURATION;
        self.action_hint = Some(ActionHint {
            text: tr!("“%1” does not apply to the selected items", definition.name),
            until,
        });
        // 计时器要在运行时里建，放进 async 块里，等任务真正开始跑时才建。
        Task::perform(async { tokio::time::sleep(HINT_DURATION).await }, |()| {
            Message::Action(ActionMessage::HintExpired)
        })
    }

    /// 适用于这一批的动作，按名字排序。回收站里、压缩包里面没有动作：那里的东西是只读的。
    pub(crate) fn applicable_actions(
        &self,
        subject: &ActionSubject,
    ) -> Vec<std::sync::Arc<ActionDefinition>> {
        let Some(items) = self.action_items(subject) else {
            return Vec::new();
        };
        let target = self.action_library.target(
            items
                .iter()
                .map(|(path, is_directory)| (path.as_path(), *is_directory)),
        );
        self.action_library.matching(&target).cloned().collect()
    }

    fn invoke_action(&mut self, action: &ActionId, subject: ActionSubject) -> Task<Message> {
        self.context_menu = None;
        let Some(definition) = self
            .applicable_actions(&subject)
            .into_iter()
            .find(|definition| definition.id == *action)
        else {
            return Task::none();
        };
        let Some(items) = self.action_items(&subject) else {
            return Task::none();
        };
        self.start_action_run(
            definition,
            items.into_iter().map(|(path, _)| path).collect(),
        )
    }

    /// 这一批的路径和它是不是文件夹；这里不能有动作时为 None。是不是文件夹看已经列出来的
    /// 条目，不读盘。
    fn action_items(&self, subject: &ActionSubject) -> Option<Vec<(PathBuf, bool)>> {
        let tab = self.tab();
        if tab.location == BrowserLocation::Trash {
            return None;
        }
        if let Some(directory) = tab.current_directory() {
            if file_core::archive_path_identity(directory)
                != file_core::ArchivePathIdentity::RealFile
            {
                return None;
            }
        }
        let items = match subject {
            ActionSubject::Folder(folder) => vec![(folder.clone(), true)],
            ActionSubject::Selection => self
                .selected_paths_for_operation()
                .into_iter()
                .map(|path| {
                    let is_directory = self
                        .entry_for_path(&path)
                        .is_some_and(|entry| entry.kind() == file_core::FileKind::Directory);
                    (path, is_directory)
                })
                .collect(),
        };
        (!items.is_empty()).then_some(items)
    }
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod menu_tests;
