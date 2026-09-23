//! 预览主面板：按 `PreviewState` 内容分派到各分类型子面板。自 app-ui
//! view/preview_panel.rs 下沉。宿主专属词汇（查看器实例化、Markdown 模式
//! 切换行、SQLite 标签行、拖选结束事件）与七个滚动区域的接线经参数注入；
//! 事件统一以 `PreviewMessage` 表达（宿主 Message 需 `From<PreviewMessage>`）。

use iced::widget::{button, column};
use iced::{Alignment, Element};

use crate::animated_image_preview_panel::animated_image_preview_panel;
use crate::audio_preview_panel::audio_preview_panel;
use crate::document_preview_panel::document_preview_panel;
use crate::engine::SqlitePreviewState;
use crate::image_preview_panel::{
    image_preview_panel, raster_image_preview_panel, svg_preview_panel,
};
use crate::image_preview_viewport::ImagePreviewViewport;
use crate::operation_progress::remote_preview_download_panel;
use crate::panel_text::{localized_text, readable_text};
use crate::preview::PreviewSize;
use crate::preview::VideoPreviewPlayback;
use crate::preview::{AudioPreviewPlayback, ImagePreviewContent};
use crate::preview::{PreviewContent, PreviewState};
use crate::preview_message::PreviewMessage;
use crate::preview_surface::{preview_scroll_height, preview_surface};
use crate::preview_tree_panel::{archive_preview_panel, directory_preview_panel};
use crate::scroll_wiring::ScrollRegionState;
use crate::sqlite_preview_panel::sqlite_preview_panel;
use crate::text_preview::{MarkdownPreviewMode, TextPreviewDocument};
use crate::text_preview_panel::text_preview_panel;
use crate::video_preview_panel::video_preview_panel;

