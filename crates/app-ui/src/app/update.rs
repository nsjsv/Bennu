use super::entry_hover_recalc::ScrolledView;
use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::model::{Message, NavigationMode, ScrollbarRegion as Region};
use iced::Task;
impl FileBrowser {
    pub(super) fn update(&mut self, message: Message) -> Task<Message> {
        if !self.application_shutdown_phase.is_running() {
            return match message {
                Message::FileOperationFinished(task_id, generation, completion) => self
                    .accept_application_shutdown_operation_finished(
                        task_id, generation, completion,
                    ),
                Message::FileOperationPersistenceFinished(outcome) => {
                    self.accept_file_operation_persistence_finished(outcome)
                }
                Message::BrowserSessionSaved(outcome) => {
                    self.accept_application_shutdown_browser_session_saved(outcome)
                }
                Message::UserPreferencesSaved(outcome) => {
                    self.accept_application_shutdown_user_preferences_saved(outcome)
                }
                Message::ApplicationWindowClosed(window) => {
                    self.accept_application_window_closed(window)
                }
                Message::ApplicationWindowCloseCommandsFinished => {
                    self.accept_application_window_close_commands_finished()
                }
                Message::ApplicationShutdownPersisted(outcome) => {
                    self.accept_application_shutdown_persisted(outcome)
                }
                _ => Task::none(),
            };
        }
        let command = self.dispatch(message);
        // D1 单点收敛:面板开启时让预览状态跟上活动窗格选中项。
        // 面板与 Space 预览共用引擎的 preview 槽,而 clear_preview 被 30 多个
        // 交互入口(拖拽起手、菜单、选择变化……)顺手调用,面板内容随时可能被
        // 清掉;不把槽拆开,就只能在出口重新对齐一次。面板关闭时是空操作。
        let right_preview_panel_command = self.sync_right_preview_panel();
        Task::batch([command, right_preview_panel_command])
    }

