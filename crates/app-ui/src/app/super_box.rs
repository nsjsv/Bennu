//! 超级输入框接到文件管理器上：打开时拼候选，按键和打字交给模型，执行选中的那一行；
//! 路径补全和路径检查在后台。查索引在 `super_box_find.rs`，执行命令在 `super_box_shell.rs`。
//! 框的规则在 `crate::super_box`。

use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::model::{IndexedSearchOutcome, Message, NavigationMode, SettingsCategory};
use crate::shortcuts::{ShortcutAction, ShortcutScope, ShortcutTarget};
use crate::super_box::{
    BoxContext, BoxEntry, BoxLookup, BoxMode, BoxRow, BoxTarget, EntryKind, NewEntry, SuperBox,
};
use file_operations::QueuedFileOperation;

#[derive(Debug, Clone)]
pub(crate) enum SuperBoxMessage {
    InputChanged(String),
    RowPressed(usize),
    RowHovered(usize),
    /// 点了输入框左边的模式按钮：切到下一种。
    ModePressed,
    SuggestionsLoaded {
        generation: u64,
        suggestions: Vec<PathBuf>,
    },
    /// 跳转前在后台看了一眼路径：是文件夹为 `Ok(true)`，是文件为 `Ok(false)`。
    JumpChecked {
        generation: u64,
        path: PathBuf,
        is_directory: Result<bool, String>,
    },
    /// 打字停了一会儿，该查索引了（输入还没变才查）。
    FindSettled(Box<BoxLookup>),
    FoundLoaded {
        generation: u64,
        outcome: Box<IndexedSearchOutcome>,
    },
    /// 命令模式的停止按钮。
    StopPressed,
    PastRunsLoaded(Result<Vec<crate::super_box::PastRun>, String>),
    /// 记命令结果没写进状态库时是原因。
    PastRunSaved(Option<String>),
}

pub(crate) fn super_box_input_id() -> iced::widget::Id {
    iced::widget::Id::new("super-box-input")
}

impl FileBrowser {
    pub(super) fn handle_super_box_message(&mut self, message: SuperBoxMessage) -> Task<Message> {
        match message {
            SuperBoxMessage::InputChanged(text) => {
                self.update_super_box(|super_box, context| super_box.set_input(text, context))
            }
            SuperBoxMessage::ModePressed => {
                let lookup =
                    self.update_super_box(|super_box, context| super_box.next_mode(context));
                Task::batch([lookup, iced::widget::operation::focus(super_box_input_id())])
            }
            SuperBoxMessage::FindSettled(lookup) => self.run_super_box_find(*lookup),
            SuperBoxMessage::FoundLoaded {
                generation,
                outcome,
            } => self.accept_super_box_found(generation, *outcome),
            SuperBoxMessage::StopPressed => self.stop_super_box_command(),
            SuperBoxMessage::PastRunsLoaded(loaded) => {
                self.accept_past_super_box_runs(loaded);
                Task::none()
            }
            SuperBoxMessage::PastRunSaved(None) => Task::none(),
            // 只是重启后看不到这次的输出，不打断用户。
            SuperBoxMessage::PastRunSaved(Some(error)) => {
                tracing::warn!(
                    target: "app_ui::super_box",
                    event = "command_run_not_saved",
                    error = %error,
                    "a command result could not be saved"
                );
                Task::none()
            }
            SuperBoxMessage::RowPressed(index) => {
                if let Some(super_box) = &mut self.super_box {
                    super_box.select(index);
                }
                self.run_selected_super_box_row()
            }
            SuperBoxMessage::RowHovered(index) => {
                if let Some(super_box) = &mut self.super_box {
                    super_box.select(index);
                }
                Task::none()
            }
            SuperBoxMessage::SuggestionsLoaded {
                generation,
                suggestions,
            } => {
                let Some(mut super_box) = self.super_box.take() else {
                    return Task::none();
                };
                self.with_super_box_context(|context| {
                    super_box.accept_suggestions(generation, suggestions, context)
                });
                self.super_box = Some(super_box);
                Task::none()
            }
            SuperBoxMessage::JumpChecked {
                generation,
                path,
                is_directory,
            } => self.finish_super_box_jump(generation, path, is_directory),
        }
    }

