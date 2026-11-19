use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use file_core::FileKind;
use iced::{Point, Rectangle, Task};

use super::{FileBrowser, POINTER_DRAG_ACTIVATION_DISTANCE};
use crate::animation::elapsed_fraction as sidebar_bookmark_animation_progress;
use crate::model::{
    ContextMenuState, FileDragPhase, FileDropTarget, Message, NavigationMode,
    SidebarBookmarkContextMenuState, SidebarBookmarkDragState, SidebarBookmarkDropSlot,
    SidebarLocation, SidebarLocationKind,
};
use bennu_sidebar::sidebar_favorite_configs;
use bennu_theme::animation::ease_out_cubic as sidebar_bookmark_ease_out_cubic;

#[cfg(test)]
mod tests;

use crate::view::{SIDEBAR_ROW_HEIGHT as SIDEBAR_LOCATION_ROW_HEIGHT, SIDEBAR_ROW_SPACING};

const SIDEBAR_BOOKMARK_INSERT_EDGE_HEIGHT: f32 = 8.0;
const SIDEBAR_BOOKMARK_MOTION_DURATION: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy)]
pub(crate) struct SidebarBookmarkMotionState {
    started_at: Instant,
    initial_y_offset: f32,
}

impl SidebarBookmarkMotionState {
    pub(crate) fn offset_y(self) -> f32 {
        let progress = sidebar_bookmark_ease_out_cubic(sidebar_bookmark_animation_progress(
            self.started_at,
            SIDEBAR_BOOKMARK_MOTION_DURATION,
        ));
        self.initial_y_offset * (1.0 - progress)
    }

    fn is_animating(self) -> bool {
        sidebar_bookmark_animation_progress(self.started_at, SIDEBAR_BOOKMARK_MOTION_DURATION) < 1.0
    }
}

impl FileBrowser {
    pub(crate) fn sidebar_bookmark_motion_offset(&self, path: &Path) -> f32 {
        if let Some(offset_y) = self.dragged_sidebar_bookmark_offset(path) {
            return offset_y;
        }

        self.sidebar_bookmark_motion
            .get(path)
            .map(|motion| motion.offset_y())
            .unwrap_or(0.0)
    }

    pub(super) fn sidebar_bookmark_motion_is_active(&self) -> bool {
        !self.sidebar_bookmark_motion.is_empty()
    }

    pub(super) fn advance_sidebar_bookmark_motion(&mut self) -> Task<Message> {
        self.prune_sidebar_bookmark_motion();
        Task::none()
    }

    /// 正在拖一个能成为新收藏的文件夹：收藏区这时才接插入落点，末尾多出一条空白带。
    pub(crate) fn can_drop_sidebar_bookmark(&self) -> bool {
        self.file_drag
            .as_ref()
            .is_some_and(|file_drag| file_drag.is_dragging() && file_drag.bookmark_source.is_some())
    }

    /// 拖动开始时定下：只拖了一个还没收藏的文件夹，这次拖动才能在收藏区插成新收藏。
    /// 已收藏的拖过来，收藏行只当目录，不出插入线。
    pub(super) fn new_favorite_from_drag(&self, sources: &[PathBuf]) -> Option<PathBuf> {
        match sources {
            [source]
                if self.entry_kind(source) == Some(FileKind::Directory)
                    && !self.sidebar_favorite_exists(source) =>
            {
                Some(source.clone())
            }
            _ => None,
        }
    }

    /// 应用内拖放（X11、原生拖放没起来）在侧栏上移动。原生拖放期间侧栏收不到指针移动，
    /// 所以这里只服务应用内拖放：和原生拖放一样，拿拖动开始时量好的布局和窗口坐标算落点。
    pub(super) fn follow_in_app_drag_over_sidebar(&mut self) -> Task<Message> {
        let position = self.cursor_position.get();
        if let Some(session) = &mut self.file_drop_session {
            session.position = position;
        }
        self.cursor_paste_directory = None;
        self.refresh_hovered_target_from_ready_layout()
    }

    pub(super) fn clear_sidebar_bookmark_drop_slot(&mut self) -> Task<Message> {
        self.sidebar_bookmark_drop_slot = None;
        self.clear_sidebar_bookmark_drop_target();
        Task::none()
    }

    /// 右键「加入收藏夹」：加到最后。
    pub(super) fn add_sidebar_bookmark(&mut self, path: PathBuf) -> Task<Message> {
        self.context_menu = None;
        let index = self.sidebar_favorite_locations().len();
        self.insert_sidebar_bookmark(SidebarBookmarkDropSlot::Insert { index }, path)
    }