/// 预览窗口/右侧停靠面板共用入口：无预览会话时显示空态提示。
pub fn view_preview_window<'a, Message>(
    preview: Option<&'a PreviewState>,
    text_preview_document: Option<&'a TextPreviewDocument>,
    sqlite_preview_state: Option<&'a SqlitePreviewState>,
    size: PreviewSize,
    image_preview_viewport: &ImagePreviewViewport,
    audio_preview: Option<&'a AudioPreviewPlayback>,
    video_preview: Option<&'a VideoPreviewPlayback>,
    preview_bottom_controls_opacity: f32,
    operation_progress_animation_frame: u8,
    text_preview_content_height: f32,
    directory_scroll: ScrollRegionState<'static, Message>,
    archive_scroll: ScrollRegionState<'static, Message>,
    document_scroll: ScrollRegionState<'static, Message>,
    text_scroll: ScrollRegionState<'a, Message>,
    markdown_scroll: ScrollRegionState<'static, Message>,
    sqlite_tables_scroll: ScrollRegionState<'a, Message>,
    sqlite_data_scroll: ScrollRegionState<'a, Message>,
    plain_viewer: impl Fn(&'a TextPreviewDocument, f32) -> Element<'a, Message> + 'a,
    markdown_mode_switch: impl Fn(MarkdownPreviewMode) -> Element<'static, Message>,
    sqlite_tabs: Element<'static, Message>,
    on_sqlite_resize_drag_finished: impl Fn() -> Message + 'static,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    match preview {
        Some(preview) => preview_panel(
            preview,
            text_preview_document,
            sqlite_preview_state,
            size,
            image_preview_viewport,
            audio_preview,
            video_preview,
            preview_bottom_controls_opacity,
            operation_progress_animation_frame,
            text_preview_content_height,
            directory_scroll,
            archive_scroll,
            document_scroll,
            text_scroll,
            markdown_scroll,
            sqlite_tables_scroll,
            sqlite_data_scroll,
            plain_viewer,
            markdown_mode_switch,
            sqlite_tabs,
            on_sqlite_resize_drag_finished,
        ),
        None => preview_surface(
            localized_text("Select a file and press Space to load preview")
                .size(14)
                .into(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn preview_panel<'a, Message>(
    preview: &'a PreviewState,
    text_preview_document: Option<&'a TextPreviewDocument>,
    sqlite_preview_state: Option<&'a SqlitePreviewState>,
    size: PreviewSize,
    image_preview_viewport: &ImagePreviewViewport,
    audio_preview: Option<&'a AudioPreviewPlayback>,
    video_preview: Option<&'a VideoPreviewPlayback>,
    preview_bottom_controls_opacity: f32,
    operation_progress_animation_frame: u8,
    text_preview_content_height: f32,
    directory_scroll: ScrollRegionState<'static, Message>,
    archive_scroll: ScrollRegionState<'static, Message>,
    document_scroll: ScrollRegionState<'static, Message>,
    text_scroll: ScrollRegionState<'a, Message>,
    markdown_scroll: ScrollRegionState<'static, Message>,
    sqlite_tables_scroll: ScrollRegionState<'a, Message>,
    sqlite_data_scroll: ScrollRegionState<'a, Message>,
    plain_viewer: impl Fn(&'a TextPreviewDocument, f32) -> Element<'a, Message> + 'a,
    markdown_mode_switch: impl Fn(MarkdownPreviewMode) -> Element<'static, Message>,
    sqlite_tabs: Element<'static, Message>,
    on_sqlite_resize_drag_finished: impl Fn() -> Message + 'static,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let scroll_height = preview_scroll_height(size);
    let panel: Element<'a, Message> = match preview {
        PreviewState::Loading(_) => column![readable_text("Loading preview...").size(14)].into(),
        PreviewState::DownloadingRemoteFile(download) => {
            remote_preview_download_panel(download, operation_progress_animation_frame).into()
        }
        PreviewState::Ready(PreviewContent::Directory { entries, .. }) => directory_preview_panel(
            entries,
            scroll_height,
            directory_scroll.visibility,
            directory_scroll.viewport,
            directory_scroll.wiring,
        )
        .into(),
        PreviewState::Ready(PreviewContent::Text {
            path,
            rendered,
            format,
            line_limit_notice,
            ..
        }) => text_preview_panel(
            rendered,
            *format,
            *line_limit_notice,
            text_preview_document
                .filter(|document: &&TextPreviewDocument| document.path() == path.as_path()),
            scroll_height,
            text_preview_content_height,
            text_scroll.visibility,
            text_scroll.viewport,
            markdown_scroll.visibility,
            markdown_scroll.viewport,
            plain_viewer,
            text_scroll.wiring,
            markdown_scroll.wiring,
            markdown_mode_switch,
        )
        .into(),
        PreviewState::Ready(PreviewContent::Archive { entries, .. }) => archive_preview_panel(
            entries,
            scroll_height,
            archive_scroll.visibility,
            archive_scroll.viewport,
            archive_scroll.wiring,
        )
        .into(),
        PreviewState::Ready(PreviewContent::Sqlite(database)) => sqlite_preview_panel(
            database,
            sqlite_preview_state,
            sqlite_tables_scroll.visibility,
            sqlite_tables_scroll.viewport,
            sqlite_data_scroll.visibility,
            sqlite_data_scroll.viewport,
            sqlite_tabs,
            sqlite_tables_scroll.wiring,
            sqlite_data_scroll.wiring,
            on_sqlite_resize_drag_finished,
        ),
        PreviewState::Ready(PreviewContent::PagedDocument(document)) => document_preview_panel(
            document,
            size,
            document_scroll.visibility,
            document_scroll.viewport,
            document_scroll.wiring,
        ),
        PreviewState::Ready(PreviewContent::Image(content)) => match content {
            ImagePreviewContent::Thumbnail {
                handle,
                width,
                height,
                ..
            } => image_preview_panel(handle, *width, *height, size, image_preview_viewport),
            ImagePreviewContent::OriginalRaster {
                raster_handle,
                placeholder_handle,
                width,
                height,
            } => raster_image_preview_panel(
                placeholder_handle,
                raster_handle,
                *width,
                *height,
                size,
                image_preview_viewport,
            ),
            ImagePreviewContent::OriginalSvg {
                handle,
                width,
                height,
                ..
            } => svg_preview_panel(handle, *width, *height, size, image_preview_viewport),
        },
        PreviewState::Ready(PreviewContent::AnimatedImage(preview)) => {
            animated_image_preview_panel(preview, size, preview_bottom_controls_opacity)
        }
        PreviewState::Ready(PreviewContent::Audio {
            path,
            duration,
            len,
        }) => audio_preview_panel(path, *duration, *len, audio_preview).into(),
        PreviewState::Ready(PreviewContent::Video {
            path,
            frame,
            width,
            height,
            duration,
            ..
        }) => video_preview_panel(
            path,
            frame.as_ref(),
            *width,
            *height,
            *duration,
            video_preview,
            size,
            preview_bottom_controls_opacity,
        ),
        PreviewState::Error(error) => column![localized_text(error).size(14)].into(),
        PreviewState::ImageError { path, error } => column![
            localized_text(error).size(14),
            button(localized_text("Retry")).on_press(Message::from(
                PreviewMessage::RetryImagePreview(path.clone(),)
            )),
        ]
        .spacing(10)
        .align_x(Alignment::Center)
        .into(),
    };

    preview_surface(panel)
}
