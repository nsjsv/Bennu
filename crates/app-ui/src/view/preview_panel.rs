use crate::app::preview_state::SqlitePreviewState;
use std::path::Path;
use std::time::Duration;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, column, container, image, mouse_area, progress_bar, row, scrollable, slider, svg,
    Button, Column, Space, Stack,
};
use iced::{Alignment, Background, Border, Element, Length, Theme, Vector};

use crate::animated_image_preview::AnimatedImagePreview;
use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::appearance::{
    app_content_style, base_text_color, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, navigation_icon_button_style, preview_media_style,
    preview_window_bottom_gradient_style,
};
use crate::formatting::{format_duration, format_file_size, format_middle_ellipsized_text};
use crate::icons::{preview_entry_icon_symbol, rotated_chevron_right_view, IconSymbol};
use crate::matugen_theme::ui_colors;
use crate::model::{
    image_preview_size, scaled_media_size, AudioPreviewPlayback, AudioPreviewPlaybackStatus,
    ImagePreviewContent, ImagePreviewViewport, Message, PreviewContent,
    PreviewImageViewportMessage, PreviewSize, PreviewState, PreviewTreeDirectoryChildren,
    PreviewTreeEntry, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility, TextPreviewDocument,
    VideoPreviewPlayback, VideoPreviewPlaybackStatus,
};
use crate::operation_progress::remote_preview_download_panel;
use crate::translated_surface::translated_surface;
use crate::typography::{localized_text, readable_text};

use super::{
    document_preview_panel::document_preview_panel, icon_tone_style,
    text_preview_panel::text_preview_panel, themed_icon, IconTone,
};

const PREVIEW_MIN_SCROLL_HEIGHT: f32 = 160.0;
const PREVIEW_ICON_SIZE: f32 = 16.0;
const PREVIEW_ENTRY_NAME_MAX_CHARS: usize = 48;
const PREVIEW_TREE_INDENT_WIDTH: f32 = 18.0;
const PREVIEW_TREE_TOGGLE_WIDTH: f32 = 16.0;
const PREVIEW_TREE_TOGGLE_ROTATION_DEGREES: f32 = 90.0;
const AUDIO_PREVIEW_CONTROL_HEIGHT: f32 = 92.0;
const AUDIO_CONTROL_BUTTON_SIZE: f32 = 30.0;
const AUDIO_CONTROL_ICON_SIZE: f32 = 14.0;
const AUDIO_TIMELINE_CONTROL_GAP: f32 = 10.0;
const AUDIO_PROGRESS_SLIDER_WIDTH: f32 = 280.0;
const AUDIO_PROGRESS_SLIDER_STEP_SECONDS: f32 = 0.05;
const AUDIO_VOLUME_SLIDER_WIDTH: f32 = 100.0;
const AUDIO_VOLUME_SLIDER_STEP: f32 = 0.01;
const VIDEO_PREVIEW_CONTROL_HEIGHT: f32 = 88.0;
const VIDEO_PROGRESS_SLIDER_PORTION: u16 = 4;
const VIDEO_VOLUME_SLIDER_PORTION: u16 = 1;
const VIDEO_CONTROL_SLIDER_GAP: f32 = 14.0;
const VIDEO_VOLUME_ICON_GAP: f32 = 6.0;
const VIDEO_CONTROL_HORIZONTAL_PADDING: u16 = 16;
const ANIMATED_IMAGE_CONTROL_SIDE_PADDING: f32 = 28.0;
const ANIMATED_IMAGE_MIN_CONTROL_WIDTH: f32 = 220.0;
const MINI_PROGRESS_BAR_HEIGHT: f32 = 3.0;

pub(crate) fn view_preview_window<'a>(
    preview: Option<&'a PreviewState>,
    text_preview_document: Option<&'a TextPreviewDocument>,
    sqlite_preview_state: Option<&'a SqlitePreviewState>,
    size: PreviewSize,
    image_preview_viewport: &ImagePreviewViewport,
    audio_preview: Option<&'a AudioPreviewPlayback>,
    video_preview: Option<&'a VideoPreviewPlayback>,
    preview_bottom_controls_opacity: f32,
    operation_progress_animation_frame: u8,
    directory_scrollbar_visibility: ScrollbarVisibility,
    directory_scrollbar_viewport: Option<ScrollbarViewport>,
    archive_scrollbar_visibility: ScrollbarVisibility,
    archive_scrollbar_viewport: Option<ScrollbarViewport>,
    document_scrollbar_visibility: ScrollbarVisibility,
    document_scrollbar_viewport: Option<ScrollbarViewport>,
    text_scrollbar_visibility: ScrollbarVisibility,
    text_scrollbar_viewport: Option<ScrollbarViewport>,
    text_preview_content_height: f32,
    markdown_scrollbar_visibility: ScrollbarVisibility,
    markdown_scrollbar_viewport: Option<ScrollbarViewport>,
    sqlite_tables_visibility: ScrollbarVisibility,
    sqlite_tables_viewport: Option<ScrollbarViewport>,
    sqlite_data_visibility: ScrollbarVisibility,
    sqlite_data_viewport: Option<ScrollbarViewport>,
) -> Element<'a, Message> {
    preview
        .map(|preview| {
            preview_panel(
                preview,
                text_preview_document,
                sqlite_preview_state,
                size,
                image_preview_viewport,
                audio_preview,
                video_preview,
                preview_bottom_controls_opacity,
                operation_progress_animation_frame,
                directory_scrollbar_visibility,
                directory_scrollbar_viewport,
                archive_scrollbar_visibility,
                archive_scrollbar_viewport,
                document_scrollbar_visibility,
                document_scrollbar_viewport,
                text_scrollbar_visibility,
                text_scrollbar_viewport,
                text_preview_content_height,
                markdown_scrollbar_visibility,
                markdown_scrollbar_viewport,
                sqlite_tables_visibility,
                sqlite_tables_viewport,
                sqlite_data_visibility,
                sqlite_data_viewport,
            )
        })
        .unwrap_or_else(|| {
            preview_surface(
                localized_text("Select a file and press Space to load preview")
                    .size(14)
                    .into(),
            )
        })
}

