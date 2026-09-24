use std::path::{Path, PathBuf};

use desktop_linux::{
    ClipboardImage, DesktopClipboardContent, FileClipboardOperation, FileClipboardSelection,
};
use iced::Task;

use crate::app::paths::{self, PasteTargetMode};
use crate::app::FileBrowser;
use crate::commands::{
    create_clipboard_file_command, read_desktop_clipboard_command, write_file_clipboard_command,
};
use crate::model::{
    entry_exists, unique_duplicated_directory_name, unique_duplicated_file_name,
    unique_gathered_folder_directory, BrowserViewMode, ContextMenuState,
    DestructiveActionConfirmation, FileDropPrompt, Message, PendingOperation, TransferConflictMode,
};
use crate::operation_queue::{QueuedFileOperation, QueuedTransfer};

impl FileBrowser {
    pub(in crate::app) fn copy_selected(&mut self) -> Task<Message> {
        self.context_menu = None;
        if self.search_workspace.is_none() && self.is_trash_view {
            return Task::none();
        }
        let paths = self.active_file_selection();
        if paths.is_empty() {
            return Task::none();
        }
        self.pending_operation = Some(PendingOperation::Copy(paths.clone()));
        write_file_clipboard_command(FileClipboardSelection::new(
            FileClipboardOperation::Copy,
            paths,
        ))
    }

    pub(in crate::app) fn move_selected(&mut self) -> Task<Message> {
        self.context_menu = None;
        if self.search_workspace.is_none() && self.is_trash_view {
            return Task::none();
        }
        let paths = self.active_file_selection();
        if paths.is_empty() {
            return Task::none();
        }
        self.pending_operation = Some(PendingOperation::Move(paths.clone()));
        write_file_clipboard_command(FileClipboardSelection::new(
            FileClipboardOperation::Move,
            paths,
        ))
    }