    pub(super) fn insert_sidebar_bookmark(
        &mut self,
        slot: SidebarBookmarkDropSlot,
        source: PathBuf,
    ) -> Task<Message> {
        if self.sidebar_favorite_exists(&source) {
            return Task::none();
        }

        let mut favorites = self.sidebar_favorite_locations();
        let location = SidebarLocation {
            label: sidebar_bookmark_label(&source),
            path: source,
            kind: SidebarLocationKind::Bookmark,
        };
        match slot {
            SidebarBookmarkDropSlot::Insert { index } => {
                favorites.insert(index.min(favorites.len()), location)
            }
        }
        self.replace_sidebar_favorites(favorites);
        self.save_sidebar_favorites()
    }

    pub(super) fn start_sidebar_bookmark_drag(&mut self, path: PathBuf) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        if !self.sidebar_favorite_exists(&path) {
            return rename_command;
        }
        self.context_menu = None;
        self.cancel_file_drag_interaction();
        self.selection_marquee = None;
        self.sidebar_bookmark_drop_slot = None;
        self.sidebar_bookmark_motion.clear();
        let favorites = self.sidebar_favorite_locations();
        let source_index = favorites
            .iter()
            .position(|location| location.path == path)
            .unwrap_or(0);
        self.sidebar_bookmark_drag = Some(SidebarBookmarkDragState {
            path,
            origin: self.cursor_or_origin(),
            source_index,
            phase: FileDragPhase::WaitingForMovement {
                origin: self.cursor_or_origin(),
            },
            order_changed: false,
        });
        rename_command
    }

    pub(super) fn handle_sidebar_bookmark_right_clicked(&mut self, path: PathBuf) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        if !self.sidebar_favorite_exists(&path) {
            return rename_command;
        }

        self.preview_engine.clear_preview();
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.selection_marquee = None;
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        let _ = self.cancel_address_editing();
        if self
            .user_config
            .context_menus
            .sidebar_bookmark_items()
            .is_empty()
        {
            return rename_command;
        }
        self.context_menu = Some(ContextMenuState::SidebarBookmark(
            SidebarBookmarkContextMenuState {
                path,
                position: self.cursor_or_origin(),
            },
        ));
        rename_command
    }

    pub(super) fn delete_sidebar_bookmark(&mut self, path: PathBuf) -> Task<Message> {
        self.context_menu = None;
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.sidebar_bookmark_motion.remove(path.as_path());
        if self.hovered_sidebar.as_ref() == Some(&path) {
            self.hovered_sidebar = None;
        }
        if !self.sidebar_favorite_exists(&path) {
            return Task::none();
        }

        let favorites = self
            .sidebar_favorite_locations()
            .into_iter()
            .filter(|location| location.path != path)
            .collect::<Vec<_>>();
        self.replace_sidebar_favorites(favorites);
        self.save_sidebar_favorites()
    }

    pub(super) fn update_sidebar_bookmark_drag(&mut self, position: Point) {
        let Some(drag) = self.sidebar_bookmark_drag.as_ref() else {
            return;
        };
        let origin = match drag.phase {
            FileDragPhase::WaitingForMovement { origin } => origin,
            FileDragPhase::Dragging => drag.origin,
        };
        let delta_x = position.x - origin.x;
        let delta_y = position.y - origin.y;
        let favorites = self.sidebar_favorite_locations();
        let current_index = favorites
            .iter()
            .position(|location| location.path == drag.path)
            .unwrap_or(drag.source_index);
        let projected_index =
            projected_sidebar_bookmark_index(drag.source_index, delta_y, favorites.len());
        let cursor_inside_sidebar_x = (0.0..=self.sidebar_width).contains(&position.x);
        let should_activate = matches!(drag.phase, FileDragPhase::WaitingForMovement { .. })
            && delta_x * delta_x + delta_y * delta_y
                >= POINTER_DRAG_ACTIVATION_DISTANCE * POINTER_DRAG_ACTIVATION_DISTANCE;

        if should_activate {
            let Some(drag) = &mut self.sidebar_bookmark_drag else {
                return;
            };
            drag.phase = FileDragPhase::Dragging;
        }

        if !cursor_inside_sidebar_x
            && self
                .sidebar_bookmark_drag
                .as_ref()
                .is_some_and(SidebarBookmarkDragState::is_dragging)
            && projected_index != current_index
        {
            if let Some(target) = favorites.get(projected_index) {
                self.hovered_sidebar = Some(target.path.clone());
                self.reorder_sidebar_bookmark(target.path.clone());
            }
        }
    }

    pub(super) fn handle_sidebar_bookmark_entered(&mut self, path: PathBuf) -> Task<Message> {
        if self.file_drag.is_some() {
            return Task::none();
        }

        if self
            .sidebar_bookmark_drag
            .as_ref()
            .is_some_and(SidebarBookmarkDragState::is_dragging)
        {
            self.hovered_sidebar = Some(path.clone());
            self.reorder_sidebar_bookmark(path);
            Task::none()
        } else {
            self.handle_sidebar_hovered(path)
        }
    }

    pub(super) fn finish_sidebar_bookmark_drag(&mut self) -> Task<Message> {
        let Some(drag) = self.sidebar_bookmark_drag.take() else {
            return Task::none();
        };
        self.sidebar_bookmark_drop_slot = None;
        if drag.is_dragging() {
            let release_offset = self.dragged_sidebar_bookmark_offset_for(&drag);
            if release_offset.abs() > f32::EPSILON {
                self.start_sidebar_bookmark_motion(vec![drag.path.clone()], release_offset);
            }

            if drag.order_changed {
                return self.save_sidebar_favorites();
            }
            return Task::none();
        }
        Task::batch([
            self.commit_rename_if_active(),
            self.navigate_to(drag.path, NavigationMode::RecordHistory),
        ])
    }

    fn reorder_sidebar_bookmark(&mut self, entered_path: PathBuf) {
        let Some(dragged_path) = self
            .sidebar_bookmark_drag
            .as_ref()
            .map(|drag| drag.path.clone())
        else {
            return;
        };
        if dragged_path == entered_path {
            return;
        }
        let mut favorites = self.sidebar_favorite_locations();
        let Some(dragged_index) = favorites
            .iter()
            .position(|location| location.path == dragged_path)
        else {
            return;
        };
        let Some(entered_index) = favorites
            .iter()
            .position(|location| location.path == entered_path)
        else {
            return;
        };
        let shifted_paths =
            shifted_sidebar_bookmark_paths(&favorites, dragged_index, entered_index);
        let row_stride = sidebar_bookmark_motion_stride();
        let shifted_offset = if dragged_index < entered_index {
            row_stride
        } else {
            -row_stride
        };

        let dragged = favorites.remove(dragged_index);
        favorites.insert(entered_index, dragged);
        self.replace_sidebar_favorites(favorites);
        self.start_sidebar_bookmark_motion(shifted_paths, shifted_offset);
        if let Some(drag) = &mut self.sidebar_bookmark_drag {
            drag.order_changed = true;
        }
    }

    fn start_sidebar_bookmark_motion(&mut self, paths: Vec<PathBuf>, initial_y_offset: f32) {
        let started_at = Instant::now();
        for path in paths {
            let current_y_offset = self
                .sidebar_bookmark_motion
                .get(path.as_path())
                .map(|motion| motion.offset_y())
                .unwrap_or(0.0);
            self.sidebar_bookmark_motion.insert(
                path,
                SidebarBookmarkMotionState {
                    started_at,
                    initial_y_offset: current_y_offset + initial_y_offset,
                },
            );
        }
    }

    fn prune_sidebar_bookmark_motion(&mut self) {
        let favorite_paths = self
            .sidebar_favorite_locations()
            .into_iter()
            .map(|location| location.path)
            .collect::<Vec<_>>();
        self.sidebar_bookmark_motion.retain(|path, motion| {
            favorite_paths.iter().any(|favorite| favorite == path) && motion.is_animating()
        });
    }

    fn dragged_sidebar_bookmark_offset(&self, path: &Path) -> Option<f32> {
        let drag = self.sidebar_bookmark_drag.as_ref()?;
        if drag.path != path || !drag.is_dragging() {
            return None;
        }
        Some(self.dragged_sidebar_bookmark_offset_for(drag))
    }

    fn dragged_sidebar_bookmark_offset_for(&self, drag: &SidebarBookmarkDragState) -> f32 {
        let cursor_offset = self.cursor_or_origin().y - drag.origin.y;
        let favorites = self.sidebar_favorite_locations();
        let current_index = favorites
            .iter()
            .position(|location| location.path == drag.path)
            .unwrap_or(drag.source_index);
        let layout_offset =
            (drag.source_index as f32 - current_index as f32) * sidebar_bookmark_motion_stride();
        let raw_visual_offset = cursor_offset + layout_offset;
        let (min_visual_offset, max_visual_offset) =
            sidebar_bookmark_drag_offset_bounds(current_index, favorites.len());
        let visual_offset = raw_visual_offset.clamp(min_visual_offset, max_visual_offset);
        visual_offset
    }

    fn sidebar_favorite_locations(&self) -> Vec<SidebarLocation> {
        self.sidebar_locations
            .iter()
            .filter(|location| location.kind.is_user_favorite())
            .cloned()
            .collect()
    }

    fn replace_sidebar_favorites(&mut self, favorites: Vec<SidebarLocation>) {
        let mut locations = self
            .sidebar_locations
            .iter()
            .filter(|location| !location.kind.is_user_favorite())
            .cloned()
            .collect::<Vec<_>>();
        locations.extend(favorites);
        self.sidebar_locations = locations;
    }

    fn save_sidebar_favorites(&mut self) -> Task<Message> {
        let favorites = self.sidebar_favorite_locations();
        self.user_config.sidebar_favorites = Some(sidebar_favorite_configs(&favorites));
        self.persist_user_preferences_command()
    }

    fn clear_sidebar_bookmark_drop_target(&mut self) {
        let is_bookmark_target = self.file_drop_session.as_ref().is_some_and(|session| {
            matches!(
                session.hovered_target.as_ref(),
                Some(FileDropTarget::SidebarBookmarkSlot(_))
            )
        });
        if is_bookmark_target {
            self.set_file_drop_target(None);
        }
    }

    pub(super) fn sidebar_favorite_exists(&self, path: &Path) -> bool {
        self.sidebar_locations
            .iter()
            .any(|location| location.kind.is_user_favorite() && location.path == path)
    }
}

