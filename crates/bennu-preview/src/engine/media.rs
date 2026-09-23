//! 音频/视频预览播放状态机：自 app-ui 的 app/preview_state.rs 迁入
//! （impl FileBrowser → impl PreviewEngine，逐字节保真；命令直连本
//! crate 命令层，Task 输出统一为 PreviewMessage）。
//! `seek_audio_preview_playback` 的全局错误提示归宿主通知域，失败时
//! 把错误串返回给宿主转发层展示。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iced::widget::image;
use iced::{Subscription, Task};

use super::PreviewLoadSurface;
use crate::audio_preview::AudioPreviewRuntime;
use crate::commands::preview::{
    start_audio_preview_command, start_video_preview_audio_command, video_preview_frame_command,
    video_preview_metadata_command,
};
use crate::preview::{
    AudioPreviewPlayback, AudioPreviewPlaybackStatus, PreviewContent, PreviewState,
    PreviewWindowProfile, VideoPreviewFrame, VideoPreviewPlayback, VideoPreviewPlaybackStatus,
    VideoPreviewSeekCompletion,
};
use crate::preview_message::PreviewMessage;

impl super::PreviewEngine {
    pub fn accept_video_preview_frame(
        &mut self,
        video_frame: VideoPreviewFrame,
    ) -> Task<PreviewMessage> {
        let frame_path = video_frame.path.clone();
        let frame_generation = video_frame.generation;
        let frame_position = video_frame.position;
        if !self.video_preview_matches(&video_frame.path, video_frame.generation) {
            return Task::none();
        }

        let Some((current_frame, width, height)) =
            self.active_video_preview_frame_mut(&video_frame.path)
        else {
            return Task::none();
        };

        let should_resize_window = *width != video_frame.width || *height != video_frame.height;
        *current_frame = Some(video_frame.handle);
        *width = video_frame.width;
        *height = video_frame.height;
        // 视频帧到达时的窗口适配只归独立窗口会话;面板会话按面板视口适配,
        // 不得弹出/缩放 Space 窗口。
        let resize_command = if should_resize_window
            && self.preview_load_surface == PreviewLoadSurface::StandaloneWindow
        {
            self.fit_preview_window_to_video_frame(video_frame.width, video_frame.height)
        } else {
            Task::none()
        };
        Task::batch([
            resize_command,
            self.finish_video_seek_frame_decode(frame_path, frame_generation, frame_position),
        ])
    }

    pub fn accept_video_preview_metadata(
        &mut self,
        path: PathBuf,
        metadata_outcome: Result<Option<Duration>, String>,
    ) -> Task<PreviewMessage> {
        let Ok(metadata_duration) = metadata_outcome else {
            return Task::none();
        };
        let Some(duration) = self.active_video_preview_duration_mut(&path) else {
            return Task::none();
        };

        *duration = metadata_duration;
        if let Some(playback) = self.video_preview_for_path_mut(&path) {
            playback.duration = metadata_duration;
            playback.position = clamp_video_preview_position(playback.position, metadata_duration);
        }
        Task::none()
    }

    pub fn accept_video_preview_error(
        &mut self,
        path: PathBuf,
        generation: u64,
        error: String,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(&path, generation) else {
            return Task::none();
        };
        finish_video_preview_audio(playback);
        playback.status = VideoPreviewPlaybackStatus::Error;
        playback.seek_completion = None;
        playback.seek_frame_in_flight = None;
        playback.pending_seek_frame = None;
        playback.error = Some(error.clone());

        if self.active_video_preview_path_matches(&path) {
            self.preview = Some(PreviewState::Error(error));
            // 面板会话不弹独立窗口;错误态直接呈现在面板里。
            if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow {
                return self.ensure_preview_window_for_standalone_load(PreviewWindowProfile::Video);
            }
        }
        Task::none()
    }

