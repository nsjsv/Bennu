//! 高级新建文件夹弹窗的应用层：打开、更新草稿、确认入队。

use std::collections::HashSet;
use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::model::{AdvancedNewFolderMessage, AdvancedNewFolderState, Message};
use crate::operation_queue::QueuedFileOperation;

impl FileBrowser {
    pub(super) fn handle_advanced_new_folder_message(
        &mut self,
        message: AdvancedNewFolderMessage,
    ) -> Task<Message> {
        match message {
            AdvancedNewFolderMessage::Confirmed => {
                self.pending_advanced_new_folder_after = None;
                self.confirm_advanced_new_folder()
            }
            AdvancedNewFolderMessage::ConfirmedWithAfter(after) => {
                self.pending_advanced_new_folder_after = Some(after);
                self.confirm_advanced_new_folder()
            }
            AdvancedNewFolderMessage::Cancel => {
                self.advanced_new_folder = None;
                Task::none()
            }
            AdvancedNewFolderMessage::ModeSelected(mode) => {
                self.update_advanced_new_folder(|state| state.mode = mode);
                Task::none()
            }
            AdvancedNewFolderMessage::DraftEdited(action) => {
                self.update_advanced_new_folder(|state| {
                    state.editor_content.perform(action);
                    state.draft = state.editor_content.text();
                });
                Task::none()
            }
            AdvancedNewFolderMessage::PrefixChanged(prefix) => {
                self.update_advanced_new_folder(|state| state.prefix = prefix);
                Task::none()
            }
            AdvancedNewFolderMessage::StartNumberChanged(start) => {
                self.update_advanced_new_folder(|state| state.start_number = start);
                Task::none()
            }
            AdvancedNewFolderMessage::CountChanged(count) => {
                self.update_advanced_new_folder(|state| state.count = count);
                Task::none()
            }
            AdvancedNewFolderMessage::GatherToggled(gather) => {
                self.update_advanced_new_folder(|state| state.gather = gather);
                Task::none()
            }
            AdvancedNewFolderMessage::AfterMenuOpenChanged(open) => {
                if open {
                    self.open_advanced_new_folder_menu();
                } else {
                    self.close_advanced_new_folder_menu();
                }
                Task::none()
            }
            AdvancedNewFolderMessage::MenuHoverChanged(hover) => {
                if hover {
                    self.open_advanced_new_folder_menu();
                } else {
                    return self.schedule_advanced_new_folder_menu_close();
                }
                Task::none()
            }
            AdvancedNewFolderMessage::SplitButtonHoverChanged(hover) => {
                self.update_advanced_new_folder(|state| state.split_button_hover = hover);
                if hover {
                    self.open_advanced_new_folder_menu();
                } else {
                    return self.schedule_advanced_new_folder_menu_close();
                }
                Task::none()
            }
            AdvancedNewFolderMessage::MenuCloseTick(generation) => {
                self.update_advanced_new_folder(|state| {
                    if state.after_menu_open && state.menu_generation == generation {
                        state.after_menu_open = false;
                    }
                });
                Task::none()
            }
            AdvancedNewFolderMessage::NestedNamesProbed { prefix, names } => {
                self.update_advanced_new_folder(|state| {
                    state.taken_deep.insert(prefix, names);
                });
                Task::none()
            }
        }
        .chain(self.probe_missing_nested_prefixes())
    }

    /// 输入引用了但还没有快照的嵌套父目录,逐个异步探测。
    fn probe_missing_nested_prefixes(&mut self) -> Task<Message> {
        let Some(state) = &self.advanced_new_folder else {
            return Task::none();
        };
        let prefixes = state.needed_prefixes();
        if prefixes.is_empty() {
            return Task::none();
        }
        let parent = state.directory.clone();
        Task::batch(prefixes.into_iter().map(move |prefix| {
            let path = prefix
                .split('/')
                .fold(parent.clone(), |current, segment| current.join(segment));
            Task::perform(
                async move {
                    let mut names = HashSet::new();
                    if let Ok(mut entries) = tokio::fs::read_dir(path).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if let Some(name) = entry.file_name().to_str() {
                                names.insert(name.to_owned());
                            }
                        }
                    }
                    Message::AdvancedNewFolder(AdvancedNewFolderMessage::NestedNamesProbed {
                        prefix,
                        names,
                    })
                },
                |message| message,
            )
        }))
    }

    /// Shift+新建文件夹入口：打开弹窗。目标目录下的选中项作为
    /// 「建完移入」候选（与「用选中项新建文件夹」同一收纳语义）。
    pub(super) fn open_advanced_new_folder(&mut self, directory: PathBuf) -> Task<Message> {
        self.context_menu = None;
        if self.is_trash_view || self.search_workspace.is_some() {
            return Task::none();
        }
        let taken_names = self
            .entry_paths_in_directory(&directory)
            .into_iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect::<HashSet<_>>();
        let gather_sources = super::selection::gather_sources_in_directory(
            &self.selected_paths_for_operation(),
            &directory,
        );
        self.advanced_new_folder = Some(AdvancedNewFolderState::new(
            directory,
            taken_names,
            gather_sources,
        ));
        Task::none()
    }

    fn update_advanced_new_folder(&mut self, update: impl FnOnce(&mut AdvancedNewFolderState)) {
        if let Some(state) = &mut self.advanced_new_folder {
            update(state);
        }
    }

    /// 展开菜单并作废待执行的延迟关闭。
    fn open_advanced_new_folder_menu(&mut self) {
        self.update_advanced_new_folder(|state| {
            state.menu_generation += 1;
            state.after_menu_open = true;
        });
    }

    fn close_advanced_new_folder_menu(&mut self) {
        self.update_advanced_new_folder(|state| {
            state.after_menu_open = false;
            state.menu_generation += 1;
        });
    }

    /// 离开按钮/菜单后延迟关闭:期间重新悬停(代际自增)则本次失效,
    /// 让鼠标从按钮跨到菜单的间隙不闪关。
    fn schedule_advanced_new_folder_menu_close(&mut self) -> Task<Message> {
        let Some(state) = &mut self.advanced_new_folder else {
            return Task::none();
        };
        if !state.after_menu_open {
            return Task::none();
        }
        state.menu_generation += 1;
        let generation = state.menu_generation;
        Task::perform(
            async move {
                // 光标从按钮跨到菜单的宽限期,期间重新悬停会使本任务失效。
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                Message::AdvancedNewFolder(AdvancedNewFolderMessage::MenuCloseTick(generation))
            },
            |message| message,
        )
    }

    fn confirm_advanced_new_folder(&mut self) -> Task<Message> {
        let Some(state) = self.advanced_new_folder.take() else {
            return Task::none();
        };
        let names = state.plan();
        if names.is_empty() {
            return Task::none();
        }
        let gather_sources = if state.gather {
            state.gather_sources
        } else {
            Vec::new()
        };
        self.clear_preview();
        self.renaming = None;
        self.cancel_file_drag_interaction();
        self.enqueue_file_operation(QueuedFileOperation::CreateDirectories {
            parent: state.directory,
            names,
            gather_sources,
        })
    }
}
