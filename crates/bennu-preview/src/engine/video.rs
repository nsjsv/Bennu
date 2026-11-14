//! 视频预览的播放状态机。画面（帧流订阅）、声音（声卡输出）、进度都跟同一个时钟走：
//! 有声音以声卡为准，没声音按墙钟。暂停只停时钟和声卡，解码那一段不关，继续时不重开；
//! 拖进度条时停掉解码、只解关键帧看个大概，松手才从精确位置重开一段。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bennu_localization::tr;
use iced::{Size, Task};

use super::PreviewLoadSurface;
use crate::audio_preview::AudioPreviewRuntime;
use crate::commands::preview::{
    measure_video_display_command, start_video_preview_audio_command, video_still_command,
};
use crate::preview::{
    PreviewContent, PreviewState, PreviewWindowProfile, VideoPreviewDetails, VideoPreviewFrame,
    VideoPreviewPlayback, VideoPreviewPlaybackStatus, VideoSeekDrag, VideoSound, VideoSubtitle,
};
use crate::preview_message::PreviewMessage;
use crate::video_preview::{FramePlan, SeekPrecision, VideoSegment, FALLBACK_DISPLAY};

/// 右侧面板没播时显示哪一帧：和缩略图一样取第 1 秒，跳过开头的黑场。
const STILL_PICTURE_TIME: Duration = Duration::from_secs(1);
/// 单击要等这么久没有第二下才算（第二下就是双击）。
pub(crate) const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(300);

impl super::PreviewEngine {
    /// 视频内容到了：先量屏幕（出帧大小按它定），量好再开始。
    pub(crate) fn open_video_preview(
        &mut self,
        path: &Path,
        details: &VideoPreviewDetails,
    ) -> Task<PreviewMessage> {
        self.clear_video_preview();
        self.video_preview = Some(VideoPreviewPlayback::ready(
            path.to_path_buf(),
            details.probe.duration,
            !details.probe.audio_tracks.is_empty(),
        ));
        // 独立窗口按画面比例开好，第一帧到了不再跳一下。
        let window = match &details.probe.video {
            Some(video) if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow => {
                let (width, height) = video.display_size();
                self.fit_preview_window_to_video_frame(width.round() as u32, height.round() as u32)
            }
            _ => Task::none(),
        };
        Task::batch([
            window,
            measure_video_display_command(path.to_path_buf()),
            self.choose_default_video_tracks(details),
        ])
    }

