use std::path::{Path, PathBuf};

use iced::Task;

use super::super::paths::{self, PasteTargetMode};
use super::super::wayland_dnd::WaylandFileDragRequest;
use super::super::{FileBrowser, POINTER_DRAG_ACTIVATION_DISTANCE};
use crate::model::{
    entry_exists, unique_duplicated_directory_name, unique_duplicated_file_name,
    BrowserPaneId, FileDragDropIntent, FileDragGestureId, FileDragNativeDndState, FileDragPhase,
    FileDragState, FileDragStationaryAction, FileDropTarget, Message, SelectionMarqueeSource,
    TabDropDestination, TransferConflictMode,
};
use crate::operation_queue::{QueuedFileOperation, QueuedTransfer};

impl FileBrowser {
    pub(crate) fn update_file_drag(&mut self, position: iced::Point) -> Task<Message> {
        let mut activated = false;
        let mut dragging = false;
        if let Some(file_drag) = &mut self.file_drag {
            match file_drag.phase {
                FileDragPhase::WaitingForMovement { origin } => {
                    let delta_x = position.x - origin.x;
                    let delta_y = position.y - origin.y;
                    if delta_x * delta_x + delta_y * delta_y
                        >= POINTER_DRAG_ACTIVATION_DISTANCE * POINTER_DRAG_ACTIVATION_DISTANCE
                    {
                        file_drag.phase = FileDragPhase::Dragging;
                        activated = true;
                    }
                }
                FileDragPhase::Dragging => dragging = true,
            }
        } else {
            return Task::none();
        }

        if activated {
            // 快速甩动可能一步越出窗口:应用内设施照常启动,同时立即交接。
            return if self.cursor_strictly_outside_main_window(position) {
                Task::batch([
                    self.begin_iced_file_drag_after_activation(),
                    self.start_native_file_drag_for_cursor_left(),
                ])
            } else {
                self.begin_iced_file_drag_after_activation()
            };
        }
        if dragging && self.cursor_in_window_handoff_band(position) {
            return self.start_native_file_drag_for_cursor_left();
        }
        Task::none()
    }

    /// 拖动中(左键隐式 grab)Wayland/X11 不发 CursorLeft——指针事件
    /// 持续发给本窗口,光标越界也一样——所以交接时机只能按坐标判定。
    /// 光标贴近边缘(缓冲带)就交接:越过边缘后合成器会结束隐式
    /// grab,按压记录随之失效,越界后才请求的拖放注定被拒。
    fn cursor_in_window_handoff_band(&self, position: iced::Point) -> bool {
        const EDGE_HANDOFF_BAND: f32 = 16.0;
        self.cursor_strictly_outside_main_window(position)
            || position.x < EDGE_HANDOFF_BAND
            || position.y < EDGE_HANDOFF_BAND
            || position.x > self.main_window_width - EDGE_HANDOFF_BAND
            || position.y > self.main_window_height - EDGE_HANDOFF_BAND
    }

    fn cursor_strictly_outside_main_window(&self, position: iced::Point) -> bool {
        position.x < 0.0
            || position.y < 0.0
            || position.x > self.main_window_width
            || position.y > self.main_window_height
    }

