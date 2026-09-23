//! 预览会话状态机：accept_preview 的内容分派与 clear_preview 的会话
//! 收尾，自 app-ui 的 app/preview_state.rs 迁入（impl FileBrowser →
//! impl PreviewEngine，逐字节保真；Task 输出统一为 PreviewMessage）。
//! 目录展开的扫描选项与展开层数是宿主目录域快照，由宿主转发层实时
//! 读取传入（引擎配置快照只随持久化刷新，宿主直读保证测试与运行期
//! 行为一致）；全局错误通知的清除归宿主通知域，经验收结论回传。

use std::path::PathBuf;

use file_core::ScanOptions;
use iced::widget::image;
use iced::Task;

use super::PreviewAcceptance;
use crate::preview::{ImagePreviewContent, PreviewContent, PreviewState, PreviewWindowProfile};
use crate::preview_message::PreviewMessage;
use crate::text_preview::TextPreviewDocument;

impl super::PreviewEngine {
    pub fn accept_preview(
        &mut self,
        path: PathBuf,
        preview_outcome: Result<PreviewContent, String>,
        options: ScanOptions,
        expand_levels: u8,
    ) -> (PreviewAcceptance, Task<PreviewMessage>) {
        let is_active_preview_request = matches!(
            &self.preview,
            Some(PreviewState::Loading(loading_path)) if loading_path == &path
        );
        if !is_active_preview_request {
            return (PreviewAcceptance::NotDisplayed, Task::none());
        }

        match preview_outcome {
            Ok(mut preview) => {
                let mut directory_expand_command = Task::none();
                if let PreviewContent::Directory { entries, .. } = &mut preview {
                    let range = 0..entries.len();
                    directory_expand_command = super::auto_expand_preview_tree_directories(
                        entries,
                        range,
                        expand_levels,
                        &options,
                    );
                }
                let command = match &preview {
                    PreviewContent::Text {
                        path,
                        rendered,
                        format,
                        next_offset,
                        loaded_line_count,
                        line_limit_notice,
                    } => {
                        self.clear_audio_preview();
                        self.clear_video_preview();
                        self.text_preview_generation = self.text_preview_generation.wrapping_add(1);
                        self.text_preview_document = Some(TextPreviewDocument::new_initial(
                            path.clone(),
                            &rendered,
                            *format,
                            self.text_preview_generation,
                            *next_offset,
                            *loaded_line_count,
                            *line_limit_notice,
                        ));
                        Task::none()
                    }
                    PreviewContent::Audio { .. } => {
                        self.text_preview_document = None;
                        self.clear_video_preview();
                        Task::none()
                    }
                    PreviewContent::Video { path, duration, .. } => {
                        self.text_preview_document = None;
                        self.clear_audio_preview();
                        self.start_video_preview_playback(path.clone(), *duration)
                    }
                    PreviewContent::Sqlite(database) => {
                        self.text_preview_document = None;
                        self.clear_audio_preview();
                        self.clear_video_preview();
                        self.start_sqlite_preview(path.clone(), database)
                    }
                    _ => {
                        self.text_preview_document = None;
                        self.clear_audio_preview();
                        self.clear_video_preview();
                        Task::none()
                    }
                };
                self.preview = Some(PreviewState::Ready(preview));
                (
                    PreviewAcceptance::ContentReady,
                    Task::batch([command, directory_expand_command]),
                )
            }
            Err(error) => {
                self.text_preview_document = None;
                self.clear_audio_preview();
                self.clear_video_preview();
                self.preview = Some(PreviewState::Error(error));
                // 面板会话不弹独立窗口;错误态直接呈现在面板里。
                // 不可达兜底：standalone 会话 start 时已开窗，面板会话被
                // surface 守卫拦截；档位取 Regular 与错误面板呈现一致。
                (
                    PreviewAcceptance::NotDisplayed,
                    self.ensure_preview_window_for_standalone_load(PreviewWindowProfile::Regular),
                )
            }
        }
    }

    pub fn clear_preview(&mut self) {
        // 固定生效期间，主窗口的点击/框选等交互不得清空预览内容。
        // 打开预览的流程总是先经 close_preview_window（preview 已为 None），不受此守卫影响。
        if self.preview_window_pinned && self.preview_window.is_some() && self.preview.is_some() {
            return;
        }
        self.invalidate_animated_image_preview();
        self.invalidate_original_image_preview();
        self.cancel_document_preview();
        self.cancel_remote_preview_download();
        self.text_preview_document = None;
        self.clear_sqlite_preview();
        self.clear_audio_preview();
        self.clear_video_preview();
        let preview = self.preview.take();
        if let Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::OriginalRaster {
                raster_handle,
                placeholder_handle,
                ..
            },
        ))) = preview
        {
            Self::release_original_raster_handles(raster_handle, placeholder_handle);
        }
    }

    fn release_original_raster_handles(
        raster_handle: image::Handle,
        placeholder_handle: image::Handle,
    ) {
        let release = move || {
            drop((raster_handle, placeholder_handle));
            // glibc 默认可能保留大块已释放堆页，连续预览会把这些保留页表现为 RSS 泄漏。
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            let _ = unsafe { libc::malloc_trim(0) };
        };
        // 原图 RGBA 缓冲区可能很大，不能让释放最后一个句柄阻塞 UI 更新线程。
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let _ = runtime.spawn_blocking(release);
        } else {
            release();
        }
    }
}