    /// 「复制副本」(Finder Duplicate):每个选中项在所在父目录原位复制,
    /// 目标名按共享命名规则起名;完成后由 accept_file_operation_finished 选中副本。
    pub(in crate::app) fn duplicate_selected(&mut self) -> Task<Message> {
        self.context_menu = None;
        if self.search_workspace.is_none() && self.is_trash_view {
            return Task::none();
        }
        let sources = self.active_file_selection();
        if sources.is_empty() {
            return Task::none();
        }
        let transfers = sources
            .iter()
            .map(|source| {
                let parent = self.entry_parent_directory(source);
                let name = source
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("item"));
                let unique_name = if self.entry_kind(source) == Some(file_core::FileKind::Directory)
                {
                    unique_duplicated_directory_name(name, |candidate| {
                        entry_exists(&parent.join(candidate))
                    })
                } else {
                    unique_duplicated_file_name(name, |candidate| {
                        entry_exists(&parent.join(candidate))
                    })
                };
                QueuedTransfer::new(source.clone(), parent.join(unique_name))
            })
            .collect::<Vec<_>>();
        self.enqueue_file_operation(QueuedFileOperation::Duplicate {
            transfers,
            verification: self.file_operation_verification(),
        })
    }

    /// 「用选中项新建文件夹」:在活动栏目录下创建「新建文件夹( 2/3…)」,
    /// 把 parent 等于该目录的选中项整批移入;跨栏选中项留在原地。
    pub(in crate::app) fn new_folder_from_selection(&mut self) -> Task<Message> {
        self.context_menu = None;
        if self.search_workspace.is_some() || self.is_trash_view {
            return Task::none();
        }
        let directory = self.gather_target_directory();
        let sources = gather_sources_in_directory(&self.selected_paths_for_operation(), &directory);
        if sources.is_empty() {
            return Task::none();
        }
        self.clear_preview();
        self.renaming = None;
        self.drag_selection_anchor = None;
        self.cancel_file_drag_interaction();
        self.enqueue_file_operation(QueuedFileOperation::GatherSelectionIntoNewFolder {
            directory: unique_gathered_folder_directory(&directory),
            sources,
        })
    }

    /// 与 keyboard_paste_directory 同源的「活动栏」语义:多栏取聚焦已渲染栏,
    /// 列表/图标视图就是当前目录。
    fn gather_target_directory(&self) -> PathBuf {
        if self.view_mode != BrowserViewMode::Columns {
            return self.current_dir.clone();
        }
        self.focused_rendered_column_directory()
            .or_else(|| self.deepest_open_column_directory.clone())
            .unwrap_or_else(|| self.current_dir.clone())
    }

    pub(in crate::app) fn trash_selected(&mut self) -> Task<Message> {
        self.context_menu = None;
        if self.search_workspace.is_none() && self.is_trash_view {
            return self.delete_selected_trash_entries();
        }
        self.trash_explicit_paths(self.active_file_selection())
    }

    /// Shift+删除:跳过回收站直接永久删除,仍走破坏性操作确认框。
    pub(in crate::app) fn delete_selected_permanently(&mut self) -> Task<Message> {
        self.context_menu = None;
        let paths = self.active_file_selection();
        if paths.is_empty() {
            return Task::none();
        }
        self.request_destructive_action_confirmation(
            DestructiveActionConfirmation::DeletePermanently { paths },
        );
        Task::none()
    }

    pub(in crate::app) fn trash_explicit_paths(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        self.context_menu = None;
        if paths.is_empty() {
            return Task::none();
        }
        let (remote_paths, local_paths): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .partition(|path| self.path_is_remote_mount(path));
        match (remote_paths.is_empty(), local_paths.is_empty()) {
            (true, false) => {
                self.enqueue_file_operation(QueuedFileOperation::Trash { paths: local_paths })
            }
            (false, true) => {
                self.request_destructive_action_confirmation(
                    DestructiveActionConfirmation::DeletePermanently {
                        paths: remote_paths,
                    },
                );
                Task::none()
            }
            (false, false) => {
                self.show_global_error(
                    "Delete local and remote items separately so local files can use Trash"
                        .to_owned(),
                );
                Task::none()
            }
            (true, true) => Task::none(),
        }
    }

    pub(in crate::app) fn restore_selected(&mut self) -> Task<Message> {
        self.context_menu = None;
        if !self.is_trash_view {
            return Task::none();
        }

        let entries = self.selected_trash_entries_for_operation();
        if entries.is_empty() {
            Task::none()
        } else {
            self.enqueue_file_operation(QueuedFileOperation::Restore { entries })
        }
    }

    fn delete_selected_trash_entries(&mut self) -> Task<Message> {
        let entries = self.selected_trash_entries_for_operation();
        if entries.is_empty() {
            Task::none()
        } else {
            self.request_destructive_action_confirmation(
                DestructiveActionConfirmation::DeleteTrashEntries { entries },
            );
            Task::none()
        }
    }

    pub(in crate::app) fn empty_trash_requested(&mut self) -> Task<Message> {
        self.context_menu = None;
        if !self.is_trash_view || self.trash_entries.is_empty() {
            return Task::none();
        }
        self.request_destructive_action_confirmation(DestructiveActionConfirmation::EmptyTrash);
        Task::none()
    }

    pub(in crate::app) fn confirm_destructive_action(&mut self) -> Task<Message> {
        let Some(confirmation) = self.destructive_action_confirmation.take() else {
            return Task::none();
        };

        match confirmation {
            DestructiveActionConfirmation::DeleteTrashEntries { entries } => {
                if entries.is_empty() {
                    Task::none()
                } else {
                    self.enqueue_file_operation(QueuedFileOperation::DeleteTrashEntries { entries })
                }
            }
            DestructiveActionConfirmation::DeletePermanently { paths } => {
                if paths.is_empty() {
                    Task::none()
                } else {
                    self.enqueue_file_operation(QueuedFileOperation::DeletePermanently { paths })
                }
            }
            DestructiveActionConfirmation::EmptyTrash => {
                self.enqueue_file_operation(QueuedFileOperation::EmptyTrash)
            }
        }
    }

    pub(in crate::app) fn cancel_destructive_action(&mut self) -> Task<Message> {
        self.destructive_action_confirmation = None;
        Task::none()
    }

    pub(in crate::app) fn request_destructive_action_confirmation(
        &mut self,
        confirmation: DestructiveActionConfirmation,
    ) {
        self.destructive_action_confirmation = Some(confirmation);
        self.transfer_conflict = None;
        self.context_menu = None;
        self.operation_queue.close_panel();
    }

    pub(in crate::app) fn create_directory_in(&mut self, directory: PathBuf) -> Task<Message> {
        self.context_menu = None;
        if self.is_trash_view {
            return Task::none();
        }
        self.clear_preview();
        self.renaming = None;
        self.drag_selection_anchor = None;
        self.cancel_file_drag_interaction();
        self.enqueue_file_operation(QueuedFileOperation::CreateDirectory { parent: directory })
    }

    pub(in crate::app) fn create_empty_file_in(&mut self, directory: PathBuf) -> Task<Message> {
        self.context_menu = None;
        if self.is_trash_view {
            return Task::none();
        }
        self.clear_preview();
        self.renaming = None;
        self.drag_selection_anchor = None;
        self.cancel_file_drag_interaction();
        self.enqueue_file_operation(QueuedFileOperation::CreateEmptyFile { parent: directory })
    }

    pub(in crate::app) fn paste_pending(&mut self) -> Task<Message> {
        if self.is_trash_view {
            self.context_menu = None;
            return Task::none();
        }
        let paste_directory = self.paste_target_directory();
        self.context_menu = None;
        read_desktop_clipboard_command(paste_directory, self.pending_operation.clone())
    }

    pub(in crate::app) fn accept_file_clipboard_write(
        &mut self,
        result: Result<(), String>,
    ) -> Task<Message> {
        match result {
            Ok(()) => self.clear_global_error(),
            Err(error) => self.show_global_error(error),
        }
        Task::none()
    }

    pub(in crate::app) fn accept_desktop_clipboard_paste(
        &mut self,
        paste_directory: PathBuf,
        fallback_operation: Option<PendingOperation>,
        content: Result<Option<DesktopClipboardContent>, String>,
    ) -> Task<Message> {
        match content {
            Ok(Some(content)) => self.paste_desktop_clipboard_content(paste_directory, content),
            Ok(None) => self.paste_optional_operation(paste_directory, fallback_operation),
            Err(error) => {
                if fallback_operation.is_some() {
                    self.paste_optional_operation(paste_directory, fallback_operation)
                } else {
                    self.show_global_error(error);
                    Task::none()
                }
            }
        }
    }

    pub(in crate::app) fn accept_clipboard_file_created(
        &mut self,
        result: Result<PathBuf, String>,
    ) -> Task<Message> {
        match result {
            Ok(path) => {
                self.invalidate_list_directory_summary_subtree_and_ancestor_chain(&path);
                self.reload_current_preserving_list_directory_summaries()
            }
            Err(error) => {
                self.show_global_error(error);
                Task::none()
            }
        }
    }

    pub(super) fn request_file_drop_prompt(
        &mut self,
        paste_directory: PathBuf,
        paths: Vec<PathBuf>,
    ) -> Task<Message> {
        if paths.is_empty() {
            return Task::none();
        }
        if self.destructive_action_confirmation.is_some()
            || self.file_drop_prompt.is_some()
            || self.transfer_conflict.is_some()
        {
            self.show_global_error(
                "Finish the current file operation prompt before dropping files".to_owned(),
            );
            return Task::none();
        }
        self.context_menu = None;
        self.open_with = None;
        self.operation_queue.close_panel();
        let _ = self.cancel_address_editing();
        self.file_drop_prompt = Some(FileDropPrompt {
            paste_directory,
            paths,
        });
        Task::none()
    }

    pub(in crate::app) fn apply_file_drop_operation(
        &mut self,
        operation: FileClipboardOperation,
    ) -> Task<Message> {
        let Some(prompt) = self.file_drop_prompt.take() else {
            return Task::none();
        };
        self.paste_file_clipboard_selection(
            prompt.paste_directory,
            FileClipboardSelection::new(operation, prompt.paths),
        )
    }

    pub(in crate::app) fn cancel_file_drop(&mut self) -> Task<Message> {
        self.file_drop_prompt = None;
        Task::none()
    }

    fn paste_desktop_clipboard_content(
        &mut self,
        paste_directory: PathBuf,
        content: DesktopClipboardContent,
    ) -> Task<Message> {
        match content {
            DesktopClipboardContent::Files(selection) => {
                self.paste_file_clipboard_selection(paste_directory, selection)
            }
            DesktopClipboardContent::Text(text) => {
                self.create_clipboard_text_file(paste_directory, text)
            }
            DesktopClipboardContent::Image(image) => {
                self.create_clipboard_image_file(paste_directory, image)
            }
        }
    }

    fn paste_file_clipboard_selection(
        &mut self,
        paste_directory: PathBuf,
        selection: FileClipboardSelection,
    ) -> Task<Message> {
        let operation = match selection.operation {
            FileClipboardOperation::Copy => PendingOperation::Copy(selection.paths),
            FileClipboardOperation::Move => PendingOperation::Move(selection.paths),
        };
        self.paste_operation(paste_directory, operation)
    }

    fn create_clipboard_text_file(
        &mut self,
        paste_directory: PathBuf,
        text: String,
    ) -> Task<Message> {
        self.context_menu = None;
        let target = paste_directory.join("Pasted Text.txt");
        create_clipboard_file_command(target, text.into_bytes())
    }

    fn create_clipboard_image_file(
        &mut self,
        paste_directory: PathBuf,
        image: ClipboardImage,
    ) -> Task<Message> {
        self.context_menu = None;
        let target = paste_directory.join(format!("Screenshot.{}", image.extension));
        create_clipboard_file_command(target, image.bytes)
    }

    fn paste_optional_operation(
        &mut self,
        paste_directory: PathBuf,
        operation: Option<PendingOperation>,
    ) -> Task<Message> {
        let Some(operation) = operation else {
            return Task::none();
        };
        self.paste_operation(paste_directory, operation)
    }

    /// 粘贴落地唯一入口(桌面剪贴板/内部待粘贴共用)。包内只读门控与
    /// 源拆分在此收口;拖拽落地复用同一套语义,可见性对同目录测试模块开放。
    pub(super) fn paste_operation(
        &mut self,
        paste_directory: PathBuf,
        operation: PendingOperation,
    ) -> Task<Message> {
        // 包内只读：目标在包内时粘贴语义不成立，直接吞掉。
        if file_core::archive_path_identity(&paste_directory)
            != file_core::ArchivePathIdentity::RealFile
        {
            return Task::none();
        }
        let (mode, transfers, archive_members) = match operation {
            PendingOperation::Copy(sources) => {
                let (archive_sources, real_sources) = split_archive_member_sources(sources);
                let members = (!archive_sources.is_empty()).then(|| {
                    QueuedFileOperation::ExtractArchiveMembers {
                        sources: archive_sources,
                        destination: paste_directory.clone(),
                    }
                });
                let transfers =
                    paths::transfer_targets(&paste_directory, &real_sources, PasteTargetMode::Copy)
                        .into_iter()
                        .map(|(source, target)| QueuedTransfer::new(source, target))
                        .collect::<Vec<_>>();
                (TransferConflictMode::Copy, transfers, members)
            }
            PendingOperation::Move(sources) => {
                // 包内不可写，「移动」里的包内源降级为提取(复制后源无法删除)。
                let (archive_sources, real_sources) = split_archive_member_sources(sources);
                let members = (!archive_sources.is_empty()).then(|| {
                    QueuedFileOperation::ExtractArchiveMembers {
                        sources: archive_sources,
                        destination: paste_directory.clone(),
                    }
                });
                let transfers = move_paste_transfers(&paste_directory, &real_sources);
                self.pending_operation = None;
                (TransferConflictMode::Move, transfers, members)
            }
        };

        if transfers.is_empty() && archive_members.is_none() {
            return Task::none();
        }

        let mut commands = Vec::new();
        if let Some(members) = archive_members {
            commands.push(self.enqueue_file_operation(members));
        }
        if !transfers.is_empty() {
            commands.push(self.enqueue_or_confirm_transfers(mode, transfers));
        }
        Task::batch(commands)
    }

    pub(super) fn paste_target_directory(&self) -> PathBuf {
        self.context_menu
            .as_ref()
            .and_then(ContextMenuState::paste_directory)
            .cloned()
            .unwrap_or_else(|| self.keyboard_paste_directory())
    }

    /// Cmd+V 的目的地对齐 Finder:多栏视图贴进"活动栏"(聚焦且已渲染的那一栏),
    /// 不看指针悬停;无聚焦时回退最深打开栏,再回退当前目录。搜索结果浮层下
    /// 浏览器栏不是粘贴语境,保持指针回退。列表/图标单目录,悬停父目录即当前目录。
    fn keyboard_paste_directory(&self) -> PathBuf {
        if self.view_mode != BrowserViewMode::Columns || self.search_workspace.is_some() {
            return self
                .cursor_paste_directory
                .clone()
                .unwrap_or_else(|| self.current_dir.clone());
        }
        self.focused_rendered_column_directory()
            .or_else(|| self.deepest_open_column_directory.clone())
            .unwrap_or_else(|| self.current_dir.clone())
    }
}