fn preview_panel<'a>(
    preview: &'a PreviewState,
    text_preview_document: Option<&'a TextPreviewDocument>,
    sqlite_preview_state: Option<&'a SqlitePreviewState>,
    size: PreviewSize,
    image_preview_viewport: &ImagePreviewViewport,
    audio_preview: Option<&'a AudioPreviewPlayback>,
    video_preview: Option<&'a VideoPreviewPlayback>,
    preview_bottom_controls_opacity: f32,
    operation_progress_animation_frame: u8,
    directory_scrollbar_visibility: ScrollbarVisibility,
    directory_scrollbar_viewport: Option<ScrollbarViewport>,
    archive_scrollbar_visibility: ScrollbarVisibility,
    archive_scrollbar_viewport: Option<ScrollbarViewport>,
    document_scrollbar_visibility: ScrollbarVisibility,
    document_scrollbar_viewport: Option<ScrollbarViewport>,
    text_scrollbar_visibility: ScrollbarVisibility,
    text_scrollbar_viewport: Option<ScrollbarViewport>,
    text_preview_content_height: f32,
    markdown_scrollbar_visibility: ScrollbarVisibility,
    markdown_scrollbar_viewport: Option<ScrollbarViewport>,
    sqlite_tables_visibility: ScrollbarVisibility,
    sqlite_tables_viewport: Option<ScrollbarViewport>,
    sqlite_data_visibility: ScrollbarVisibility,
    sqlite_data_viewport: Option<ScrollbarViewport>,
) -> Element<'a, Message> {
    let scroll_height = preview_scroll_height(size);
    let panel: Element<'a, Message> = match preview {
        PreviewState::Loading(_) => column![readable_text("Loading preview...").size(14)].into(),
        PreviewState::DownloadingRemoteFile(download) => {
            remote_preview_download_panel(download, operation_progress_animation_frame).into()
        }
        PreviewState::Ready(PreviewContent::Directory { entries, .. }) => directory_preview_panel(
            entries,
            scroll_height,
            directory_scrollbar_visibility,
            directory_scrollbar_viewport,
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
            text_preview_document.filter(|document| document.path() == path.as_path()),
            scroll_height,
            text_preview_content_height,
            text_scrollbar_visibility,
            text_scrollbar_viewport,
            markdown_scrollbar_visibility,
            markdown_scrollbar_viewport,
        )
        .into(),
        PreviewState::Ready(PreviewContent::Archive { entries, .. }) => archive_preview_panel(
            entries,
            scroll_height,
            archive_scrollbar_visibility,
            archive_scrollbar_viewport,
        )
        .into(),
        PreviewState::Ready(PreviewContent::Sqlite(database)) => {
            super::sqlite_preview_panel::sqlite_preview_panel(
                database,
                sqlite_preview_state,
                sqlite_tables_visibility,
                sqlite_tables_viewport,
                sqlite_data_visibility,
                sqlite_data_viewport,
            )
        }
        PreviewState::Ready(PreviewContent::PagedDocument(document)) => document_preview_panel(
            document,
            size,
            document_scrollbar_visibility,
            document_scrollbar_viewport,
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
            button(localized_text("Retry")).on_press(Message::RetryImagePreview(path.clone())),
        ]
        .spacing(10)
        .align_x(Alignment::Center)
        .into(),
    };

    preview_surface(panel)
}

fn preview_surface<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_content_style)
        .into()
}

fn preview_scroll_height(size: PreviewSize) -> f32 {
    size.height.max(PREVIEW_MIN_SCROLL_HEIGHT)
}

fn directory_preview_panel(
    entries: &[PreviewTreeEntry],
    scroll_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Column<'static, Message> {
    let listing = preview_tree_listing(entries, "Empty directory");
    let scroll_region = ScrollbarRegion::PreviewDirectory;
    let scroller = scrollable(smooth_scroll_content(listing, scroll_region.clone()))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .height(Length::Fixed(scroll_height))
        .on_scroll(scrollbar_on_scroll(scroll_region.clone(), |_| {
            Message::PreviewDirectoryScrolled
        }));
    let scroller = enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        6.0,
    );

    column![scroller]
}