    /// 光标离开主窗口才把拖放交给 Wayland 原生会话(跨窗口拖放):窗口内
    /// 保持应用内拖拽,滚轮/shift 滚轮/边缘自动滚全程可用。请求失败时保持
    /// 应用内拖拽——回到窗口即恢复移动事件,重进后松手仍正常收尾,只有
    /// 窗口外落放会丢失;此处不能像激活失败那样整段取消,用户可能只是
    /// 晃过窗口边缘。
    pub(crate) fn start_native_file_drag_for_cursor_left(&mut self) -> Task<Message> {
        // 光标不在窗口内就没有后续 motion 来重算边缘滚计划,先无条件
        // 清掉,防止帧循环带着残留计划在窗口外继续滚动。
        self.stop_file_drag_edge_scroll();
        let can_start_native_drag = self.wayland_dnd.is_some()
            && self
                .file_drag
                .as_ref()
                .is_some_and(FileDragState::can_start_native_dnd);
        if !can_start_native_drag {
            return Task::none();
        }

        let drag_sources = self
            .file_drag
            .as_ref()
            .expect("native drag handoff requires an active file drag")
            .sources
            .clone();
        match self.request_wayland_file_drag(drag_sources) {
            WaylandFileDragRequest::Unavailable => {
                if let Some(file_drag) = &mut self.file_drag {
                    file_drag.native_dnd = FileDragNativeDndState::NotRequested;
                }
                Task::none()
            }
            WaylandFileDragRequest::Rejected(error) => {
                if let Some(file_drag) = &mut self.file_drag {
                    file_drag.native_dnd = FileDragNativeDndState::NotRequested;
                }
                // 请求已发出但失败:跨窗口拖放不可用需要让用户知道,
                // 但应用内拖拽仍然有效,不能整段取消。
                self.show_global_error(error);
                Task::none()
            }
            WaylandFileDragRequest::Requested(session_id) => {
                if let Some(file_drag) = &mut self.file_drag {
                    file_drag.native_dnd = FileDragNativeDndState::Requested(session_id);
                }
                // 交接后落点归合成器管辖,冻结的应用内落点高亮必须清掉。
                self.clear_file_drag_target();
                Task::none()
            }
        }
    }

    fn begin_iced_file_drag_after_activation(&mut self) -> Task<Message> {
        Task::batch([
            crate::column_entry_bounds::column_entry_bounds_command(),
            self.begin_iced_file_drop_session(),
        ])
    }

    pub(crate) fn finish_drag_selection(
        &mut self,
        release_directory: Option<PathBuf>,
    ) -> Task<Message> {
        self.stop_file_drag_edge_scroll();
        let native_dnd = self
            .file_drag
            .as_ref()
            .map(|file_drag| file_drag.native_dnd);
        if native_dnd.is_some_and(|state| state.session_id().is_some()) {
            return Task::none();
        }

        let column_blank_click = self.selection_marquee.as_ref().and_then(|marquee| {
            if marquee.is_selecting() {
                return None;
            }
            match &marquee.source {
                SelectionMarqueeSource::PaneBlank
                | SelectionMarqueeSource::IconGridPanel { .. } => None,
                SelectionMarqueeSource::ColumnBlank { directory } => Some(directory.clone()),
            }
        });
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.sidebar_bookmark_drop_slot = None;
        if let Some(directory) = column_blank_click {
            return self.handle_column_blank_clicked(directory);
        }
        let Some(file_drag) = self.file_drag.take() else {
            return Task::none();
        };

        if !file_drag.is_dragging() {
            self.file_drop_session = None;
            return self.finish_stationary_file_drag(file_drag);
        }

        self.finish_iced_file_drop(file_drag, release_directory)
    }

    pub(crate) fn start_file_drag(
        &mut self,
        pressed_path: PathBuf,
        stationary_action: FileDragStationaryAction,
        column_directories_snapshot: Vec<PathBuf>,
    ) {
        self.sidebar_bookmark_drop_slot = None;
        self.file_drop_session = None;
        if self.is_trash_view {
            self.file_drag = None;
            return;
        }

        let source_pane_id = self.active_pane_id();
        let source_tab_id = self.active_tab_id;
        let sources = self.selected_paths_for_operation();
        let bookmark_source = (sources.len() == 1
            && self.entry_kind(&sources[0]) == Some(file_core::FileKind::Directory))
        .then(|| sources[0].clone());
        self.next_file_drag_gesture_id = self.next_file_drag_gesture_id.wrapping_add(1);
        self.file_drag = (!sources.is_empty()).then_some(FileDragState {
            gesture_id: FileDragGestureId(self.next_file_drag_gesture_id),
            source_pane_id,
            source_tab_id,
            sources,
            pressed_path,
            bookmark_source,
            stationary_action,
            phase: FileDragPhase::WaitingForMovement {
                origin: self.cursor_position,
            },
            native_dnd: FileDragNativeDndState::NotRequested,
            column_directories_snapshot,
            press_origin: self.cursor_position,
            preview_entries: Vec::new(),
        });
    }

