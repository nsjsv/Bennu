//! 原图预览状态机已迁 bennu-preview 的 PreviewEngine
//! （engine/original_image.rs：代际失效/验收、缩略图等待标记）；此处
//! 保留与宿主列表缩略图管线（thumbnail_cache 入队/泵送、目录条目查询）
//! 与宿主通知域耦合的档位刷新/尺寸入口，以及返回宿主 Task 的同名薄
//! 转发（任务附录 A：列表缩略图子系统不随预览窗口下沉）。

use std::path::PathBuf;

use file_core::DirectoryEntry;
use iced::Task;
use thumbnails::ThumbnailRequest;
use tokio_util::sync::CancellationToken;

use super::super::windows::image_preview_size_from_dimensions;
use super::super::FileBrowser;
use crate::commands::original_image_preview_command;
use crate::model::{ImagePreviewContent, Message, PreviewContent, PreviewSize, PreviewState};
use crate::thumbnail_cache::{
    request_for_entry, ThumbnailHandleEntry, ThumbnailPriority, ThumbnailPurpose,
    PREVIEW_THUMBNAIL_MAX_EDGE,
};
use bennu_preview::engine::PendingPreviewThumbnailDisplay;

#[cfg(test)]
mod tests;
const PREVIEW_THUMBNAIL_MIN_EDGE: u32 = 512;
const PREVIEW_RESIZE_EXTRA_PIXELS: u32 = 128;

#[derive(Debug)]
struct OriginalImagePreviewRequest {
    path: PathBuf,
    generation: u64,
    max_file_bytes: u64,
    cancellation: CancellationToken,
    placeholder_handle: Option<iced::widget::image::Handle>,
}

impl OriginalImagePreviewRequest {
    fn load_command(self) -> Task<Message> {
        original_image_preview_command(
            self.path,
            self.generation,
            self.max_file_bytes,
            self.placeholder_handle,
            self.cancellation,
        )
    }
}

impl FileBrowser {
    pub(in crate::app) fn accept_original_image_preview(
        &mut self,
        path: PathBuf,
        generation: u64,
        outcome: Result<crate::original_image_preview::OriginalImagePreview, String>,
    ) -> Task<Message> {
        let command = self
            .preview_engine
            .accept_original_image_preview(path, generation, outcome);
        self.sync_preview_window_focus();
        command.map(Message::Preview)
    }

    pub(in crate::app) fn retry_image_preview(&mut self, path: PathBuf) -> Task<Message> {
        self.open_preview_for_resolved_path(path, file_core::FileKind::File)
    }

    fn request_preview_thumbnail_for_entry(
        &mut self,
        entry: DirectoryEntry,
        mut original: OriginalImagePreviewRequest,
        max_edge: u32,
    ) -> Task<Message> {
        let Some(request) = request_for_entry(&entry, max_edge) else {
            tracing::debug!(
                target: "app_ui::preview",
                path = ?entry.path,
                max_edge,
                "preview thumbnail request skipped"
            );
            return original.load_command();
        };

        // 首帧不变量:任一就绪缩略图(含列表阶段的小图)立即铺满上屏,
        // 原图并行解码;预览档位缩略图只作展示替换,永不阻塞原图启动。
        if let Some(ready) = self
            .thumbnail_cache
            .largest_ready_for_source(&entry.path)
            .cloned()
        {
            tracing::debug!(
                target: "app_ui::preview",
                path = ?entry.path,
                max_edge = ready.max_edge,
                "preview opens with cached thumbnail"
            );
            let ready_max_edge = ready.max_edge;
            original.placeholder_handle = Some(ready.handle.clone());
            self.preview_engine.preview = Some(PreviewState::Ready(thumbnail_preview_content(
                entry.path, ready,
            )));
            // 已有缩略图明显小于预览档位时,并行生成预览档位缩略图升级展示
            // (release 实测参考图 1500x2104 冷生成 24-68ms,远低于 200ms 预算)。
            if max_edge > ready_max_edge + PREVIEW_RESIZE_EXTRA_PIXELS {
                self.thumbnail_cache.enqueue_request(
                    request,
                    ThumbnailPurpose::Preview,
                    ThumbnailPriority::Preview,
                );
                return Task::batch([self.pump_thumbnail_queue(), original.load_command()]);
            }
            return original.load_command();
        }

        tracing::debug!(
            target: "app_ui::preview",
            path = ?entry.path,
            max_edge,
            "preview thumbnail queued"
        );
        // 内存无缩略图:并行生成预览档位缩略图,到达即铺满上屏,
        // 与原图解码互不等待。
        let waits_for_thumbnail = self.thumbnail_cache.enqueue_request(
            request.clone(),
            ThumbnailPurpose::Preview,
            ThumbnailPriority::Preview,
        );
        if waits_for_thumbnail {
            self.preview_engine.pending_preview_thumbnail_display =
                Some(PendingPreviewThumbnailDisplay::new(
                    original.path.clone(),
                    original.generation,
                    request.key(),
                ));
        }
        Task::batch([self.pump_thumbnail_queue(), original.load_command()])
    }