    pub fn accept_video_preview_seek_frame_error(
        &mut self,
        path: PathBuf,
        generation: u64,
        position: Duration,
        error: String,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(&path, generation) else {
            return Task::none();
        };

        if playback.seek_frame_in_flight != Some(position) {
            return Task::none();
        }
        playback.seek_frame_in_flight = None;
        if let Some(next_position) = playback.pending_seek_frame.take() {
            return start_video_seek_frame_decode(playback, next_position);
        }
        playback.error = Some(error);
        Task::none()
    }

    pub fn accept_video_preview_finished(
        &mut self,
        path: PathBuf,
        generation: u64,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(&path, generation) else {
            return Task::none();
        };

        refresh_video_preview_position(playback);
        finish_video_preview_audio(playback);
        playback.status = VideoPreviewPlaybackStatus::Finished;
        playback.seek_completion = None;
        playback.seek_frame_in_flight = None;
        playback.pending_seek_frame = None;
        playback.started_at = None;
        if let Some(duration) = playback.duration {
            playback.position = duration;
        }
        Task::none()
    }

    pub fn active_video_preview_stream(&self) -> Option<(PathBuf, u64, Duration)> {
        let playback = self.video_preview.as_ref()?;
        if playback.status != VideoPreviewPlaybackStatus::Playing {
            return None;
        }
        if active_video_preview_path(self.preview.as_ref())? != playback.path.as_path() {
            return None;
        }

        Some((
            playback.path.clone(),
            playback.generation,
            playback.stream_start_position,
        ))
    }

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
                AudioPreviewPlaybackStatus::Stopped
                | AudioPreviewPlaybackStatus::Finished
                | AudioPreviewPlaybackStatus::Error => {}
            }
        }

        self.start_audio_preview_playback(path)
    }

    pub fn accept_audio_preview_started(
        &mut self,
        path: PathBuf,
        playback_outcome: Result<AudioPreviewRuntime, String>,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.loading_audio_preview_mut(&path) else {
            return Task::none();
        };

        match playback_outcome {
            Ok(runtime) => {
                runtime.set_volume(playback.volume);
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

    /// 跳转失败时返回错误串：播放状态内的 error 已由引擎落位，全局
    /// 错误通知属宿主通知域，由宿主转发层展示。
    pub fn seek_audio_preview_playback(&mut self, position_seconds: f32) -> Option<String> {
        let Some(playback) = self.audio_preview.as_mut() else {
            return None;
        };
        let Some(runtime) = playback.runtime.as_ref() else {
            return None;
        };

        let position = Duration::from_secs_f32(position_seconds.max(0.0));
        match runtime.seek_to(position) {
            Ok(()) => {
                playback.position = position;
                playback.error = None;
                None
            }
            Err(error) => {
                playback.error = Some(error.clone());
                Some(error)
            }
        }
    }

    pub fn change_audio_preview_volume(&mut self, volume: f32) -> Task<PreviewMessage> {
        let Some(playback) = self.audio_preview.as_mut() else {
            return Task::none();
        };
        let volume = volume.clamp(0.0, 1.0);
        playback.volume = volume;
        if let Some(runtime) = playback.runtime.as_ref() {
            runtime.set_volume(volume);
        }
        Task::none()
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

        playback.position = runtime.position();
        if runtime.is_finished() {
            runtime.stop();
            playback.runtime = None;
            playback.status = AudioPreviewPlaybackStatus::Finished;
        }

        Task::none()
    }

    pub fn audio_preview_is_active(&self) -> bool {
        self.audio_preview.as_ref().is_some_and(|playback| {
            playback.status == AudioPreviewPlaybackStatus::Playing && playback.runtime.is_some()
        })
    }

    pub fn toggle_video_preview_playback(&mut self) -> Task<PreviewMessage> {
        let Some(status) = self.video_preview.as_ref().map(|playback| playback.status) else {
            return Task::none();
        };

        match status {
            VideoPreviewPlaybackStatus::Playing => {
                let Some(playback) = self.video_preview.as_mut() else {
                    return Task::none();
                };
                refresh_video_preview_position(playback);
                if let Some(runtime) = playback.audio_runtime.as_ref() {
                    runtime.pause();
                }
                playback.status = VideoPreviewPlaybackStatus::Paused;
                playback.seek_completion = None;
                playback.seek_frame_in_flight = None;
                playback.pending_seek_frame = None;
                playback.started_at = None;
                Task::none()
            }
            VideoPreviewPlaybackStatus::Paused | VideoPreviewPlaybackStatus::Error => {
                self.resume_video_preview_playback()
            }
            VideoPreviewPlaybackStatus::Finished => {
                let Some(playback) = self.video_preview.as_mut() else {
                    return Task::none();
                };
                playback.position = Duration::ZERO;
                self.resume_video_preview_playback()
            }
        }
    }

    pub fn accept_video_preview_audio_started(
        &mut self,
        path: PathBuf,
        generation: u64,
        audio_outcome: Result<AudioPreviewRuntime, String>,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(&path, generation) else {
            if let Ok(runtime) = audio_outcome {
                runtime.stop();
            }
            return Task::none();
        };

        if playback.status != VideoPreviewPlaybackStatus::Playing {
            if let Ok(runtime) = audio_outcome {
                runtime.stop();
            }
            return Task::none();
        }

        match audio_outcome {
            Ok(runtime) => {
                runtime.set_volume(playback.volume);
                playback.audio_runtime = Some(runtime);
                playback.error = None;
            }
            Err(error) => {
                playback.audio_runtime = None;
                playback.error = Some(format!("Audio unavailable: {error}"));
            }
        }
        Task::none()
    }

    pub fn seek_video_preview_playback(&mut self, position_seconds: f32) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.seek_completion.is_none() {
            playback.seek_completion = Some(match playback.status {
                VideoPreviewPlaybackStatus::Playing => VideoPreviewSeekCompletion::ResumePlayback,
                VideoPreviewPlaybackStatus::Paused
                | VideoPreviewPlaybackStatus::Finished
                | VideoPreviewPlaybackStatus::Error => VideoPreviewSeekCompletion::StayPaused,
            });
            playback.seek_frame_in_flight = None;
            playback.pending_seek_frame = None;
            playback.generation = playback.generation.saturating_add(1);
        }
        if playback.status == VideoPreviewPlaybackStatus::Playing {
            if let Some(runtime) = playback.audio_runtime.as_ref() {
                runtime.pause();
            }
        }
        let position = Duration::from_secs_f32(position_seconds.max(0.0));
        playback.position = clamp_video_preview_position(position, playback.duration);
        playback.status = VideoPreviewPlaybackStatus::Paused;
        playback.started_at = None;
        playback.error = None;
        if playback.seek_frame_in_flight.is_some() {
            playback.pending_seek_frame = Some(playback.position);
            Task::none()
        } else {
            let seek_position = playback.position;
            start_video_seek_frame_decode(playback, seek_position)
        }
    }

    pub fn commit_video_preview_seek(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        let seek_completion = playback.seek_completion.take();
        match seek_completion {
            Some(VideoPreviewSeekCompletion::ResumePlayback) => {
                self.resume_video_preview_playback()
            }
            Some(VideoPreviewSeekCompletion::StayPaused) | None => Task::none(),
        }
    }

    pub fn change_video_preview_volume(&mut self, volume: f32) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        let volume = volume.clamp(0.0, 1.0);
        playback.volume = volume;
        if let Some(runtime) = playback.audio_runtime.as_ref() {
            runtime.set_volume(volume);
        }
        Task::none()
    }

    pub fn update_video_preview_playback(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.status != VideoPreviewPlaybackStatus::Playing {
            return Task::none();
        }

        refresh_video_preview_position(playback);
        if video_preview_reached_end(playback) {
            finish_video_preview_audio(playback);
            playback.status = VideoPreviewPlaybackStatus::Finished;
            playback.seek_completion = None;
            playback.seek_frame_in_flight = None;
            playback.pending_seek_frame = None;
            playback.started_at = None;
        }
        Task::none()
    }

    pub fn video_preview_is_active(&self) -> bool {
        self.video_preview.as_ref().is_some_and(|playback| {
            playback.status == VideoPreviewPlaybackStatus::Playing
                && self.active_video_preview_path_matches(&playback.path)
        })
    }

    pub(crate) fn start_audio_preview_playback(&mut self, path: PathBuf) -> Task<PreviewMessage> {
        self.clear_audio_preview();
        self.audio_preview = Some(AudioPreviewPlayback::loading(path.clone()));
        start_audio_preview_command(path)
    }

    pub(crate) fn clear_audio_preview(&mut self) {
        if let Some(mut playback) = self.audio_preview.take() {
            if let Some(runtime) = playback.runtime.take() {
                runtime.stop();
            }
        }
    }

    pub(crate) fn start_video_preview_playback(
        &mut self,
        path: PathBuf,
        duration: Option<Duration>,
    ) -> Task<PreviewMessage> {
        self.clear_video_preview();
        let playback = VideoPreviewPlayback::playing(path.clone(), duration);
        let generation = playback.generation;
        self.video_preview = Some(playback);
        Task::batch([
            start_video_preview_audio_command(path.clone(), generation, Duration::ZERO),
            video_preview_frame_command(path.clone(), generation, Duration::ZERO),
            video_preview_metadata_command(path),
        ])
    }

    fn resume_video_preview_playback(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        let path = playback.path.clone();
        let position = playback.position;
        playback.generation = playback.generation.saturating_add(1);
        playback.status = VideoPreviewPlaybackStatus::Playing;
        playback.stream_start_position = position;
        playback.seek_completion = None;
        playback.seek_frame_in_flight = None;
        playback.pending_seek_frame = None;
        playback.started_at = Some(Instant::now());
        playback.error = None;
        let generation = playback.generation;

        let should_start_audio = if let Some(runtime) = playback.audio_runtime.as_ref() {
            runtime.set_volume(playback.volume);
            match runtime.seek_to(position) {
                Ok(()) => {
                    runtime.play();
                    false
                }
                Err(error) => {
                    playback.error = Some(error);
                    true
                }
            }
        } else {
            true
        };

        let frame_command = video_preview_frame_command(path.clone(), generation, position);
        if should_start_audio {
            finish_video_preview_audio(playback);
            Task::batch([
                start_video_preview_audio_command(path, generation, position),
                frame_command,
            ])
        } else {
            frame_command
        }
    }

    fn finish_video_seek_frame_decode(
        &mut self,
        path: PathBuf,
        generation: u64,
        position: Duration,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(&path, generation) else {
            return Task::none();
        };

        if playback.seek_frame_in_flight != Some(position) {
            return Task::none();
        }
        playback.seek_frame_in_flight = None;
        let Some(next_position) = playback.pending_seek_frame.take() else {
            return Task::none();
        };
        if next_position == position {
            return Task::none();
        }
        start_video_seek_frame_decode(playback, next_position)
    }

    pub(crate) fn clear_video_preview(&mut self) {
        if let Some(mut playback) = self.video_preview.take() {
            finish_video_preview_audio(&mut playback);
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

    fn video_preview_matches(&self, path: &Path, generation: u64) -> bool {
        self.video_preview.as_ref().is_some_and(|playback| {
            playback.path.as_path() == path && playback.generation == generation
        })
    }

    fn active_video_preview_mut(
        &mut self,
        path: &Path,
        generation: u64,
    ) -> Option<&mut VideoPreviewPlayback> {
        self.video_preview
            .as_mut()
            .filter(|playback| playback.path.as_path() == path && playback.generation == generation)
    }

    fn video_preview_for_path_mut(&mut self, path: &Path) -> Option<&mut VideoPreviewPlayback> {
        self.video_preview
            .as_mut()
            .filter(|playback| playback.path.as_path() == path)
    }

    fn active_video_preview_frame_mut(
        &mut self,
        path: &Path,
    ) -> Option<(&mut Option<image::Handle>, &mut u32, &mut u32)> {
        match &mut self.preview {
            Some(PreviewState::Ready(PreviewContent::Video {
                path: preview_path,
                frame,
                width,
                height,
                ..
            })) if preview_path.as_path() == path => Some((frame, width, height)),
            _ => None,
        }
    }

    fn active_video_preview_duration_mut(&mut self, path: &Path) -> Option<&mut Option<Duration>> {
        match &mut self.preview {
            Some(PreviewState::Ready(PreviewContent::Video {
                path: preview_path,
                duration,
                ..
            })) if preview_path.as_path() == path => Some(duration),
            _ => None,
        }
    }

    fn active_video_preview_path_matches(&self, path: &Path) -> bool {
        active_video_preview_path(self.preview.as_ref()) == Some(path)
    }
}

fn active_audio_preview_path(preview: Option<&PreviewState>) -> Option<&Path> {
    match preview? {
        PreviewState::Loading(path) => Some(path.as_path()),
        PreviewState::Ready(PreviewContent::Audio { path, .. }) => Some(path.as_path()),
        _ => None,
    }
}

fn active_video_preview_path(preview: Option<&PreviewState>) -> Option<&Path> {
    match preview? {
        PreviewState::Ready(PreviewContent::Video { path, .. }) => Some(path.as_path()),
        _ => None,
    }
}

fn refresh_video_preview_position(playback: &mut VideoPreviewPlayback) {
    if playback.status != VideoPreviewPlaybackStatus::Playing {
        return;
    }
    let Some(started_at) = playback.started_at.replace(Instant::now()) else {
        return;
    };
    playback.position =
        clamp_video_preview_position(playback.position + started_at.elapsed(), playback.duration);
}

fn clamp_video_preview_position(position: Duration, duration: Option<Duration>) -> Duration {
    match duration {
        Some(duration) => position.min(duration),
        None => position,
    }
}

fn start_video_seek_frame_decode(
    playback: &mut VideoPreviewPlayback,
    position: Duration,
) -> Task<PreviewMessage> {
    let position = clamp_video_preview_position(position, playback.duration);
    playback.seek_frame_in_flight = Some(position);
    video_preview_frame_command(playback.path.clone(), playback.generation, position)
}

fn video_preview_reached_end(playback: &VideoPreviewPlayback) -> bool {
    playback
        .duration
        .is_some_and(|duration| playback.position >= duration)
}

fn finish_video_preview_audio(playback: &mut VideoPreviewPlayback) {
    if let Some(runtime) = playback.audio_runtime.take() {
        runtime.stop();
    }
}

// 播放位置刷新节拍（自 app-ui 订阅桥迁入）：音频与视频共用同一周期，
// 驱动 update_*_preview_playback 前进；门禁条件（*_is_active）由宿主
// 读引擎状态后决定是否挂订阅，与迁移前 app.rs 的写法一致。
const MEDIA_PLAYBACK_TICK_INTERVAL: Duration = Duration::from_millis(250);

pub fn audio_preview_tick_subscription() -> Subscription<PreviewMessage> {
    iced::time::every(MEDIA_PLAYBACK_TICK_INTERVAL).map(|_| PreviewMessage::AudioPreviewTick)
}

pub fn video_preview_tick_subscription() -> Subscription<PreviewMessage> {
    iced::time::every(MEDIA_PLAYBACK_TICK_INTERVAL).map(|_| PreviewMessage::VideoPreviewTick)
}
