use iced::keyboard::{self, key};
use iced::{event, Task};

use super::text_input_shortcuts;
use super::FileBrowser;
use crate::model::{Message, PathSuggestionDirection};
use crate::shortcuts::{
    FileBrowserShortcutOwnership, KeyBinding, ShortcutAction, ShortcutBindingId,
    ShortcutCaptureState, ShortcutConfig, ShortcutRoutingContext,
};

impl FileBrowser {
    pub(crate) fn shortcut_config(&self) -> &ShortcutConfig {
        &self.user_config.shortcuts
    }

    pub(super) fn handle_keyboard_key_pressed(
        &mut self,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        status: event::Status,
    ) -> Task<Message> {
        if self.shortcut_capture.is_some() {
            return match KeyBinding::from_iced_key(&key, modifiers) {
                Some(binding) => self.accept_shortcut_capture(binding),
                None => self.reject_unsupported_shortcut_capture(),
            };
        }

        // 终端面板聚焦时键盘归 PTY;除宿主保留键外不进入文件管理器快捷键路由。
        if let Some(task) = self.terminal_panel_keyboard_input(&key, modifiers) {
            return task;
        }

        if let Some(command) = self.handle_path_suggestion_keyboard_key(&key, modifiers) {
            return command;
        }

        let Some(action) = self.user_config.shortcuts.matching_action(&key, modifiers) else {
            return Task::none();
        };
        match keyboard_shortcut_route(action, &key, status) {
            KeyboardShortcutRoute::InvokeApplication => self.invoke_shortcut(action),
            KeyboardShortcutRoute::InvokeFileBrowserContent => {
                self.invoke_file_browser_content_shortcut(action)
            }
            KeyboardShortcutRoute::QueryTextInputFocus => {
                text_input_shortcuts::route_ignored_file_content_shortcut(action)
            }
            KeyboardShortcutRoute::CapturedPreview => self.handle_captured_preview_shortcut(),
            KeyboardShortcutRoute::Stop => Task::none(),
        }
    }

    fn handle_path_suggestion_keyboard_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        if !self.address_suggestion_keyboard_is_active() {
            return None;
        }