    /// 记录拖拽源条目所在的列表滚动视口:聚合判断的"屏幕显示范围"
    /// 以列表可视区域为准(比整个窗口小,上下还隔着工具栏等)。
    pub(crate) fn note_file_drag_viewport(
        &mut self,
        bounds: &[crate::model::ColumnEntryBounds],
        viewports: &[iced::Rectangle],
    ) {
        self.file_drag_viewport = viewports.iter().copied().find(|viewport| {
            bounds.iter().any(|bound| {
                self.file_drag
                    .as_ref()
                    .is_some_and(|file_drag| {
                        file_drag.sources.iter().any(|source| source == &bound.path)
                    })
                    && viewport.contains(bound.bounds.center())
            })
        });
    }

    /// 用最近的条目 bounds 测量填充拖拽预览偏移快照。只填充一次:
    /// 拖动中源视图滚动重排会改变条目原点,重算会让已提起的预览组跳位。
    pub(crate) fn refresh_file_drag_preview_layout(
        &mut self,
        bounds: &[crate::model::ColumnEntryBounds],
    ) {
        let Some(file_drag) = &mut self.file_drag else {
            return;
        };
        if !file_drag.is_dragging() || !file_drag.preview_entries.is_empty() {
            return;
        }
        let source_pane_id = file_drag.source_pane_id;
        let press_origin = file_drag.press_origin;
        let sources: std::collections::HashSet<&std::path::Path> =
            file_drag.sources.iter().map(|path| path.as_path()).collect();
        file_drag.preview_entries = bounds
            .iter()
            .filter(|bound| {
                bound.pane_id == source_pane_id && sources.contains(bound.path.as_path())
            })
            .map(|bound| crate::model::FileDragPreviewEntry {
                path: bound.path.clone(),
                offset: bound.bounds.position() - press_origin,
            })
            .collect();
    }

    fn finish_stationary_file_drag(&mut self, file_drag: FileDragState) -> Task<Message> {
        if file_drag.sources.len() > 1
            && file_drag
                .sources
                .iter()
                .any(|source| source == &file_drag.pressed_path)
        {
            self.select_path(file_drag.pressed_path.clone());
        }

        match file_drag.stationary_action {
            FileDragStationaryAction::SelectionOnly => Task::none(),
            FileDragStationaryAction::ActivateColumnEntry => {
                self.update_open_column_directory_for_entry(&file_drag.pressed_path);
                Task::batch([
                    self.open_column_for_directory(file_drag.pressed_path),
                    self.request_browser_session_save(),
                ])
            }
        }
    }

    pub(crate) fn set_file_drag_target(&mut self, directory: PathBuf) {
        self.set_file_drop_target(Some(FileDropTarget::Directory(directory)));
    }

    pub(crate) fn clear_file_drag_target(&mut self) {
        self.set_file_drop_target(None);
    }

    pub(crate) fn clear_file_drag_target_if_matching(&mut self, directory: &Path) {
        let matches = self.file_drop_session.as_ref().is_some_and(|session| {
            matches!(
                session.hovered_target.as_ref(),
                Some(FileDropTarget::Directory(target)) if target == directory
            ) || (directory == crate::model::trash_location_path().as_path()
                && matches!(session.hovered_target.as_ref(), Some(FileDropTarget::Trash)))
        });
        if matches {
            self.set_file_drop_target(None);
        }
    }

    pub(crate) fn file_drag_release_directory_for_entry(
        &self,
        pane_id: BrowserPaneId,
        path: &Path,
    ) -> Option<PathBuf> {
        self.directory_drop_target_for_entry_in_pane(pane_id, path)
    }

