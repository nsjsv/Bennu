use iced::keyboard::{self, key};
use iced::{event, Task};

use super::tabs::TabDirection;
use super::text_input_shortcuts;
use super::FileBrowser;
use crate::actions::ActionId;
use crate::config::{ViewDensityLevel, ViewDensityStep};
use crate::model::{
    AdvancedNewFolderMessage, BrowserViewMode, Message, PathSuggestionDirection, PendingOperation,
};
use crate::shortcuts::{
    fixed_key, routing_context, CaptureRejection, CaptureSlot, FileBrowserShortcutOwnership,
    FileContentKey, FixedKey, KeyArea, KeyPress, LetterMode, LiveKeymap, NamedKey, ShortcutAction,
    ShortcutCaptureState, ShortcutRoutingContext, ShortcutTarget, TypedLetter,
};

impl FileBrowser {
    /// 现在实际的键位：用户设的键，加上动作文件夹里现在有哪些动作。
    pub(crate) fn keymap(&self) -> LiveKeymap<'_, impl Fn(&ActionId) -> bool + '_> {
        let library = &self.action_library;
        self.user_config
            .shortcuts
            .live(move |id| library.contains(id))
    }

    pub(super) fn handle_keyboard_key_pressed(
        &mut self,
        key: keyboard::Key,
        modified_key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        status: event::Status,
    ) -> Task<Message> {
        if self.shortcut_capture.is_some() {
            return self.capture_shortcut_key(&key, modifiers);
        }

        if let Some(task) = self.video_fullscreen_key(&key) {
            return task;
        }

        // 终端面板聚焦时键盘归 PTY;除宿主保留键外不进入文件管理器快捷键路由。
        if let Some(task) = self.terminal_panel_keyboard_input(&key, &modified_key, modifiers) {
            return task;
        }

        if self.shortcut_help_open {
            return self.shortcut_help_key(&key, modifiers);
        }
        if let Some(task) = self.super_box_key(&key, modifiers) {
            return task;
        }
        if let Some(task) = self.file_context_menu_key(&key, modifiers) {
            return task;
        }

        // 高级新建文件夹弹窗:Ctrl+Enter 快捷创建,优先于内容快捷键路由;
        // 编辑器把 Ctrl+Enter 当换行捕获,但事件仍会到达全局路由层。
        if self.advanced_new_folder.is_some()
            && matches!(key.as_ref(), keyboard::Key::Named(key::Named::Enter))
            && modifiers.control()
        {
            return self.handle_advanced_new_folder_message(AdvancedNewFolderMessage::Confirmed);
        }

        if let Some(command) = self.handle_path_suggestion_keyboard_key(&key, modifiers) {
            return command;
        }

        // 字母层：不带 Ctrl/Alt 的字符键（含别的布局的字母，它们进不了快捷键表）。
        if let Some(letter) = TypedLetter::from_iced(&key, modifiers) {
            return self.route_file_content_key(
                FileContentKey::Letter(letter),
                ShortcutRoutingContext::FileBrowserContent(
                    FileBrowserShortcutOwnership::FocusedTextInputProbe,
                ),
                status,
            );
        }
        let Some(press) = KeyPress::from_iced(&key, modifiers) else {
            return Task::none();
        };
        // vim 的 `/` 跳转用 Enter 收尾：光标已经在匹配项上，这一下不再打开它。
        if press == KeyPress::named(NamedKey::Enter)
            && status == event::Status::Ignored
            && self.vim.finish_jump(std::time::Instant::now())
        {
            return Task::none();
        }
        match fixed_key(&press) {
            Some(FixedKey::Escape) => return self.handle_escape_key(status),
            Some(FixedKey::TabNumber(number)) if self.application_shortcuts_enabled() => {
                return self.select_tab_by_number(number);
            }
            Some(FixedKey::TabNumber(_)) => return Task::none(),
            Some(FixedKey::Move(movement)) => {
                return self.route_file_content_key(
                    FileContentKey::Move(movement),
                    routing_context(
                        &press,
                        crate::shortcuts::DeclaredContext::FileBrowserContent,
                    ),
                    status,
                );
            }
            None => {}
        }

        let target = self.keymap().target_for(KeyArea::FileBrowser, &press);
        let action = match target {
            None => return Task::none(),
            Some(ShortcutTarget::Builtin(action)) => action,
            // 动作作用在选中的项上，和删除、复制这些文件区的键一样先让给文本框。
            Some(ShortcutTarget::Action(id)) => {
                return self.route_file_content_key(
                    FileContentKey::UserAction(id),
                    routing_context(
                        &press,
                        crate::shortcuts::DeclaredContext::FileBrowserContent,
                    ),
                    status,
                )
            }
        };
        if action == ShortcutAction::Preview && status == event::Status::Captured {
            return self.handle_captured_preview_shortcut();
        }
        // 文件区的键在辅助窗口里不起作用；属性窗口里的刷新键刷新的是属性。
        if action == ShortcutAction::Refresh && self.properties_window == Some(self.focused_window)
        {
            return self.refresh_file_properties();
        }
        self.route_file_content_key(
            FileContentKey::Action(action),
            routing_context(&press, action.declared_context()),
            status,
        )
    }

