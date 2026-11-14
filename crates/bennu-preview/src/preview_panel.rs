//! 预览主面板：按 `PreviewState` 内容分派到各分类型子面板。自 app-ui
//! view/preview_panel.rs 下沉。宿主专属词汇（查看器实例化、Markdown 模式
//! 切换行、SQLite 标签行、拖选结束事件）与七个滚动区域的接线经参数注入；
//! 事件统一以 `PreviewMessage` 表达（宿主 Message 需 `From<PreviewMessage>`）。

use bennu_localization::tr;
use bennu_theme::controls::normal_button;
use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::styles::muted_text_color;
use iced::widget::{column, container};
use iced::{Alignment, Element, Length};

use crate::animated_image_preview_panel::animated_image_preview_panel;
use crate::audio_preview_panel::{audio_preview_panel, AudioPanelInput};
use crate::document_preview_panel::document_preview_panel;
use crate::engine::SqlitePreviewState;
use crate::image_preview_panel::{
    image_preview_panel, raster_image_preview_panel, svg_preview_panel,
};
use crate::image_preview_viewport::ImagePreviewViewport;
use crate::media_keys::media_keys;
use crate::operation_progress::remote_preview_download_panel;
use crate::panel_text::readable_text;
use crate::preview::PreviewSize;
use crate::preview::VideoPreviewPlayback;
use crate::preview::{AudioAnalysis, AudioPreviewPlayback, ImagePreviewContent};
use crate::preview::{PreviewContent, PreviewState};
use crate::preview_message::PreviewMessage;
use crate::preview_surface::preview_surface;
use crate::preview_tree_panel::{archive_preview_panel, directory_preview_panel};
use crate::scroll_wiring::ScrollRegionWiring;
use crate::sqlite_preview_panel::sqlite_preview_panel;
use crate::text_highlight::HighlightPalette;
use crate::text_preview::TextPreviewDocument;
use crate::text_preview_panel::text_preview_panel;
use crate::text_view::TextViewSettings;
use crate::video_preview_panel::video_preview_panel;

