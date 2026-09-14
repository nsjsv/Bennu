use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::*;
use crate::animated_image_preview::{
    AnimatedImageFrame, AnimatedImagePlayback, AnimatedImagePreview,
};
use crate::config::ui_thread_startup_config;
use crate::model::{ImagePreviewContent, PreviewContent, PreviewState};
use crate::thumbnail_cache::{
    ThumbnailLoadOutcome, ThumbnailLoadPolicy, ThumbnailLoadResult, ThumbnailPriority,
    ThumbnailPurpose, ThumbnailWork, LIST_THUMBNAIL_EDGE,
};

#[test]
fn preview_thumbnail_refresh_skips_same_edge_window_resize() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    let image_entry = image_entry("/workspace/vector.svg");
    browser.entries = vec![image_entry.clone()].into();
    browser.preview_size = PreviewSize {
        width: 640.0,
        height: 480.0,
    };
    browser.preview = Some(PreviewState::Ready(PreviewContent::Image(
        ImagePreviewContent::Thumbnail {
            path: image_entry.path.clone(),
            handle: iced::widget::image::Handle::from_path("/tmp/vector-thumb.png"),
            width: 320,
            height: 240,
            max_edge: 640,
        },
    )));

    let command = browser.refresh_preview_thumbnail_for_size();

    assert_eq!(command.units(), 0);
}

#[test]
fn preview_thumbnail_refresh_keeps_current_image_visible() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    let image_entry = image_entry("/workspace/vector.svg");
    browser.entries = vec![image_entry.clone()].into();
    browser.preview_size = PreviewSize {
        width: 1400.0,
        height: 1000.0,
    };
    browser.preview = Some(PreviewState::Ready(PreviewContent::Image(
        ImagePreviewContent::Thumbnail {
            path: image_entry.path,
            handle: iced::widget::image::Handle::from_path("/tmp/vector-thumb.png"),
            width: 320,
            height: 240,
            max_edge: 512,
        },
    )));

    let command = browser.refresh_preview_thumbnail_for_size();

    assert!(matches!(
        browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail { max_edge: 512, .. }
        )))
    ));
    assert!(command.units() > 0);
}

#[test]
fn preview_thumbnail_refresh_skips_animated_image_preview() {
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.preview_size = PreviewSize {
        width: 1400.0,
        height: 1000.0,
    };
    let animated_path = PathBuf::from("/workspace/loop.gif");
    let first_frame = AnimatedImageFrame {
        path: animated_path.clone(),
        generation: 1,
        position: std::time::Duration::ZERO,
        delay: std::time::Duration::from_millis(20),
        handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
        width: 1,
        height: 1,
    };
    browser.preview = Some(PreviewState::Ready(PreviewContent::AnimatedImage(
        AnimatedImagePreview::new(
            animated_path,
            first_frame,
            1,
            Some(std::time::Duration::from_millis(40)),
            AnimatedImagePlayback::Animated,
        )
        .expect("animated image preview"),
    )));

    let command = browser.refresh_preview_thumbnail_for_size();

    assert_eq!(command.units(), 0);
}

#[test]
fn stale_image_dimensions_do_not_start_an_original_preview_request() {
    let path = PathBuf::from("/workspace/photo.png");
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let stale_generation = browser.next_original_image_preview_generation();
    browser.clear_preview();

    let command = browser.accept_image_preview_dimensions(path, stale_generation, Ok((320, 240)));

    assert_eq!(command.units(), 0);
    assert!(browser.preview.is_none());
}

#[test]
fn image_preview_without_directory_entry_opens_window_before_original_load() {
    let path = PathBuf::from("/tmp/remote-preview-cache/photo.png");
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser.next_original_image_preview_generation();

    let command = browser.accept_image_preview_dimensions(path.clone(), generation, Ok((320, 240)));

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Loading(current)) if current == &path
    ));
    assert!(browser.preview_window.is_some());
    assert!(browser.pending_preview_thumbnail_display.is_none());
    assert!(command.units() > 0);
}