    /// 辅助窗口里被捕获的 Escape，是聚焦的输入框吃掉的（iced text_input 只在聚焦时
    /// 捕获 Escape，并借此失焦）：这一下只离开输入框，第二下才关窗口。主窗口不算：
    /// 地址栏编辑正是靠这次被捕获的 Escape 来取消编辑。
    fn handle_escape_key(&mut self, status: event::Status) -> Task<Message> {
        if status == event::Status::Captured && self.auxiliary_window(self.focused_window).is_some()
        {
            return Task::none();
        }
        self.handle_escape_shortcut()
    }

    fn route_file_content_key(
        &mut self,
        content_key: FileContentKey,
        context: ShortcutRoutingContext,
        status: event::Status,
    ) -> Task<Message> {
        match (context, status) {
            (ShortcutRoutingContext::Application, _) if self.application_shortcuts_enabled() => {
                self.invoke_file_content_key(content_key)
            }
            (ShortcutRoutingContext::Application, _) => Task::none(),
            (ShortcutRoutingContext::FileBrowserContent(_), event::Status::Captured) => {
                Task::none()
            }
            (
                ShortcutRoutingContext::FileBrowserContent(
                    FileBrowserShortcutOwnership::CapturedDeleteEvent,
                ),
                event::Status::Ignored,
            ) => self.invoke_routed_file_content_key(content_key),
            (
                ShortcutRoutingContext::FileBrowserContent(
                    FileBrowserShortcutOwnership::FocusedTextInputProbe,
                ),
                event::Status::Ignored,
            ) => text_input_shortcuts::route_ignored_file_content_key(content_key),
        }
    }

    /// 已确认没有文本框在用这个键的文件区按键：弹窗、菜单、辅助窗口在前台时仍不执行。
    pub(super) fn invoke_routed_file_content_key(
        &mut self,
        content_key: FileContentKey,
    ) -> Task<Message> {
        if self.file_browser_content_shortcuts_enabled() {
            self.invoke_file_content_key(content_key)
        } else {
            Task::none()
        }
    }