    /// 设置页“超级输入框默认模式”。
    pub(super) fn select_super_box_default_mode(
        &mut self,
        mode: crate::super_box::BoxMode,
    ) -> Task<Message> {
        if self.user_config.super_box_mode == mode {
            return Task::none();
        }
        self.user_config.super_box_mode = mode;
        self.persist_user_preferences_command()
    }

    /// 设置页“多栏视图里打开找到的文件”。
    pub(super) fn select_super_box_found_in_columns(
        &mut self,
        choice: crate::super_box::FoundInColumns,
    ) -> Task<Message> {
        if self.user_config.super_box_found_in_columns == choice {
            return Task::none();
        }
        self.user_config.super_box_found_in_columns = choice;
        self.persist_user_preferences_command()
    }

    /// 快捷键：开着就关，关着就开。
    pub(super) fn toggle_super_box(&mut self) -> Task<Message> {
        if self.super_box.take().is_some() {
            return Task::none();
        }
        self.context_menu = None;
        let commit_rename = self.commit_rename_if_active();
        let _ = self.cancel_address_editing();
        let entries = self.super_box_entries();
        let history = self.user_config.super_box_history.entries().to_vec();
        let mode = self.user_config.super_box_mode;
        let opened =
            self.with_super_box_context(|context| SuperBox::open(mode, entries, history, context));
        self.super_box = Some(opened);
        Task::batch([
            commit_rename,
            iced::widget::operation::focus(super_box_input_id()),
        ])
    }