fn archive_preview_panel(
    entries: &[PreviewTreeEntry],
    scroll_height: f32,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Column<'static, Message> {
    let listing = preview_tree_listing(entries, "Empty archive");
    let scroll_region = ScrollbarRegion::PreviewArchive;
    let scroller = scrollable(smooth_scroll_content(listing, scroll_region.clone()))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .height(Length::Fixed(scroll_height))
        .on_scroll(scrollbar_on_scroll(scroll_region.clone(), |_| {
            Message::PreviewArchiveScrolled
        }));
    let scroller = enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        6.0,
    );

    column![scroller]
}

fn preview_tree_listing(
    entries: &[PreviewTreeEntry],
    empty_message: &'static str,
) -> Column<'static, Message> {
    let mut listing = Column::new().spacing(3);
    if entries.is_empty() {
        return listing.push(readable_text(empty_message).size(14));
    }

    for entry in visible_preview_tree_entries(entries) {
        listing = listing.push(preview_tree_entry_row(entry));
        if let Some(message) = preview_tree_directory_status_message(entry) {
            listing = listing.push(preview_tree_status_row(entry, message));
        }
    }

    listing
}

fn preview_tree_directory_status_message(entry: &PreviewTreeEntry) -> Option<String> {
    if !entry.is_expanded {
        return None;
    }

    match entry.directory_children.as_ref()? {
        PreviewTreeDirectoryChildren::Error(error) => Some(format!("Could not load: {error}")),
        PreviewTreeDirectoryChildren::Loading
        | PreviewTreeDirectoryChildren::Pending
        | PreviewTreeDirectoryChildren::Loaded => None,
    }
}

fn visible_preview_tree_entries(entries: &[PreviewTreeEntry]) -> Vec<&PreviewTreeEntry> {
    entries
        .iter()
        .filter(|entry| preview_tree_entry_visible(entry, entries))
        .collect()
}

fn preview_tree_entry_visible(entry: &PreviewTreeEntry, entries: &[PreviewTreeEntry]) -> bool {
    let mut parent = entry.parent;
    while let Some(parent_id) = parent {
        let Some(parent_entry) = entries.get(parent_id) else {
            return false;
        };
        if !(parent_entry.is_expanded || parent_entry.toggle_rotation_progress > 0.0) {
            return false;
        }
        parent = parent_entry.parent;
    }

    true
}

fn preview_tree_entry_row(entry: &PreviewTreeEntry) -> Element<'static, Message> {
    let name = format_middle_ellipsized_text(&entry.name, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let indent = Space::new().width(Length::Fixed(
        entry.depth as f32 * PREVIEW_TREE_INDENT_WIDTH,
    ));
    let toggle: Element<'static, Message> = if entry.is_directory() {
        container(
            rotated_chevron_right_view(
                entry.toggle_rotation_progress * PREVIEW_TREE_TOGGLE_ROTATION_DEGREES,
                PREVIEW_TREE_TOGGLE_WIDTH,
            )
            .style(icon_tone_style(IconTone::Normal)),
        )
        .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .height(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .center_x(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .center_y(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
        .into()
    } else {
        Space::new()
            .width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH))
            .into()
    };
    let row_content = row![
        indent,
        toggle,
        themed_icon(
            preview_entry_icon_symbol(entry.kind, &entry.name),
            IconTone::Normal,
            PREVIEW_ICON_SIZE,
        ),
        readable_text(name).size(14).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let row_container = container(row_content).padding([3, 6]).width(Length::Fill);

    if entry.is_directory() {
        mouse_area(row_container)
            .on_press(Message::PreviewTreeDirectoryToggled(entry.id))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    } else {
        row_container.into()
    }
}

fn preview_tree_status_row(entry: &PreviewTreeEntry, message: String) -> Element<'static, Message> {
    let message = format_middle_ellipsized_text(&message, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let indent = Space::new().width(Length::Fixed(
        (entry.depth + 1) as f32 * PREVIEW_TREE_INDENT_WIDTH,
    ));
    let row_content = row![
        indent,
        Space::new().width(Length::Fixed(PREVIEW_TREE_TOGGLE_WIDTH)),
        Space::new().width(Length::Fixed(PREVIEW_ICON_SIZE)),
        localized_text(message).size(13).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(row_content)
        .padding([3, 6])
        .width(Length::Fill)
        .into()
}

fn image_preview_panel(
    handle: &image::Handle,
    width: u32,
    height: u32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
) -> Element<'static, Message> {
    let (fit_width, fit_height) = image_preview_size(size, width, height);
    zoomable_media_area(
        fit_width,
        fit_height,
        size,
        viewport,
        |image_width, image_height| preview_image_frame(handle, image_width, image_height).into(),
    )
}

fn raster_image_preview_panel(
    placeholder_handle: &image::Handle,
    raster_handle: &image::Handle,
    width: u32,
    height: u32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
) -> Element<'static, Message> {
    let (fit_width, fit_height) = image_preview_size(size, width, height);
    zoomable_media_area(
        fit_width,
        fit_height,
        size,
        viewport,
        |image_width, image_height| {
            let image = image::Image::new(raster_handle.clone())
                .width(Length::Fixed(image_width))
                .height(Length::Fixed(image_height))
                .content_fit(iced::ContentFit::Contain);
            let placeholder = image::Image::new(placeholder_handle.clone())
                .width(Length::Fixed(image_width))
                .height(Length::Fixed(image_height))
                .content_fit(iced::ContentFit::Contain);
            Stack::new()
                .width(Length::Fixed(image_width))
                .height(Length::Fixed(image_height))
                .push(placeholder)
                .push(image)
                .into()
        },
    )
}

