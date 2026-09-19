use std::path::PathBuf;

use file_core::DirectoryEntry;
use iced::Task;
use thumbnails::{ThumbnailKey, ThumbnailRequest};
use tokio_util::sync::CancellationToken;

use super::super::right_preview_panel::PreviewLoadSurface;
use super::super::{windows::image_preview_size_from_dimensions, FileBrowser};
use crate::commands::original_image_preview_command;
use crate::model::{ImagePreviewContent, Message, PreviewContent, PreviewSize, PreviewState};
use crate::thumbnail_cache::{
    request_for_entry, ThumbnailHandleEntry, ThumbnailPriority, ThumbnailPurpose,
    PREVIEW_THUMBNAIL_MAX_EDGE,
};

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

/// 预览档位缩略图的展示占位标记：原图已并行加载,该标记只决定
/// 磁盘缓存探测结果到达时是否替换 Loading 展示,与原图启动无关。
#[derive(Debug)]
pub(in crate::app) struct PendingPreviewThumbnailDisplay {
    path: PathBuf,
    generation: u64,
    thumbnail_key: ThumbnailKey,
}

impl FileBrowser {
    pub(super) fn invalidate_original_image_preview(&mut self) {
        self.pending_preview_thumbnail_display = None;
        if let Some(cancellation) = self.original_image_preview_cancel.take() {
            cancellation.cancel();
        }
        self.original_image_preview_generation =
            self.original_image_preview_generation.wrapping_add(1);
    }

    pub(in crate::app) fn next_original_image_preview_generation(&mut self) -> u64 {
        self.invalidate_original_image_preview();
        self.original_image_preview_cancel = Some(CancellationToken::new());
        self.original_image_preview_generation
    }

    pub(in crate::app) fn accept_original_image_preview(
        &mut self,
        path: PathBuf,
        generation: u64,
        outcome: Result<crate::original_image_preview::OriginalImagePreview, String>,
    ) -> Task<Message> {
        let active_preview = matches!(
            &self.preview,
            Some(PreviewState::Loading(current)) if current == &path
        ) || matches!(
            &self.preview,
            Some(PreviewState::Ready(PreviewContent::Image(
                ImagePreviewContent::Thumbnail { path: current, .. }
            ))) if current == &path
        );
        if generation != self.original_image_preview_generation || !active_preview {
            return Task::none();
        }
        self.pending_preview_thumbnail_display = None;
        // 独立窗口会话在内容就绪后按内容尺寸补开/适配窗口;
        // 面板会话始终不动窗口,内容按面板视口渲染。
        let presents_in_window = self.preview_load_surface == PreviewLoadSurface::StandaloneWindow;
        let window_is_missing = self.preview_window.is_none();

        match outcome {
            Ok(crate::original_image_preview::OriginalImagePreview::Raster {
                raster_handle,
                placeholder_handle: decoded_placeholder_handle,
                width,
                height,
            }) => {
                let placeholder_handle = match &self.preview {
                    Some(PreviewState::Ready(PreviewContent::Image(
                        ImagePreviewContent::Thumbnail { handle, .. },
                    ))) => handle.clone(),
                    _ => decoded_placeholder_handle,
                };
                self.preview = Some(PreviewState::Ready(PreviewContent::Image(
                    ImagePreviewContent::OriginalRaster {
                        raster_handle,
                        placeholder_handle,
                        width,
                        height,
                    },
                )));
                if presents_in_window && window_is_missing {
                    self.open_image_preview_window_for_dimensions(width, height)
                } else {
                    Task::none()
                }
            }
            Ok(crate::original_image_preview::OriginalImagePreview::Svg {
                handle,
                width,
                height,
                has_intrinsic_size,
            }) => {
                let window_command = if presents_in_window {
                    if has_intrinsic_size {
                        self.open_image_preview_window_for_dimensions(width, height)
                    } else {
                        self.open_image_preview_window_with_default_size()
                    }
                } else {
                    Task::none()
                };
                self.preview = Some(PreviewState::Ready(PreviewContent::Image(
                    ImagePreviewContent::OriginalSvg {
                        handle,
                        width,
                        height,
                    },
                )));
                window_command
            }
            Err(error) => {
                self.preview = Some(PreviewState::ImageError { path, error });
                if presents_in_window && window_is_missing {
                    self.open_image_preview_error_window()
                } else {
                    Task::none()
                }
            }
        }
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
            self.preview = Some(PreviewState::Ready(thumbnail_preview_content(
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
            self.pending_preview_thumbnail_display = Some(PendingPreviewThumbnailDisplay {
                path: original.path.clone(),
                generation: original.generation,
                thumbnail_key: request.key(),
            });
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
            self.preview = Some(PreviewState::Ready(thumbnail_preview_content(
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

    fn pending_preview_thumbnail_display_matches(&self, request: &ThumbnailRequest) -> bool {
        let Some(pending) = self.pending_preview_thumbnail_display.as_ref() else {
            return false;
        };
        // 标记仅在展示仍处于 Loading 时成立;缩略图/原图上屏即清除,
        // 迟到的探测结果不允许回退已替换的展示内容。
        matches!(
            &self.preview,
            Some(PreviewState::Loading(current)) if current == &pending.path
        ) && pending.path == request.source
            && pending.thumbnail_key == request.key()
            && pending.generation == self.original_image_preview_generation
    }

    pub(in crate::app) fn accept_preview_thumbnail_ready(
        &mut self,
        request: &ThumbnailRequest,
        ready: ThumbnailHandleEntry,
    ) -> Task<Message> {
        if self.pending_preview_thumbnail_display_matches(request) {
            self.pending_preview_thumbnail_display = None;
            self.preview = Some(PreviewState::Ready(thumbnail_preview_content(
                request.source.clone(),
                ready,
            )));
            return Task::none();
        }
        let current_max_edge =
            match &self.preview {
                Some(PreviewState::Ready(PreviewContent::Image(
                    ImagePreviewContent::Thumbnail { path, max_edge, .. },
                ))) if path == &request.source => Some(*max_edge),
                _ => None,
            };
        if current_max_edge.is_some_and(|max_edge| ready.max_edge >= max_edge) {
            self.preview = Some(PreviewState::Ready(thumbnail_preview_content(
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
        if self.pending_preview_thumbnail_display_matches(request) {
            self.pending_preview_thumbnail_display = None;
        }
    }

    pub(in crate::app) fn refresh_preview_thumbnail_for_size(&mut self) -> Task<Message> {
        let Some((path, max_edge)) = self.preview.as_ref().and_then(|preview| match preview {
            PreviewState::Ready(PreviewContent::Image(ImagePreviewContent::Thumbnail {
                path,
                max_edge,
                ..
            })) => Some((path.clone(), *max_edge)),
            _ => None,
        }) else {
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
        let active_preview_loading = generation == self.original_image_preview_generation
            && matches!(
                &self.preview,
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
                .original_image_preview_cancel
                .clone()
                .expect("image preview generation must own cancellation"),
        };
        let window_command = if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow {
            self.open_image_preview_window_for_dimensions(width, height)
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
        match self.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => self.close_preview_window(),
            PreviewLoadSurface::RightDockedPanel => {
                self.preview = Some(PreviewState::ImageError { path, error });
                Task::none()
            }
        }
    }

    fn preview_thumbnail_edge(&self) -> u32 {
        preview_thumbnail_edge_for_size(self.preview_size)
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