#[test]
fn ready_listing_thumbnail_shows_immediately_and_starts_original() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser.next_original_image_preview_generation();
    let listing_request =
        request_for_entry(&image_entry, LIST_THUMBNAIL_EDGE).expect("listing request");
    browser.thumbnail_cache.insert_ready(
        thumbnails::CachedThumbnail {
            key: listing_request.key(),
            source: listing_request.source.clone(),
            output: PathBuf::from("/tmp/listing-thumbnail.png"),
            width: 96,
            height: 64,
            cache_hit: false,
        },
        LIST_THUMBNAIL_EDGE,
    );

    let command = browser.accept_image_preview_dimensions(path.clone(), generation, Ok((320, 240)));

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail { path: current, max_edge: 96, .. }
        ))) if current == &path
    ));
    assert!(browser.pending_preview_thumbnail_display.is_none());
    assert!(browser.preview_window.is_some());
    assert!(command.units() > 0);
}

#[test]
fn small_ready_thumbnail_upgrades_when_preview_thumbnail_arrives() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser.next_original_image_preview_generation();
    let listing_request =
        request_for_entry(&image_entry, LIST_THUMBNAIL_EDGE).expect("listing request");
    browser.thumbnail_cache.insert_ready(
        thumbnails::CachedThumbnail {
            key: listing_request.key(),
            source: listing_request.source.clone(),
            output: PathBuf::from("/tmp/listing-thumbnail.png"),
            width: 96,
            height: 64,
            cache_hit: false,
        },
        LIST_THUMBNAIL_EDGE,
    );

    drop(browser.accept_image_preview_dimensions(path.clone(), generation, Ok((320, 240))));

    // 预览档位缩略图已并行生成并派发;模拟生成完成返回:
    // 展示升级到更大尺寸帧,原图并行加载不受影响。
    let desired_request = preview_request(&image_entry, 320, 240);
    browser.accept_thumbnail_batch(vec![ready_preview_outcome(desired_request, 512, 384)]);

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail { max_edge: 512, .. }
        )))
    ));
}

#[test]
fn cold_cache_starts_original_and_display_upgrades_from_generated_thumbnail() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser.next_original_image_preview_generation();

    let command = browser.accept_image_preview_dimensions(path.clone(), generation, Ok((320, 240)));

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Loading(current)) if current == &path
    ));
    assert!(browser.pending_preview_thumbnail_display.is_some());
    assert!(browser.preview_window.is_some());
    assert!(command.units() > 0);

    let request = preview_request(&image_entry, 320, 240);
    browser.accept_thumbnail_batch(vec![ready_preview_outcome(request, 320, 240)]);

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail { path: current, .. }
        ))) if current == &path
    ));
    assert!(browser.pending_preview_thumbnail_display.is_none());
}

#[test]
fn failed_preview_thumbnail_keeps_window_open_while_original_loads() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser.next_original_image_preview_generation();
    let request = preview_request(&image_entry, 320, 240);
    drop(browser.accept_image_preview_dimensions(path.clone(), generation, Ok((320, 240))));

    assert!(browser.preview_window.is_some());

    let command = browser.accept_thumbnail_batch(vec![failed_preview_outcome(request)]);

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Loading(current)) if current == &path
    ));
    assert!(browser.preview_window.is_some());
    assert!(browser.pending_preview_thumbnail_display.is_none());
    assert_eq!(command.units(), 0);
}

#[test]
fn stale_preview_thumbnail_key_cannot_consume_current_request() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser.next_original_image_preview_generation();
    let expected_request = preview_request(&image_entry, 320, 240);
    let stale_request = request_for_entry(&image_entry, expected_request.max_edge + 128)
        .expect("stale preview thumbnail request");
    drop(browser.accept_image_preview_dimensions(path.clone(), generation, Ok((320, 240))));

    drop(browser.accept_thumbnail_batch(vec![ready_preview_outcome(stale_request, 400, 300)]));

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Loading(current)) if current == &path
    ));
    assert!(browser.preview_window.is_some());

    browser.accept_thumbnail_batch(vec![ready_preview_outcome(expected_request, 320, 240)]);

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail { path: current, .. }
        ))) if current == &path
    ));
    assert!(browser.preview_window.is_some());
    assert!(browser.pending_preview_thumbnail_display.is_none());
}