    /// 屏幕量好了：独立窗口（含文件选择对话框）一打开就播；右侧面板只显示静止画面，点了才播。
    pub fn accept_video_display(
        &mut self,
        path: &Path,
        display: Option<Size<u32>>,
    ) -> Task<PreviewMessage> {
        let Some(video) = self
            .active_video_details(path)
            .and_then(|details| details.probe.video.clone())
        else {
            return Task::none();
        };
        let Some(playback) = self.video_preview_for_path_mut(path) else {
            return Task::none();
        };
        let plan = Arc::new(FramePlan::new(&video, display.unwrap_or(FALLBACK_DISPLAY)));
        playback.plan = Some(Arc::clone(&plan));
        let generation = playback.generation;
        let still_at = match playback.duration {
            Some(duration) if duration < STILL_PICTURE_TIME * 2 => duration / 2,
            _ => STILL_PICTURE_TIME,
        };
        match self.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => {
                self.start_video_segment(Duration::ZERO, VideoPreviewPlaybackStatus::Playing)
            }
            PreviewLoadSurface::RightDockedPanel => video_still_command(
                path.to_path_buf(),
                plan,
                generation,
                still_at,
                SeekPrecision::Exact,
            ),
        }
    }

    /// 播放键、回车、单击画面。
    pub fn toggle_video_preview_playback(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        let position = playback.position;
        match playback.status {
            VideoPreviewPlaybackStatus::Playing => {
                pause_video(playback);
                Task::none()
            }
            VideoPreviewPlaybackStatus::Paused if playback.segment_start.is_some() => {
                self.resume_video()
            }
            VideoPreviewPlaybackStatus::Finished => {
                self.start_video_segment(Duration::ZERO, VideoPreviewPlaybackStatus::Playing)
            }
            VideoPreviewPlaybackStatus::Ready
            | VideoPreviewPlaybackStatus::Paused
            | VideoPreviewPlaybackStatus::Error => {
                self.start_video_segment(position, VideoPreviewPlaybackStatus::Playing)
            }
        }
    }

    /// 拖进度条：第一下停掉解码和声音，之后每个位置解一张关键帧（同时只解一张）。
    pub fn seek_video_preview_playback(&mut self, position_seconds: f32) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.seek_drag.is_none() {
            playback.track_menu = None;
            playback.seek_drag = Some(VideoSeekDrag {
                resume_after: playback.status == VideoPreviewPlaybackStatus::Playing,
            });
            pause_video(playback);
            playback.segment_start = None;
            playback.generation += 1;
            playback.still_in_flight = None;
            playback.pending_still = None;
        }
        playback.position = clamp_position(
            Duration::from_secs_f32(position_seconds.max(0.0)),
            playback.duration,
        );
        let position = playback.position;
        if playback.still_in_flight.is_some() {
            playback.pending_still = Some(position);
            return Task::none();
        }
        request_keyframe_still(playback, position)
    }

    /// 松手：从精确位置开一段；按下时在播就接着播，否则停在那一帧。
    pub fn commit_video_preview_seek(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        let Some(drag) = playback.seek_drag.take() else {
            return Task::none();
        };
        let status = if drag.resume_after {
            VideoPreviewPlaybackStatus::Playing
        } else {
            VideoPreviewPlaybackStatus::Paused
        };
        let position = playback.position;
        self.start_video_segment(position, status)
    }

    /// ←/→：直接精确跳，播放状态不变。
    pub(super) fn seek_video_to(&mut self, target: Duration) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_ref() else {
            return Task::none();
        };
        let status = if playback.status == VideoPreviewPlaybackStatus::Playing {
            VideoPreviewPlaybackStatus::Playing
        } else {
            VideoPreviewPlaybackStatus::Paused
        };
        let target = clamp_position(target, playback.duration);
        self.start_video_segment(target, status)
    }

    pub fn accept_video_preview_frame(&mut self, frame: VideoPreviewFrame) -> Task<PreviewMessage> {
        if !self.video_preview_matches(&frame.path, frame.generation) {
            return Task::none();
        }
        self.show_video_picture(frame)
    }

    pub fn accept_video_preview_still(&mut self, frame: VideoPreviewFrame) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(&frame.path, frame.generation) else {
            return Task::none();
        };
        let next = finish_still(playback, frame.position);
        Task::batch([self.show_video_picture(frame), next])
    }

    pub fn accept_video_preview_still_error(
        &mut self,
        path: &Path,
        generation: u64,
        position: Duration,
        error: String,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(path, generation) else {
            return Task::none();
        };
        let next = finish_still(playback, position);
        playback.error = Some(error);
        next
    }

    pub fn accept_video_preview_audio_started(
        &mut self,
        path: &Path,
        generation: u64,
        outcome: Result<AudioPreviewRuntime, String>,
    ) -> Task<PreviewMessage> {
        let volume = self.media_settings.volume;
        let Some(playback) = self.video_preview_for_path_mut(path) else {
            if let Ok(runtime) = outcome {
                runtime.stop();
            }
            return Task::none();
        };
        match outcome {
            Ok(runtime) => {
                runtime.set_volume(volume);
                // 起声音期间换过音轨或跳转过：换到现在的音轨、从现在这段的位置放。
                let resync = if runtime.track() != playback.audio_track {
                    runtime.switch_track(playback.audio_track, playback.clock.position())
                } else if generation != playback.generation {
                    runtime.seek_to(playback.clock.position())
                } else {
                    Ok(())
                };
                if let Err(error) = resync {
                    playback.error = Some(tr!("Audio unavailable: %1", error));
                }
                if playback.status != VideoPreviewPlaybackStatus::Playing {
                    runtime.pause();
                }
                playback.clock.follow_audio(runtime.clone());
                playback.sound = VideoSound::Running(runtime);
            }
            Err(error) => {
                playback.clock.follow_wall();
                playback.sound = VideoSound::Failed;
                playback.error = Some(tr!("Audio unavailable: %1", error));
            }
        }
        Task::none()
    }

    /// 播放中的刷新节拍：进度从时钟取；声音坏了或先放完了，时钟改按墙钟，画面接着走。
    pub fn update_video_preview_playback(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.status != VideoPreviewPlaybackStatus::Playing {
            return Task::none();
        }
        playback.position = clamp_position(playback.clock.position(), playback.duration);
        if let VideoSound::Running(runtime) = &playback.sound {
            if let Some(failure) = runtime.take_failure() {
                playback.error = Some(tr!("Audio unavailable: %1", failure));
                playback.clock.follow_wall();
                playback.sound = VideoSound::Failed;
            } else if runtime.is_finished() {
                playback.clock.follow_wall();
            }
        }
        Task::none()
    }

    pub fn accept_video_preview_finished(
        &mut self,
        path: &Path,
        generation: u64,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(path, generation) else {
            return Task::none();
        };
        pause_video(playback);
        playback.status = VideoPreviewPlaybackStatus::Finished;
        playback.segment_start = None;
        playback.position = playback.duration.unwrap_or(playback.position);
        Task::none()
    }

    pub fn accept_video_preview_error(
        &mut self,
        path: &Path,
        generation: u64,
        error: String,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.active_video_preview_mut(path, generation) else {
            return Task::none();
        };
        pause_video(playback);
        playback.status = VideoPreviewPlaybackStatus::Error;
        playback.segment_start = None;
        playback.error = Some(error.clone());
        if self.active_video_details(path).is_some() {
            self.preview = Some(PreviewState::Error(error));
            if self.preview_load_surface == PreviewLoadSurface::StandaloneWindow {
                return self.ensure_preview_window_for_standalone_load(PreviewWindowProfile::Video);
            }
        }
        Task::none()
    }

    /// 在画面上按了一下：菜单开着就只是关菜单；已经有一下在等判定就是双击（不切播放），
    /// 否则等一会儿再算单击。
    pub fn video_surface_pressed(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.track_menu.take().is_some() {
            return Task::none();
        }
        if playback.pending_click.take().is_some() {
            return Task::none();
        }
        playback.clicks += 1;
        let click = playback.clicks;
        playback.pending_click = Some(click);
        Task::perform(tokio::time::sleep(DOUBLE_CLICK_WINDOW), move |()| {
            PreviewMessage::VideoPreviewClickSettled(click)
        })
    }

    pub fn video_click_settled(&mut self, click: u64) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.pending_click != Some(click) {
            return Task::none();
        }
        playback.pending_click = None;
        self.toggle_video_preview_playback()
    }

    /// 帧流订阅：有一段解码就挂着（暂停时也挂着，不重开 ffmpeg）。
    pub fn active_video_preview_stream(&self) -> Option<VideoSegment> {
        let playback = self.video_preview.as_ref()?;
        self.active_video_details(&playback.path)?;
        Some(VideoSegment {
            path: playback.path.clone(),
            plan: Arc::clone(playback.plan.as_ref()?),
            clock: Arc::clone(&playback.clock),
            generation: playback.generation,
            start: playback.segment_start?,
            subtitle: match &playback.subtitle {
                VideoSubtitle::Showing { burn, .. } => Some(Arc::clone(burn)),
                VideoSubtitle::Off | VideoSubtitle::Preparing { .. } => None,
            },
        })
    }

    /// 进度刷新节拍只在播放中挂。
    pub fn video_preview_is_active(&self) -> bool {
        self.video_preview.as_ref().is_some_and(|playback| {
            playback.status == VideoPreviewPlaybackStatus::Playing
                && self.active_video_details(&playback.path).is_some()
        })
    }

    pub(crate) fn clear_video_preview(&mut self) {
        if let Some(playback) = self.video_preview.take() {
            if let Some(runtime) = playback.audio_runtime() {
                runtime.stop();
            }
        }
    }

    /// 从 `position` 开一段新的解码，`status` 是开好之后的状态（播放或停在那一帧）。
    pub(super) fn start_video_segment(
        &mut self,
        position: Duration,
        status: VideoPreviewPlaybackStatus,
    ) -> Task<PreviewMessage> {
        let volume = self.media_settings.volume;
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        if playback.plan.is_none() {
            return Task::none();
        }
        playback.generation += 1;
        playback.segment_start = Some(position);
        playback.position = position;
        playback.status = status;
        playback.seek_drag = None;
        playback.still_in_flight = None;
        playback.pending_still = None;
        playback.error = None;
        playback.clock.pause();
        playback.clock.seek(position);
        if let VideoSound::Running(runtime) = &playback.sound {
            // 先放完了改按墙钟的，跳回来要重新跟声音。
            playback.clock.follow_audio(runtime.clone());
            runtime.set_volume(volume);
            if let Err(error) = runtime.seek_to(position) {
                playback.error = Some(tr!("Audio unavailable: %1", error));
            }
            runtime.pause();
        }
        if status == VideoPreviewPlaybackStatus::Playing {
            return self.resume_video();
        }
        Task::none()
    }

    /// 接着当前这段放：时钟和声卡继续，第一次播还要把声音起起来。
    fn resume_video(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        playback.status = VideoPreviewPlaybackStatus::Playing;
        playback.clock.resume();
        match &playback.sound {
            VideoSound::Running(runtime) => {
                runtime.play();
                Task::none()
            }
            VideoSound::NotStarted => {
                playback.sound = VideoSound::Starting;
                start_video_preview_audio_command(
                    playback.path.clone(),
                    playback.generation,
                    playback.audio_track,
                    playback.clock.position(),
                )
            }
            VideoSound::Silent | VideoSound::Starting | VideoSound::Failed => Task::none(),
        }
    }

    /// 换上新的一帧（窗口在打开时已按画面比例定好）。
    fn show_video_picture(&mut self, frame: VideoPreviewFrame) -> Task<PreviewMessage> {
        if let Some(PreviewState::Ready(PreviewContent::Video {
            path,
            frame: current,
            ..
        })) = &mut self.preview
        {
            if *path == frame.path {
                *current = Some(frame.picture);
            }
        }
        Task::none()
    }

    pub(super) fn active_video_details(&self, path: &Path) -> Option<&VideoPreviewDetails> {
        match &self.preview {
            Some(PreviewState::Ready(PreviewContent::Video {
                path: preview_path,
                details,
                ..
            })) if preview_path == path => Some(details),
            _ => None,
        }
    }

    fn video_preview_matches(&self, path: &Path, generation: u64) -> bool {
        self.video_preview
            .as_ref()
            .is_some_and(|playback| playback.path == path && playback.generation == generation)
    }

    fn active_video_preview_mut(
        &mut self,
        path: &Path,
        generation: u64,
    ) -> Option<&mut VideoPreviewPlayback> {
        self.video_preview
            .as_mut()
            .filter(|playback| playback.path == path && playback.generation == generation)
    }

    fn video_preview_for_path_mut(&mut self, path: &Path) -> Option<&mut VideoPreviewPlayback> {
        self.video_preview
            .as_mut()
            .filter(|playback| playback.path == path)
    }
}

