use std::path::{Path, PathBuf};
use std::sync::Arc;

use desktop_linux::{
    WaylandDndController, WaylandDndWindowHandle, WaylandFileDragIcon, WaylandFileDragSessionId,
};
use iced::widget::image;
use iced::Task;

use super::FileBrowser;
use crate::appearance::subtle_border_color;
use crate::matugen_theme::ui_colors;
use crate::model::{FileDragGestureId, Message};
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

/// 位图渲染放到后台线程:缩略图 PNG 读盘与 SVG 光栅化都不占主线程,
/// 预渲染与激活兜底共用同一个渲染入口。
async fn render_wayland_drag_icon_offline(
    entries: Vec<FileDragIconEntry>,
    summary: String,
    palette: FileDragPillPalette,
) -> Result<WaylandFileDragIcon, String> {
    tokio::task::spawn_blocking(move || {
        render_wayland_file_drag_icon(&entries, Some(&summary), palette)
    })
    .await
    .map_err(|error| format!("could not join drag icon render task: {error}"))?
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

    /// 拖出位图的条目缩略图:缓存句柄是磁盘 PNG 路径,渲染时才读取并
    /// 以 data URI 嵌入位图 SVG;非文件句柄回退类型图标。收集输入的
    /// 主线程不做 IO,读盘留给渲染线程。
    fn drag_preview_thumbnail_path(&self, path: &Path) -> Option<PathBuf> {
        match self.drag_preview_thumbnail(path)? {
            image::Handle::Path(_, thumbnail_path) => Some(thumbnail_path),
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
                        thumbnail_png_path: self.drag_preview_thumbnail_path(&entry.path),
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
                    thumbnail_png_path: self.drag_preview_thumbnail_path(path),
                }]
            })
            .unwrap_or_default()
    }

    /// 取走当前手势预渲染就绪的拖出位图;未就绪(极速甩动)或无手势
    /// 时返回 None,由调用方回退现场生成。
    fn take_wayland_drag_icon(&mut self) -> Option<WaylandFileDragIcon> {
        self.file_drag
            .as_mut()
            .and_then(|drag| drag.wayland_drag_icon.take())
    }

    pub(crate) fn request_wayland_file_drag(
        &mut self,
        paths: Vec<PathBuf>,
    ) -> WaylandFileDragRequest {
        if self.is_trash_view || paths.is_empty() {
            return WaylandFileDragRequest::Unavailable;
        }
        // 包内成员是应用内虚拟路径,外部应用拿到的只是一串不存在的
        // 文件名,拖出去必然失败。不把虚拟路径交给外部:返回
        // Unavailable 让调用方回退应用内拖拽,窗口内松手仍走提取管线。
        if paths.iter().any(|path| {
            file_core::archive_path_identity(path) != file_core::ArchivePathIdentity::RealFile
        }) {
            tracing::debug!(
                path_count = paths.len(),
                "Wayland file drag skipped because a source is inside an archive"
            );
            return WaylandFileDragRequest::Unavailable;
        }
        let Some(runtime) = &self.wayland_dnd else {
            tracing::debug!(
                path_count = paths.len(),
                "Wayland file drag skipped because no handle is available"
            );
            return WaylandFileDragRequest::Unavailable;
        };
        // Arc 先取走:生成位图需要 &mut self(消费就绪位图),不能与
        // runtime 借用共存。
        let controller = runtime.controller.clone();
        let path_count = paths.len();
        // 按下后预渲染的位图就绪则直接消费,激活帧零生成;未就绪(极
        // 速甩动)回退现场同步生成,输入仍取自当前快照,行为不回归。
        let icon = match self.take_wayland_drag_icon() {
            Some(icon) => icon,
            None => {
                let entries = self.file_drag_icon_entries(&paths);
                let (folders, files) = self.file_drag_group_counts();
                let summary = file_drag_group_summary_text(folders, files);
                match render_wayland_file_drag_icon(
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
                }
            }
        };

        match controller.start_file_drag(paths, icon) {
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

    /// 按下后 bounds 快照回填 preview_entries 的那一刻发起的位图预渲
    /// 染:输入在主线程取齐(按下时刻快照),PNG 读盘与 SVG 渲染全部
    /// 在后台线程。按下→激活之间恰好新生成的缩略图不追补(用户已确
    /// 认的取舍);结果按 gesture_id 回存,迟到结果由接受侧丢弃。
    pub(in crate::app) fn preload_wayland_drag_icon(&self) -> Task<Message> {
        let Some(drag) = self.file_drag.as_ref() else {
            return Task::none();
        };
        let gesture_id = drag.gesture_id;
        let paths = drag.sources.clone();
        let entries = self.file_drag_icon_entries(&paths);
        let (folders, files) = self.file_drag_group_counts();
        let summary = file_drag_group_summary_text(folders, files);
        let palette = self.file_drag_pill_palette();
        Task::perform(
            render_wayland_drag_icon_offline(entries, summary, palette),
            move |icon| Message::WaylandDragIconReady { gesture_id, icon },
        )
    }

    /// 预渲染位图就绪:手势匹配才存入手势状态;拖拽已取消或手势更替
    /// 的迟到结果直接丢弃。渲染失败不在此报错——激活兜底路径会重新生
    /// 成一次,错误在那里统一呈现,避免双份提示。
    pub(in crate::app) fn accept_wayland_drag_icon_ready(
        &mut self,
        gesture_id: FileDragGestureId,
        icon: Result<WaylandFileDragIcon, String>,
    ) -> Task<Message> {
        let Ok(icon) = icon else {
            tracing::debug!("Wayland drag icon pre-render failed; activation renders on demand");
            return Task::none();
        };
        let Some(drag) = self.file_drag.as_mut() else {
            return Task::none();
        };
        if drag.gesture_id != gesture_id {
            return Task::none();
        }
        drag.wayland_drag_icon = Some(icon);
        Task::none()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileDragStationaryAction;

    fn browser_with_pressed_drag(browser: &mut FileBrowser) {
        let source = PathBuf::from("/workspace/report.txt");
        browser.selected = Some(source.clone());
        browser.cursor_position = iced::Point::new(0.0, 0.0);
        drop(browser.start_file_drag(source, FileDragStationaryAction::SelectionOnly, Vec::new()));
    }

    fn browser_with_wayland_runtime() -> FileBrowser {
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        browser.wayland_dnd = Some(WaylandDndRuntime {
            window_handle: WaylandDndWindowHandle::new(0x1, 0x2),
            controller: WaylandDndController::new(),
        });
        browser
    }

    fn test_icon() -> WaylandFileDragIcon {
        WaylandFileDragIcon::new(1, 1, vec![10, 20, 30, 255]).expect("test icon")
    }

    #[test]
    fn late_drag_icon_ready_is_discarded_when_gesture_changes() {
        let mut browser = browser_with_wayland_runtime();
        browser_with_pressed_drag(&mut browser);
        let first_gesture = browser.file_drag.as_ref().unwrap().gesture_id;

        // 手势匹配:位图存入,激活交接时可直接消费。
        drop(browser.accept_wayland_drag_icon_ready(first_gesture, Ok(test_icon())));
        assert!(browser
            .file_drag
            .as_ref()
            .is_some_and(|drag| drag.wayland_drag_icon.is_some()));

        // 手势结束后重新按下:旧 gesture_id 的迟到结果不得落入新手势。
        drop(browser.finish_drag_selection(None));
        assert!(browser.file_drag.is_none());
        browser_with_pressed_drag(&mut browser);
        let second_gesture = browser.file_drag.as_ref().unwrap().gesture_id;
        assert_ne!(first_gesture, second_gesture);
        drop(browser.accept_wayland_drag_icon_ready(first_gesture, Ok(test_icon())));
        assert!(browser
            .file_drag
            .as_ref()
            .is_some_and(|drag| drag.wayland_drag_icon.is_none()));

        // 拖拽取消后(无手势):迟到结果同样无处落,不 panic。
        browser.cancel_file_drag_interaction();
        drop(browser.accept_wayland_drag_icon_ready(second_gesture, Ok(test_icon())));
    }

    #[test]
    fn archive_member_paths_are_never_handed_to_native_drag() {
        let mut browser = browser_with_wayland_runtime();
        let directory = tempfile::tempdir().unwrap();
        // 归档边界只需真实存在的归档文件,成员按路径形态判定。
        let archive = directory.path().join("docs.zip");
        std::fs::write(&archive, b"payload").unwrap();
        let member = archive.join("photos/1.txt");

        // 包内成员不发起原生交接:返回 Unavailable,调用方回退应用内
        // 拖拽,窗口内落地仍走提取管线。
        assert!(matches!(
            browser.request_wayland_file_drag(vec![member]),
            WaylandFileDragRequest::Unavailable
        ));

        // 真实路径不受影响,照常请求原生会话。
        let real = directory.path().join("report.txt");
        std::fs::write(&real, b"data").unwrap();
        assert!(matches!(
            browser.request_wayland_file_drag(vec![real]),
            WaylandFileDragRequest::Requested(_)
        ));
    }
}