    /// 消息分派体：自 update 的主 match 提出，出口收敛钩子只在 update 里执行一次。
    fn dispatch(&mut self, message: Message) -> Task<Message> {
        let command = match message {
            Message::StartupEnvironmentLoaded(startup_environment) => {
                self.accept_startup_environment(*startup_environment)
            }
            Message::Preview(preview_message) => self.handle_preview_message(preview_message),
            Message::SidebarLocationsLoaded(sidebar_locations) => {
                self.accept_sidebar_locations(sidebar_locations)
            }
            Message::SidebarDevicesLoaded(devices) => self.accept_sidebar_devices(devices),
            Message::SidebarDevicesRefreshRequested => self.refresh_sidebar_devices(),
            Message::SidebarDeviceHovered(id) => self.handle_sidebar_device_hovered(id),
            Message::SidebarDeviceHoverCleared(id) => self.handle_sidebar_device_hover_cleared(id),
            Message::SidebarDevicePressed(id) => self.handle_sidebar_device_pressed(id),
            Message::SidebarDeviceMiddlePressed(pane_id, id) => {
                self.handle_sidebar_device_middle_pressed(pane_id, id)
            }
            Message::SidebarDeviceRightClicked(id) => self.handle_sidebar_device_right_clicked(id),
            Message::SidebarDeviceActionSelected(id, action) => {
                self.perform_sidebar_device_action(id, action)
            }
            Message::SidebarDeviceActionFinished(request, action, result) => {
                self.accept_sidebar_device_action_finished(request, action, result)
            }
            Message::NetworkConnection(message) => self.handle_network_connection_message(message),
            Message::OperationStoreLoaded(operation_store) => {
                self.accept_operation_store(operation_store)
            }
            Message::DirectoryDiscoveryBatch(request, batch) => {
                self.accept_directory_discovery_batch(request, batch)
            }
            Message::DirectoryEntriesReady(request, Ok(prebuilt)) => {
                self.accept_directory_discovery(request, prebuilt)
            }
            Message::DirectoryEntriesReady(request, Err(failure)) => {
                self.accept_directory_load_failure(request, failure)
            }
            Message::DirectoryMetadataResolved(request, outcome) => {
                self.accept_directory_metadata_resolution(request, outcome)
            }
            Message::TrashLoaded(generation, outcome) => {
                self.accept_trash_refresh_completion(generation, outcome)
            }
            Message::TrashWarningsToggled => {
                self.trash_refresh.toggle_warning_details();
                Task::none()
            }
            Message::OpenFileFinished(_, Ok(())) => {
                self.retract_global_error(GlobalErrorSource::Launch);
                Task::none()
            }
            Message::OpenFileFinished(path, Err(error)) => {
                self.request_open_with_after_default_open_failed(path, error)
            }
            Message::OpenWithRequested(path) => self.request_open_with_applications(path),
            Message::OpenWithApplicationsLoaded(path, applications) => {
                self.accept_open_with_applications(path, applications)
            }
            Message::OpenWithDefaultApplicationToggled(selected) => {
                self.toggle_open_with_default_application(selected)
            }
            Message::OpenWithApplicationSelected(desktop_id) => {
                self.select_open_with_application(desktop_id)
            }
            Message::OpenWithApplicationFinished(result) => {
                self.accept_open_with_application_finished(result)
            }
            Message::OpenTerminalFinished(Ok(())) => {
                self.retract_global_error(GlobalErrorSource::Launch);
                Task::none()
            }
            Message::OpenTerminalFinished(Err(error)) => {
                self.show_global_error_from(GlobalErrorSource::Launch, error);
                Task::none()
            }
            Message::FileProperties(properties_message) => {
                self.accept_file_properties_message(properties_message)
            }
            Message::FileOperationProgressed(task_id, progress, transfer_snapshots) => {
                self.operation_queue.note_driver_signal(task_id);
                if let Some(error) =
                    self.operation_queue
                        .update_progress(task_id, progress, transfer_snapshots)
                {
                    self.show_global_error(error);
                }
                Task::none()
            }
            Message::FileOperationDirectMovesCommitted { task_id, commits } => {
                self.operation_queue.note_driver_signal(task_id);
                self.accept_file_operation_direct_moves_committed(task_id, commits)
            }
            Message::FileOperationMovesRenamed { task_id, moves } => {
                self.operation_queue.note_driver_signal(task_id);
                self.accept_file_operation_moves_renamed(moves)
            }
            Message::FileOperationFinished(task_id, generation, completion) => {
                self.operation_queue.note_driver_signal(task_id);
                if !self
                    .operation_queue
                    .driver_generation_is_current(task_id, generation)
                {
                    // 换代重启后被取消的旧驱动者迟到的终态:任务已由新驱动者接手。
                    tracing::warn!(
                        target: "app_ui::operation_supervision",
                        task_id,
                        generation,
                        "stale driver completion was rejected"
                    );
                    return Task::none();
                }
                self.accept_file_operation_finished(task_id, completion)
            }
            Message::OperationSupervisionTick => {
                if let Some(error) = self.operation_queue.supervise(std::time::Instant::now()) {
                    self.show_global_error(error);
                }
                self.launch_ready_driver()
            }
            Message::FileOperationPersistenceFinished(outcome) => Task::batch([
                self.accept_file_operation_persistence_finished(outcome),
                // 写库回执给可恢复任务配上存储 id，它此刻才具备启动条件。
                self.launch_ready_driver(),
            ]),
            Message::OperationProgressAnimationTick => {
                self.operation_progress_animation_frame =
                    self.operation_progress_animation_frame.wrapping_add(1);
                Task::none()
            }
            Message::FileDragSpringOpenTick => self.handle_file_drag_spring_open_tick(),
            Message::DesktopNotificationPublished(outcome) => {
                self.accept_desktop_notification_published(outcome)
            }
            Message::FileOperationIndicatorPressed => {
                self.context_menu = None;
                self.open_with = None;
                if self.operation_queue.is_panel_open() {
                    self.operation_queue.close_panel();
                } else {
                    self.operation_queue.open_panel();
                }
                Task::none()
            }
            Message::FileOperationPauseToggled(task_id) => {
                self.operation_queue.toggle_pause(task_id);
                self.continue_file_operation_persistence()
            }
            Message::FileOperationCancelRequested(task_id) => {
                self.operation_queue.cancel(task_id);
                Task::batch([
                    self.continue_file_operation_persistence(),
                    self.launch_ready_driver(),
                ])
            }
            Message::FileOperationDetailsCopyRequested(task_id) => self
                .operation_queue
                .tasks()
                .iter()
                .find(|task| task.id == task_id)
                .map(crate::operation_queue_display::file_operation_copy_text)
                .map(iced::clipboard::write)
                .unwrap_or_else(Task::none),
            Message::FileOperationClearRequested(task_id) => {
                self.operation_queue.clear_terminal_task(task_id);
                self.continue_file_operation_persistence()
            }
            Message::FileOperationClearFinishedRequested => {
                self.operation_queue.clear_terminal_tasks();
                self.continue_file_operation_persistence()
            }
            Message::ThumbnailBatchLoaded(outcomes) => self.accept_thumbnail_batch(outcomes),
            Message::BrowserViewModeSelected(pane_id, view_mode) => {
                self.select_browser_view_mode(pane_id, view_mode)
            }
            Message::IconGridDirectoryToggled(pane_id, anchor) => {
                self.toggle_icon_grid_directory(pane_id, anchor)
            }
            Message::IconGridPanelPressed(pane_id, directory) => {
                self.press_icon_grid_panel(pane_id, directory)
            }
            Message::ListDirectoryToggled(pane_id, path) => {
                self.toggle_list_directory(pane_id, path)
            }
            Message::FlatEntryClicked(pane_id, path) => {
                self.activate_pane(pane_id);
                if self.pane_drag.is_some() || self.ctrl_shift_pane_drag_shortcut_is_pressed() {
                    return Task::none();
                }
                self.handle_flat_entry_clicked(path)
            }
            Message::ListHeaderRightClicked(pane_id) => self.open_list_column_menu(pane_id),
            Message::ListColumnVisibilityToggled(column) => {
                self.toggle_list_column_visibility(column)
            }
            Message::FileGroupingModeSelected(mode) => self.select_file_grouping_mode(mode),
            Message::FileGroupingRailTargetSelected { pane, group_index } => {
                self.scroll_to_file_group_target(pane, group_index)
            }
            Message::FileGroupingRailCursorEntered(pane_id) => {
                self.enter_file_grouping_rail(pane_id)
            }
            Message::FileGroupingRailCursorExited(pane_id) => {
                self.exit_file_grouping_rail(pane_id);
                Task::none()
            }
            Message::ListColumnResizeStarted(pane_id, column) => {
                self.start_list_column_resize_drag(pane_id, column)
            }
            Message::ListColumnReorderStarted(pane_id, column) => {
                self.start_list_column_reorder_drag(pane_id, column)
            }
            Message::ListHeaderColumnEntered(pane_id, column) => {
                self.enter_list_header_column(pane_id, column)
            }
            Message::ListHeaderColumnExited(pane_id, column) => {
                self.exit_list_header_column(pane_id, column)
            }
            Message::ListDirectorySummaryLoaded(request, outcome) => {
                self.accept_list_directory_summary(request, outcome)
            }
            Message::ColumnEntryClicked(pane_id, path) => {
                self.activate_pane(pane_id);
                if self.pane_drag.is_some() || self.ctrl_shift_pane_drag_shortcut_is_pressed() {
                    return Task::none();
                }
                self.handle_column_entry_clicked(path)
            }
            Message::ColumnBlankClicked(pane_id, path) => {
                self.activate_pane(pane_id);
                if self.pane_drag.is_some() || self.ctrl_shift_pane_drag_shortcut_is_pressed() {
                    return Task::none();
                }
                self.start_column_blank_selection_marquee(path)
            }
            Message::ColumnPlaceholderPressed(pane_id) => {
                self.activate_pane(pane_id);
                if self.pane_drag.is_some() || self.ctrl_shift_pane_drag_shortcut_is_pressed() {
                    return Task::none();
                }
                self.handle_column_placeholder_pressed()
            }
            Message::EntryReleased(pane_id, path) => {
                let releasing_file_drag = self.file_drag.is_some();
                let release_directory = releasing_file_drag
                    .then(|| self.file_drag_release_directory_for_entry(pane_id, &path))
                    .flatten();
                if !releasing_file_drag {
                    self.activate_pane(pane_id);
                }
                let tab_drag_finished = self.finish_tab_drag();
                let pane_drag_finished = self.finish_pane_drag();
                Task::batch([
                    tab_drag_finished,
                    pane_drag_finished,
                    self.finish_sidebar_bookmark_drag(),
                    self.finish_sidebar_resize_drag_command(),
                    self.finish_right_preview_panel_drag_commands(),
                    self.finish_column_resize_drag_command(),
                    self.finish_list_column_resize_drag_command(),
                    self.finish_list_column_reorder_drag_command(),
                    self.finish_drag_selection(release_directory),
                    self.schedule_thumbnail_refresh(),
                    self.request_breadcrumb_drop_target_bounds_measurement(),
                ])
            }
            Message::EntryRightClicked(pane_id, path) => {
                self.activate_pane(pane_id);
                self.handle_entry_right_clicked(path)
            }
            Message::EntryHovered(pane_id, path) => {
                if pane_id == self.active_pane_id() {
                    self.handle_entry_hovered(path)
                } else if self.file_drag.is_some() {
                    self.handle_file_drag_entry_hovered_in_pane(pane_id, path)
                } else {
                    Task::none()
                }
            }
            Message::EntryHoverCleared(pane_id, path) => {
                if pane_id == self.active_pane_id() {
                    self.handle_entry_hover_cleared(path)
                } else if self.file_drag.is_some() {
                    self.handle_file_drag_entry_hover_cleared_in_pane(pane_id, path)
                } else {
                    Task::none()
                }
            }
            Message::DropTargetHovered(pane_id, directory) => {
                if pane_id == self.active_pane_id() {
                    self.handle_drop_target_hovered(directory)
                } else if self.file_drag.is_some() {
                    self.handle_file_drag_drop_target_hovered_in_pane(pane_id, directory)
                } else {
                    Task::none()
                }
            }
            Message::BreadcrumbDropTargetHovered(pane_id, directory) => {
                if pane_id == self.active_pane_id() {
                    self.handle_breadcrumb_drop_target_hovered(directory)
                } else if self.file_drag.is_some() {
                    self.handle_file_drag_drop_target_hovered_in_pane(pane_id, directory)
                } else {
                    Task::none()
                }
            }
            Message::BreadcrumbDropTargetHoverCleared(pane_id, directory) => {
                if pane_id == self.active_pane_id() {
                    self.handle_breadcrumb_drop_target_hover_cleared(directory)
                } else if self.file_drag.is_some() {
                    self.handle_file_drag_drop_target_hover_cleared_in_pane(pane_id, directory)
                } else {
                    Task::none()
                }
            }
            Message::DropTargetHoverCleared(pane_id, directory) => {
                if pane_id == self.active_pane_id() {
                    self.handle_drop_target_hover_cleared(directory)
                } else if self.file_drag.is_some() {
                    self.handle_file_drag_drop_target_hover_cleared_in_pane(pane_id, directory)
                } else {
                    Task::none()
                }
            }
            Message::DropTargetReleased(pane_id, directory) => {
                let release_directory = if self.file_drag.is_some() {
                    self.file_drag_release_directory_for_drop_target(pane_id, directory)
                } else {
                    None
                };
                let tab_drag_finished = self.finish_tab_drag();
                let pane_drag_finished = self.finish_pane_drag();
                Task::batch([
                    tab_drag_finished,
                    pane_drag_finished,
                    self.finish_sidebar_bookmark_drag(),
                    self.finish_sidebar_resize_drag_command(),
                    self.finish_right_preview_panel_drag_commands(),
                    self.finish_column_resize_drag_command(),
                    self.finish_list_column_resize_drag_command(),
                    self.finish_list_column_reorder_drag_command(),
                    self.finish_drag_selection(release_directory),
                    self.schedule_thumbnail_refresh(),
                    self.request_breadcrumb_drop_target_bounds_measurement(),
                ])
            }
            Message::BlankAreaPressed(pane_id) => {
                self.activate_pane(pane_id);
                if self.pane_drag.is_some() || self.ctrl_shift_pane_drag_shortcut_is_pressed() {
                    return Task::none();
                }
                self.start_selection_marquee()
            }
            Message::ColumnBlankRightClicked(pane_id, directory) => {
                self.activate_pane(pane_id);
                self.handle_column_blank_right_clicked(directory)
            }
            Message::BlankAreaRightClicked(pane_id, directory) => {
                self.activate_pane(pane_id);
                self.handle_blank_area_right_clicked(directory)
            }
            Message::SidebarHovered(path) => self.handle_sidebar_hovered(path),
            Message::SidebarHoverCleared(path) => self.handle_sidebar_hover_cleared(path),
            Message::SidebarPointerMoved(position) => {
                self.update_sidebar_bookmark_drop_slot(position)
            }
            Message::SidebarPointerExited => self.clear_sidebar_bookmark_drop_slot(),
            Message::SidebarBookmarkPressed(path) => self.start_sidebar_bookmark_drag(path),
            Message::SidebarBookmarkRightClicked(path) => {
                self.handle_sidebar_bookmark_right_clicked(path)
            }
            Message::SidebarBookmarkEntered(path) => self.handle_sidebar_bookmark_entered(path),
            Message::SidebarBookmarkReleased => self.finish_sidebar_bookmark_drag(),
            Message::SidebarBookmarkDeleteRequested(path) => self.delete_sidebar_bookmark(path),
            Message::SidebarResizeStarted => self.start_sidebar_resize_drag(),
            Message::RightPreviewPanelResizeStarted => self.start_right_preview_panel_resize_drag(),
            Message::RightPreviewPanelRatioResizeStarted => {
                self.start_right_preview_panel_ratio_resize_drag()
            }
            Message::ToggleRightPreviewPanel => self.toggle_right_preview_panel(),
            Message::SplitResizeStarted => self.start_split_resize(self.cursor_or_origin()),
            Message::TerminalPanel(message) => self.handle_terminal_panel_message(message),
            Message::CursorMoved { window, position } => {
                self.update_pointer_motion(window, position)
            }
            Message::CursorLeft { window } => {
                if self.preview_engine.preview_window == Some(window) {
                    let preview_engine = &mut self.preview_engine;
                    preview_engine.cancel_preview_window_initial_chrome_hide();
                    preview_engine.preview_window_pointer_y = None;
                    // 指针带着按下的左键离开预览窗口时，窗口外收不到
                    // release，就地结束平移，避免回进窗口后误跳偏移。
                    preview_engine.preview_image_viewport.panning = false;
                    preview_engine.preview_window_bottom_controls.start_hide();
                    if !preview_engine.preview_window_drag_active
                        && !preview_engine.preview_window_uses_window_chrome()
                    {
                        preview_engine.preview_window_chrome.start_hide();
                    }
                    return Task::none();
                }
                if window == self.main_window {
                    // 窗口内拖动期间收不到 release，必须在离开窗口时把
                    // 拖放交给合成器，否则窗口外松手会挂起。
                    return self.start_native_file_drag_for_cursor_left();
                }
                Task::none()
            }
            Message::ColumnBrowserCursorEntered(pane_id) => {
                // 悬停不聚焦:窗格激活只由点击/键盘驱动,悬停仅记录
                // 光标所在的浏览内容区,供 Ctrl+Shift 分栏拖拽判定起点。
                self.cursor_over_browser_content_pane = Some(pane_id);
                Task::none()
            }
            Message::ColumnBrowserCursorExited(pane_id) => {
                if self.cursor_over_browser_content_pane == Some(pane_id) {
                    self.cursor_over_browser_content_pane = None;
                    self.clear_cursor_paste_target()
                } else {
                    Task::none()
                }
            }
            Message::ColumnEntryBoundsMeasured(bounds, viewports) => {
                let drag_icon_preload = self.refresh_file_drag_preview_layout(&bounds);
                self.note_file_drag_viewport(&bounds, &viewports);
                Task::batch([
                    drag_icon_preload,
                    self.update_selection_from_column_entry_bounds(bounds),
                ])
            }
            Message::BreadcrumbDropTargetBoundsMeasured(generation, bounds) => {
                self.accept_breadcrumb_drop_target_bounds(generation, bounds)
            }
            Message::FileDropLayoutMeasured(request, bounds) => {
                self.accept_drop_layout(request, bounds)
            }
            Message::PaneCursorEntered(pane_id) => {
                self.hovered_pane_id = Some(pane_id);
                Task::none()
            }
            Message::PaneCursorExited(pane_id) => {
                self.clear_list_header_hover_in_pane(pane_id);
                if self.hovered_pane_id == Some(pane_id) {
                    self.hovered_pane_id = None;
                }
                Task::none()
            }
            Message::KeyboardModifiersChanged(modifiers) => {
                self.keyboard_modifiers = modifiers;
                if self.promote_ctrl_shift_pane_drag_from_active_pointer_drag() {
                    self.update_pane_drag(self.cursor_or_origin());
                }
                Task::none()
            }
            Message::KeyboardKeyPressed {
                key,
                modifiers,
                status,
            } => {
                let traverses_focus = matches!(
                    key.as_ref(),
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab)
                ) && !modifiers.alt()
                    && !modifiers.control()
                    && !modifiers.command();
                let keyboard_command = self.handle_keyboard_key_pressed(key, modifiers, status);
                if traverses_focus {
                    Task::batch([
                        keyboard_command,
                        self.request_search_input_focus_check(
                            crate::model::SearchInputFocusCheckOrigin::KeyboardTraversal,
                        ),
                    ])
                } else {
                    keyboard_command
                }
            }
            Message::FileContentShortcutRouted(action) => self.invoke_shortcut(action),
            Message::ShortcutCaptureStarted(binding_id) => self.start_shortcut_capture(binding_id),
            Message::ShortcutCaptureCanceled => self.cancel_shortcut_capture(),
            Message::ShortcutBindingReset(binding_id) => self.reset_shortcut_binding(binding_id),
            Message::WindowPointerReleased { window, status } => match status {
                iced::event::Status::Captured => self.finish_tab_drag_from_captured_release(window),
                iced::event::Status::Ignored => self.finish_pointer_drag_interactions(window),
            },
            Message::DragSelectionFinished => {
                self.finish_pointer_drag_interactions(self.main_window)
            }
            Message::GlobalErrorNotificationElapsed(generation) => {
                self.expire_global_error_notification(generation);
                Task::none()
            }
            Message::GlobalErrorNotificationPointerEntered(generation) => {
                self.pause_global_error_notification(generation);
                Task::none()
            }
            Message::GlobalErrorNotificationPointerExited(generation) => {
                self.resume_global_error_notification(generation);
                Task::none()
            }
            Message::GlobalErrorNotificationDismissed(generation) => {
                self.dismiss_global_error_notification(generation);
                Task::none()
            }
            Message::DismissFloating => self.dismiss_floating(),
            Message::ArchiveCreation(message) => self.handle_archive_creation_message(message),
            Message::Convert(message) => self.handle_convert_message(message),
            Message::Checksum(message) => self.handle_checksum_message(message),
            Message::ArchiveExtraction(message) => self.handle_archive_extraction_message(message),
            Message::ArchiveMemberPassword(message) => {
                self.handle_archive_member_password_message(message)
            }
            Message::ArchiveMemberPasswordRequested {
                action,
                invalid_retry,
            } => self.request_archive_member_password(action, invalid_retry),
            Message::BatchRename(message) => self.handle_batch_rename_message(message),
            Message::Transfer(message) => self.handle_transfer_message(message),
            Message::FileContextMenuExpansionChanged(expansion) => {
                self.update_file_context_menu_expansion(expansion)
            }
            Message::DeleteSelectedPermanently => self.delete_selected_permanently(),
            Message::ContextMenuPreviewExpansionChanged(expansion) => {
                self.update_context_menu_preview_expansion(expansion)
            }
            Message::DestructiveActionConfirmed => self.confirm_destructive_action(),
            Message::DestructiveActionCanceled => self.cancel_destructive_action(),
            Message::AuxiliaryWindowCloseRequested(window) => self.close_auxiliary_window(window),
            Message::AuxiliaryWindowResized(window, width, height) => {
                let resize_command = self.handle_auxiliary_window_resized(window, width, height);
                Task::batch([resize_command, self.observe_window_maximized(window)])
            }
            Message::WindowMinimizeRequested(window) => self.minimize_window(window),
            Message::WindowMaximizeToggled(window) => self.toggle_window_maximized(window),
            Message::WindowMaximizedObserved(window, frame_state) => {
                self.accept_window_maximized_observation(window, frame_state)
            }
            Message::WindowDragRequested(window) => self.start_window_drag(window),
            Message::WindowResizeRequested(window, direction) => {
                self.start_window_resize(window, direction)
            }
            Message::WindowFocused(window) => self.handle_window_focused(window),
            Message::WindowUnfocused(window) => self.handle_window_unfocused(window),
            Message::WindowPointerPressed {
                window,
                button,
                status,
            } => self.handle_window_pointer_pressed(window, button, status),
            Message::AddressEditingRequested(pane_id) => self.begin_address_editing(pane_id),
            Message::BreadcrumbSegmentPressed(pane_id, target) => {
                self.activate_breadcrumb_target(pane_id, target)
            }
            Message::AddressDraftChanged(pane_id, value) => {
                self.update_address_draft(pane_id, value)
            }
            Message::AddressEditingSubmitted(pane_id) => self.submit_address_editing(pane_id),
            Message::AddressSuggestionSelected(pane_id, target) => {
                self.submit_address_suggestion(pane_id, target)
            }
            Message::AddressSuggestionInputStabilized(request) => {
                self.load_stable_address_suggestions(request)
            }
            Message::AddressSuggestionsLoaded(request, suggestions) => {
                self.accept_address_suggestions(request, suggestions)
            }
            Message::AddressBarScrolled(pane_id) => Task::batch([
                self.show_scrollbars_temporarily(Region::AddressBar(pane_id)),
                self.request_breadcrumb_drop_target_bounds_measurement(),
            ]),
            Message::SearchInputChanged(value) => self.update_search_input(value),
            Message::SearchInputStabilized(request) => {
                self.accept_search_input_stabilization(request)
            }
            Message::SearchSubmitted => self.submit_search_input(),
            Message::SearchHistoryKeywordSelected(keyword) => {
                self.select_search_history_keyword(keyword)
            }
            Message::SearchHistoryKeywordRemoved(keyword) => {
                self.remove_search_history_keyword(&keyword)
            }
            Message::SearchHistoryCleared => self.clear_search_history(),
            Message::SearchInputFocusChecked(request, focus) => {
                self.search_history_interaction
                    .accept_input_focus_check(request, focus);
                Task::none()
            }
            Message::SearchHistoryPopupPointerEntered => {
                self.search_history_interaction.enter_popup();
                Task::none()
            }
            Message::SearchHistoryPopupPointerExited => {
                self.search_history_interaction.exit_popup();
                Task::none()
            }
            Message::SearchEntryTypesMenuOpened => self.open_search_entry_types_menu(),
            Message::SearchEntryTypeToggled(entry_type) => {
                self.toggle_search_entry_type(entry_type)
            }
            Message::SearchDirectoryScopeSelected(scope) => {
                self.select_search_directory_scope(scope)
            }
            Message::SearchTextScopeSelected(text_scope) => {
                self.select_search_text_scope(text_scope)
            }
            Message::SearchRegexToggled => self.toggle_search_regex(),
            Message::SearchDateFieldSelected(date_field) => {
                self.select_search_date_field(date_field)
            }
            Message::SearchDatePresetSelected(date_preset) => {
                self.select_search_date_preset(date_preset)
            }
            Message::SearchCustomExtensionsToggled => self.toggle_search_custom_extensions(),
            Message::SearchCustomExtensionsChanged(value) => {
                self.update_search_custom_extensions(value)
            }
            Message::SearchSizePresetSelected(size_preset) => {
                self.select_search_size_preset(size_preset)
            }
            Message::SearchCustomSizeMinChanged(value) => self.update_search_custom_size_min(value),
            Message::SearchCustomSizeMaxChanged(value) => self.update_search_custom_size_max(value),
            Message::SearchFiltersReset => self.reset_search_filters(),
            Message::SearchKeywordCleared => self.clear_search_keyword(),
            Message::SearchWorkspaceClosed => self.close_search_workspace(),
            Message::SearchResultsLoaded(request, outcome) => {
                self.accept_search_results(request, outcome)
            }
            Message::SearchScopeRootValidated(request, query, cancellation, root_is_available) => {
                self.accept_search_scope_root_validation(
                    request,
                    query,
                    cancellation,
                    root_is_available,
                )
            }
            Message::SearchDirectoryBatchLoaded(generation, hits) => {
                self.accept_directory_search_batch(generation, hits)
            }
            Message::SearchDirectoryFinished(generation, completion) => {
                self.accept_directory_search_finished(generation, completion)
            }
            Message::SearchResultPressed(path) => self.press_search_result(path),
            Message::SearchResultRightClicked(path) => self.right_click_search_result(path),
            Message::SearchOpenContainingDirectory(path) => {
                self.open_search_containing_directory(path)
            }
            Message::SearchDeletePermanentlySelected => self.delete_search_selection_permanently(),
            Message::SearchResultsScrolled {
                offset_y,
                viewport_height,
            } => Task::batch([
                self.show_scrollbars_temporarily(Region::SearchResults),
                self.update_search_results_viewport(offset_y, viewport_height),
            ]),
            Message::SearchServiceEnsured(request, outcome) => {
                self.accept_search_service_status(request, outcome)
            }
            Message::SearchServiceStatusRefreshRequested => self.refresh_search_service_status(),
            Message::SearchServiceStatusLoaded(request, outcome) => {
                self.accept_search_service_status(request, outcome)
            }
            Message::SearchPathConfigurationLoaded(outcome) => {
                self.accept_search_path_configuration(outcome)
            }
            Message::SearchPathConfigurationApplied(outcome) => {
                self.accept_search_path_configuration_applied(outcome)
            }
            Message::SearchDirectoryFallbackConfigurationLoaded(
                generation,
                unavailable_message,
                outcome,
            ) => self.start_verified_directory_fallback(generation, unavailable_message, outcome),
            Message::SearchPathInputChanged(kind, value) => {
                self.update_search_path_input(kind, value)
            }
            Message::SearchPathInputCommitted(kind) => self.commit_search_path_input(kind),
            Message::SearchPathDirectoryChooserPressed(kind) => {
                self.open_search_path_directory_chooser(kind)
            }
            Message::SearchPathDirectoryChosen(kind, outcome) => {
                self.accept_search_path_directory(kind, outcome)
            }
            Message::SearchPathEntryRemoved(kind, path) => self.remove_search_path(kind, path),
            Message::SearchPathConfigurationRetryPressed => self.retry_search_path_configuration(),
            Message::SearchServiceRestartRequested => self.restart_search_service(),
            Message::SearchServiceForceRestartPressed => self.press_force_restart_search_service(),
            Message::SearchServiceRecoveryFinished(action, outcome) => {
                self.accept_search_service_recovery(action, outcome)
            }
            Message::SearchServiceIncidentDetailsToggled(kind) => {
                self.toggle_search_service_incident_details(kind)
            }
            Message::SearchServiceIncidentDetailsCopyRequested(kind) => {
                self.copy_search_service_incident_details(kind)
            }
            Message::SystemThemeDetected(theme) => {
                self.application_theme.replace_system_fallback(theme);
                Task::none()
            }
            Message::MatugenThemeUpdated(Ok(theme)) => {
                self.application_theme.replace_matugen_override(theme);
                Task::none()
            }
            Message::MatugenThemeUpdated(Err(error)) => {
                tracing::warn!(
                    target: "app_ui::matugen_theme",
                    error = %error,
                    "matugen theme update was ignored"
                );
                Task::none()
            }
            Message::ThemeModeSelected(theme_mode) => self.select_theme_mode(theme_mode),
            Message::ColorSchemeFamilySelected(family) => self.select_color_scheme_family(family),
            Message::ColorSchemePresetSelected(color_scheme) => {
                self.select_color_scheme_preset(color_scheme)
            }
            Message::CustomColorSchemeImportPressed => self.import_custom_color_scheme(),
            Message::CustomColorSchemeImportCompleted(outcome) => {
                self.accept_custom_color_scheme_import(outcome)
            }
            Message::UserPreferencesSaved(result) => self.accept_user_preferences_saved(result),
            Message::AppConfigSaved(result) => self.accept_app_config_saved(result),
            Message::ColumnWidthOverrideSaved(result) => self.accept_column_width_saved(result),
            Message::BrowserSessionSaved(result) => self.accept_browser_session_saved(result),
            Message::BrowserSessionSaveDelayElapsed => {
                self.maybe_flush_pending_browser_session_save()
            }
            Message::ApplicationWindowClosed(window) => {
                self.accept_application_window_closed(window)
            }
            Message::ApplicationWindowCloseCommandsFinished
            | Message::ApplicationShutdownPersisted(_) => Task::none(),
            Message::ExpandedDirectoryDiscoveryBatch(request, batch) => {
                self.accept_expanded_directory_discovery_batch(request, batch)
            }
            Message::ExpandedDirectoryEntriesReady(request, discovery) => {
                self.accept_expanded_directory_discovery(request, discovery)
            }
            Message::ObservedDirectoryChanges(change) => self.reload_observed_directory(change),
            Message::SettingsOpened => self.open_settings(),
            Message::SettingsCategorySelected(category) => self.select_settings_category(category),
            Message::SettingsSubpageOpened(subpage) => self.select_settings_subpage(subpage),
            Message::SettingsSubpageClosed => self.close_settings_subpage(),
            Message::AboutRepositoryLinkPressed => {
                crate::commands::open_about_repository_link_command()
            }
            Message::AboutRepositoryLinkOpened(Ok(())) => {
                self.retract_global_error(GlobalErrorSource::Launch);
                Task::none()
            }
            Message::AboutRepositoryLinkOpened(Err(error)) => {
                self.show_global_error_from(GlobalErrorSource::Launch, error);
                Task::none()
            }
            Message::ContextMenuSettingsPageShifted(step) => {
                self.shift_context_menu_settings_page(step)
            }
            Message::ContextMenuSettingsItemToggled { page, index } => {
                self.toggle_context_menu_settings_item(page, index)
            }
            Message::ContextMenuSettingsDragStarted { page, index } => {
                self.start_context_menu_settings_drag(page, index)
            }
            Message::ContextMenuSettingsResetRequested(page) => {
                self.request_context_menu_settings_reset(page)
            }
            Message::ContextMenuSettingsResetConfirmed(page) => {
                self.confirm_context_menu_settings_reset(page)
            }
            Message::WindowChromeLayoutSelected(layout) => self.select_window_chrome_layout(layout),
            Message::WindowControlVisibilityToggled(kind) => {
                self.toggle_window_control_visibility(kind)
            }
            Message::WindowControlSideSelected(kind, side) => {
                self.select_window_control_side(kind, side)
            }
            Message::WindowControlMoveRequested(kind, direction) => {
                self.move_window_control_within_side(kind, direction)
            }
            Message::WindowControlsReset => self.reset_window_controls(),
            Message::ApplicationLogsRefreshRequested => self.refresh_application_logs(),
            Message::ApplicationLogsLoaded(request, outcome) => {
                self.accept_application_logs(request, outcome)
            }
            Message::ApplicationLogThresholdSelected(threshold) => {
                self.select_application_log_threshold(threshold)
            }
            Message::ShowHiddenFilesToggled => self.toggle_show_hidden_files(),
            Message::ListDirectorySizeDisplayModeToggled => {
                self.toggle_list_directory_size_display_mode()
            }
            Message::NetworkListThumbnailDownloadsToggled => {
                self.toggle_network_list_thumbnail_downloads()
            }
            Message::SearchContentIndexingToggled => self.toggle_search_content_indexing(),
            Message::PreviewSizeLimitInputChanged(kind_index, value) => {
                self.update_preview_size_limit_mib_input(kind_index, value)
            }
            Message::PreviewSizeLimitInputCommitted(kind_index) => {
                self.commit_preview_size_limit_mib_input(kind_index)
            }
            Message::PreviewDirectoryExpandLevelsInputChanged(value) => {
                self.update_preview_directory_expand_levels_input(value)
            }
            Message::PreviewDirectoryExpandLevelsInputCommitted => {
                self.commit_preview_directory_expand_levels_input()
            }
            Message::PreviewExtensionInputChanged(kind_index, value) => {
                self.update_preview_extension_input(kind_index, value)
            }
            Message::PreviewExtensionInputCommitted(kind_index) => {
                self.add_preview_extension(kind_index)
            }
            Message::PreviewExtensionExpandToggled(kind_index) => {
                self.preview_extension_expanded[kind_index] =
                    !self.preview_extension_expanded[kind_index];
                // 确认区只存在于展开态，切换折叠状态时一并撤销。
                if self.preview_extension_reset_confirmation == Some(kind_index) {
                    self.preview_extension_reset_confirmation = None;
                }
                Task::none()
            }
            Message::PreviewExtensionRemoved(kind_index, extension) => {
                self.remove_preview_extension(kind_index, &extension)
            }
            Message::PreviewExtensionResetRequested(kind_index) => {
                self.request_preview_extension_reset(kind_index)
            }
            Message::PreviewExtensionResetConfirmed(kind_index) => {
                self.confirm_preview_extension_reset(kind_index)
            }
            Message::LanguageSettingSelected(language_setting) => {
                self.select_language_setting(language_setting)
            }
            Message::VisibleColumnCountSelected(count) => self.select_visible_column_count(count),
            Message::ColumnWidthAdjustModeSelected(mode) => {
                self.select_column_width_adjust_mode(mode)
            }
            Message::StartupLocationPolicySelected(policy) => {
                self.select_startup_location_policy(policy)
            }
            Message::LaunchWindowPolicySelected(policy) => self.select_launch_window_policy(policy),
            Message::StartupSessionClassified(classified) => self.accept_startup_plan(classified),
            Message::StartupCustomDirectoryInputChanged(value) => {
                self.update_startup_custom_directory_input(value)
            }
            Message::StartupCustomDirectoryCommitted => {
                self.commit_startup_custom_directory_input()
            }
            Message::StartupCustomDirectoryValidated(request, availability) => {
                self.accept_startup_directory_validation(request, availability)
            }
            Message::FileOperationVerificationSelected(verification) => {
                self.user_config.file_operation_verification = verification;
                self.persist_user_preferences_command()
            }
            Message::TerminalEmulatorSelected(terminal_emulator) => {
                self.terminal_emulator = terminal_emulator;
                self.user_config.terminal_emulator = terminal_emulator;
                self.persist_user_preferences_command()
            }
            Message::TerminalShellSelected(shell) => {
                self.user_config.terminal_shell = shell;
                self.persist_user_preferences_command()
            }
            Message::RenderingGpuPreferenceSelected(preference) => {
                self.select_rendering_gpu_preference(preference)
            }
            Message::RendererRestartRequested => self.restart_with_selected_renderer(),
            Message::RendererRestartNoticeDismissed => self.dismiss_renderer_restart_notice(),
            Message::SmoothScrollWheel(region, delta) => {
                self.handle_smooth_scroll_wheel(region, delta)
            }
            Message::ScrollbarViewportChanged {
                region,
                viewport,
                event,
            } => {
                self.remember_scrollbar_viewport(region, viewport);
                self.update(*event)
            }
            Message::ScrollbarLayoutVerified { region, viewport } => {
                self.handle_scrollbar_layout_verified(region, viewport)
            }
            Message::ScrollbarAutoHideElapsed(generation) => {
                self.start_global_scrollbar_hide(generation);
                Task::none()
            }
            Message::WindowChromeAnimationTick => self.advance_window_animation_frame(),
            Message::SidebarScrolled => self.show_scrollbars_temporarily(Region::Sidebar),
            Message::SettingsScrolled => self.show_scrollbars_temporarily(Region::Settings),
            Message::PropertiesScrolled => self.show_scrollbars_temporarily(Region::Properties),
            Message::OpenWithApplicationsScrolled => {
                self.show_scrollbars_temporarily(Region::OpenWithApplications)
            }
            Message::OperationQueueScrolled => {
                self.show_scrollbars_temporarily(Region::OperationQueue)
            }
            Message::BatchRenamePreviewScrolled => {
                self.show_scrollbars_temporarily(Region::BatchRenamePreview)
            }
            Message::SearchHistoryScrolled => {
                self.show_scrollbars_temporarily(Region::SearchHistory)
            }
            Message::ColumnBrowserScrolled(pane_id, offset_x, width) => Task::batch([
                self.show_scrollbars_temporarily(Region::ColumnBrowser(pane_id)),
                self.handle_column_browser_scrolled(pane_id, offset_x, width),
            ]),
            Message::ColumnScrolled(pane_id, directory, offset_y, height) => Task::batch([
                self.show_scrollbars_temporarily(Region::Column {
                    pane_id,
                    directory: directory.clone(),
                }),
                self.handle_column_scrolled(pane_id, directory, offset_y, height),
                self.request_browser_session_save(),
            ]),
            Message::ListScrolled(pane_id, offset_y, viewport) => {
                let scrolled = self.handle_list_scrolled(pane_id, offset_y, viewport.height);
                self.recalculate_hovered_entry_after_scroll(
                    pane_id,
                    ScrolledView::List,
                    offset_y,
                    viewport,
                );
                Task::batch([
                    scrolled,
                    self.show_scrollbars_temporarily(Region::PaneList(pane_id)),
                    self.schedule_visible_directory_metadata(
                        pane_id,
                        Some(crate::thumbnail_cache::ColumnViewport {
                            offset_y,
                            height: viewport.height,
                        }),
                    ),
                    self.request_browser_session_save(),
                ])
            }
            Message::IconGridScrolled(pane_id, offset_y, viewport) => {
                let scrolled = self.handle_icon_grid_scrolled(
                    pane_id,
                    offset_y,
                    viewport.width,
                    viewport.height,
                );
                self.recalculate_hovered_entry_after_scroll(
                    pane_id,
                    ScrolledView::IconGrid,
                    offset_y,
                    viewport,
                );
                Task::batch([
                    scrolled,
                    self.show_scrollbars_temporarily(Region::PaneIcons(pane_id)),
                ])
            }
            Message::ColumnResizeStarted(pane_id, column_index) => {
                self.activate_pane(pane_id);
                self.start_column_resize_drag(column_index)
            }
            Message::OpenDirectoryFromMiddleClick(pane_id, path) => {
                self.activate_pane(pane_id);
                Task::batch([
                    self.commit_rename_if_active(),
                    self.open_directory_from_middle_click(path),
                ])
            }
            Message::ArchiveMiddlePressed(pane_id, path) => {
                // 智能解压守卫读 FileBrowser 级状态（回收站/包内），该状态
                // 跟随激活 pane；先激活被点击 pane，守卫才按真实点击上下文
                // 判定，与右键 EntryRightClicked 的入口契约一致。
                self.activate_pane(pane_id);
                self.extract_archive_from_middle_click(path)
            }
            Message::OpenTrashInNewTab(pane_id) => {
                self.activate_pane(pane_id);
                Task::batch([self.commit_rename_if_active(), self.open_trash_in_new_tab()])
            }
            Message::TabPressed(pane_id, tab_id) => {
                self.activate_pane(pane_id);
                let rename_command = self.commit_rename_if_active();
                self.start_tab_drag(pane_id, tab_id);
                Task::batch([rename_command, self.select_tab(tab_id)])
            }
            Message::TabCloseRequested(pane_id, tab_id) => {
                self.activate_pane(pane_id);
                self.close_tab(tab_id)
            }
            Message::TabDragEntered(pane_id, tab_id) => self.reorder_dragged_tab(pane_id, tab_id),
            Message::TabDragFinished => {
                self.finish_tab_drag_from_captured_release(self.main_window)
            }
            Message::TabFileDropEntered(id, tab) => self.accept_tab_file_drop_entered(id, tab),
            Message::TabFileDropExited(id, tab) => self.accept_tab_file_drop_exited(id, tab),
            Message::TabFileDropReleased(id, tab) => self.accept_tab_file_drop_released(id, tab),
            Message::TabFileDropHoverElapsed(hover) => self.accept_tab_hover_elapsed(hover),
            Message::NavigateTo(path) => Task::batch([
                self.commit_rename_if_active(),
                self.navigate_to(path, NavigationMode::RecordHistory),
            ]),
            Message::OpenPath(path) => {
                Task::batch([self.commit_rename_if_active(), self.open_path(path)])
            }
            Message::SmartExtractSelected => self.smart_extract_selected(),
            Message::SmartExtractDestinationResolved {
                archive,
                single_root,
            } => self.accept_smart_extract_destination(archive, single_root),
            Message::ExtractSelectedToArchiveFolder => self.extract_selected_to_archive_folder(),
            Message::TrashOpened => Task::batch([
                self.commit_rename_if_active(),
                self.open_trash_view(NavigationMode::RecordHistory),
            ]),
            Message::Back => Task::batch([self.commit_rename_if_active(), self.navigate_back()]),
            Message::Forward => {
                Task::batch([self.commit_rename_if_active(), self.navigate_forward()])
            }
            Message::PaneUp(pane_id) => {
                self.activate_pane(pane_id);
                Task::batch([self.commit_rename_if_active(), self.navigate_up()])
            }
            Message::PaneBack(pane_id) => {
                self.activate_pane(pane_id);
                Task::batch([self.commit_rename_if_active(), self.navigate_back()])
            }
            Message::PaneForward(pane_id) => {
                self.activate_pane(pane_id);
                Task::batch([self.commit_rename_if_active(), self.navigate_forward()])
            }
            Message::AddressInputFocusChecked(pane_id, is_focused) => {
                let checked_session_is_current = self
                    .address_editing
                    .as_ref()
                    .is_some_and(|editing| editing.pane_id == pane_id);
                if is_focused || !checked_session_is_current {
                    Task::none()
                } else {
                    self.cancel_address_editing()
                }
            }
            Message::RenameInputFocusChecked(is_focused) => {
                if is_focused {
                    Task::none()
                } else {
                    self.commit_rename_if_active()
                }
            }
            Message::RenameInputChanged(value) => self.apply_rename_input_change(value),
            Message::RenameInputUndoRequested => self.undo_rename_input_change(),
            Message::RenameInputRedoRequested => self.redo_rename_input_change(),
            Message::BeginRename(path) => self.begin_rename(path),
            Message::OpenTerminalHere(directory) => self.open_terminal_here(directory),
            Message::RenameSelected => self.commit_rename(),
            Message::CreateDirectory(directory) => {
                if self.keyboard_modifiers.shift() {
                    self.open_advanced_new_folder(directory)
                } else {
                    self.create_directory_in(directory)
                }
            }
            Message::AdvancedNewFolder(message) => self.handle_advanced_new_folder_message(message),
            Message::CreateEmptyFile(directory) => self.create_empty_file_in(directory),
            Message::TrashSelected => self.trash_selected(),
            Message::RestoreSelected => self.restore_selected(),
            Message::EmptyTrashRequested => self.empty_trash_requested(),
            Message::CopySelected => self.copy_selected(),
            Message::DuplicateSelected => self.duplicate_selected(),
            Message::NewFolderFromSelection => self.new_folder_from_selection(),
            Message::CopyPathSelected => self.copy_selected_path_text(),
            Message::CreateSymlinkSelected => self.create_symlinks_for_selection(),
            Message::PathTextCopied(result) => {
                match result {
                    Ok(()) => self.retract_global_error(GlobalErrorSource::ClipboardWrite),
                    Err(error) => {
                        self.show_global_error_from(GlobalErrorSource::ClipboardWrite, error)
                    }
                };
                Task::none()
            }
            Message::MoveSelected => self.move_selected(),
            Message::PastePending => self.paste_pending(),
            Message::FileClipboardWriteFinished(result) => self.accept_file_clipboard_write(result),
            Message::DesktopClipboardReadFinished {
                paste_directory,
                fallback_operation,
                content,
            } => self.accept_desktop_clipboard_paste(paste_directory, fallback_operation, content),
            Message::ClipboardFileCreated(result) => self.accept_clipboard_file_created(result),
            Message::DesktopActivationReceived(event) => self.accept_desktop_activation(event),
            Message::DesktopActivationRuntimeFailed(error) => {
                self.accept_desktop_activation_runtime_failure(error)
            }
            Message::WaylandDndWindowHandleLoaded(handle) => self.accept_wayland_dnd_handle(handle),
            Message::WaylandFilesDropped(drop) => self.accept_wayland_file_drop(drop),
            Message::WaylandFileDropFailed(target_session_id, details) => {
                self.accept_wayland_drop_failure(target_session_id, details)
            }
            Message::WaylandFileDragSourceEvent(event) => self.accept_wayland_source_event(event),
            Message::WaylandFileDropTargetEvent(event) => self.accept_wayland_target_event(event),
            Message::WaylandDragIconReady { gesture_id, icon } => {
                self.accept_wayland_drag_icon_ready(gesture_id, icon)
            }
            Message::WaylandDndRuntimeFailed(error) => {
                self.accept_wayland_dnd_runtime_failure(error)
            }
            Message::X11Dnd(message) => self.accept_x11_dnd_message(message),
            Message::FileDropOperationSelected(operation) => {
                self.apply_file_drop_operation(operation)
            }
            Message::FileDropCancelled => self.cancel_file_drop(),
            Message::TransferConflictsChecked {
                mode,
                transfers,
                conflicts,
            } => self.accept_transfer_conflict_review(
                mode,
                conflicts.map(|conflicts| (transfers, conflicts)),
            ),
            Message::TransferConflictMergesExpanded { mode, expansion } => {
                self.accept_transfer_conflict_review(mode, expansion)
            }
            Message::TransferConflictChoiceSelected(_)
            | Message::TransferConflictApplyToAllToggled
            | Message::TransferConflictCancelRequested => {
                self.apply_transfer_conflict_message(message)
            }
        };
        command
    }
}

#[cfg(test)]
mod theme_selection_tests {
    use super::*;
    use crate::config;
    use bennu_theme::{ColorSchemePreset, ThemeMode};

    #[test]
    fn built_in_theme_selection_updates_the_active_theme() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.update(Message::ThemeModeSelected(ThemeMode::Dark)));
        drop(browser.update(Message::ColorSchemePresetSelected(ColorSchemePreset::Nord)));

        assert_eq!(browser.user_config.theme_mode, ThemeMode::Dark);
        assert_eq!(browser.user_config.color_scheme, ColorSchemePreset::Nord);
        assert!(
            browser
                .application_theme
                .active(
                    browser.user_config.theme_mode,
                    browser.user_config.color_scheme
                )
                .extended_palette()
                .is_dark
        );
    }

    #[test]
    fn matugen_selection_preserves_the_previous_mode() {
        let mut user_config = config::default_user_config();
        user_config.theme_mode = ThemeMode::Light;
        user_config.color_scheme = ColorSchemePreset::Matugen;
        let (mut browser, _) = FileBrowser::new(user_config);

        drop(browser.update(Message::ThemeModeSelected(ThemeMode::Dark)));

        assert_eq!(browser.user_config.theme_mode, ThemeMode::Light);
    }
}