fn svg_preview_panel(
    handle: &svg::Handle,
    width: u32,
    height: u32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
) -> Element<'static, Message> {
    let (fit_width, fit_height) = image_preview_size(size, width, height);
    zoomable_media_area(
        fit_width,
        fit_height,
        size,
        viewport,
        |render_width, render_height| {
            svg::Svg::new(handle.clone())
                .width(Length::Fixed(render_width))
                .height(Length::Fixed(render_height))
                .content_fit(iced::ContentFit::Contain)
                .into()
        },
    )
}

/// 静态图片媒体区：适应窗口时居中；缩放后按视口位移摆放，
/// 并用 mouse_area 提供滚轮缩放、拖动平移与双击重置。
fn zoomable_media_area(
    fit_width: f32,
    fit_height: f32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
    content_at_scaled_size: impl Fn(f32, f32) -> Element<'static, Message>,
) -> Element<'static, Message> {
    let media: Element<'static, Message> = if viewport.is_zoomed() {
        let scaled_width = fit_width * viewport.scale;
        let scaled_height = fit_height * viewport.scale;
        let x = size.width / 2.0 + viewport.offset.x - scaled_width / 2.0;
        let y = size.height / 2.0 + viewport.offset.y - scaled_height / 2.0;
        translated_surface(
            content_at_scaled_size(scaled_width, scaled_height),
            Vector::new(x, y),
        )
    } else {
        container(content_at_scaled_size(fit_width, fit_height))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    };

    mouse_area(
        container(media)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(preview_media_style),
    )
    .on_move(|position| {
        Message::PreviewImageViewport(PreviewImageViewportMessage::PointerMoved(position))
    })
    .on_press(Message::PreviewImageViewport(
        PreviewImageViewportMessage::PanStarted,
    ))
    .on_release(Message::PreviewImageViewport(
        PreviewImageViewportMessage::PanEnded,
    ))
    .on_double_click(Message::PreviewImageViewport(
        PreviewImageViewportMessage::ResetRequested,
    ))
    .on_scroll(|delta| Message::PreviewImageViewport(PreviewImageViewportMessage::Zoomed(delta)))
    .interaction(iced::mouse::Interaction::Grab)
    .into()
}

fn animated_image_preview_panel(
    preview: &AnimatedImagePreview,
    size: PreviewSize,
    preview_bottom_controls_opacity: f32,
) -> Element<'static, Message> {
    let (image_width, image_height) = image_preview_size(size, preview.width(), preview.height());
    let mut frames = Stack::new()
        .width(Length::Fixed(image_width))
        .height(Length::Fixed(image_height));

    if let Some(handle) = preview.previous_frame_handle() {
        frames = frames.push(preview_image_frame(handle, image_width, image_height));
    }

    frames = frames.push(preview_image_frame(
        preview.current_frame_handle(),
        image_width,
        image_height,
    ));

    let frame_view: Element<'static, Message> = container(frames)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(preview_media_style)
        .into();
    let effective_opacity =
        animated_image_controls_opacity_for_preview(preview, preview_bottom_controls_opacity);

    let mini_progress_opacity = mini_progress_opacity_for_controls_opacity(effective_opacity);
    let mut overlay = Stack::with_children([frame_view])
        .width(Length::Fill)
        .height(Length::Fill);
    if mini_progress_opacity > f32::EPSILON {
        if let Some(fraction) = animated_image_progress_fraction(preview) {
            overlay = overlay.push(mini_progress_bar_layer(fraction, mini_progress_opacity));
        }
    }

    if effective_opacity > f32::EPSILON {
        let Some(controls) = animated_image_controls(preview, size, image_width, effective_opacity)
        else {
            return overlay.into();
        };
        let gradient: Element<'static, Message> = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme| preview_window_bottom_gradient_style(theme, effective_opacity))
            .into();
        let controls: Element<'static, Message> = container(controls)
            .width(Length::Fill)
            .height(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT))
            .center_x(Length::Fill)
            .center_y(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT))
            .into();
        let bottom_controls: Element<'static, Message> = container(
            Stack::with_children([gradient, controls])
                .width(Length::Fill)
                .height(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Bottom)
        .into();
        overlay = overlay.push(bottom_controls);
    }

    overlay.into()
}