    pub(super) fn invoke_file_content_key(&mut self, content_key: FileContentKey) -> Task<Message> {
        match content_key {
            FileContentKey::Action(action) => self.invoke_shortcut(action),
            FileContentKey::UserAction(id) => self.invoke_action_from_keyboard(&id),
            FileContentKey::Move(movement) => {
                self.type_ahead.clear();
                self.move_file_selection(movement)
            }
            FileContentKey::Letter(letter) => match self.user_config.letter_mode {
                LetterMode::TypeToJump => self.jump_to_typed_name(letter.character),
                LetterMode::Vim => self.run_vim_letter(letter),
            },
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

    /// 主窗口上必须先处理完的对话框（确认、冲突、解压、批量重命名……）开着。
    pub(crate) fn main_window_dialog_is_open(&self) -> bool {
        self.super_box.is_some() || self.dialog_above_super_box_is_open()
    }

    /// 超级输入框以外的对话框。它们排在框的上面（画面上盖住框，见主视图的浮层顺序），有的是后台
    /// 事件弹出来的（收到的传输、打开失败后的“打开方式”），框开着时也会出现：这时键给它们，
    /// 框等它们关掉再回来。
    pub(crate) fn dialog_above_super_box_is_open(&self) -> bool {
        self.destructive_action_confirmation.is_some()
            || self.incoming_transfer.is_some()
            || !self.incoming_messages.is_empty()
            || self.file_drop_prompt.is_some()
            || self.transfer_conflict.is_some()
            || self.archive_creation.is_some()
            || self.convert.is_some()
            || self.checksum.is_some()
            || self.archive_extraction.is_some()
            || self.archive_member_password.is_some()
            || self.batch_rename.is_some()
            || self.advanced_new_folder.is_some()
            || self.network_connection_editor.is_some()
            || self.open_with.is_some()
            || self.shortcut_help_open
    }

    /// 应用级快捷键（导航、标签页、视图、面板）只作用于主窗口：焦点在设置、属性等辅助窗口
    /// 或主窗口开着对话框时不执行。
    fn application_shortcuts_enabled(&self) -> bool {
        self.focused_window == self.main_window && !self.main_window_dialog_is_open()
    }

    pub(super) fn file_browser_content_shortcuts_enabled(&self) -> bool {
        !self.main_window_dialog_is_open()
            && self.context_menu.is_none()
            && self.settings_window != Some(self.focused_window)
            && self.properties_window != Some(self.focused_window)
            && self.preview_engine.preview_window != Some(self.focused_window)
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

    pub(crate) fn invoke_shortcut(&mut self, action: ShortcutAction) -> Task<Message> {
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
            ShortcutAction::FileProperties => self.open_selected_file_properties(),
            ShortcutAction::Refresh => {
                Task::batch([self.commit_rename_if_active(), self.reload_visible_panes()])
            }
            ShortcutAction::Preview => self.request_preview(),
            ShortcutAction::ToggleRightDock => self.toggle_right_dock(),
            ShortcutAction::TogglePreviewPanel => {
                self.toggle_dock_panel(crate::right_dock::DockPanel::Preview)
            }
            ShortcutAction::ToggleInfoPanel => {
                self.toggle_dock_panel(crate::right_dock::DockPanel::Info)
            }
            ShortcutAction::ToggleFolderTreePanel => {
                self.toggle_dock_panel(crate::right_dock::DockPanel::FolderTree)
            }
            ShortcutAction::ToggleQuickActionsPanel => {
                self.toggle_dock_panel(crate::right_dock::DockPanel::QuickActions)
            }
            ShortcutAction::ToggleTaskLogPanel => {
                self.toggle_dock_panel(crate::right_dock::DockPanel::TaskLog)
            }
            ShortcutAction::ToggleRightDockLock => self.toggle_right_dock_lock(),
            ShortcutAction::ToggleTerminal => self.toggle_terminal_via_shortcut(),
            ShortcutAction::TerminalNewTab => self.add_terminal_tab(),
            ShortcutAction::TerminalCloseTab => self.close_active_terminal_tab_via_shortcut(),
            ShortcutAction::TerminalFind => self.open_terminal_search(),
            ShortcutAction::SelectAll => self.select_all_in_file_selection_scope(),
            ShortcutAction::InvertSelection => self.invert_selection_in_file_selection_scope(),
            ShortcutAction::SelectDuplicates => self.select_duplicate_files(),
            ShortcutAction::Copy => self.copy_selected(),
            ShortcutAction::DuplicateSelected => self.duplicate_selected(),
            ShortcutAction::NewFolderFromSelection => self.new_folder_from_selection(),
            ShortcutAction::Paste => self.paste_pending(),
            ShortcutAction::Cut => self.move_selected(),
            ShortcutAction::Delete => self.trash_selected(),
            ShortcutAction::Undo => self.undo_file_operation(),
            ShortcutAction::Redo => self.redo_file_operation(),
            ShortcutAction::NavigateHome => self.navigate_home(),
            ShortcutAction::Search => self.focus_search_input(),
            ShortcutAction::FindLargeFiles => self.find_large_files_here(),
            ShortcutAction::ToggleHiddenFiles => self.toggle_show_hidden_files(),
            ShortcutAction::OpenInNewTab => self.open_selected_in_new_tab(),
            ShortcutAction::NewFolder => self.create_folder_from_keyboard(),
            ShortcutAction::NewFile => self.create_file_from_keyboard(),
            ShortcutAction::CopyPath => self.copy_selected_path_text(),
            ShortcutAction::CreateSymlink => self.create_symlinks_for_selection(),
            ShortcutAction::DeletePermanently => self.delete_selected_permanently(),
            ShortcutAction::AddToFavorites => self.add_keyboard_target_to_favorites(),
            ShortcutAction::NewTab => self.open_new_tab_here(),
            ShortcutAction::CloseTab => self.close_active_tab_from_keyboard(),
            ShortcutAction::RestoreClosedTab => self.restore_closed_tab(),
            ShortcutAction::NewWindow => self.open_new_window_here(),
            ShortcutAction::OpenInNewWindow => self.open_selected_in_new_window(),
            ShortcutAction::CloseWindow => self.close_auxiliary_window(self.main_window),
            ShortcutAction::ToggleSplit => self.toggle_split_view(),
            ShortcutAction::FocusOtherPane => self.focus_other_pane(),
            ShortcutAction::CopyToOtherPane => {
                self.transfer_selection_to_other_pane(PendingOperation::Copy)
            }
            ShortcutAction::MoveToOtherPane => {
                self.transfer_selection_to_other_pane(PendingOperation::Move)
            }
            ShortcutAction::NextTab => self.select_neighbor_tab(TabDirection::Right),
            ShortcutAction::PreviousTab => self.select_neighbor_tab(TabDirection::Left),
            ShortcutAction::MoveTabLeft => self.move_active_tab(TabDirection::Left),
            ShortcutAction::MoveTabRight => self.move_active_tab(TabDirection::Right),
            ShortcutAction::ColumnsView => {
                self.select_browser_view_mode(self.active_pane_id(), BrowserViewMode::Columns)
            }
            ShortcutAction::ListView => {
                self.select_browser_view_mode(self.active_pane_id(), BrowserViewMode::List)
            }
            ShortcutAction::IconsView => {
                self.select_browser_view_mode(self.active_pane_id(), BrowserViewMode::Icons)
            }
            ShortcutAction::ZoomIn => {
                self.change_active_view_density(|density| density.step(ViewDensityStep::Increase))
            }
            ShortcutAction::ZoomOut => {
                self.change_active_view_density(|density| density.step(ViewDensityStep::Decrease))
            }
            ShortcutAction::ZoomReset => {
                self.change_active_view_density(|_| ViewDensityLevel::DEFAULT)
            }
            ShortcutAction::OpenExternalTerminal => self.open_external_terminal_from_keyboard(),
            ShortcutAction::ToggleSidebar => self.toggle_sidebar(),
            ShortcutAction::OpenSettings => self.open_settings(),
            ShortcutAction::ShowShortcuts => self.toggle_shortcut_help(),
            ShortcutAction::OpenSuperBox => self.toggle_super_box(),
            ShortcutAction::OpenContextMenu => self.open_context_menu_from_keyboard(),
        }
    }

    /// 先收搜索历史浮层、筛选下拉；在搜索结果里且没有别的浮层占着键盘时退出搜索。
    pub(super) fn handle_escape_shortcut(&mut self) -> Task<Message> {
        if self
            .search_history_interaction
            .popup_is_visible(&self.user_config.search_history)
        {
            self.search_history_interaction.dismiss_popup();
            return Task::none();
        }
        if self.search_filter_menu.is_some() {
            return self.close_search_filter_menu();
        }
        if self.active_search().is_some() && self.file_browser_content_shortcuts_enabled() {
            return self.close_search_workspace();
        }
        self.handle_focused_window_escape_pressed()
    }

    pub(super) fn start_shortcut_capture(
        &mut self,
        target: ShortcutTarget,
        slot: CaptureSlot,
    ) -> Task<Message> {
        self.shortcut_capture = Some(ShortcutCaptureState::new(target, slot));
        Task::none()
    }

    pub(super) fn cancel_shortcut_capture(&mut self) -> Task<Message> {
        self.shortcut_capture = None;
        Task::none()
    }

    /// 录键：单独按下的修饰键是组合键的开头，不算；Esc 是固定键，按它就是取消。
    fn capture_shortcut_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> Task<Message> {
        let Some(capture) = self.shortcut_capture.clone() else {
            return Task::none();
        };
        match key.as_ref() {
            keyboard::Key::Named(key::Named::Escape) => return self.cancel_shortcut_capture(),
            keyboard::Key::Named(
                key::Named::Control
                | key::Named::Shift
                | key::Named::Alt
                | key::Named::AltGraph
                | key::Named::Super
                | key::Named::Meta,
            ) => return Task::none(),
            _ => {}
        }
        let captured = KeyPress::from_iced(key, modifiers)
            .ok_or(CaptureRejection::Unsupported)
            .and_then(|press| {
                self.keymap()
                    .keys_after_capture(&capture.target, capture.slot, press)
            });
        match captured {
            Ok(keys) => self.save_shortcut_keys(capture.target, keys),
            Err(rejection) => {
                self.shortcut_capture = Some(ShortcutCaptureState {
                    rejection: Some(rejection),
                    ..capture
                });
                Task::none()
            }
        }
    }

    /// 录键提示行上的“删掉这个键”：只对替换某个已有键的录键有效。
    pub(super) fn remove_captured_shortcut_key(&mut self) -> Task<Message> {
        let Some(ShortcutCaptureState {
            target,
            slot: CaptureSlot::Replace(index),
            ..
        }) = self.shortcut_capture.clone()
        else {
            return Task::none();
        };
        let mut keys = self.keymap().keys(target.clone());
        if index < keys.len() {
            keys.remove(index);
        }
        self.save_shortcut_keys(target, keys)
    }

    pub(super) fn reset_shortcut_keys(&mut self, target: ShortcutTarget) -> Task<Message> {
        self.user_config.shortcuts.reset(target);
        self.shortcut_capture = None;
        self.persist_user_preferences_command()
    }

    fn save_shortcut_keys(&mut self, target: ShortcutTarget, keys: Vec<KeyPress>) -> Task<Message> {
        self.user_config.shortcuts.set_keys(target, keys);
        self.shortcut_capture = None;
        self.persist_user_preferences_command()
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
    use crate::model::BrowserLocation;
    use file_operations::QueuedFileOperation;

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
        browser.tab_mut().location = BrowserLocation::Trash;
        browser.tab_mut().directory_collection_phase =
            crate::model::DirectoryCollectionPhase::Ready;
        browser.tab_mut().entries = vec![trash_entry.entry.clone()].into();
        browser.tab_mut().trash_entries = vec![trash_entry];
        browser.tab_mut().selected = Some(trash_path.clone());
        *browser.tab_mut().selected_paths = HashSet::from([trash_path]);
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
        ignored_browser.tab_mut().location =
            BrowserLocation::Directory(PathBuf::from("/workspace"));
        ignored_browser.tab_mut().entries = vec![test_entry(&selected_path)].into();
        ignored_browser.tab_mut().selected = Some(selected_path.clone());
        *ignored_browser.tab_mut().selected_paths = HashSet::from([selected_path.clone()]);

        drop(ignored_browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
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
        captured_browser.tab_mut().location =
            BrowserLocation::Directory(PathBuf::from("/workspace"));
        captured_browser.tab_mut().entries = vec![test_entry(&selected_path)].into();
        captured_browser.tab_mut().selected = Some(selected_path.clone());
        *captured_browser.tab_mut().selected_paths = HashSet::from([selected_path]);

        drop(captured_browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Modifiers::default(),
            event::Status::Captured,
        ));

        assert!(captured_browser.operation_queue.tasks().is_empty());
    }

    fn escape(browser: &mut FileBrowser, status: event::Status) {
        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Escape),
            keyboard::Key::Named(key::Named::Escape),
            keyboard::Modifiers::default(),
            status,
        ));
    }

    #[test]
    fn escape_taken_by_a_text_field_leaves_auxiliary_windows_open() {
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        let settings_window = iced::window::Id::unique();
        browser.settings_window = Some(settings_window);
        browser.focused_window = settings_window;

        escape(&mut browser, event::Status::Captured);
        assert_eq!(browser.settings_window, Some(settings_window));

        escape(&mut browser, event::Status::Ignored);
        assert_eq!(browser.settings_window, None);
    }

    #[test]
    fn captured_escape_still_cancels_main_window_address_editing() {
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        drop(browser.begin_address_editing(browser.active_pane_id()));
        assert!(browser.address_editing.is_some());

        escape(&mut browser, event::Status::Captured);
        assert!(browser.address_editing.is_none());
    }

    #[test]
    fn ignored_delete_does_not_cross_destructive_confirmation() {
        let selected_path = PathBuf::from("/workspace/report.txt");
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
        browser.tab_mut().entries = vec![test_entry(&selected_path)].into();
        browser.tab_mut().selected = Some(selected_path.clone());
        *browser.tab_mut().selected_paths = HashSet::from([selected_path.clone()]);
        browser.destructive_action_confirmation = Some(
            crate::model::DestructiveActionConfirmation::DeletePermanently {
                paths: vec![selected_path],
            },
        );

        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
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
        browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
        browser.tab_mut().entries = vec![test_entry(&selected_path)].into();
        browser.tab_mut().selected = Some(selected_path.clone());
        *browser.tab_mut().selected_paths = HashSet::from([selected_path]);
        browser.file_drop_prompt = Some(crate::model::FileDropPrompt {
            paste_directory: PathBuf::from("/workspace"),
            paths: vec![PathBuf::from("/incoming/report.txt")],
        });

        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
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
        browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
        browser.tab_mut().entries = vec![test_entry(&selected_path)].into();
        browser.tab_mut().selected = Some(selected_path.clone());
        *browser.tab_mut().selected_paths = HashSet::from([selected_path]);
        browser.network_connection_editor =
            Some(crate::network_connections::NetworkConnectionEditorState::add());

        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Key::Named(key::Named::Delete),
            keyboard::Modifiers::default(),
            event::Status::Ignored,
        ));

        assert!(browser.operation_queue.tasks().is_empty());
        assert!(browser.network_connection_editor.is_some());
    }

    #[test]
    fn captured_backspace_stays_with_the_text_field() {
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        let directory = PathBuf::from("/workspace/project");
        browser.tab_mut().location = BrowserLocation::Directory(directory.clone());

        drop(browser.handle_keyboard_key_pressed(
            keyboard::Key::Named(key::Named::Backspace),
            keyboard::Key::Named(key::Named::Backspace),
            keyboard::Modifiers::default(),
            event::Status::Captured,
        ));

        assert_eq!(
            browser.tab().location,
            BrowserLocation::Directory(directory)
        );
    }

    #[test]
    fn routed_file_content_keys_wait_for_an_open_prompt_to_close() {
        let selected_path = PathBuf::from("/workspace/report.txt");
        let mut browser = FileBrowser::new(config::default_user_config()).0;
        browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
        browser.tab_mut().entries = vec![test_entry(&selected_path)].into();
        browser.tab_mut().selected = Some(selected_path.clone());
        *browser.tab_mut().selected_paths = HashSet::from([selected_path]);
        browser.file_drop_prompt = Some(crate::model::FileDropPrompt {
            paste_directory: PathBuf::from("/workspace"),
            paths: vec![PathBuf::from("/incoming/report.txt")],
        });

        drop(
            browser.invoke_routed_file_content_key(FileContentKey::Action(ShortcutAction::Delete)),
        );

        assert!(browser.operation_queue.tasks().is_empty());
    }
}