    fn request_preview_thumbnail_refresh(
        &mut self,
        entry: DirectoryEntry,
        max_edge: u32,
    ) -> Task<Message> {
        let Some(request) = request_for_entry(&entry, max_edge) else {
            return Task::none();
        };
        if let Some(ready) = self.thumbnail_cache.ready_for_request(&request).cloned() {
            self.preview_engine.preview = Some(PreviewState::Ready(thumbnail_preview_content(
                entry.path, ready,
            )));
            return Task::none();
        }
        self.thumbnail_cache.enqueue_request(
            request,
            ThumbnailPurpose::Preview,
            ThumbnailPriority::Preview,
        );
        self.pump_thumbnail_queue()
    }

    pub(in crate::app) fn accept_preview_thumbnail_ready(
        &mut self,
        request: &ThumbnailRequest,
        ready: ThumbnailHandleEntry,
    ) -> Task<Message> {
        if self
            .preview_engine
            .pending_preview_thumbnail_display_matches(request)
        {
            self.preview_engine.pending_preview_thumbnail_display = None;
            self.preview_engine.preview = Some(PreviewState::Ready(thumbnail_preview_content(
                request.source.clone(),
                ready,
            )));
            return Task::none();
        }
        let current_max_edge =
            match &self.preview_engine.preview {
                Some(PreviewState::Ready(PreviewContent::Image(
                    ImagePreviewContent::Thumbnail { path, max_edge, .. },
                ))) if path == &request.source => Some(*max_edge),
                _ => None,
            };
        if current_max_edge.is_some_and(|max_edge| ready.max_edge >= max_edge) {
            self.preview_engine.preview = Some(PreviewState::Ready(thumbnail_preview_content(
                request.source.clone(),
                ready,
            )));
        }
        Task::none()
    }

    pub(in crate::app) fn accept_preview_thumbnail_unavailable(
        &mut self,
        request: &ThumbnailRequest,
    ) {
        if self
            .preview_engine
            .pending_preview_thumbnail_display_matches(request)
        {
            self.preview_engine.pending_preview_thumbnail_display = None;
        }
    }

    pub(in crate::app) fn refresh_preview_thumbnail_for_size(&mut self) -> Task<Message> {
        let Some((path, max_edge)) =
            self.preview_engine
                .preview
                .as_ref()
                .and_then(|preview| match preview {
                    PreviewState::Ready(PreviewContent::Image(
                        ImagePreviewContent::Thumbnail { path, max_edge, .. },
                    )) => Some((path.clone(), *max_edge)),
                    _ => None,
                })
        else {
            return Task::none();
        };
        let desired_edge = self.preview_thumbnail_edge();
        if desired_edge <= max_edge + PREVIEW_RESIZE_EXTRA_PIXELS {
            tracing::debug!(
                target: "app_ui::preview",
                path = ?path,
                current_max_edge = max_edge,
                desired_edge,
                "preview thumbnail refresh skipped"
            );
            return Task::none();
        }

        let Some(entry) = self.entry_for_path(&path).cloned() else {
            return Task::none();
        };
        self.request_preview_thumbnail_refresh(entry, desired_edge)
    }

