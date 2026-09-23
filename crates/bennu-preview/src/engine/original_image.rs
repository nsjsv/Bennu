//! 原图预览状态机：自 app-ui 的 preview_state/original_image.rs 迁入
//! （impl FileBrowser → impl PreviewEngine，逐字节保真）。依赖宿主
//! 列表缩略图管线（thumbnail_cache 入队/泵送）与目录条目查询的
//! 档位刷新路径留在宿主侧（附录 A：列表缩略图子系统不随预览窗口下沉）。

use std::path::PathBuf;

use iced::Task;
use thumbnails::{ThumbnailKey, ThumbnailRequest};
use tokio_util::sync::CancellationToken;

use crate::original_image_preview::OriginalImagePreview;
use crate::preview::{ImagePreviewContent, PreviewContent, PreviewState};
use crate::preview_message::PreviewMessage;

/// 预览档位缩略图的展示占位标记：原图已并行加载,该标记只决定
/// 磁盘缓存探测结果到达时是否替换 Loading 展示,与原图启动无关。
#[derive(Debug)]
pub struct PendingPreviewThumbnailDisplay {
    path: PathBuf,
    generation: u64,
    thumbnail_key: ThumbnailKey,
}

impl PendingPreviewThumbnailDisplay {
    /// 由宿主缩略图入队路径构造（队列等待标记的写入点在宿主管线）。
    pub fn new(path: PathBuf, generation: u64, thumbnail_key: ThumbnailKey) -> Self {
        Self {
            path,
            generation,
            thumbnail_key,
        }
    }
}

impl super::PreviewEngine {
    pub(crate) fn invalidate_original_image_preview(&mut self) {
        self.pending_preview_thumbnail_display = None;
        if let Some(cancellation) = self.original_image_preview_cancel.take() {
            cancellation.cancel();
        }
        self.original_image_preview_generation =
            self.original_image_preview_generation.wrapping_add(1);
    }

    pub fn next_original_image_preview_generation(&mut self) -> u64 {
        self.invalidate_original_image_preview();
        self.original_image_preview_cancel = Some(CancellationToken::new());
        self.original_image_preview_generation
    }

    pub fn accept_original_image_preview(
        &mut self,
        path: PathBuf,
        generation: u64,
        outcome: Result<OriginalImagePreview, String>,
    ) -> Task<PreviewMessage> {
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
        let presents_in_window =
            self.preview_load_surface == super::PreviewLoadSurface::StandaloneWindow;
        let window_is_missing = self.preview_window.is_none();

        match outcome {
            Ok(OriginalImagePreview::Raster {
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
            Ok(OriginalImagePreview::Svg {
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

    /// 缩略图回流是否命中等待标记：回流入口在宿主缩略图管线（产物
    /// 类型归宿主），命中判定与标记清理语义归预览会话。
    pub fn pending_preview_thumbnail_display_matches(&self, request: &ThumbnailRequest) -> bool {
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
}
