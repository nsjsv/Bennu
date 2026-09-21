use std::path::PathBuf;
use std::time::Duration;

use iced::Task;

use crate::animated_image_preview::{AnimatedImageFrame, AnimatedImagePreview};
use crate::app::right_preview_panel::PreviewLoadSurface;
use crate::app::FileBrowser;
use crate::model::{Message, PreviewContent, PreviewState};

impl FileBrowser {
    pub(super) fn invalidate_animated_image_preview(&mut self) {
        self.animated_image_preview_generation =
            self.animated_image_preview_generation.wrapping_add(1);
    }

    pub(in crate::app) fn next_animated_image_preview_generation(&mut self) -> u64 {
        self.invalidate_animated_image_preview();
        self.animated_image_preview_generation
    }

    pub(in crate::app) fn accept_animated_image_preview_loaded(
        &mut self,
        path: PathBuf,
        generation: u64,
        preview_outcome: Result<AnimatedImagePreview, String>,
    ) -> Task<Message> {
        if generation != self.animated_image_preview_generation {
            return Task::none();
        }

        self.accept_animated_image_preview_load_result(path, preview_outcome)
    }

    fn accept_animated_image_preview_load_result(
        &mut self,
        path: PathBuf,
        preview_outcome: Result<AnimatedImagePreview, String>,
    ) -> Task<Message> {
        if !matches!(
            &self.preview,
            Some(PreviewState::Loading(loading_path)) if loading_path == &path
        ) {
            return Task::none();
        }

        match preview_outcome {
            Ok(preview) => {
                let width = preview.width();
                let height = preview.height();
                self.text_preview_document = None;
                self.clear_audio_preview();
                self.clear_video_preview();
                self.preview = Some(PreviewState::Ready(PreviewContent::AnimatedImage(preview)));
                self.clear_global_error();
                // 窗口尺寸跟随内容只归独立窗口会话;面板按面板视口适配。
                if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow {
                    self.open_animated_image_preview_window_for_dimensions(width, height)
                } else {
                    Task::none()
                }
            }
            Err(error) => {
                self.text_preview_document = None;
                self.clear_audio_preview();
                self.clear_video_preview();
                self.preview = Some(PreviewState::Error(error));
                if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow {
                    self.open_image_preview_error_window()
                } else {
                    Task::none()
                }
            }
        }
    }

    pub(in crate::app) fn active_animated_image_preview_stream(
        &self,
    ) -> Option<(PathBuf, u64, Duration)> {
        let PreviewState::Ready(PreviewContent::AnimatedImage(preview)) = self.preview.as_ref()?
        else {
            return None;
        };

        preview.is_playing().then(|| {
            (
                preview.path().to_path_buf(),
                preview.generation(),
                preview.stream_start_position(),
            )
        })
    }

    pub(in crate::app) fn accept_animated_image_frame(
        &mut self,
        frame: AnimatedImageFrame,
    ) -> Task<Message> {
        let Some(PreviewState::Ready(PreviewContent::AnimatedImage(preview))) =
            self.preview.as_mut()
        else {
            return Task::none();
        };
        if preview.path() != frame.path.as_path() || preview.generation() != frame.generation {
            return Task::none();
        }

        preview.accept_frame(frame);
        Task::none()
    }

    pub(in crate::app) fn accept_animated_image_preview_finished(
        &mut self,
        path: PathBuf,
        generation: u64,
    ) -> Task<Message> {
        let Some(PreviewState::Ready(PreviewContent::AnimatedImage(preview))) =
            self.preview.as_mut()
        else {
            return Task::none();
        };
        if preview.path() != path.as_path() || preview.generation() != generation {
            return Task::none();
        }

        preview.finish();
        Task::none()
    }

    pub(in crate::app) fn accept_animated_image_preview_error(
        &mut self,
        path: PathBuf,
        generation: u64,
        error: String,
    ) -> Task<Message> {
        let Some(PreviewState::Ready(PreviewContent::AnimatedImage(preview))) =
            self.preview.as_ref()
        else {
            return Task::none();
        };
        if preview.path() != path.as_path() || preview.generation() != generation {
            return Task::none();
        }

        self.preview = Some(PreviewState::Error(error));
        if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow {
            return self.open_image_preview_error_window();
        }
        Task::none()
    }

    pub(in crate::app) fn seek_animated_image_preview(
        &mut self,
        position_seconds: f32,
    ) -> Task<Message> {
        let Some(PreviewState::Ready(PreviewContent::AnimatedImage(preview))) =
            self.preview.as_mut()
        else {
            return Task::none();
        };
        let Some(duration) = preview.playback_duration() else {
            return Task::none();
        };

        let position = Duration::from_secs_f32(position_seconds.max(0.0)).min(duration);
        preview.seek_to_position(position);
        Task::none()
    }

    pub(in crate::app) fn commit_animated_image_preview_seek(&mut self) -> Task<Message> {
        if !matches!(
            self.preview,
            Some(PreviewState::Ready(PreviewContent::AnimatedImage(_)))
        ) {
            return Task::none();
        }

        let generation = self.next_animated_image_preview_generation();
        let Some(PreviewState::Ready(PreviewContent::AnimatedImage(preview))) =
            self.preview.as_mut()
        else {
            return Task::none();
        };

        preview.commit_seek(generation);
        Task::none()
    }
}