#[test]
fn expected_thumbnail_for_removed_entry_falls_back_to_original_result_window() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Loading(path.clone()));
    let generation = browser.next_original_image_preview_generation();
    let request = preview_request(&image_entry, 320, 240);
    drop(browser.accept_image_preview_dimensions(path.clone(), generation, Ok((320, 240))));
    browser.entries = Vec::new().into();

    let fallback_command =
        browser.accept_thumbnail_batch(vec![ready_preview_outcome(request, 320, 240)]);

    assert!(matches!(
        &browser.preview,
        Some(PreviewState::Loading(current)) if current == &path
    ));
    assert!(browser.pending_preview_thumbnail_display.is_none());
    assert!(browser.preview_window.is_some());
    assert_eq!(fallback_command.units(), 0);

    let original_command = browser.accept_original_image_preview(
        path,
        generation,
        Ok(
            crate::original_image_preview::OriginalImagePreview::Raster {
                raster_handle: iced::widget::image::Handle::from_rgba(4, 3, vec![0; 48]),
                placeholder_handle: iced::widget::image::Handle::from_rgba(
                    1,
                    1,
                    vec![0, 0, 0, 255],
                ),
                width: 4,
                height: 3,
            },
        ),
    );

    assert!(matches!(
        browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::OriginalRaster {
                width: 4,
                height: 3,
                ..
            }
        )))
    ));
    assert!(browser.preview_window.is_some());
    assert_eq!(original_command.units(), 0);
}

#[test]
fn late_smaller_preview_thumbnail_does_not_replace_larger_thumbnail() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Ready(PreviewContent::Image(
        ImagePreviewContent::Thumbnail {
            path,
            handle: iced::widget::image::Handle::from_path("/tmp/large-thumbnail.png"),
            width: 1024,
            height: 768,
            max_edge: 1024,
        },
    )));
    let smaller_request =
        request_for_entry(&image_entry, 512).expect("smaller preview thumbnail request");

    drop(browser.accept_thumbnail_batch(vec![ready_preview_outcome(smaller_request, 512, 384)]));

    assert!(matches!(
        browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail { max_edge: 1024, .. }
        )))
    ));
}

#[test]
fn late_thumbnail_result_does_not_replace_original_preview() {
    let path = PathBuf::from("/workspace/photo.png");
    let image_entry = image_entry(path.to_string_lossy().as_ref());
    let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
    browser.entries = vec![image_entry.clone()].into();
    browser.preview = Some(PreviewState::Ready(PreviewContent::Image(
        ImagePreviewContent::OriginalRaster {
            raster_handle: iced::widget::image::Handle::from_rgba(4, 3, vec![255; 48]),
            placeholder_handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]),
            width: 4,
            height: 3,
        },
    )));
    let request = request_for_entry(&image_entry, 512).expect("image request");

    drop(browser.accept_thumbnail_batch(vec![ready_preview_outcome(request, 512, 384)]));

    assert!(matches!(
        browser.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::OriginalRaster {
                width: 4,
                height: 3,
                ..
            }
        )))
    ));
}

fn preview_request(entry: &DirectoryEntry, width: u32, height: u32) -> ThumbnailRequest {
    let target_size = image_preview_size_from_dimensions(width, height);
    request_for_entry(entry, preview_thumbnail_edge_for_size(target_size))
        .expect("preview thumbnail request")
}

fn ready_preview_outcome(
    request: ThumbnailRequest,
    width: u32,
    height: u32,
) -> ThumbnailLoadOutcome {
    let thumbnail = thumbnails::CachedThumbnail {
        key: request.key(),
        source: request.source.clone(),
        output: PathBuf::from("/tmp/preview-thumbnail.png"),
        width,
        height,
        cache_hit: false,
    };
    ThumbnailLoadOutcome {
        work: preview_work(request),
        result: ThumbnailLoadResult::Ready(thumbnail),
    }
}

fn failed_preview_outcome(request: ThumbnailRequest) -> ThumbnailLoadOutcome {
    ThumbnailLoadOutcome {
        work: preview_work(request),
        result: ThumbnailLoadResult::Failed("thumbnail failed".to_owned()),
    }
}

fn preview_work(request: ThumbnailRequest) -> ThumbnailWork {
    ThumbnailWork {
        request,
        purpose: ThumbnailPurpose::Preview,
        priority: ThumbnailPriority::Preview,
        load_policy: ThumbnailLoadPolicy::LoadOrGenerate,
        scope: None,
    }
}

fn image_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata {
            len: 10,
            modified: None,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}