/// 预览窗口/右侧停靠面板共用入口：无预览会话时显示空态提示。
// 参数集合是共享预览状态到视图的完整投影，镜像 bennu-preview 内部各面板签名
#[allow(clippy::too_many_arguments)]
pub fn view_preview_window<'a, Message>(
    preview: Option<&'a PreviewState>,
    text_preview_document: Option<&'a TextPreviewDocument>,
    sqlite_preview_state: Option<&'a SqlitePreviewState>,
    size: PreviewSize,
    image_preview_viewport: &ImagePreviewViewport,
    audio_preview: Option<&'a AudioPreviewPlayback>,
    audio_analysis: Option<&'a AudioAnalysis>,
    video_preview: Option<&'a VideoPreviewPlayback>,
    media_volume: f32,
    preview_bottom_controls_opacity: f32,
    operation_progress_animation_frame: u8,
    directory_scroll: ScrollRegionWiring<'static, Message>,
    archive_scroll: ScrollRegionWiring<'static, Message>,
    document_scroll: ScrollRegionWiring<'static, Message>,
    markdown_scroll: ScrollRegionWiring<'a, Message>,
    sqlite_tables_scroll: ScrollRegionWiring<'a, Message>,
    sqlite_data_scroll: ScrollRegionWiring<'a, Message>,
    text_view: TextViewSettings,
    text_palette: HighlightPalette,
    plain_viewer: impl Fn(&'a TextPreviewDocument, TextViewSettings) -> Element<'a, Message> + 'a,
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
            audio_analysis,
            video_preview,
            media_volume,
            preview_bottom_controls_opacity,
            operation_progress_animation_frame,
            directory_scroll,
            archive_scroll,
            document_scroll,
            markdown_scroll,
            sqlite_tables_scroll,
            sqlite_data_scroll,
            text_view,
            text_palette,
            plain_viewer,
            sqlite_tabs,
            on_sqlite_resize_drag_finished,
        ),
        None => preview_surface(
            readable_text(tr!("Select a file and press Space to load preview")).into(),
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
    audio_analysis: Option<&'a AudioAnalysis>,
    video_preview: Option<&'a VideoPreviewPlayback>,
    media_volume: f32,
    preview_bottom_controls_opacity: f32,
    operation_progress_animation_frame: u8,
    directory_scroll: ScrollRegionWiring<'static, Message>,
    archive_scroll: ScrollRegionWiring<'static, Message>,
    document_scroll: ScrollRegionWiring<'static, Message>,
    markdown_scroll: ScrollRegionWiring<'a, Message>,
    sqlite_tables_scroll: ScrollRegionWiring<'a, Message>,
    sqlite_data_scroll: ScrollRegionWiring<'a, Message>,
    text_view: TextViewSettings,
    text_palette: HighlightPalette,
    plain_viewer: impl Fn(&'a TextPreviewDocument, TextViewSettings) -> Element<'a, Message> + 'a,
    sqlite_tabs: Element<'static, Message>,
    on_sqlite_resize_drag_finished: impl Fn() -> Message + 'static,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let panel: Element<'a, Message> = match preview {
        PreviewState::Loading(_) => column![readable_text(tr!("Loading preview..."))].into(),
        PreviewState::DownloadingRemoteFile(download) => {
            remote_preview_download_panel(download, operation_progress_animation_frame).into()
        }
        PreviewState::Ready(PreviewContent::Directory { entries, .. }) => {
            directory_preview_panel(entries, directory_scroll).into()
        }
        PreviewState::Ready(PreviewContent::Text { path, .. }) => text_preview_panel(
            text_preview_document
                .filter(|document: &&TextPreviewDocument| document.path() == path.as_path()),
            text_view,
            text_palette,
            plain_viewer,
            markdown_scroll,
        )
        .into(),
        PreviewState::Ready(PreviewContent::Archive { entries, .. }) => {
            archive_preview_panel(entries, archive_scroll).into()
        }
        PreviewState::Ready(PreviewContent::Sqlite(database)) => sqlite_preview_panel(
            database,
            sqlite_preview_state,
            sqlite_tabs,
            sqlite_tables_scroll,
            sqlite_data_scroll,
            on_sqlite_resize_drag_finished,
        ),
        PreviewState::Ready(PreviewContent::PagedDocument(document)) => {
            document_preview_panel(document, size, document_scroll)
        }
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
            details,
            cover,
            lyrics,
        }) => audio_preview_panel(AudioPanelInput {
            path,
            details,
            cover: cover.as_ref(),
            lyrics: lyrics.as_deref(),
            playback: audio_preview,
            analysis: audio_analysis,
            media_volume,
            size,
        }),
        PreviewState::Ready(PreviewContent::FfmpegRequired { .. }) => ffmpeg_required_panel(),
        PreviewState::Ready(PreviewContent::Video {
            path,
            frame,
            width,
            height,
            duration,
            ..
        }) => media_keys(video_preview_panel(
            path,
            frame.as_ref(),
            *width,
            *height,
            *duration,
            video_preview,
            media_volume,
            size,
            preview_bottom_controls_opacity,
        )),
        PreviewState::Error(error) => preview_error_panel(error.clone(), None),
        PreviewState::ImageError { path, error } => preview_error_panel(
            error.clone(),
            Some(
                normal_button(readable_text(tr!("Retry")))
                    .on_press(Message::from(PreviewMessage::RetryImagePreview(
                        path.clone(),
                    )))
                    .into(),
            ),
        ),
    };

    preview_surface(panel)
}

/// 预览失败（二进制、不支持的类型、读取失败）：居中，上面一个警告图标，文字用次要色、
/// 窄一点自动换行，不再贴在左上角。
fn ffmpeg_required_panel<'a, Message>() -> Element<'a, Message>
where
    Message: 'a + Clone + From<PreviewMessage>,
{
    match crate::ffmpeg_install::ffmpeg_install_command() {
        Some(command) => preview_error_panel(
            tr!("Audio and video previews need ffmpeg. Install it with:\n%1", command),
            Some(
                normal_button(readable_text(tr!("Copy Command")))
                    .on_press(Message::from(PreviewMessage::CopyFfmpegInstallCommand))
                    .into(),
            ),
        ),
        None => preview_error_panel(
            tr!("Audio and video previews need ffmpeg. Install it with your system's package manager."),
            None,
        ),
    }
}

fn preview_error_panel<'a, Message: 'a>(
    error: String,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut content = column![
        themed_icon(IconSymbol::TriangleAlert, IconTone::Warning, 32.0),
        readable_text(error)
            .style(|theme: &iced::Theme| iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            })
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Shrink),
    ]
    .spacing(12)
    .max_width(420)
    .align_x(Alignment::Center);
    if let Some(action) = action {
        content = content.push(action);
    }
    container(content).center(Length::Fill).into()
}
