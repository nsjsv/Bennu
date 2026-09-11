use std::path::{Path, PathBuf};
use std::sync::Arc;

use desktop_linux::{WaylandDndController, WaylandDndWindowHandle, WaylandFileDragSessionId};
use iced::Task;
use iced::widget::image;

use super::FileBrowser;
use crate::appearance::subtle_border_color;
use crate::matugen_theme::ui_colors;
use crate::model::Message;
use crate::wayland_drag_icon::{
    file_drag_display_name, file_drag_group_summary_text, render_wayland_file_drag_icon,
    FileDragIconEntry, FileDragPillPalette,
};

/// 拖出位图中光标与单胶囊回退内容之间的缝隙。
const DRAG_ICON_CURSOR_GAP: f32 = 6.0;

#[derive(Clone, Debug)]
pub(super) struct WaylandDndRuntime {
    pub(super) window_handle: WaylandDndWindowHandle,
    pub(super) controller: Arc<WaylandDndController>,
}

impl WaylandDndRuntime {
    fn new(window_handle: WaylandDndWindowHandle) -> Self {
        Self {
            window_handle,
            controller: WaylandDndController::new(),
        }
    }
}

pub(crate) enum WaylandFileDragRequest {
    Unavailable,
    Requested(WaylandFileDragSessionId),
    Rejected(String),
}

impl FileBrowser {
    pub(in crate::app) fn accept_wayland_dnd_handle(
        &mut self,
        handle: Result<Option<WaylandDndWindowHandle>, String>,
    ) -> Task<Message> {
        match handle {
            Ok(Some(handle)) => {
                tracing::debug!("Wayland drag-and-drop handle loaded");
                self.wayland_dnd = Some(WaylandDndRuntime::new(handle));
            }
            Ok(None) => {
                tracing::debug!("Wayland drag-and-drop unavailable for this window backend");
                self.wayland_dnd = None;
            }
            Err(error) => self.show_global_error(error),
        }
        Task::none()
    }

    /// 拖拽图标配色:与窗口内 drag_preview_panel 同源取自当前主题。
    fn file_drag_pill_palette(&self) -> FileDragPillPalette {
        let theme = self
            .application_theme
            .active(self.user_config.theme_mode, self.user_config.color_scheme);
        let colors = ui_colors(&theme);
        FileDragPillPalette {
            background: colors.surface_bright,
            border: subtle_border_color(&theme),
            content: colors.on_surface,
        }
    }

    /// 拖出位图的条目缩略图:缓存句柄是磁盘 PNG 路径,读出原始字节
    /// 后以 data URI 嵌入位图 SVG;非文件句柄/读取失败回退类型图标。
    fn drag_preview_thumbnail_png(&self, path: &Path) -> Option<Vec<u8>> {
        let handle = self.drag_preview_thumbnail(path)?;
        match handle {
            image::Handle::Path(_, thumbnail_path) => std::fs::read(thumbnail_path).ok(),
            _ => None,
        }
    }

    /// 拖出位图内容:优先用提起时的偏移快照;快照尚未就绪(极快
    /// 交接)时退回按下条目的单胶囊,给光标留一小段缝隙。
    fn file_drag_icon_entries(&self, paths: &[PathBuf]) -> Vec<FileDragIconEntry> {
        if let Some(drag) = self.file_drag.as_ref() {
            if !drag.preview_entries.is_empty() {
                return drag
                    .preview_entries
                    .iter()
                    .map(|entry| FileDragIconEntry {
                        symbol: self.file_drag_icon_symbol(&entry.path),
                        label: file_drag_display_name(&entry.path),
                        offset: entry.offset,
                        thumbnail_png: self.drag_preview_thumbnail_png(&entry.path),
                    })
                    .collect();
            }
        }
        paths
            .first()
            .map(|path| {
                vec![FileDragIconEntry {
                    symbol: self.file_drag_icon_symbol(path),
                    label: file_drag_display_name(path),
                    offset: iced::Vector::new(DRAG_ICON_CURSOR_GAP, DRAG_ICON_CURSOR_GAP),
                    thumbnail_png: self.drag_preview_thumbnail_png(path),
                }]
            })
            .unwrap_or_default()
    }

    pub(crate) fn request_wayland_file_drag(&self, paths: Vec<PathBuf>) -> WaylandFileDragRequest {
        if self.is_trash_view || paths.is_empty() {
            return WaylandFileDragRequest::Unavailable;
        }
        let Some(runtime) = &self.wayland_dnd else {
            tracing::debug!(
                path_count = paths.len(),
                "Wayland file drag skipped because no handle is available"
            );
            return WaylandFileDragRequest::Unavailable;
        };
        let path_count = paths.len();
        let entries = self.file_drag_icon_entries(&paths);
        let (folders, files) = self.file_drag_group_counts();
        let summary = file_drag_group_summary_text(folders, files);
        let icon = match render_wayland_file_drag_icon(
            &entries,
            Some(&summary),
            self.file_drag_pill_palette(),
        ) {
            Ok(icon) => icon,
            Err(error) => {
                tracing::warn!(%error, path_count, "Wayland file drag icon rendering failed");
                return WaylandFileDragRequest::Rejected(format!(
                    "Could not create Wayland file drag feedback: {error}"
                ));
            }
        };

        match runtime.controller.start_file_drag(paths, icon) {
            Ok(session_id) => {
                tracing::debug!(%session_id, path_count, "Wayland file drag request sent");
                WaylandFileDragRequest::Requested(session_id)
            }
            Err(error) => {
                tracing::warn!(%error, path_count, "Wayland file drag request failed");
                WaylandFileDragRequest::Rejected(format!(
                    "Could not start Wayland file drag: {error}"
                ))
            }
        }
    }

    pub(in crate::app) fn accept_wayland_dnd_runtime_failure(
        &mut self,
        error: String,
    ) -> Task<Message> {
        self.wayland_dnd = None;
        self.cancel_file_drag_interaction();
        self.show_global_error(error);
        Task::none()
    }
}
