//! 设置里的动作编辑页接到文件管理器上：打开（后台读文件）、改、保存（后台写）、复制、删除，
//! 以及有未保存的修改时离开前先问。编辑页的状态和规则在 `crate::actions::editor`。

use std::path::PathBuf;

use iced::Task;

use super::ActionMessage;
use crate::actions::editor::{
    save, ActionEditor, ActionTemplate, DraftEdit, SaveOutcome, SaveTarget,
};
use crate::actions::{ensure_action_folder, ActionId, ActionStep, RunValues};
use crate::app::FileBrowser;
use crate::model::{Message, SettingsCategory, SettingsSubpage};
use file_operations::QueuedFileOperation;

#[derive(Debug, Clone)]
pub(crate) enum EditorMessage {
    /// 动作页上点了一个动作。
    Open(ActionId),
    /// 后台读完了文件。
    Opened(Result<Box<ActionEditor>, String>),
    New(ActionTemplate),
    Duplicate,
    Edit(DraftEdit),
    Save,
    Finished {
        session: u64,
        outcome: SaveOutcome,
    },
    /// 文件在别处改过了：丢掉这里的修改，重新读。
    Reload,
    /// 文件在别处改过了：仍然用这里的内容盖掉它。
    SaveAnyway,
    Delete,
    LeaveConfirmed,
    LeaveCanceled,
}

impl FileBrowser {
    pub(super) fn handle_editor_message(&mut self, message: EditorMessage) -> Task<Message> {
        match message {
            EditorMessage::Open(id) => self.open_action_editor(id),
            EditorMessage::Opened(Ok(editor)) => self.show_opened_editor(editor),
            EditorMessage::Opened(Err(reason)) => {
                self.show_global_error(reason);
                Task::none()
            }
            EditorMessage::New(template) => {
                if self.editor_can_open() {
                    let editor = ActionEditor::from_template(
                        template,
                        self.action_library.types(),
                        RunValues::starting_now(),
                    );
                    return self
                        .select_settings_subpage(SettingsSubpage::ActionEditor(Box::new(editor)));
                }
                Task::none()
            }
            EditorMessage::Duplicate => {
                let types = self.action_library.types().clone();
                match self.action_editor() {
                    Some(editor) if !editor.has_unsaved_changes() => {
                        let copy = editor.duplicate(&types);
                        self.select_settings_subpage(SettingsSubpage::ActionEditor(Box::new(copy)))
                    }
                    _ => Task::none(),
                }
            }
            EditorMessage::Edit(edit) => {
                let types = self.action_library.types().clone();
                if let Some(editor) = self.action_editor_mut() {
                    editor.edit(edit, &types);
                }
                Task::none()
            }
            EditorMessage::Save => self.save_action_editor(),
            EditorMessage::SaveAnyway => {
                if let Some(editor) = self.action_editor_mut() {
                    editor.accept_change_on_disk();
                }
                self.save_action_editor()
            }
            EditorMessage::Finished { session, outcome } => {
                let Some(editor) = self
                    .action_editor_mut()
                    .filter(|editor| editor.session == session)
                else {
                    return Task::none();
                };
                editor.finish_save(outcome);
                match editor.saved_on_the_way_out() {
                    Some(destination) => {
                        self.settings_subpage = None;
                        self.select_settings_category(destination)
                    }
                    None => Task::none(),
                }
            }
            EditorMessage::Reload => match self.action_editor() {
                Some(editor) => match (editor.file(), editor.action_id()) {
                    (Some(path), Some(id)) => {
                        self.read_action_for_editor(path.to_path_buf(), id.clone())
                    }
                    _ => Task::none(),
                },
                None => Task::none(),
            },
            EditorMessage::Delete => {
                let Some(path) = self
                    .action_editor()
                    .and_then(ActionEditor::file)
                    .map(PathBuf::from)
                else {
                    return Task::none();
                };
                // 进回收站、走任务队列：主窗口里 Ctrl+Z 能撤回。符号链接只删链接。
                self.settings_subpage = None;
                Task::batch([
                    self.select_settings_category(SettingsCategory::Actions),
                    self.enqueue_file_operation(QueuedFileOperation::Trash { paths: vec![path] }),
                ])
            }
            EditorMessage::LeaveConfirmed => {
                let Some(destination) = self.action_editor().and_then(|editor| editor.leaving)
                else {
                    return Task::none();
                };
                self.settings_subpage = None;
                self.select_settings_category(destination)
            }
            EditorMessage::LeaveCanceled => {
                if let Some(editor) = self.action_editor_mut() {
                    editor.leaving = None;
                }
                Task::none()
            }
        }
    }