    pub(in crate::app) fn accept_image_preview_dimensions(
        &mut self,
        path: PathBuf,
        generation: u64,
        dimensions: Result<(u32, u32), String>,
    ) -> Task<Message> {
        let active_preview_loading = generation
            == self.preview_engine.original_image_preview_generation
            && matches!(
                &self.preview_engine.preview,
                Some(PreviewState::Loading(current)) if current == &path
            );
        if !active_preview_loading {
            return Task::none();
        }

        let (width, height) = match dimensions {
            Ok((width, height)) if width > 0 && height > 0 => (width, height),
            Ok(_) => {
                let error = "Image preview has invalid dimensions".to_owned();
                tracing::warn!(
                    target: "app_ui::preview",
                    path = ?path,
                    error = %error,
                    "image preview dimensions failed"
                );
                return self.fail_image_preview_dimensions(path, error);
            }
            Err(error) => {
                tracing::warn!(
                    target: "app_ui::preview",
                    path = ?path,
                    error = %error,
                    "image preview dimensions failed"
                );
                return self.fail_image_preview_dimensions(path, error);
            }
        };

        tracing::debug!(
            target: "app_ui::preview",
            path = ?path,
            width,
            height,
            "image preview dimensions accepted"
        );

        let original = OriginalImagePreviewRequest {
            path: path.clone(),
            generation,
            max_file_bytes: self.user_config.preview_size_limits.image_bytes,
            placeholder_handle: None,
            cancellation: self
                .preview_engine
                .original_image_preview_cancel
                .clone()
                .expect("image preview generation must own cancellation"),
        };
        let window_command = if self.preview_engine.preview_load_surface
            == bennu_preview::engine::PreviewLoadSurface::StandaloneWindow
        {
            let command = self
                .preview_engine
                .open_image_preview_window_for_dimensions(width, height);
            self.sync_preview_window_focus();
            command.map(Message::Preview)
        } else {
            Task::none()
        };
        let Some(entry) = self.entry_for_path(&path).cloned() else {
            return Task::batch([window_command, original.load_command()]);
        };
        let thumbnail_command = self.request_preview_thumbnail_for_entry(
            entry,
            original,
            preview_thumbnail_edge_for_size(image_preview_size_from_dimensions(width, height)),
        );
        Task::batch([window_command, thumbnail_command])
    }

    /// 图片尺寸无效/读取失败的统一出口:独立窗口会话维持原语义
    /// (全局错误提示 + 关闭预览窗口);面板会话改以可重试的错误态呈现。
    fn fail_image_preview_dimensions(&mut self, path: PathBuf, error: String) -> Task<Message> {
        self.show_global_error(error.clone());
        match self.preview_engine.preview_load_surface {
            bennu_preview::engine::PreviewLoadSurface::StandaloneWindow => {
                self.close_preview_window()
            }
            bennu_preview::engine::PreviewLoadSurface::RightDockedPanel => {
                self.preview_engine.preview = Some(PreviewState::ImageError { path, error });
                Task::none()
            }
        }
    }

    fn preview_thumbnail_edge(&self) -> u32 {
        preview_thumbnail_edge_for_size(self.preview_engine.preview_size)
    }
}

fn preview_thumbnail_edge_for_size(size: PreviewSize) -> u32 {
    size.width
        .max(size.height)
        .ceil()
        .max(PREVIEW_THUMBNAIL_MIN_EDGE as f32)
        .min(PREVIEW_THUMBNAIL_MAX_EDGE as f32) as u32
}

fn thumbnail_preview_content(path: PathBuf, ready: ThumbnailHandleEntry) -> PreviewContent {
    PreviewContent::Image(ImagePreviewContent::Thumbnail {
        path,
        handle: ready.handle,
        width: ready.width,
        height: ready.height,
        max_edge: ready.max_edge,
    })
}