    pub(crate) fn file_drag_release_directory_for_drop_target(
        &self,
        pane_id: BrowserPaneId,
        directory: PathBuf,
    ) -> Option<PathBuf> {
        self.pane_accepts_file_drag(pane_id).then_some(directory)
    }

    pub(super) fn file_drag_drop_directory_at_cursor(&self) -> Option<PathBuf> {
        let pane_id = self.pane_id_at_position(self.cursor_position)?;
        if pane_id == self.active_pane_id() {
            return None;
        }

        let pane = self.pane_view(pane_id)?;
        (!pane.is_trash_view).then(|| pane.current_dir.clone())
    }

    /// 拖放修饰键实时意图:Ctrl=强制复制(Finder Option 语义,优先于
    /// Alt);Alt=在落点创建指向源的符号链接——源与落点都在本地挂载才
    /// 成立,gvfs 等远程挂载上 symlink 不可靠,与右键菜单「创建符号链接」
    /// 同源 gating,不成立时回退移动;Shift 与无修饰均为移动意图(Shift
    /// 本身是移动修饰键),跨盘是否降级为复制由传输引擎决定。修饰键在
    /// 拖放落点实时读取,拖拽途中切换立即生效;胶囊文案与落地共用此判定。
    pub(crate) fn file_drag_drop_intent(
        &self,
        sources: &[PathBuf],
        target_directory: &Path,
    ) -> FileDragDropIntent {
        let modifiers = self.keyboard_modifiers;
        if modifiers.control() && !modifiers.shift() {
            return FileDragDropIntent::Copy;
        }
        let symlinks_supported = !modifiers.shift()
            && sources
                .iter()
                .all(|source| !self.path_is_remote_mount(source))
            && !self.path_is_remote_mount(target_directory);
        if modifiers.alt() && symlinks_supported {
            return FileDragDropIntent::CreateLink;
        }
        FileDragDropIntent::Move
    }

    pub(super) fn move_dragged_files(
        &mut self,
        sources: Vec<PathBuf>,
        target_directory: PathBuf,
    ) -> Task<Message> {
        let intent = self.file_drag_drop_intent(&sources, &target_directory);
        if intent == FileDragDropIntent::CreateLink {
            return self.enqueue_dragged_symbolic_links(&sources, &target_directory);
        }
        let mode = intent.conflict_mode();
        let transfer_targets =
            paths::transfer_targets(&target_directory, &sources, PasteTargetMode::Move);
        if mode == TransferConflictMode::Copy {
            return self.copy_dragged_files(transfer_targets, target_directory);
        }
        if transfer_targets.is_empty()
            || transfer_targets.iter().any(|(source, target)| {
                source == target
                    || target.starts_with(source)
                    || source
                        .parent()
                        .is_some_and(|parent| parent == target_directory)
            })
        {
            return Task::none();
        }
        let transfers = transfer_targets
            .into_iter()
            .map(|(source, target)| QueuedTransfer::new(source, target))
            .collect::<Vec<_>>();

        let open_drop_target = if sources
            .first()
            .and_then(|source| source.parent())
            .is_some_and(|source_parent| {
                target_directory != source_parent && target_directory.starts_with(source_parent)
            }) {
            self.select_path(target_directory.clone());
            self.open_column_for_directory(target_directory)
        } else {
            Task::none()
        };
        Task::batch([
            open_drop_target,
            self.enqueue_or_confirm_transfers(mode, transfers),
        ])
    }