    /// 框开着时它自己的键：上下选、Tab 换模式、回车执行、Ctrl+Enter 换一种执行法、Esc 和打开它的
    /// 那个键关掉它。其余的键只给输入框（输入框自己收键盘事件），一个都不往文件区的快捷键、字母
    /// 跳转那边走。
    ///
    /// 回车只在这里处理，输入框不设 `on_submit`：输入框的回车和这里的按键是两条消息，先后不定，
    /// 靠“Ctrl 按着没有”去挡其中一条挡不住（虚拟机里 Ctrl+Enter 先跑了普通回车）。
    pub(super) fn super_box_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        use iced::keyboard::key::Named;
        if self.dialog_above_super_box_is_open() {
            return None;
        }
        let super_box = self.super_box.as_mut()?;
        match key.as_ref() {
            iced::keyboard::Key::Named(Named::ArrowDown) => super_box.select_next(),
            iced::keyboard::Key::Named(Named::ArrowUp) => super_box.select_previous(),
            iced::keyboard::Key::Named(Named::Tab) if modifiers.shift() => {
                return Some(
                    self.update_super_box(|super_box, context| super_box.previous_mode(context)),
                );
            }
            iced::keyboard::Key::Named(Named::Tab) => {
                return Some(
                    self.update_super_box(|super_box, context| super_box.next_mode(context)),
                );
            }
            iced::keyboard::Key::Named(Named::Escape) => self.super_box = None,
            iced::keyboard::Key::Named(Named::Enter) if modifiers.control() => {
                return Some(match super_box.mode() {
                    BoxMode::Files => self.continue_super_box_find_in_search_page(),
                    BoxMode::Shell => self.run_super_box_command_in_terminal(),
                    BoxMode::Search | BoxMode::Path | BoxMode::New => Task::none(),
                });
            }
            iced::keyboard::Key::Named(Named::Enter) => {
                return Some(self.run_selected_super_box_row());
            }
            _ => {
                let toggles =
                    crate::shortcuts::KeyPress::from_iced(key, modifiers).is_some_and(|press| {
                        self.keymap()
                            .keys(ShortcutTarget::Builtin(ShortcutAction::OpenSuperBox))
                            .contains(&press)
                    });
                if toggles {
                    self.super_box = None;
                }
            }
        }
        Some(Task::none())
    }

    /// 候选在打开时拼一次：功能、对现在的选择适用的动作、设置分类、收藏。
    fn super_box_entries(&self) -> Vec<BoxEntry> {
        let keymap = self.keymap();
        let key_hint =
            |target: ShortcutTarget| keymap.keys(target).first().map(|key| key.display_label());
        let mut entries = ShortcutAction::all()
            .iter()
            .copied()
            .filter(|action| {
                *action != ShortcutAction::OpenSuperBox && action.scope() != ShortcutScope::Terminal
            })
            .map(|action| {
                BoxEntry::new(
                    EntryKind::Feature,
                    action.label(),
                    BoxTarget::Builtin(action),
                )
                .with_keywords(action.config_key())
                .with_key_hint(key_hint(ShortcutTarget::Builtin(action)))
            })
            .collect::<Vec<_>>();
        if let Some(subject) = self.keyboard_action_subject() {
            let applicable = self.applicable_actions(&subject);
            let own = applicable
                .own
                .iter()
                .map(|definition| (definition.id.clone(), definition.name.clone()));
            let service_menus = applicable
                .service_menus
                .iter()
                .map(|action| (action.id(), action.name.clone()));
            entries.extend(own.chain(service_menus).map(|(id, name)| {
                BoxEntry::new(EntryKind::Action, name, BoxTarget::Action(id.clone()))
                    .with_key_hint(key_hint(ShortcutTarget::Action(id)))
            }));
        }
        entries.extend(SettingsCategory::ALL.into_iter().map(|category| {
            BoxEntry::new(
                EntryKind::Setting,
                category.label(),
                BoxTarget::Settings(category),
            )
        }));
        entries.extend(
            self.sidebar_favorite_locations()
                .into_iter()
                .map(|favorite| {
                    BoxEntry::new(
                        EntryKind::Favorite,
                        favorite.label,
                        BoxTarget::Folder(favorite.path.clone()),
                    )
                    .with_detail(favorite.path.display().to_string())
                }),
        );
        entries
    }

    /// 按当前标签页给框算结果行要的情况。
    pub(super) fn with_super_box_context<R>(
        &self,
        compute: impl FnOnce(&BoxContext<'_>) -> R,
    ) -> R {
        let target = self.keyboard_target_directory();
        let cannot_create = match &target {
            None => Some(tr!("New items cannot be made here")),
            Some(directory)
                if file_core::archive_path_identity(directory)
                    != file_core::ArchivePathIdentity::RealFile =>
            {
                Some(tr!("Archives cannot be changed here"))
            }
            Some(_) => None,
        };
        let taken = |name: &str| {
            target
                .as_ref()
                .is_some_and(|directory| self.entry_for_path(&directory.join(name)).is_some())
        };
        compute(&BoxContext {
            current_dir: self.tab().current_directory(),
            home: &self.home_dir,
            cannot_create,
            taken: &taken,
        })
    }

    /// 改框，路径模式要补全时在后台列子文件夹。
    fn update_super_box(
        &mut self,
        change: impl FnOnce(&mut SuperBox, &BoxContext<'_>) -> Option<BoxLookup>,
    ) -> Task<Message> {
        let Some(mut super_box) = self.super_box.take() else {
            return Task::none();
        };
        let lookup = self.with_super_box_context(|context| change(&mut super_box, context));
        self.super_box = Some(super_box);
        let (generation, path) = match lookup {
            None => return Task::none(),
            Some(find @ BoxLookup::Find { .. }) => return self.schedule_super_box_find(find),
            Some(BoxLookup::Jump { generation, path }) => (generation, path),
        };
        // 路径已经是完整的，“当前文件夹”用不上。
        Task::perform(
            crate::commands::load_path_suggestions(path, self.home_dir.clone()),
            move |suggestions| {
                Message::SuperBox(SuperBoxMessage::SuggestionsLoaded {
                    generation,
                    suggestions,
                })
            },
        )
    }

    fn run_selected_super_box_row(&mut self) -> Task<Message> {
        let Some(super_box) = &self.super_box else {
            return Task::none();
        };
        let Some(row) = super_box.selected_row().cloned() else {
            return Task::none();
        };
        let history = super_box.history_text();
        let generation = super_box.generation();
        match row {
            BoxRow::Entry(index) => {
                let target = super_box.entry(index).target.clone();
                self.super_box = None;
                let remember = self.remember_super_box_input(history);
                Task::batch([remember, self.run_super_box_target(target)])
            }
            BoxRow::History(text) => {
                self.update_super_box(|super_box, context| super_box.set_input(text, context))
            }
            BoxRow::JumpTo(path) | BoxRow::JumpSuggestion(path) => {
                let checked = path.clone();
                Task::perform(
                    async move {
                        tokio::fs::metadata(&checked)
                            .await
                            .map(|metadata| metadata.is_dir())
                            .map_err(|error| error.to_string())
                    },
                    move |is_directory| {
                        Message::SuperBox(SuperBoxMessage::JumpChecked {
                            generation,
                            path: path.clone(),
                            is_directory,
                        })
                    },
                )
            }
            BoxRow::Create(entry) => {
                let Some(parent) = self.keyboard_target_directory() else {
                    return Task::none();
                };
                self.super_box = None;
                let operation = match entry {
                    NewEntry::File(name) => QueuedFileOperation::CreateEmptyFile { parent, name },
                    NewEntry::Folder(name) => QueuedFileOperation::CreateDirectories {
                        parent,
                        names: vec![name],
                        gather_sources: Vec::new(),
                    },
                };
                // 名字已经写好了：建好后只选中，不再进重命名。
                self.pending_created_entry_after = Some(crate::model::CreatedEntryAfter::Select);
                let remember = self.remember_super_box_input(history);
                Task::batch([remember, self.enqueue_file_operation(operation)])
            }
            BoxRow::Found(index) => {
                let hit = super_box.found(index).clone();
                self.super_box = None;
                let remember = self.remember_super_box_input(history);
                Task::batch([remember, self.open_super_box_found(hit)])
            }
            BoxRow::Run(command) => self.run_super_box_command(command),
            BoxRow::Problem(_) => Task::none(),
        }
    }

    fn run_super_box_target(&mut self, target: BoxTarget) -> Task<Message> {
        match target {
            BoxTarget::Builtin(action) => self.invoke_shortcut(action),
            BoxTarget::Action(id) => self.invoke_action_from_keyboard(&id),
            BoxTarget::Settings(category) => Task::batch([
                self.open_settings(),
                self.select_settings_category(category),
            ]),
            BoxTarget::Folder(path) => self.navigate_to(path, NavigationMode::RecordHistory),
        }
    }

    fn finish_super_box_jump(
        &mut self,
        generation: u64,
        path: PathBuf,
        is_directory: Result<bool, String>,
    ) -> Task<Message> {
        let Some(super_box) = self.super_box.as_mut() else {
            return Task::none();
        };
        if super_box.generation() != generation {
            return Task::none();
        }
        let history = super_box.history_text();
        let navigation = match is_directory {
            Ok(true) => self.navigate_to(path, NavigationMode::RecordHistory),
            Ok(false) => {
                let Some(parent) = path.parent().map(PathBuf::from) else {
                    return Task::none();
                };
                let navigation = self.navigate_to(parent, NavigationMode::RecordHistory);
                self.select_path(path);
                navigation
            }
            Err(reason) => {
                super_box.show_notice(generation, tr!("%1: %2", path.display(), reason));
                return Task::none();
            }
        };
        self.super_box = None;
        Task::batch([self.remember_super_box_input(history), navigation])
    }

    pub(super) fn remember_super_box_input(&mut self, text: String) -> Task<Message> {
        if self.user_config.super_box_history.record_submission(&text) {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }
}

#[cfg(test)]
#[path = "super_box_tests.rs"]
mod tests;