fn animated_image_controls(
    preview: &AnimatedImagePreview,
    size: PreviewSize,
    image_width: f32,
    opacity: f32,
) -> Option<Element<'static, Message>> {
    let duration = preview.playback_duration()?;
    let opacity = opacity.clamp(0.0, 1.0);
    let width = animated_image_control_width(size, image_width);
    let position = preview.playback_position().min(duration);
    let duration_seconds = duration
        .as_secs_f32()
        .max(AUDIO_PROGRESS_SLIDER_STEP_SECONDS);
    let position_seconds = position.as_secs_f32().min(duration_seconds);
    let progress_slider = slider(
        0.0..=duration_seconds,
        position_seconds,
        Message::AnimatedImageSeekRequested,
    )
    .step(AUDIO_PROGRESS_SLIDER_STEP_SECONDS)
    .on_release(Message::AnimatedImageSeekCommitted)
    .width(Length::Fixed(width))
    .style(move |theme, status| faded_video_slider_style(theme, status, opacity));
    let position_text = readable_text(animated_image_position_text(position, duration))
        .size(12)
        .style(move |theme| iced::widget::text::Style {
            color: Some(base_text_color(theme).scale_alpha(opacity)),
        });
    let controls = column![position_text, progress_slider]
        .spacing(4)
        .align_x(Alignment::Center)
        .width(Length::Fixed(width));

    Some(
        container(controls)
            .padding([0, VIDEO_CONTROL_HORIZONTAL_PADDING])
            .width(Length::Fixed(width))
            .into(),
    )
}

fn animated_image_controls_opacity_for_preview(
    preview: &AnimatedImagePreview,
    opacity: f32,
) -> f32 {
    if preview.is_seeking() {
        1.0
    } else {
        opacity.clamp(0.0, 1.0)
    }
}

fn animated_image_position_text(position: Duration, duration: Duration) -> String {
    format!(
        "{} / {}",
        format_duration(position),
        format_duration(duration)
    )
}

fn preview_image_frame(
    handle: &image::Handle,
    width: f32,
    height: f32,
) -> image::Image<image::Handle> {
    image::Image::new(handle.clone())
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
}

fn animated_image_control_width(size: PreviewSize, image_width: f32) -> f32 {
    let available_width = (size.width - ANIMATED_IMAGE_CONTROL_SIDE_PADDING * 2.0).max(1.0);
    available_width.min(image_width.max(ANIMATED_IMAGE_MIN_CONTROL_WIDTH))
}

fn audio_preview_panel(
    path: &Path,
    duration: Option<Duration>,
    len: u64,
    playback: Option<&AudioPreviewPlayback>,
) -> Column<'static, Message> {
    let playback = playback.filter(|playback| playback.path.as_path() == path);
    let title = column![
        readable_text("Audio preview").size(17),
        localized_text(audio_preview_summary(duration, len)).size(12),
        localized_text(audio_preview_status(playback, duration)).size(12),
    ]
    .spacing(4)
    .width(Length::Fixed(150.0));

    let controls = row![
        title,
        audio_timeline_control(playback, duration),
        audio_volume_control(playback),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    column![container(controls)
        .width(Length::Fill)
        .height(Length::Fixed(AUDIO_PREVIEW_CONTROL_HEIGHT))
        .center_y(Length::Fixed(AUDIO_PREVIEW_CONTROL_HEIGHT)),]
}

fn audio_primary_button(playback: Option<&AudioPreviewPlayback>) -> Button<'static, Message> {
    let icon = match playback.map(|playback| playback.status) {
        Some(AudioPreviewPlaybackStatus::Playing) => IconSymbol::Pause,
        _ => IconSymbol::Play,
    };
    let button = button(themed_icon(icon, IconTone::Normal, AUDIO_CONTROL_ICON_SIZE))
        .padding(8)
        .width(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .height(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .style(navigation_icon_button_style());
    if matches!(
        playback.map(|playback| playback.status),
        Some(AudioPreviewPlaybackStatus::Loading)
    ) {
        button
    } else {
        button.on_press(Message::AudioPreviewPlaybackToggled)
    }
}

fn audio_timeline_control(
    playback: Option<&AudioPreviewPlayback>,
    duration: Option<Duration>,
) -> Element<'static, Message> {
    let position = playback
        .map(|playback| playback.position)
        .unwrap_or(Duration::ZERO);
    let duration_seconds = duration
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_else(|| (position.as_secs_f32() + 1.0).max(1.0))
        .max(1.0);
    let position_seconds = position.as_secs_f32().min(duration_seconds);

    let slider_row = row![
        audio_primary_button(playback),
        slider(
            0.0..=duration_seconds,
            position_seconds,
            Message::AudioPreviewSeekRequested,
        )
        .step(AUDIO_PROGRESS_SLIDER_STEP_SECONDS)
        .width(Length::Fixed(AUDIO_PROGRESS_SLIDER_WIDTH)),
    ]
    .spacing(AUDIO_TIMELINE_CONTROL_GAP)
    .align_y(Alignment::Center);
    let label_offset = AUDIO_CONTROL_BUTTON_SIZE + AUDIO_TIMELINE_CONTROL_GAP;

    column![
        slider_row,
        row![
            Space::new().width(Length::Fixed(label_offset)),
            readable_text(audio_position_text(position, duration)).size(12),
        ],
    ]
    .spacing(4)
    .width(Length::Fixed(label_offset + AUDIO_PROGRESS_SLIDER_WIDTH))
    .into()
}