    /// Ctrl 拖拽的复制分支:同目录拖放视为原位副本(共享命名规则起名),
    /// 其余目标沿用源名;把条目拖进自身子树仍然拒绝。
    fn copy_dragged_files(
        &mut self,
        transfer_targets: Vec<(PathBuf, PathBuf)>,
        target_directory: PathBuf,
    ) -> Task<Message> {
        if transfer_targets
            .iter()
            .any(|(source, target)| target.starts_with(source))
        {
            return Task::none();
        }
        let transfers = transfer_targets
            .into_iter()
            .map(|(source, target)| {
                if source != target {
                    return QueuedTransfer::new(source, target);
                }
                let is_directory =
                    self.entry_kind(&source) == Some(file_core::FileKind::Directory);
                QueuedTransfer::new(
                    source.clone(),
                    in_place_duplicate_target(&source, &target_directory, is_directory),
                )
            })
            .collect::<Vec<_>>();
        self.enqueue_or_confirm_transfers(TransferConflictMode::Copy, transfers)
    }

    /// Alt 拖放的落地:在落点目录为每个源创建符号链接,目标一律源路径,
    /// 命名与右键菜单「创建符号链接」走同一共享唯一规则。
    fn enqueue_dragged_symbolic_links(
        &mut self,
        sources: &[PathBuf],
        target_directory: &Path,
    ) -> Task<Message> {
        let links = sources
            .iter()
            .map(|source| self.symbolic_link_creation_for(source, target_directory))
            .collect::<Vec<_>>();
        self.enqueue_file_operation(QueuedFileOperation::CreateSymbolicLinks { links })
    }

    /// 拖拽动作胶囊文案:悬停落点+修饰键意图实时合成。无拖拽、出窗交接
    /// 原生拖放、无落点、书签槽(非传输语义)时返回 None 不渲染。目录名
    /// 动态拼接,这里按当前语言产出成品——readable_text 对 String 不做
    /// 翻译,渲染层不会兜底。
    pub(crate) fn file_drag_action_capsule_label(&self) -> Option<String> {
        let drag = self.file_drag.as_ref()?;
        if !drag.displays_iced_drag_preview() {
            return None;
        }
        // 光标正压在被拖的源条目上:提起的内容还悬在自己身上,落点是
        // 自身或原目录,落地均为空操作,不显示误导性动作。
        if self
            .hovered_entry
            .as_ref()
            .is_some_and(|hovered| drag.sources.contains(hovered))
        {
            return None;
        }
        let session = self.file_drop_session.as_ref()?;
        match session.hovered_target.as_ref()? {
            FileDropTarget::Directory(directory) => {
                self.file_drag_directory_capsule(&drag.sources, directory)
            }
            FileDropTarget::Trash => {
                Some(crate::localization::translate_current("Move to Trash"))
            }
            FileDropTarget::Tab(tab) => match &tab.destination {
                TabDropDestination::Trash => {
                    Some(crate::localization::translate_current("Move to Trash"))
                }
                TabDropDestination::Directory(directory) => {
                    self.file_drag_directory_capsule(&drag.sources, directory)
                }
            },
            FileDropTarget::SidebarBookmarkSlot(_) => None,
        }
    }

    /// 目录落点的胶囊文案。移动意图下落点是源自身、自身子树或源父
    /// 目录(拖起后悬在自己目录的条目或空白上)时,落地均为空操作,
    /// 返回 None 不显示误导性动作;复制/链接在同落点有原位副本/链接
    /// 行为,照常显示。判定条件与 move_dragged_files 的空操作分支一致。
    fn file_drag_directory_capsule(
        &self,
        sources: &[PathBuf],
        directory: &Path,
    ) -> Option<String> {
        let intent = self.file_drag_drop_intent(sources, directory);
        if intent == FileDragDropIntent::Move
            && sources.iter().any(|source| {
                source == directory
                    || directory.starts_with(source)
                    || source.parent().is_some_and(|parent| parent == directory)
            })
        {
            return None;
        }
        Some(file_drag_transfer_label(intent, directory))
    }

    pub(crate) fn extend_drag_selection_to(&mut self, path: PathBuf) {
        let Some(anchor) = self.drag_selection_anchor.clone() else {
            if self.selection_marquee.is_some() {
                self.drag_selection_anchor = Some(path.clone());
                self.select_drag_range(path.clone(), path, self.keyboard_modifiers.control());
            }
            return;
        };
        self.select_drag_range(anchor, path, self.keyboard_modifiers.control());
    }
}