/// 拖新收藏时收藏区里的落点。按顺序走收藏行：行顶往下 8px 以内插到这一行前，行底往上
/// 8px 以内留给下一行去算（插到它前面），中间是放进这个目录；走完还没落下就是加到最后。
/// 这样区里每一点都有归属：标题落在第一行之前，行间缝落在下一行之前，末尾空白带落在最后，
/// 没有收藏时整区就是加进去。插入线就画在缝上，对着线松手一定插得进去。
/// 8px 是行高 32 的四分之一，和地方面板常见的插入指示条一样高。
pub(in crate::app) fn favorite_drop_target<'a>(
    rows: impl IntoIterator<Item = (&'a Path, Rectangle)>,
    position_y: f32,
) -> FileDropTarget {
    let mut row_count = 0;
    for (index, (directory, bounds)) in rows.into_iter().enumerate() {
        if position_y < bounds.y + SIDEBAR_BOOKMARK_INSERT_EDGE_HEIGHT {
            return FileDropTarget::SidebarBookmarkSlot(SidebarBookmarkDropSlot::Insert { index });
        }
        if position_y <= bounds.y + bounds.height - SIDEBAR_BOOKMARK_INSERT_EDGE_HEIGHT {
            return FileDropTarget::Directory(directory.to_path_buf());
        }
        row_count = index + 1;
    }
    FileDropTarget::SidebarBookmarkSlot(SidebarBookmarkDropSlot::Insert { index: row_count })
}