fn audio_volume_control(playback: Option<&AudioPreviewPlayback>) -> Element<'static, Message> {
    let volume = playback.map(|playback| playback.volume).unwrap_or(1.0);
    column![
        localized_text(format!("Volume {:.0}%", volume * 100.0)).size(12),
        slider(0.0..=1.0, volume, Message::AudioPreviewVolumeChanged)
            .step(AUDIO_VOLUME_SLIDER_STEP)
            .width(Length::Fixed(AUDIO_VOLUME_SLIDER_WIDTH)),
    ]
    .spacing(4)
    .width(Length::Fixed(AUDIO_VOLUME_SLIDER_WIDTH))
    .into()
}

fn audio_preview_summary(duration: Option<Duration>, len: u64) -> String {
    match duration {
        Some(duration) => format!("{} · {}", format_duration(duration), format_file_size(len)),
        None => format!("Duration unknown · {}", format_file_size(len)),
    }
}

fn audio_preview_status(
    playback: Option<&AudioPreviewPlayback>,
    duration: Option<Duration>,
) -> String {
    let Some(playback) = playback else {
        return "Ready to play".to_owned();
    };

    match playback.status {
        AudioPreviewPlaybackStatus::Loading => "Opening audio output...".to_owned(),
        AudioPreviewPlaybackStatus::Playing => {
            format!(
                "Playing · {}",
                audio_position_text(playback.position, duration)
            )
        }
        AudioPreviewPlaybackStatus::Paused => {
            format!(
                "Paused · {}",
                audio_position_text(playback.position, duration)
            )
        }
        AudioPreviewPlaybackStatus::Stopped => "Stopped".to_owned(),
        AudioPreviewPlaybackStatus::Finished => "Finished".to_owned(),
        AudioPreviewPlaybackStatus::Error => playback
            .error
            .as_ref()
            .map(|error| format!("Audio unavailable: {error}"))
            .unwrap_or_else(|| "Could not start audio preview".to_owned()),
    }
}

fn audio_position_text(position: Duration, duration: Option<Duration>) -> String {
    match duration {
        Some(duration) => format!(
            "{} / {}",
            format_duration(position),
            format_duration(duration)
        ),
        None => format_duration(position),
    }
}

fn video_preview_panel(
    path: &Path,
    frame: Option<&image::Handle>,
    width: u32,
    height: u32,
    duration: Option<Duration>,
    playback: Option<&VideoPreviewPlayback>,
    size: PreviewSize,
    preview_bottom_controls_opacity: f32,
) -> Element<'static, Message> {
    let playback = playback.filter(|playback| playback.path.as_path() == path);
    let (frame_width, frame_height) = video_frame_size(size, width, height);
    let effective_opacity =
        video_controls_opacity_for_playback(playback, preview_bottom_controls_opacity);
    let frame_content: Element<'static, Message> = if let Some(frame) = frame {
        image::Image::new(frame.clone())
            .width(Length::Fixed(frame_width))
            .height(Length::Fixed(frame_height))
            .into()
    } else {
        container(Space::new().width(Length::Fixed(frame_width)))
            .width(Length::Fixed(frame_width))
            .height(Length::Fixed(frame_height))
            .center_x(Length::Fixed(frame_width))
            .center_y(Length::Fixed(frame_height))
            .into()
    };
    let frame_view: Element<'static, Message> = container(frame_content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(preview_media_style)
        .into();

    let mini_progress_opacity = mini_progress_opacity_for_controls_opacity(effective_opacity);
    let mut overlay = Stack::with_children([frame_view])
        .width(Length::Fill)
        .height(Length::Fill);
    if mini_progress_opacity > f32::EPSILON {
        overlay = overlay.push(mini_progress_bar_layer(
            video_progress_fraction(playback, duration),
            mini_progress_opacity,
        ));
    }

    if effective_opacity > f32::EPSILON {
        let gradient: Element<'static, Message> = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme| preview_window_bottom_gradient_style(theme, effective_opacity))
            .into();
        let controls: Element<'static, Message> = container(video_controls(
            playback,
            duration,
            frame_width,
            effective_opacity,
        ))
        .width(Length::Fill)
        .height(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT))
        .center_x(Length::Fill)
        .center_y(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT))
        .into();
        let bottom_controls: Element<'static, Message> = container(
            Stack::with_children([gradient, controls])
                .width(Length::Fill)
                .height(Length::Fixed(VIDEO_PREVIEW_CONTROL_HEIGHT)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Bottom)
        .into();
        overlay = overlay.push(bottom_controls);
    }

    overlay.into()
}

