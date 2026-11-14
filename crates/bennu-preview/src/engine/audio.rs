//! 音频预览的播放状态机：开始/暂停/重播、拖动与跳转、播完与出错、共享音量，以及
//! 「独立窗口自动播、右侧面板等点击」的打开规则。与视频共用的部分（快捷键分发、
//! 播放节拍）在 `media.rs`。

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::widget::image;
use iced::Task;

use super::PreviewLoadSurface;
use crate::audio_preview::AudioPreviewRuntime;
use crate::commands::preview::start_audio_preview_at_command;
use crate::preview::{
    AudioPreviewPlayback, AudioPreviewPlaybackStatus, PreviewContent, PreviewState,
};
use crate::preview_message::PreviewMessage;

impl super::PreviewEngine {
    pub fn toggle_audio_preview_playback(&mut self) -> Task<PreviewMessage> {
        let Some(path) = active_audio_preview_path(self.preview.as_ref()).map(Path::to_path_buf)
        else {
            return Task::none();
        };

        if let Some(playback) = self
            .audio_preview
            .as_mut()
            .filter(|playback| playback.path == path)
        {
            match playback.status {
                AudioPreviewPlaybackStatus::Loading => return Task::none(),
                AudioPreviewPlaybackStatus::Playing => {
                    if let Some(runtime) = &playback.runtime {
                        playback.position = runtime.position();
                        runtime.pause();
                        playback.status = AudioPreviewPlaybackStatus::Paused;
                        return Task::none();
                    }
                }
                AudioPreviewPlaybackStatus::Paused => {
                    if let Some(runtime) = &playback.runtime {
                        runtime.play();
                        playback.status = AudioPreviewPlaybackStatus::Playing;
                        playback.error = None;
                        return Task::none();
                    }
                }
                // 播完重播复用同一个声卡输出，只从头换一条解码流。
                AudioPreviewPlaybackStatus::Finished => {
                    if let Some(runtime) = &playback.runtime {
                        match runtime.seek_to(Duration::ZERO) {
                            Ok(()) => {
                                runtime.play();
                                playback.position = Duration::ZERO;
                                playback.status = AudioPreviewPlaybackStatus::Playing;
                                playback.error = None;
                            }
                            Err(error) => playback.error = Some(error),
                        }
                        return Task::none();
                    }
                }
                AudioPreviewPlaybackStatus::Ready
                | AudioPreviewPlaybackStatus::Stopped
                | AudioPreviewPlaybackStatus::Error => {}
            }
        }

        // 就绪（可能已拖过进度条）和意外停下的从当前位置开始，出错的从头。
        let resume_from = self
            .audio_preview
            .as_ref()
            .filter(|playback| {
                playback.path == path
                    && matches!(
                        playback.status,
                        AudioPreviewPlaybackStatus::Ready | AudioPreviewPlaybackStatus::Stopped
                    )
            })
            .map_or(Duration::ZERO, |playback| playback.position);
        self.start_audio_preview_playback(path, resume_from)
    }