/// 拖放意图的胶囊文案:目录名拼进英文 key 后按当前语言翻译,中文由
/// dynamic_translation 的前缀规则还原语序。
fn file_drag_transfer_label(intent: FileDragDropIntent, directory: &Path) -> String {
    let name = display_directory_name(directory);
    let key = match intent {
        FileDragDropIntent::Copy => format!("Copy to {name}"),
        FileDragDropIntent::CreateLink => format!("Create link to {name}"),
        FileDragDropIntent::Move => format!("Move to {name}"),
    };
    crate::localization::translate_current(&key)
}

/// 落点显示名:file_name 为空(如根目录"/")时退回完整路径。
fn display_directory_name(directory: &Path) -> String {
    directory
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| directory.to_string_lossy().into_owned())
}

/// 同目录复制拖放的原位副本目标:按共享命名规则在源父目录里起唯一名。
fn in_place_duplicate_target(
    source: &Path,
    fallback_directory: &Path,
    source_is_directory: bool,
) -> PathBuf {
    let parent = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fallback_directory.to_path_buf());
    let name = source
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("item"));
    let unique_name = if source_is_directory {
        unique_duplicated_directory_name(name, |candidate| entry_exists(&parent.join(candidate)))
    } else {
        unique_duplicated_file_name(name, |candidate| entry_exists(&parent.join(candidate)))
    };
    parent.join(unique_name)
}

pub(super) fn resolve_file_drag_target(
    sources: &[PathBuf],
    release_directory: Option<PathBuf>,
    target: Option<FileDropTarget>,
    fallback_directory: Option<PathBuf>,
) -> Option<FileDropTarget> {
    if let Some(release_directory) = release_directory {
        return Some(FileDropTarget::Directory(release_directory));
    }

    match target {
        Some(FileDropTarget::Directory(target_directory)) => {
            if file_drag_directory_target_needs_fallback(sources, &target_directory) {
                fallback_directory
                    .map(FileDropTarget::Directory)
                    .or(Some(FileDropTarget::Directory(target_directory)))
            } else {
                Some(FileDropTarget::Directory(target_directory))
            }
        }
        target @ Some(
            FileDropTarget::Trash | FileDropTarget::SidebarBookmarkSlot(_) | FileDropTarget::Tab(_),
        ) => target,
        None => fallback_directory.map(FileDropTarget::Directory),
    }
}

pub(super) fn safe_file_drop_target(
    sources: &[PathBuf],
    target: Option<FileDropTarget>,
) -> Option<FileDropTarget> {
    match target {
        Some(FileDropTarget::Directory(directory))
            if file_drag_directory_target_needs_fallback(sources, &directory) =>
        {
            None
        }
        target => target,
    }
}