fn video_controls_opacity_for_playback(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> f32 {
    if playback.map_or(false, |playback| playback.seek_completion.is_some()) {
        1.0
    } else {
        opacity.clamp(0.0, 1.0)
    }
}

fn mini_progress_opacity_for_controls_opacity(controls_opacity: f32) -> f32 {
    (1.0 - controls_opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

fn video_progress_fraction(
    playback: Option<&VideoPreviewPlayback>,
    duration: Option<Duration>,
) -> f32 {
    let position = playback
        .map(|playback| playback.position)
        .unwrap_or(Duration::ZERO);
    let duration_seconds = playback
        .and_then(|playback| playback.duration)
        .or(duration)
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_else(|| position.as_secs_f32() + 1.0)
        .max(1.0);
    (position.as_secs_f32().min(duration_seconds) / duration_seconds).clamp(0.0, 1.0)
}

fn animated_image_progress_fraction(preview: &AnimatedImagePreview) -> Option<f32> {
    let duration = preview.playback_duration()?;
    let duration_seconds = duration
        .as_secs_f32()
        .max(AUDIO_PROGRESS_SLIDER_STEP_SECONDS);
    let position_seconds = preview
        .playback_position()
        .min(duration)
        .as_secs_f32()
        .min(duration_seconds);
    Some((position_seconds / duration_seconds).clamp(0.0, 1.0))
}

fn mini_progress_bar_layer(fraction: f32, opacity: f32) -> Element<'static, Message> {
    container(
        progress_bar(0.0..=1.0, fraction)
            .girth(Length::Fixed(MINI_PROGRESS_BAR_HEIGHT))
            .style(move |theme| mini_progress_bar_style(theme, opacity)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(Vertical::Bottom)
    .into()
}

fn mini_progress_bar_style(theme: &Theme, opacity: f32) -> progress_bar::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let colors = ui_colors(theme);
    progress_bar::Style {
        background: Background::Color(colors.outline_variant.scale_alpha(opacity)),
        bar: Background::Color(colors.primary.scale_alpha(opacity)),
        border: Border::default(),
    }
}

fn video_frame_size(size: PreviewSize, width: u32, height: u32) -> (f32, f32) {
    let max_width = size.width.max(1.0);
    let max_height = size.height.max(1.0);
    scaled_media_size(max_width, max_height, width, height)
}

fn video_primary_button(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> Button<'static, Message> {
    let icon = match playback.map(|playback| playback.status) {
        Some(VideoPreviewPlaybackStatus::Playing) => IconSymbol::Pause,
        _ => IconSymbol::Play,
    };
    button(faded_video_icon(icon, AUDIO_CONTROL_ICON_SIZE, opacity))
        .on_press(Message::VideoPreviewPlaybackToggled)
        .padding(8)
        .width(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .height(Length::Fixed(AUDIO_CONTROL_BUTTON_SIZE))
        .style(move |theme, status| faded_video_button_style(theme, status, opacity))
}

fn video_controls(
    playback: Option<&VideoPreviewPlayback>,
    duration: Option<Duration>,
    width: f32,
    opacity: f32,
) -> Element<'static, Message> {
    let opacity = opacity.clamp(0.0, 1.0);
    let position = playback
        .map(|playback| playback.position)
        .unwrap_or(Duration::ZERO);
    let duration = playback.and_then(|playback| playback.duration).or(duration);
    let duration_seconds = duration
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_else(|| (position.as_secs_f32() + 1.0).max(1.0))
        .max(1.0);
    let position_seconds = position.as_secs_f32().min(duration_seconds);

    let progress_slider = slider(
        0.0..=duration_seconds,
        position_seconds,
        Message::VideoPreviewSeekRequested,
    )
    .step(AUDIO_PROGRESS_SLIDER_STEP_SECONDS)
    .on_release(Message::VideoPreviewSeekCommitted)
    .width(Length::FillPortion(VIDEO_PROGRESS_SLIDER_PORTION))
    .style(move |theme, status| faded_video_slider_style(theme, status, opacity));
    let slider_row = row![
        progress_slider,
        container(video_volume_control(playback, opacity))
            .width(Length::FillPortion(VIDEO_VOLUME_SLIDER_PORTION)),
    ]
    .spacing(VIDEO_CONTROL_SLIDER_GAP)
    .width(Length::Fill)
    .align_y(Alignment::Center);

    container(
        column![
            row![
                video_primary_button(playback, opacity),
                readable_text(audio_position_text(position, duration))
                    .size(12)
                    .style(move |theme| iced::widget::text::Style {
                        color: Some(base_text_color(theme).scale_alpha(opacity)),
                    }),
            ]
            .spacing(AUDIO_TIMELINE_CONTROL_GAP)
            .align_y(Alignment::Center),
            slider_row,
        ]
        .spacing(8)
        .width(Length::Fixed(width)),
    )
    .padding([0, VIDEO_CONTROL_HORIZONTAL_PADDING])
    .width(Length::Fixed(width))
    .into()
}

fn video_volume_control(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> Element<'static, Message> {
    row![
        faded_video_icon(IconSymbol::Volume2, AUDIO_CONTROL_ICON_SIZE, opacity),
        video_volume_slider(playback, opacity).width(Length::Fill),
    ]
    .spacing(VIDEO_VOLUME_ICON_GAP)
    .align_y(Alignment::Center)
    .into()
}

fn video_volume_slider(
    playback: Option<&VideoPreviewPlayback>,
    opacity: f32,
) -> iced::widget::Slider<'static, f32, Message> {
    let volume = playback.map(|playback| playback.volume).unwrap_or(1.0);
    slider(0.0..=1.0, volume, Message::VideoPreviewVolumeChanged)
        .step(AUDIO_VOLUME_SLIDER_STEP)
        .style(move |theme, status| faded_video_slider_style(theme, status, opacity))
}

fn faded_video_icon(symbol: IconSymbol, size: f32, opacity: f32) -> Element<'static, Message> {
    themed_icon(symbol, IconTone::Normal, size)
        .opacity(opacity.clamp(0.0, 1.0))
        .into()
}

fn faded_video_button_style(
    theme: &Theme,
    status: iced::widget::button::Status,
    opacity: f32,
) -> iced::widget::button::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let mut style = navigation_icon_button_style()(theme, status);
    style.background = style
        .background
        .map(|background| background.scale_alpha(opacity));
    style.text_color = style.text_color.scale_alpha(opacity);
    style.border.color = style.border.color.scale_alpha(opacity);
    style
}