/// 停时钟和声卡；解码那一段留着（帧流停在等时钟上，ffmpeg 被管道背压挂起）。
fn pause_video(playback: &mut VideoPreviewPlayback) {
    playback.clock.pause();
    playback.position = clamp_position(playback.clock.position(), playback.duration);
    if let Some(runtime) = playback.audio_runtime() {
        runtime.pause();
    }
    if playback.status == VideoPreviewPlaybackStatus::Playing {
        playback.status = VideoPreviewPlaybackStatus::Paused;
    }
}

fn request_keyframe_still(
    playback: &mut VideoPreviewPlayback,
    position: Duration,
) -> Task<PreviewMessage> {
    let Some(plan) = playback.plan.clone() else {
        return Task::none();
    };
    playback.still_in_flight = Some(position);
    video_still_command(
        playback.path.clone(),
        plan,
        playback.generation,
        position,
        SeekPrecision::Keyframe,
    )
}

/// 一张关键帧解完：拖动中又换了位置就接着解最新的那个。
fn finish_still(playback: &mut VideoPreviewPlayback, position: Duration) -> Task<PreviewMessage> {
    if playback.still_in_flight != Some(position) {
        return Task::none();
    }
    playback.still_in_flight = None;
    match playback.pending_still.take() {
        Some(next) if next != position => request_keyframe_still(playback, next),
        _ => Task::none(),
    }
}

fn clamp_position(position: Duration, duration: Option<Duration>) -> Duration {
    match duration {
        Some(duration) => position.min(duration),
        None => position,
    }
}

#[cfg(test)]
#[path = "video_tests.rs"]
mod tests;