/// 只收纳「parent 等于目标目录」的选中项;parent 不一致的跨栏选中项留在原地。
pub(in crate::app) fn gather_sources_in_directory(
    selected: &[PathBuf],
    directory: &Path,
) -> Vec<PathBuf> {
    selected
        .iter()
        .filter(|path| path.parent() == Some(directory))
        .cloned()
        .collect()
}

/// 把粘贴源拆成「包内成员」与「真实路径」两组；包内成员走提取管线。
/// 拖拽落地(drag.rs)复用同一拆分，不在拖拽侧复刻判定。
pub(super) fn split_archive_member_sources(sources: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut archive_sources = Vec::new();
    let mut real_sources = Vec::new();
    for source in sources {
        if file_core::archive_path_identity(&source) == file_core::ArchivePathIdentity::RealFile {
            real_sources.push(source);
        } else {
            archive_sources.push(source);
        }
    }
    (archive_sources, real_sources)
}

/// 粘贴移动的目标计算:与拖拽落地共用同一空操作不变量(源等于落点、
/// 落入自身子树、已在落点目录),空操作源逐个跳过,而不是入队后由
/// 传输引擎报错。pub(super):拖拽移动落地(drag.rs)与测试模块直接
/// 消费同一计算,不在拖拽侧重写。
pub(super) fn move_paste_transfers(
    paste_directory: &Path,
    sources: &[PathBuf],
) -> Vec<QueuedTransfer> {
    paths::transfer_targets(paste_directory, sources, PasteTargetMode::Move)
        .into_iter()
        .filter(|(source, _)| !paths::move_is_no_op(source, paste_directory))
        .map(|(source, target)| QueuedTransfer::new(source, target))
        .collect()
}