fn file_drag_directory_target_needs_fallback(sources: &[PathBuf], target: &Path) -> bool {
    sources.iter().any(|source| {
        source == target
            || target.starts_with(source)
            || source.parent().is_some_and(|parent| parent == target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SidebarBookmarkDropSlot;
    use crate::sidebar_devices::SidebarDeviceEntry;
    use desktop_linux::{StorageDeviceAccess, StorageDeviceId};
    use iced::keyboard;

    #[test]
    fn drag_drop_intent_follows_live_modifiers() {
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        let target = PathBuf::from("/tmp/drag-target");

        // 无修饰=移动意图(现状)。
        assert_eq!(
            browser.file_drag_drop_intent(&[], &target),
            FileDragDropIntent::Move
        );

        browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
        assert_eq!(
            browser.file_drag_drop_intent(&[], &target),
            FileDragDropIntent::Copy
        );

        // Shift 与 Ctrl+Shift 都保持移动语义:Shift 本身就是移动修饰键。
        browser.keyboard_modifiers = keyboard::Modifiers::SHIFT;
        assert_eq!(
            browser.file_drag_drop_intent(&[], &target),
            FileDragDropIntent::Move
        );
        browser.keyboard_modifiers =
            keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT;
        assert_eq!(
            browser.file_drag_drop_intent(&[], &target),
            FileDragDropIntent::Move
        );

        // Alt=创建链接;Ctrl 与 Alt 同时按住时 Ctrl 复制优先。
        browser.keyboard_modifiers = keyboard::Modifiers::ALT;
        assert_eq!(
            browser.file_drag_drop_intent(&[], &target),
            FileDragDropIntent::CreateLink
        );
        browser.keyboard_modifiers =
            keyboard::Modifiers::CTRL | keyboard::Modifiers::ALT;
        assert_eq!(
            browser.file_drag_drop_intent(&[], &target),
            FileDragDropIntent::Copy
        );
    }

    #[test]
    fn alt_drag_intent_falls_back_to_move_on_remote_mount() {
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        browser.sidebar_devices.devices = vec![SidebarDeviceEntry {
            id: StorageDeviceId::new("gvfs"),
            label: "gvfs".to_owned(),
            detail: None,
            size_bytes: 0,
            mount_points: vec![PathBuf::from("/run/user/1000/gvfs")],
            access: StorageDeviceAccess::RemoteFilesystem,
            can_mount: true,
            can_unmount: true,
            removal: None,
        }];
        browser.keyboard_modifiers = keyboard::Modifiers::ALT;

        let remote_source = PathBuf::from("/run/user/1000/gvfs/mtp/DCIM/photo.jpg");
        let local_source = PathBuf::from("/home/user/photo.jpg");
        let local_target = PathBuf::from("/home/user/photos");
        let remote_target = PathBuf::from("/run/user/1000/gvfs/mtp/DCIM");

        // 源或落点在远程挂载:gvfs 上 symlink 不可靠,回退移动。
        assert_eq!(
            browser.file_drag_drop_intent(&[remote_source], &local_target),
            FileDragDropIntent::Move
        );
        assert_eq!(
            browser.file_drag_drop_intent(std::slice::from_ref(&local_source), &remote_target),
            FileDragDropIntent::Move
        );
        assert_eq!(
            browser.file_drag_drop_intent(std::slice::from_ref(&local_source), &local_target),
            FileDragDropIntent::CreateLink
        );
    }

    #[test]
    fn drag_action_capsule_label_follows_target_and_intent() {
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("report.pdf");
        std::fs::write(&source, b"data").unwrap();
        let project = directory.path().join("project");
        std::fs::create_dir(&project).unwrap();

        // 无拖拽会话:不渲染。
        assert!(browser.file_drag_action_capsule_label().is_none());

        // entries 为空时 sources 回退到 self.selected。
        browser.selected = Some(source.clone());
        browser.cursor_position = iced::Point::new(0.0, 0.0);
        browser.start_file_drag(
            source.clone(),
            FileDragStationaryAction::SelectionOnly,
            Vec::new(),
        );
        drop(browser.update_file_drag(iced::Point::new(10.0, 0.0)));

        // 拖拽中但无悬停落点:不渲染。
        assert!(browser.file_drag_action_capsule_label().is_none());

        drop(browser.handle_drop_target_hovered(project.clone()));
        assert_eq!(
            browser.file_drag_action_capsule_label().as_deref(),
            Some("Move to project")
        );

        // 光标悬停回被拖的源条目:提起的内容悬在自己身上,落点无意义,不显示。
        drop(browser.handle_entry_hovered(source.clone()));
        assert!(browser.file_drag_action_capsule_label().is_none());

        // 悬停源父目录(当前目录空白处):移动落地是空操作,不显示;
        // Ctrl 复制在同目录有原位副本,照常显示。
        let current_directory = directory.path().to_path_buf();
        let directory_name = current_directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_owned();
        drop(browser.handle_drop_target_hovered(current_directory.clone()));
        assert!(browser.file_drag_action_capsule_label().is_none());
        browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
        assert_eq!(
            browser.file_drag_action_capsule_label().as_deref(),
            Some(format!("Copy to {directory_name}").as_str())
        );
        browser.keyboard_modifiers = keyboard::Modifiers::empty();

        // 离开源条目回到文件夹落点:恢复显示。
        drop(browser.handle_entry_hover_cleared(source.clone()));
        drop(browser.handle_drop_target_hovered(project.clone()));
        assert_eq!(
            browser.file_drag_action_capsule_label().as_deref(),
            Some("Move to project")
        );
        browser.keyboard_modifiers = keyboard::Modifiers::CTRL;
        assert_eq!(
            browser.file_drag_action_capsule_label().as_deref(),
            Some("Copy to project")
        );
        browser.keyboard_modifiers = keyboard::Modifiers::ALT;
        assert_eq!(
            browser.file_drag_action_capsule_label().as_deref(),
            Some("Create link to project")
        );

        // 回收站落点固定移动文案,修饰键不影响(落地行为同样不看)。
        let session = browser.file_drop_session.as_mut().unwrap();
        session.hovered_target = Some(FileDropTarget::Trash);
        assert_eq!(
            browser.file_drag_action_capsule_label().as_deref(),
            Some("Move to Trash")
        );

        // 书签槽不是传输语义:不渲染。
        let session = browser.file_drop_session.as_mut().unwrap();
        session.hovered_target = Some(FileDropTarget::SidebarBookmarkSlot(
            SidebarBookmarkDropSlot::Insert { index: 0 },
        ));
        assert!(browser.file_drag_action_capsule_label().is_none());
    }

    #[test]
    fn alt_drag_release_queues_symlinks_into_target_directory() {
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("report.pdf");
        std::fs::write(&source, b"data").unwrap();
        let target = directory.path().join("archive");
        std::fs::create_dir(&target).unwrap();

        browser.keyboard_modifiers = keyboard::Modifiers::ALT;
        drop(browser.move_dragged_files(vec![source.clone()], target.clone()));

        assert_eq!(browser.operation_queue.tasks().len(), 1);
        match &browser.operation_queue.tasks()[0].operation {
            QueuedFileOperation::CreateSymbolicLinks { links } => {
                assert_eq!(links.len(), 1);
                assert_eq!(links[0].target_path, source);
                assert_eq!(links[0].link_path.parent(), Some(target.as_path()));
            }
            other => panic!("expected symlink creation, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_drag_inside_same_directory_plans_in_place_duplicate() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("report.pdf");
        std::fs::write(&source, b"data").unwrap();
        let directory_path = directory.path().to_path_buf();

        let target =
            in_place_duplicate_target(&source, &directory_path, false);

        assert_eq!(target, directory.path().join("report副本.pdf"));
        assert!(!target.exists());
    }

    #[test]
    fn unsafe_directory_targets_are_rejected_for_every_source_relationship() {
        let file_source = PathBuf::from("/workspace/report.txt");
        let directory_source = PathBuf::from("/workspace/project");

        for (sources, target) in [
            (vec![file_source.clone()], PathBuf::from("/workspace")),
            (vec![directory_source.clone()], directory_source.clone()),
            (
                vec![directory_source.clone()],
                directory_source.join("nested"),
            ),
        ] {
            assert!(
                safe_file_drop_target(&sources, Some(FileDropTarget::Directory(target)),).is_none()
            );
        }
    }

    #[test]
    fn expanded_subdirectory_source_can_move_back_to_tab_root() {
        let source = PathBuf::from("/workspace/root/expanded/report.txt");
        let root = PathBuf::from("/workspace/root");

        assert_eq!(
            safe_file_drop_target(
                std::slice::from_ref(&source),
                Some(FileDropTarget::Directory(root.clone())),
            ),
            Some(FileDropTarget::Directory(root))
        );
    }
}