        match key.as_ref() {
            keyboard::Key::Named(key::Named::ArrowDown) if no_shortcut_modifiers(modifiers) => {
                Some(
                    self.move_path_suggestion_selection_from_keyboard(
                        PathSuggestionDirection::Next,
                    ),
                )
            }
            keyboard::Key::Named(key::Named::ArrowUp) if no_shortcut_modifiers(modifiers) => {
                Some(self.move_path_suggestion_selection_from_keyboard(
                    PathSuggestionDirection::Previous,
                ))
            }
            keyboard::Key::Named(key::Named::Tab) if only_shift_modifier(modifiers) => {
                Some(self.complete_path_suggestion_from_keyboard(PathSuggestionDirection::Previous))
            }
            keyboard::Key::Named(key::Named::Tab) if no_shortcut_modifiers(modifiers) => {
                Some(self.complete_path_suggestion_from_keyboard(PathSuggestionDirection::Next))
            }
            _ => None,
        }
    }

    fn invoke_file_browser_content_shortcut(&mut self, action: ShortcutAction) -> Task<Message> {
        if self.file_browser_content_shortcuts_enabled() {
            self.invoke_shortcut(action)
        } else {
            Task::none()
        }
    }

    pub(super) fn file_browser_content_shortcuts_enabled(&self) -> bool {
        self.destructive_action_confirmation.is_none()
            && self.file_drop_prompt.is_none()
            && self.transfer_conflict.is_none()
            && self.context_menu.is_none()
            && self.open_with.is_none()
            && self.archive_creation.is_none()
            && self.archive_extraction.is_none()
            && self.batch_rename.is_none()
            && self.network_connection_editor.is_none()
            && self.settings_window != Some(self.focused_window)
            && self.properties_window != Some(self.focused_window)
            && self.preview_window != Some(self.focused_window)
            && self.renaming.is_none()
            && self.shortcut_capture.is_none()
            && !self.operation_queue.is_panel_open()
    }

    /// 键盘正被某个输入会话占用(地址栏编辑、快捷键捕获、各类模态面板);
    /// 终端面板据此让出键盘。
    pub(crate) fn keyboard_input_session_is_active(&self) -> bool {
        self.address_editing.is_some()
            || self.shortcut_capture.is_some()
            || !self.file_browser_content_shortcuts_enabled()
    }

    pub(super) fn invoke_shortcut(&mut self, action: ShortcutAction) -> Task<Message> {
        if self.search_workspace.is_some() {
            return self.invoke_search_workspace_shortcut(action);
        }
        match action {
            ShortcutAction::OpenSelected => self.activate_selected_path(),
            ShortcutAction::RenameSelected => self.begin_rename_selected(),
            ShortcutAction::FocusPathInput => self.begin_address_editing(self.active_pane_id()),
            ShortcutAction::NavigateBack => {
                Task::batch([self.commit_rename_if_active(), self.navigate_back()])
            }
            ShortcutAction::NavigateForward => {
                Task::batch([self.commit_rename_if_active(), self.navigate_forward()])
            }
            ShortcutAction::NavigateUp => {
                Task::batch([self.commit_rename_if_active(), self.navigate_up()])
            }
            ShortcutAction::MoveSelection(direction) => self.move_file_selection(direction),
            ShortcutAction::FileProperties => self.open_selected_file_properties(),
            ShortcutAction::Refresh => {
                Task::batch([self.commit_rename_if_active(), self.reload_visible_panes()])
            }
            ShortcutAction::Escape => self.handle_focused_window_escape_pressed(),
            ShortcutAction::Preview => self.request_preview(),
            ShortcutAction::ToggleRightPreviewPanel => self.toggle_right_preview_panel(),
            ShortcutAction::ToggleTerminal => self.toggle_terminal_via_shortcut(),
            ShortcutAction::TerminalNewTab => self.add_terminal_tab(),
            ShortcutAction::TerminalCloseTab => self.close_active_terminal_tab_via_shortcut(),
            ShortcutAction::SelectAll => self.select_all_in_file_selection_scope(),
            ShortcutAction::Copy => self.copy_selected(),
            ShortcutAction::DuplicateSelected => self.duplicate_selected(),
            ShortcutAction::NewFolderFromSelection => self.new_folder_from_selection(),
            ShortcutAction::Paste => self.paste_pending(),
            ShortcutAction::Cut => self.move_selected(),
            ShortcutAction::Delete => self.trash_selected(),
            ShortcutAction::Undo => self.undo_file_operation(),
            ShortcutAction::Redo => self.redo_file_operation(),
        }
    }

    pub(super) fn start_shortcut_capture(
        &mut self,
        binding_id: ShortcutBindingId,
    ) -> Task<Message> {
        self.shortcut_capture = Some(ShortcutCaptureState::new(binding_id));
        Task::none()
    }

    pub(super) fn cancel_shortcut_capture(&mut self) -> Task<Message> {
        self.shortcut_capture = None;
        Task::none()
    }

    pub(super) fn accept_shortcut_capture(&mut self, binding: KeyBinding) -> Task<Message> {
        let Some(capture) = &self.shortcut_capture else {
            return Task::none();
        };
        let binding_id = capture.binding_id;

        if let Some(conflict) = self
            .user_config
            .shortcuts
            .conflicting_binding(binding_id, &binding)
        {
            self.shortcut_capture = Some(ShortcutCaptureState::conflict(
                binding_id, binding, conflict,
            ));
            return Task::none();
        }

        self.user_config.shortcuts.set_binding(binding_id, binding);
        self.shortcut_capture = None;
        self.persist_user_preferences_command()
    }

    pub(super) fn reject_unsupported_shortcut_capture(&mut self) -> Task<Message> {
        let Some(capture) = &self.shortcut_capture else {
            return Task::none();
        };
        self.shortcut_capture = Some(ShortcutCaptureState::unsupported(capture.binding_id));
        Task::none()
    }

    pub(super) fn reset_shortcut_binding(
        &mut self,
        binding_id: ShortcutBindingId,
    ) -> Task<Message> {
        let binding = ShortcutConfig::default_binding(binding_id);
        if let Some(conflict) = self
            .user_config
            .shortcuts
            .conflicting_binding(binding_id, &binding)
        {
            self.shortcut_capture = Some(ShortcutCaptureState::conflict(
                binding_id, binding, conflict,
            ));
            return Task::none();
        }

        self.user_config.shortcuts.reset_binding(binding_id);
        self.shortcut_capture = None;
        self.persist_user_preferences_command()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyboardShortcutRoute {
    InvokeApplication,
    InvokeFileBrowserContent,
    QueryTextInputFocus,
    CapturedPreview,
    Stop,
}

fn keyboard_shortcut_route(
    action: ShortcutAction,
    key: &keyboard::Key,
    status: event::Status,
) -> KeyboardShortcutRoute {
    if action.is_preview_toggle() && matches!(status, event::Status::Captured) {
        return KeyboardShortcutRoute::CapturedPreview;
    }

    match (action.routing_context(key), status) {
        (ShortcutRoutingContext::Application, _) => KeyboardShortcutRoute::InvokeApplication,
        (ShortcutRoutingContext::FileBrowserContent(_), event::Status::Captured) => {
            KeyboardShortcutRoute::Stop
        }
        (
            ShortcutRoutingContext::FileBrowserContent(
                FileBrowserShortcutOwnership::CapturedDeleteEvent,
            ),
            event::Status::Ignored,
        ) => KeyboardShortcutRoute::InvokeFileBrowserContent,
        (
            ShortcutRoutingContext::FileBrowserContent(
                FileBrowserShortcutOwnership::FocusedTextInputProbe,
            ),
            event::Status::Ignored,
        ) => KeyboardShortcutRoute::QueryTextInputFocus,
    }
}

fn no_shortcut_modifiers(modifiers: keyboard::Modifiers) -> bool {
    !modifiers.alt() && !modifiers.control() && !modifiers.command() && !modifiers.shift()
}

fn only_shift_modifier(modifiers: keyboard::Modifiers) -> bool {
    !modifiers.alt() && !modifiers.control() && !modifiers.command() && modifiers.shift()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use file_core::{DirectoryEntry, EntryMetadata, FileKind, TrashEntry, TrashRestoreEntry};

    use super::*;
    use crate::config;
    use crate::model::trash_location_path;
    use crate::operation_queue::QueuedFileOperation;

    fn test_entry(path: &Path) -> DirectoryEntry {
        DirectoryEntry::new(
            path.to_path_buf(),
            FileKind::File,
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

    fn browser_with_selected_trash_entry() -> (FileBrowser, TrashRestoreEntry) {
        let trash_path = PathBuf::from("/home/user/.local/share/Trash/files/report.txt");
        let info_path = PathBuf::from("/home/user/.local/share/Trash/info/report.txt.trashinfo");
        let original_path = PathBuf::from("/home/user/report.txt");
        let trash_entry = TrashEntry::from_historical_entry(
            trash_path.clone(),
            info_path.clone(),
            original_path.clone(),
            None,
            test_entry(&trash_path),
        );
        let expected_restore_entry = trash_entry.restore_entry();
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.current_dir = trash_location_path();
        browser.is_trash_view = true;
        browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
        browser.entries = vec![trash_entry.entry.clone()].into();
        browser.trash_entries = vec![trash_entry];
        browser.selected = Some(trash_path.clone());
        browser.selected_paths = HashSet::from([trash_path]);
        (browser, expected_restore_entry)
    }

    #[test]
    fn open_selected_shortcut_restores_selected_trash_entry() {
        let (mut browser, expected_restore_entry) = browser_with_selected_trash_entry();

        let command = browser.invoke_shortcut(ShortcutAction::OpenSelected);
        drop(command);

        assert_eq!(browser.operation_queue.tasks().len(), 1);
        assert!(matches!(
            &browser.operation_queue.tasks()[0].operation,
            QueuedFileOperation::Restore { entries } if entries == &vec![expected_restore_entry]
        ));
    }

    #[test]
    fn ignored_delete_enqueues_immediately_while_captured_delete_stops() {
        let selected_path = PathBuf::from("/workspace/report.txt");
        let mut ignored_browser = FileBrowser::new(config::default_user_config()).0;
        ignored_browser.current_dir = PathBuf::from("/workspace");
        ignored_browser.entries = vec![test_entry(&selected_path)].into();
        ignored_browser.selected = Some(selected_path.clone());
        ignored_browser.selected_paths = HashSet::from([selected_path.clone()]);

        drop(ignored_browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Modifiers::default(),
            event::Status::Ignored,
        ));

        assert_eq!(ignored_browser.operation_queue.tasks().len(), 1);
        assert!(matches!(
            &ignored_browser.operation_queue.tasks()[0].operation,
            QueuedFileOperation::Trash { paths } if paths == &vec![selected_path.clone()]
        ));

        let mut captured_browser = FileBrowser::new(config::default_user_config()).0;
        captured_browser.current_dir = PathBuf::from("/workspace");
        captured_browser.entries = vec![test_entry(&selected_path)].into();
        captured_browser.selected = Some(selected_path.clone());
        captured_browser.selected_paths = HashSet::from([selected_path]);

        drop(captured_browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Modifiers::default(),
            event::Status::Captured,
        ));

        assert!(captured_browser.operation_queue.tasks().is_empty());
    }

    #[test]
    fn ignored_delete_does_not_cross_destructive_confirmation() {
        let selected_path = PathBuf::from("/workspace/report.txt");
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        browser.current_dir = PathBuf::from("/workspace");
        browser.entries = vec![test_entry(&selected_path)].into();
        browser.selected = Some(selected_path.clone());
        browser.selected_paths = HashSet::from([selected_path.clone()]);
        browser.destructive_action_confirmation = Some(
            crate::model::DestructiveActionConfirmation::DeletePermanently {
                paths: vec![selected_path],
            },
        );

        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Modifiers::default(),
            event::Status::Ignored,
        ));

        assert!(browser.operation_queue.tasks().is_empty());
        assert!(browser.destructive_action_confirmation.is_some());
    }

    #[test]
    fn ignored_delete_does_not_cross_file_drop_prompt() {
        let selected_path = PathBuf::from("/workspace/report.txt");
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        browser.current_dir = PathBuf::from("/workspace");
        browser.entries = vec![test_entry(&selected_path)].into();
        browser.selected = Some(selected_path.clone());
        browser.selected_paths = HashSet::from([selected_path]);
        browser.file_drop_prompt = Some(crate::model::FileDropPrompt {
            paste_directory: PathBuf::from("/workspace"),
            paths: vec![PathBuf::from("/incoming/report.txt")],
        });

        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Modifiers::default(),
            event::Status::Ignored,
        ));

        assert!(browser.operation_queue.tasks().is_empty());
        assert!(browser.file_drop_prompt.is_some());
    }

    #[test]
    fn ignored_delete_does_not_cross_network_connection_editor() {
        let selected_path = PathBuf::from("/workspace/report.txt");
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        browser.current_dir = PathBuf::from("/workspace");
        browser.entries = vec![test_entry(&selected_path)].into();
        browser.selected = Some(selected_path.clone());
        browser.selected_paths = HashSet::from([selected_path]);
        browser.network_connection_editor =
            Some(crate::network_connections::NetworkConnectionEditorState::add());

        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Modifiers::default(),
            event::Status::Ignored,
        ));

        assert!(browser.operation_queue.tasks().is_empty());
        assert!(browser.network_connection_editor.is_some());
    }

    #[test]
    fn keyboard_shortcut_route_enforces_context_ownership_matrix() {
        assert_eq!(
            keyboard_shortcut_route(
                ShortcutAction::Delete,
                &keyboard::Key::Named(key::Named::Delete),
                event::Status::Captured,
            ),
            KeyboardShortcutRoute::Stop
        );
        assert_eq!(
            keyboard_shortcut_route(
                ShortcutAction::Delete,
                &keyboard::Key::Named(key::Named::Delete),
                event::Status::Ignored,
            ),
            KeyboardShortcutRoute::InvokeFileBrowserContent
        );
        assert_eq!(
            keyboard_shortcut_route(
                ShortcutAction::Undo,
                &keyboard::Key::Character("z".into()),
                event::Status::Ignored,
            ),
            KeyboardShortcutRoute::QueryTextInputFocus
        );
        assert_eq!(
            keyboard_shortcut_route(
                ShortcutAction::Delete,
                &keyboard::Key::Character("d".into()),
                event::Status::Ignored,
            ),
            KeyboardShortcutRoute::QueryTextInputFocus
        );
        assert_eq!(
            keyboard_shortcut_route(
                ShortcutAction::Escape,
                &keyboard::Key::Named(key::Named::Escape),
                event::Status::Captured,
            ),
            KeyboardShortcutRoute::InvokeApplication
        );
        assert_eq!(
            keyboard_shortcut_route(
                ShortcutAction::Preview,
                &keyboard::Key::Named(key::Named::Space),
                event::Status::Captured,
            ),
            KeyboardShortcutRoute::CapturedPreview
        );
    }
}