fn faded_video_slider_style(
    theme: &Theme,
    status: iced::widget::slider::Status,
    opacity: f32,
) -> iced::widget::slider::Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let mut style = iced::widget::slider::default(theme, status);
    style.rail.backgrounds = (
        style.rail.backgrounds.0.scale_alpha(opacity),
        style.rail.backgrounds.1.scale_alpha(opacity),
    );
    style.rail.border.color = style.rail.border.color.scale_alpha(opacity);
    style.handle.background = style.handle.background.scale_alpha(opacity);
    style.handle.border_color = style.handle.border_color.scale_alpha(opacity);
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animated_image_preview::{AnimatedImageFrame, AnimatedImagePlayback};
    use crate::model::VideoPreviewSeekCompletion;
    use std::path::PathBuf;

    #[test]
    fn seek_keeps_video_controls_visible_until_commit() {
        let mut playback =
            VideoPreviewPlayback::playing(PathBuf::from("clip.mp4"), Some(Duration::from_secs(10)));

        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.0),
            0.0
        );

        playback.seek_completion = Some(VideoPreviewSeekCompletion::StayPaused);
        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.0),
            1.0
        );

        playback.seek_completion = None;
        assert_eq!(
            video_controls_opacity_for_playback(Some(&playback), 0.25),
            0.25
        );
    }

    #[test]
    fn seeking_keeps_animated_image_controls_visible_until_commit() {
        let first_frame = AnimatedImageFrame {
            path: PathBuf::from("animation.gif"),
            generation: 1,
            position: Duration::ZERO,
            delay: Duration::from_millis(20),
            handle: image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        };
        let mut preview = AnimatedImagePreview::new(
            PathBuf::from("animation.gif"),
            first_frame,
            1,
            Some(Duration::from_secs(10)),
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");

        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.0),
            0.0
        );
        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.25),
            0.25
        );

        preview.seek_to_position(Duration::from_secs(3));
        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.0),
            1.0
        );

        preview.commit_seek(2);
        assert_eq!(
            animated_image_controls_opacity_for_preview(&preview, 0.0),
            0.0
        );
    }

    #[test]
    fn mini_progress_bar_opacity_inverts_controls_opacity() {
        assert_eq!(mini_progress_opacity_for_controls_opacity(0.0), 1.0);
        assert_eq!(mini_progress_opacity_for_controls_opacity(0.25), 0.75);
        assert_eq!(mini_progress_opacity_for_controls_opacity(1.0), 0.0);
        assert_eq!(mini_progress_opacity_for_controls_opacity(1.5), 0.0);
        assert_eq!(mini_progress_opacity_for_controls_opacity(-0.5), 1.0);
    }

    #[test]
    fn video_progress_fraction_follows_playback() {
        let mut playback =
            VideoPreviewPlayback::playing(PathBuf::from("clip.mp4"), Some(Duration::from_secs(10)));
        playback.position = Duration::from_secs(4);
        assert_eq!(video_progress_fraction(Some(&playback), None), 0.4);

        // duration 未知时回退语义与 video_controls 一致:按 position + 1s 计算。
        let mut unknown = VideoPreviewPlayback::playing(PathBuf::from("clip.webm"), None);
        unknown.position = Duration::from_secs(5);
        assert_eq!(video_progress_fraction(Some(&unknown), None), 5.0 / 6.0);

        // 播放流未就绪时仅显示轨道。
        assert_eq!(
            video_progress_fraction(None, Some(Duration::from_secs(10))),
            0.0
        );
    }

    #[test]
    fn animated_image_progress_fraction_requires_duration() {
        let timed_frame = AnimatedImageFrame {
            path: PathBuf::from("animation.gif"),
            generation: 1,
            position: Duration::from_secs(2),
            delay: Duration::from_millis(20),
            handle: image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        };
        let timed = AnimatedImagePreview::new(
            PathBuf::from("animation.gif"),
            timed_frame,
            1,
            Some(Duration::from_secs(8)),
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");
        assert_eq!(animated_image_progress_fraction(&timed), Some(0.25));

        let untimed_frame = AnimatedImageFrame {
            path: PathBuf::from("animation.gif"),
            generation: 1,
            position: Duration::from_secs(2),
            delay: Duration::from_millis(20),
            handle: image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 1,
            height: 1,
        };
        let untimed = AnimatedImagePreview::new(
            PathBuf::from("animation.gif"),
            untimed_frame,
            1,
            None,
            AnimatedImagePlayback::Animated,
        )
        .expect("animated preview");
        assert_eq!(animated_image_progress_fraction(&untimed), None);
    }
}
