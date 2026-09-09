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
    DestructiveActionConfirmation, FileDropPrompt, Message, PendingOperation,
    TransferConflictMode,
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
        let sources =
            gather_sources_in_directory(&self.selected_paths_for_operation(), &directory);
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

    fn paste_operation(
        &mut self,
        paste_directory: PathBuf,
        operation: PendingOperation,
    ) -> Task<Message> {
        let (mode, transfers) = match operation {
            PendingOperation::Copy(sources) => {
                let transfers =
                    paths::transfer_targets(&paste_directory, &sources, PasteTargetMode::Copy)
                        .into_iter()
                        .map(|(source, target)| QueuedTransfer::new(source, target))
                        .collect::<Vec<_>>();
                (TransferConflictMode::Copy, transfers)
            }
            PendingOperation::Move(sources) => {
                let transfers =
                    paths::transfer_targets(&paste_directory, &sources, PasteTargetMode::Move)
                        .into_iter()
                        .filter(|(source, target)| source != target)
                        .map(|(source, target)| QueuedTransfer::new(source, target))
                        .collect::<Vec<_>>();
                self.pending_operation = None;
                (TransferConflictMode::Move, transfers)
            }
        };

        if transfers.is_empty() {
            return Task::none();
        }

        self.enqueue_or_confirm_transfers(mode, transfers)
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
fn gather_sources_in_directory(selected: &[PathBuf], directory: &Path) -> Vec<PathBuf> {
    selected
        .iter()
        .filter(|path| path.parent() == Some(directory))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::config;
    use crate::model::{
        BrowserViewMode, FileDragStationaryAction, FileDropTarget, FileEntryContentModifier,
    };
    use desktop_linux::{
        NetworkConnection, NetworkConnectionId, NetworkMountState, NetworkProtocol,
    };
    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    fn test_entry(path: &Path) -> DirectoryEntry {
        test_entry_with_kind(path, FileKind::File)
    }

    fn test_entry_with_kind(path: &Path, kind: FileKind) -> DirectoryEntry {
        DirectoryEntry::new(
            path.to_path_buf(),
            kind,
            EntryMetadata {
                len: 0,
                modified: None,
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    fn browser_with_entries(paths: &[PathBuf]) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.current_dir = PathBuf::from("/workspace");
        browser.entries = paths
            .iter()
            .map(|path| test_entry(path))
            .collect::<Vec<_>>()
            .into();
        browser.selected_paths = paths.iter().cloned().collect::<HashSet<_>>();
        browser.selected = paths.first().cloned();
        browser
    }

    fn mount_network_connection(browser: &mut FileBrowser, mount_path: PathBuf) {
        let connection = NetworkConnection::new(
            NetworkConnectionId::new("nas"),
            "NAS",
            NetworkProtocol::Smb,
            "smb://server/share",
        )
        .unwrap();
        let id = connection.id.clone();
        browser.network_connections =
            crate::network_connections::NetworkConnectionState::from_connections(vec![connection]);
        browser
            .network_connections
            .accept_loaded(vec![(id, NetworkMountState::Mounted(mount_path))]);
    }

    #[test]
    fn entry_hover_uses_current_level_for_paste_and_directory_for_drop() {
        let current_dir = PathBuf::from("/workspace");
        let source = current_dir.join("report.txt");
        let directory = current_dir.join("project");
        let mut browser = browser_with_entries(std::slice::from_ref(&source));
        browser.view_mode = BrowserViewMode::List;
        browser.entries = vec![
            test_entry(&source),
            test_entry_with_kind(&directory, FileKind::Directory),
        ]
        .into();

        drop(browser.handle_entry_hovered(directory.clone()));
        assert_eq!(browser.paste_target_directory(), current_dir);
        drop(browser.handle_entry_hovered(source.clone()));
        assert_eq!(browser.paste_target_directory(), current_dir);

        browser.cursor_position = iced::Point::new(0.0, 0.0);
        browser.start_file_drag(source, FileDragStationaryAction::SelectionOnly, Vec::new());
        drop(browser.update_file_drag(iced::Point::new(10.0, 0.0)));
        drop(browser.handle_entry_hovered(directory.clone()));

        assert_eq!(browser.paste_target_directory(), current_dir);
        drop(browser.handle_drop_target_hovered(directory.clone()));
        assert_eq!(browser.paste_target_directory(), current_dir);
        assert!(matches!(
            browser
                .file_drop_session
                .as_ref()
                .and_then(|session| session.hovered_target.as_ref()),
            Some(FileDropTarget::Directory(target)) if target == &directory
        ));
    }

    #[test]
    fn cut_visual_modifier_requires_exact_move_source_membership() {
        let source = PathBuf::from("/workspace/report.txt");
        let move_operation = PendingOperation::Move(vec![source.clone()]);

        assert_eq!(
            move_operation.content_modifier_for_path(&source),
            FileEntryContentModifier::Cut
        );
        assert_eq!(
            move_operation.content_modifier_for_path(Path::new("/workspace/report.txt.bak")),
            FileEntryContentModifier::None
        );
        assert_eq!(
            move_operation.content_modifier_for_path(Path::new("/archive/report.txt")),
            FileEntryContentModifier::None
        );
        assert_eq!(
            PendingOperation::Copy(vec![source.clone()]).content_modifier_for_path(&source),
            FileEntryContentModifier::Copied
        );
        assert_eq!(
            PendingOperation::Copy(vec![source.clone()])
                .content_modifier_for_path(Path::new("/workspace/report.txt.bak")),
            FileEntryContentModifier::None
        );
    }

    #[test]
    fn new_clipboard_operation_replaces_cut_visual_membership() {
        let first = PathBuf::from("/workspace/first.txt");
        let second = PathBuf::from("/workspace/second.txt");
        let mut browser = browser_with_entries(&[first.clone(), second.clone()]);
        browser.selected_paths = HashSet::from([first.clone()]);
        drop(browser.move_selected());
        assert_eq!(
            browser.file_entry_content_modifier(&first),
            FileEntryContentModifier::Cut
        );

        browser.selected = Some(second.clone());
        browser.selected_paths = HashSet::from([second.clone()]);
        drop(browser.move_selected());
        assert_eq!(
            browser.file_entry_content_modifier(&first),
            FileEntryContentModifier::None
        );
        assert_eq!(
            browser.file_entry_content_modifier(&second),
            FileEntryContentModifier::Cut
        );

        drop(browser.copy_selected());
        assert_eq!(
            browser.file_entry_content_modifier(&second),
            FileEntryContentModifier::Copied
        );
    }

    #[test]
    fn move_paste_consumes_cut_visual_membership() {
        let source = PathBuf::from("/workspace/report.txt");
        let mut browser = browser_with_entries(std::slice::from_ref(&source));
        drop(browser.move_selected());
        assert_eq!(
            browser.file_entry_content_modifier(&source),
            FileEntryContentModifier::Cut
        );

        drop(browser.paste_operation(
            PathBuf::from("/destination"),
            PendingOperation::Move(vec![source.clone()]),
        ));
        assert!(browser.pending_operation.is_none());
        assert_eq!(
            browser.file_entry_content_modifier(&source),
            FileEntryContentModifier::None
        );
    }

    #[test]
    fn local_delete_still_uses_trash_operation() {
        let local_path = PathBuf::from("/workspace/local.txt");
        let mut browser = browser_with_entries(std::slice::from_ref(&local_path));

        let command = browser.trash_selected();
        drop(command);

        assert!(browser.destructive_action_confirmation.is_none());
        assert_eq!(browser.operation_queue.tasks().len(), 1);
        assert!(matches!(
            &browser.operation_queue.tasks()[0].operation,
            QueuedFileOperation::Trash { paths } if paths == &vec![local_path]
        ));
    }

    #[test]
    fn network_delete_requests_permanent_delete_confirmation() {
        let mount_path = PathBuf::from("/run/user/1000/gvfs/smb-share:server=server,share=share");
        let network_path = mount_path.join("remote.txt");
        let mut browser = browser_with_entries(std::slice::from_ref(&network_path));
        mount_network_connection(&mut browser, mount_path);

        let command = browser.trash_selected();
        drop(command);

        assert_eq!(browser.operation_queue.tasks().len(), 0);
        assert!(matches!(
            &browser.destructive_action_confirmation,
            Some(DestructiveActionConfirmation::DeletePermanently { paths })
                if paths == &vec![network_path]
        ));
    }

    #[test]
    fn mixed_local_and_network_delete_is_rejected() {
        let mount_path = PathBuf::from("/run/user/1000/gvfs/smb-share:server=server,share=share");
        let network_path = mount_path.join("remote.txt");
        let local_path = PathBuf::from("/workspace/local.txt");
        let mut browser = browser_with_entries(&[local_path, network_path]);
        mount_network_connection(&mut browser, mount_path);

        let command = browser.trash_selected();
        drop(command);

        assert_eq!(browser.operation_queue.tasks().len(), 0);
        assert!(browser.destructive_action_confirmation.is_none());
        assert_eq!(
            browser.current_error(),
            Some("Delete local and remote items separately so local files can use Trash")
        );
    }

    #[test]
    fn columns_keyboard_paste_targets_focused_column_not_pointer_hover() {
        let project = PathBuf::from("/workspace/project");
        let mut browser = browser_with_entries(&[PathBuf::from("/workspace/a.txt")]);
        browser.view_mode = BrowserViewMode::Columns;
        browser.deepest_open_column_directory = Some(project.clone());
        browser.focused_column_directory = Some(project.clone());
        browser.cursor_paste_directory = Some(PathBuf::from("/workspace"));

        assert_eq!(browser.paste_target_directory(), project);
    }

    #[test]
    fn columns_keyboard_paste_falls_back_to_deepest_open_column_without_focus() {
        let project = PathBuf::from("/workspace/project");
        let mut browser = browser_with_entries(&[PathBuf::from("/workspace/a.txt")]);
        browser.view_mode = BrowserViewMode::Columns;
        browser.deepest_open_column_directory = Some(project.clone());
        browser.cursor_paste_directory = Some(PathBuf::from("/workspace"));

        assert_eq!(browser.paste_target_directory(), project);
    }

    #[test]
    fn columns_keyboard_paste_falls_back_to_current_dir_when_nothing_open() {
        let mut browser = browser_with_entries(&[PathBuf::from("/workspace/a.txt")]);
        browser.view_mode = BrowserViewMode::Columns;

        assert_eq!(browser.paste_target_directory(), PathBuf::from("/workspace"));
    }

    #[test]
    fn duplicate_selected_enqueues_in_place_copies_for_whole_selection() {
        let report = PathBuf::from("/workspace/report.pdf");
        let notes = PathBuf::from("/workspace/notes");
        let mut browser = browser_with_entries(&[report.clone(), notes.clone()]);
        // 副本走复制管线,恢复日志需要任务存储。
        let state_directory = tempfile::tempdir().unwrap();
        browser.operation_queue.set_store(
            file_operation_store::TaskQueueStore::new(
                state_directory.path().join("state.sqlite"),
            )
            .unwrap(),
        );

        drop(browser.duplicate_selected());

        assert_eq!(browser.operation_queue.tasks().len(), 1);
        assert!(matches!(
            &browser.operation_queue.tasks()[0].operation,
            QueuedFileOperation::Duplicate { transfers, .. }
                if transfers == &vec![
                    QueuedTransfer::new(report.clone(), PathBuf::from("/workspace/report副本.pdf")),
                    QueuedTransfer::new(notes.clone(), PathBuf::from("/workspace/notes副本")),
                ]
        ));
    }

    #[test]
    fn duplicate_completion_selects_the_new_copies() {
        let first_copy = PathBuf::from("/workspace/report副本.pdf");
        let second_copy = PathBuf::from("/workspace/notes副本");
        let mut browser = browser_with_entries(&[PathBuf::from("/workspace/report.pdf")]);
        let operation = QueuedFileOperation::Duplicate {
            transfers: vec![
                QueuedTransfer::new(PathBuf::from("/workspace/report.pdf"), first_copy.clone()),
                QueuedTransfer::new(PathBuf::from("/workspace/notes"), second_copy.clone()),
            ],
            verification: browser.file_operation_verification(),
        };

        let targets = operation.duplicate_selection_targets().unwrap();
        browser.select_operation_result_paths(targets);

        assert!(browser.selected_paths.contains(&first_copy));
        assert!(browser.selected_paths.contains(&second_copy));
        assert_eq!(browser.selected, Some(second_copy));
    }

    #[test]
    fn gather_sources_keeps_only_entries_inside_the_target_directory() {
        let in_current = PathBuf::from("/workspace/report.txt");
        let in_other_column = PathBuf::from("/workspace/project/plan.txt");

        let sources = gather_sources_in_directory(
            &[in_current.clone(), in_other_column],
            &PathBuf::from("/workspace"),
        );

        assert_eq!(sources, vec![in_current]);
    }

    #[test]
    fn new_folder_from_selection_enqueues_one_undoable_move_batch() {
        let workspace = tempfile::tempdir().unwrap();
        let report = workspace.path().join("report.txt");
        let mut browser = browser_with_entries(std::slice::from_ref(&report));
        browser.current_dir = workspace.path().to_path_buf();

        drop(browser.new_folder_from_selection());

        assert_eq!(browser.operation_queue.tasks().len(), 1);
        assert!(matches!(
            &browser.operation_queue.tasks()[0].operation,
            QueuedFileOperation::GatherSelectionIntoNewFolder { directory, sources }
                if directory == &workspace.path().join("新建文件夹")
                    && sources == &vec![report.clone()]
        ));
    }
}