    /// 宿主打开音频预览的唯一入口：独立窗口一打开就播，右侧面板只就绪、点了才播
    /// （选中项在音乐文件夹里移动时不该每移一下就出声）。
    pub fn open_audio_preview(&mut self, path: PathBuf) -> Task<PreviewMessage> {
        match self.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => {
                self.start_audio_preview_playback(path, Duration::ZERO)
            }
            PreviewLoadSurface::RightDockedPanel => {
                self.clear_audio_preview();
                self.audio_preview = Some(AudioPreviewPlayback::ready(path));
                Task::none()
            }
        }
    }

    pub(super) fn active_audio_duration(&self) -> Option<Duration> {
        match &self.preview {
            Some(PreviewState::Ready(PreviewContent::Audio { details, .. })) => {
                details.probe.duration
            }
            _ => None,
        }
    }

    pub fn accept_audio_preview_started(
        &mut self,
        path: PathBuf,
        playback_outcome: Result<AudioPreviewRuntime, String>,
    ) -> Task<PreviewMessage> {
        let volume = self.media_volume;
        let Some(playback) = self.loading_audio_preview_mut(&path) else {
            return Task::none();
        };

        match playback_outcome {
            Ok(runtime) => {
                runtime.set_volume(volume);
                playback.position = runtime.position();
                playback.runtime = Some(runtime);
                playback.status = AudioPreviewPlaybackStatus::Playing;
                playback.error = None;
            }
            Err(error) => {
                playback.runtime = None;
                playback.status = AudioPreviewPlaybackStatus::Error;
                playback.error = Some(error);
            }
        }

        Task::none()
    }

    pub(super) fn drag_audio_preview_position(
        &mut self,
        position_seconds: f32,
    ) -> Task<PreviewMessage> {
        if let Some(playback) = self.audio_preview.as_mut() {
            playback.seeking = true;
            playback.position = Duration::from_secs_f32(position_seconds.max(0.0));
        }
        Task::none()
    }

    /// 还没开始播放（或已出错）时只记下位置，下次开始播放从这里起。
    pub(super) fn commit_audio_preview_seek(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.audio_preview.as_mut() else {
            return Task::none();
        };
        playback.seeking = false;
        let Some(runtime) = playback.runtime.as_ref() else {
            return Task::none();
        };
        match runtime.seek_to(playback.position) {
            Ok(()) => {
                if playback.status == AudioPreviewPlaybackStatus::Finished {
                    playback.status = AudioPreviewPlaybackStatus::Paused;
                }
                playback.error = None;
            }
            Err(error) => playback.error = Some(error),
        }
        Task::none()
    }

    pub(super) fn accept_audio_preview_cover(
        &mut self,
        path: &Path,
        handle: image::Handle,
    ) -> Task<PreviewMessage> {
        if let Some(PreviewState::Ready(PreviewContent::Audio {
            path: preview_path,
            cover,
            ..
        })) = &mut self.preview
        {
            if preview_path == path {
                *cover = Some(handle);
            }
        }
        Task::none()
    }

    pub(super) fn accept_audio_preview_duration(
        &mut self,
        path: &Path,
        duration: Duration,
    ) -> Task<PreviewMessage> {
        if let Some(PreviewState::Ready(PreviewContent::Audio {
            path: preview_path,
            details,
            ..
        })) = &mut self.preview
        {
            if preview_path == path {
                details.probe.duration = Some(duration);
                details.probe.duration_is_estimate = false;
            }
        }
        Task::none()
    }

    /// 宿主记住音量后调用：同时作用到正在播的音频和视频的声音。
    pub fn set_media_volume(&mut self, volume: f32) {
        self.media_volume = volume.clamp(0.0, 1.0);
        let audio = self
            .audio_preview
            .as_ref()
            .and_then(|playback| playback.runtime.as_ref());
        let video = self
            .video_preview
            .as_ref()
            .and_then(|playback| playback.audio_runtime.as_ref());
        for runtime in [audio, video].into_iter().flatten() {
            runtime.set_volume(self.media_volume);
        }
    }

    pub fn update_audio_preview_playback(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.audio_preview.as_mut() else {
            return Task::none();
        };
        if playback.status != AudioPreviewPlaybackStatus::Playing {
            return Task::none();
        }
        let Some(runtime) = playback.runtime.as_ref() else {
            playback.status = AudioPreviewPlaybackStatus::Stopped;
            return Task::none();
        };

        if !playback.seeking {
            playback.position = runtime.position();
        }
        if let Some(failure) = runtime.take_failure() {
            runtime.pause();
            playback.status = AudioPreviewPlaybackStatus::Error;
            playback.error = Some(failure);
        } else if runtime.is_finished() {
            // 留着运行时：重播、往回拖都不用重新打开声卡。
            runtime.pause();
            playback.status = AudioPreviewPlaybackStatus::Finished;
        }

        Task::none()
    }

    /// 这次预览的内容自己接键盘（文本查看器、Markdown 壳、音视频键盘壳）：独立预览窗口
    /// 一打开，宿主就用 `focus_preview_keyboard` 把键盘交给它。
    pub fn preview_takes_keyboard(&self) -> bool {
        self.active_text_preview_document().is_some()
            || matches!(
                self.preview,
                Some(PreviewState::Ready(
                    PreviewContent::Audio { .. } | PreviewContent::Video { .. }
                ))
            )
    }

    pub fn audio_preview_is_active(&self) -> bool {
        self.audio_preview.as_ref().is_some_and(|playback| {
            playback.status == AudioPreviewPlaybackStatus::Playing && playback.runtime.is_some()
        })
    }

    pub(crate) fn start_audio_preview_playback(
        &mut self,
        path: PathBuf,
        from: Duration,
    ) -> Task<PreviewMessage> {
        self.clear_audio_preview();
        let mut playback = AudioPreviewPlayback::loading(path.clone());
        playback.position = from;
        self.audio_preview = Some(playback);
        start_audio_preview_at_command(path, from)
    }

    pub(crate) fn clear_audio_preview(&mut self) {
        if let Some(mut playback) = self.audio_preview.take() {
            if let Some(runtime) = playback.runtime.take() {
                runtime.stop();
            }
        }
    }

    fn loading_audio_preview_mut(&mut self, path: &Path) -> Option<&mut AudioPreviewPlayback> {
        if active_audio_preview_path(self.preview.as_ref()) != Some(path) {
            return None;
        }
        self.audio_preview.as_mut().filter(|playback| {
            playback.path.as_path() == path
                && playback.status == AudioPreviewPlaybackStatus::Loading
        })
    }
}

fn active_audio_preview_path(preview: Option<&PreviewState>) -> Option<&Path> {
    match preview? {
        PreviewState::Loading(path) => Some(path.as_path()),
        PreviewState::Ready(PreviewContent::Audio { path, .. }) => Some(path.as_path()),
        _ => None,
    }
}