    pub(crate) fn action_editor(&self) -> Option<&ActionEditor> {
        match &self.settings_subpage {
            Some(SettingsSubpage::ActionEditor(editor)) => Some(editor),
            _ => None,
        }
    }

    fn action_editor_mut(&mut self) -> Option<&mut ActionEditor> {
        match &mut self.settings_subpage {
            Some(SettingsSubpage::ActionEditor(editor)) => Some(editor),
            _ => None,
        }
    }

    /// 离开编辑页的唯一关口（侧栏、返回、Esc、从别处打开某个设置分类都经过这里）：有未保存的修改时
    /// 记下要去哪，先不走，等用户选“放弃”或“继续编辑”。
    pub(in crate::app) fn action_editor_holds_back(
        &mut self,
        destination: SettingsCategory,
    ) -> bool {
        match self.action_editor_mut() {
            Some(editor) if editor.has_unsaved_changes() => {
                editor.leaving = Some(destination);
                true
            }
            _ => false,
        }
    }

    /// 超级输入框的管道存成动作：打开设置到动作页，用这几步起草一个新动作，用户起名字、点保存。
    /// 设置里正编辑着、还没保存的动作不顶掉：交回原因。
    pub(crate) fn draft_action_from_steps(
        &mut self,
        name: String,
        steps: &[ActionStep],
    ) -> Result<Task<Message>, String> {
        if self
            .action_editor()
            .is_some_and(ActionEditor::has_unsaved_changes)
        {
            return Err(tr!(
                "An action in Settings has unsaved changes; save or discard it first"
            ));
        }
        let editor = ActionEditor::from_steps(
            name,
            steps,
            self.action_library.types(),
            RunValues::starting_now(),
        );
        Ok(Task::batch([
            self.open_settings(),
            self.select_settings_category(SettingsCategory::Actions),
            self.select_settings_subpage(SettingsSubpage::ActionEditor(Box::new(editor))),
        ]))
    }

    /// 只从动作列表打开：读文件的这几毫秒里用户已经去了别处，就不再打开。
    fn editor_can_open(&self) -> bool {
        self.selected_settings_category == SettingsCategory::Actions
            && self.settings_subpage.is_none()
    }

    fn open_action_editor(&mut self, id: ActionId) -> Task<Message> {
        let Some(path) = self
            .action_library
            .definitions()
            .iter()
            .find(|definition| definition.id == id)
            .map(|definition| definition.file.clone())
        else {
            return Task::none();
        };
        self.read_action_for_editor(path, id)
    }

    fn read_action_for_editor(&mut self, path: PathBuf, id: ActionId) -> Task<Message> {
        let types = self.action_library.types().clone();
        Task::perform(
            async move {
                let shown = path.clone();
                tokio::task::spawn_blocking(move || {
                    ActionEditor::open(path, id, &types, RunValues::starting_now())
                })
                .await
                .map_err(|error| error.to_string())?
                .map(Box::new)
                .map_err(|reason| tr!("%1: %2", shown.display(), reason))
            },
            |opened| Message::Action(ActionMessage::Editor(EditorMessage::Opened(opened))),
        )
    }

    /// 从列表打开，或者“重新载入”同一个文件。
    fn show_opened_editor(&mut self, editor: Box<ActionEditor>) -> Task<Message> {
        let reloading = self
            .action_editor()
            .and_then(ActionEditor::file)
            .is_some_and(|open| Some(open) == editor.file());
        if reloading {
            self.settings_subpage = Some(SettingsSubpage::ActionEditor(editor));
            return Task::none();
        }
        if self.editor_can_open() {
            return self.select_settings_subpage(SettingsSubpage::ActionEditor(editor));
        }
        Task::none()
    }

    fn save_action_editor(&mut self) -> Task<Message> {
        let types = self.action_library.types().clone();
        let folder = self.action_library.folder().to_path_buf();
        let Some(editor) = self.action_editor_mut().filter(|editor| editor.can_save()) else {
            return Task::none();
        };
        let session = editor.session;
        let Ok(request) = editor.begin_save(&folder, &types) else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if let SaveTarget::New { folder, .. } = &request.target {
                        if let Err(reason) = ensure_action_folder(folder) {
                            return SaveOutcome::Failed(reason);
                        }
                    }
                    save(request.target, request.text)
                })
                .await
                .unwrap_or_else(|error| SaveOutcome::Failed(error.to_string()))
            },
            move |outcome| {
                Message::Action(ActionMessage::Editor(EditorMessage::Finished {
                    session,
                    outcome,
                }))
            },
        )
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