fn shifted_sidebar_bookmark_paths(
    favorites: &[SidebarLocation],
    dragged_index: usize,
    entered_index: usize,
) -> Vec<PathBuf> {
    if dragged_index < entered_index {
        favorites[dragged_index + 1..=entered_index]
            .iter()
            .map(|location| location.path.clone())
            .collect()
    } else {
        favorites[entered_index..dragged_index]
            .iter()
            .map(|location| location.path.clone())
            .collect()
    }
}

fn sidebar_bookmark_motion_stride() -> f32 {
    SIDEBAR_LOCATION_ROW_HEIGHT + SIDEBAR_ROW_SPACING
}

fn sidebar_bookmark_drag_offset_bounds(current_index: usize, favorite_count: usize) -> (f32, f32) {
    let row_stride = sidebar_bookmark_motion_stride();
    let last_index = favorite_count.saturating_sub(1);
    let current_index = current_index.min(last_index);
    let min_offset = -(current_index as f32) * row_stride;
    let max_offset = last_index.saturating_sub(current_index) as f32 * row_stride;
    (min_offset, max_offset)
}

fn projected_sidebar_bookmark_index(
    source_index: usize,
    cursor_offset_y: f32,
    favorite_count: usize,
) -> usize {
    let last_index = favorite_count.saturating_sub(1);
    let projected = source_index as f32 + cursor_offset_y / sidebar_bookmark_motion_stride();
    projected.round().clamp(0.0, last_index as f32) as usize
}

fn sidebar_bookmark_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
