use std::path::{Path, PathBuf};

use iced::Task;

use super::super::paths::{self, PasteTargetMode};
use super::super::wayland_dnd::WaylandFileDragRequest;
use super::super::{FileBrowser, POINTER_DRAG_ACTIVATION_DISTANCE};
use super::clipboard::split_archive_member_sources;
use crate::model::{
    entry_exists, unique_duplicated_directory_name, unique_duplicated_file_name, BrowserPaneId,
    FileDragDropIntent, FileDragGestureId, FileDragNativeDndState, FileDragPhase, FileDragState,
    FileDragStationaryAction, FileDropTarget, Message, SelectionMarqueeSource, TabDropDestination,
    TransferConflictMode,
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
            // 激活即交接原生拖放:Wayland 上位图是全程唯一预览,窗口内
            // 落点由原生目标事件驱动;请求失败回退应用内拖拽,拖拽不凭空
            // 消失。X11 没有原生拖出通道,保持应用内拖拽。
            return if self.wayland_dnd.is_some() {
                self.start_native_file_drag()
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
    /// 持续发给本窗口,光标越界也一样。激活即已交接原生拖放,此路径
    /// 只服务恢复:激活时的原生请求失败(回退应用内拖拽)后,光标贴近
    /// 边缘时重试交接——越过边缘后合成器会结束隐式 grab,按压记录随之
    /// 失效,越界后才请求的拖放注定被拒。
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

    /// 激活即把拖放交给 Wayland 原生会话:窗口内外从此只有合成器位图
    /// 一种预览。请求失败时复位原生状态并回退应用内拖拽——拖拽不能
    /// 凭空消失;后续由缓冲带重试(start_native_file_drag_for_cursor_left)
    /// 负责恢复交接。
    pub(crate) fn start_native_file_drag(&mut self) -> Task<Message> {
        let drag_sources = self
            .file_drag
            .as_ref()
            .expect("native drag start requires an active file drag")
            .sources
            .clone();
        match self.request_wayland_file_drag(drag_sources) {
            WaylandFileDragRequest::Requested(session_id) => {
                if let Some(file_drag) = &mut self.file_drag {
                    file_drag.native_dnd = FileDragNativeDndState::Requested(session_id);
                }
                Task::none()
            }
            WaylandFileDragRequest::Rejected(error) => {
                if let Some(file_drag) = &mut self.file_drag {
                    file_drag.native_dnd = FileDragNativeDndState::NotRequested;
                }
                self.show_global_error(error);
                self.begin_iced_file_drag_after_activation()
            }
            WaylandFileDragRequest::Unavailable => self.begin_iced_file_drag_after_activation(),
        }
    }

    /// 缓冲带恢复交接:光标离开主窗口前把拖放交给 Wayland 原生会话。
    /// 只在激活交接失败后(native_dnd 复位 NotRequested)才有实际效果。
    /// 请求失败时保持应用内拖拽——回到窗口即恢复移动事件,重进后松手
    /// 仍正常收尾,只有窗口外落放会丢失;此处不能整段取消,用户可能只是
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
        // 条目偏移快照已改为按下时测量,这里只需启动应用内落点会话。
        self.begin_iced_file_drop_session()
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
    ) -> Task<Message> {
        self.sidebar_bookmark_drop_slot = None;
        self.file_drop_session = None;
        if self.is_trash_view {
            self.file_drag = None;
            return Task::none();
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
            wayland_drag_icon: None,
        });
        // 按下即测量条目偏移:激活瞬间要交接原生拖放并一次性生成位图,
        // 快照必须在此之前就绪(极速甩动来不及则退单胶囊兜底)。
        if self.file_drag.is_some() {
            crate::column_entry_bounds::column_entry_bounds_command()
        } else {
            Task::none()
        }
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
                self.file_drag.as_ref().is_some_and(|file_drag| {
                    file_drag.sources.iter().any(|source| source == &bound.path)
                }) && viewport.contains(bound.bounds.center())
            })
        });
    }

    /// 用最近的条目 bounds 测量填充拖拽预览偏移快照。只填充一次:
    /// 拖动中源视图滚动重排会改变条目原点,重算会让已提起的预览组跳位。
    /// WaitingForMovement 期间也填充:按下时发起的测量在激活交接原生
    /// 拖放之前到达,位图偏移依赖这份快照。填充成功的同一刻发起拖出
    /// 位图的后台预渲染——激活瞬间直接消费现成位图,"只填一次"守卫
    /// 同时保证预渲染只发起一次。
    pub(crate) fn refresh_file_drag_preview_layout(
        &mut self,
        bounds: &[crate::model::ColumnEntryBounds],
    ) -> Task<Message> {
        {
            let Some(file_drag) = &mut self.file_drag else {
                return Task::none();
            };
            if !file_drag.preview_entries.is_empty() {
                return Task::none();
            }
            let source_pane_id = file_drag.source_pane_id;
            let press_origin = file_drag.press_origin;
            let sources: std::collections::HashSet<&std::path::Path> = file_drag
                .sources
                .iter()
                .map(|path| path.as_path())
                .collect();
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
        // 快照借用在上面作用域结束:发起预渲染要重新读取浏览器状态。
        if self.wayland_dnd.is_some() {
            self.preload_wayland_drag_icon()
        } else {
            Task::none()
        }
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
        // 包内只读:落点目录在包内时拖拽落地语义不成立(与粘贴的包内
        // 门控同源),一处早退覆盖移动/复制/建链三种意图。
        if file_core::archive_path_identity(&target_directory)
            != file_core::ArchivePathIdentity::RealFile
        {
            return Task::none();
        }
        let intent = self.file_drag_drop_intent(&sources, &target_directory);
        // 与粘贴管线共用同一源拆分:包内成员入提取队列,真实路径走原
        // 传输管线,混合多选两路并行落地。移动与复制意图下的包内成员
        // 都降级为提取——包只读,源既删不掉也复制不出;提取管道对重名
        // 自动加 ` (2)` 后缀,不需要冲突弹窗。
        let (archive_sources, real_sources) = split_archive_member_sources(sources);
        if intent == FileDragDropIntent::CreateLink {
            // 符号链接指向包内虚拟路径只会悬空:含成员时整批不做,
            // 不产生只建了一半的链接。
            if !archive_sources.is_empty() {
                return Task::none();
            }
            return self.enqueue_dragged_symbolic_links(&real_sources, &target_directory);
        }
        let archive_members =
            (!archive_sources.is_empty()).then(|| QueuedFileOperation::ExtractArchiveMembers {
                sources: archive_sources,
                destination: target_directory.clone(),
            });

        let mode = intent.conflict_mode();
        let transfer_targets =
            paths::transfer_targets(&target_directory, &real_sources, PasteTargetMode::Move);
        let transfers = if real_sources.is_empty() {
            Task::none()
        } else if mode == TransferConflictMode::Copy {
            self.copy_dragged_files(transfer_targets, target_directory.clone())
        } else {
            // 空操作源(落点目录自身/自身子树/已在落点目录)逐个跳过而非整
            // 批拒绝:多选里混着落点目录自身时其余条目照常移动,全部空操作
            // 才无事发生。与 file_drag_directory_capsule 的显示条件同判定。
            let movable = transfer_targets
                .into_iter()
                .filter(|(source, _)| !paths::move_is_no_op(source, &target_directory))
                .collect::<Vec<_>>();
            if movable.is_empty() {
                Task::none()
            } else {
                let transfers = movable
                    .into_iter()
                    .map(|(source, target)| QueuedTransfer::new(source, target))
                    .collect::<Vec<_>>();

                let open_drop_target = if real_sources
                    .first()
                    .and_then(|source| source.parent())
                    .is_some_and(|source_parent| {
                        target_directory != source_parent
                            && target_directory.starts_with(source_parent)
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
        };
        // 单一出口合成:混合多选时提取与传输两路并行(与粘贴管线同构)。
        match archive_members {
            None => transfers,
            Some(members) => Task::batch([self.enqueue_file_operation(members), transfers]),
        }
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
                let is_directory = self.entry_kind(&source) == Some(file_core::FileKind::Directory);
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

    /// 拖拽动作胶囊文案:悬停落点+修饰键意图实时合成。应用内拖拽由
    /// iced 悬停驱动,原生拖放由原生目标事件驱动(file_drop_session 同源
    /// 更新),两种形态共用此判定。无拖拽、无落点、书签槽(非传输语义)
    /// 时返回 None 不渲染。目录名动态拼接,这里按当前语言产出成品——
    /// readable_text 对 String 不做翻译,渲染层不会兜底。
    pub(crate) fn file_drag_action_capsule_label(&self) -> Option<String> {
        let drag = self.file_drag.as_ref()?;
        if !drag.is_dragging() {
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
            FileDropTarget::Trash => Some(crate::localization::translate_current("Move to Trash")),
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

    /// 目录落点的胶囊文案。移动意图下整批源都是空操作(源自身、自身
    /// 子树、已在落点目录)才隐藏——多选里混着落点目录自身时其余条目
    /// 仍可移动,照常显示;落地按同一判定跳过空操作源。复制/链接在同
    /// 落点有原位副本/链接行为,照常显示。
    fn file_drag_directory_capsule(&self, sources: &[PathBuf], directory: &Path) -> Option<String> {
        let intent = self.file_drag_drop_intent(sources, directory);
        if intent == FileDragDropIntent::Move
            && sources
                .iter()
                .all(|source| paths::move_is_no_op(source, directory))
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
pub(super) fn in_place_duplicate_target(
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
    sources
        .iter()
        .any(|source| paths::move_is_no_op(source, target))
}
