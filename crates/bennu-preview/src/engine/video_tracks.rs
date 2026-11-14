//! 视频的字幕和音轨：打开时按规则挑默认的，菜单里换；信息层开关。字幕烧在画面里，换了要从当前位置
//! 重开一段解码（顿一下）；音轨换的是声卡背后的解码流，画面不停。

use std::path::Path;
use std::sync::Arc;

use bennu_localization::tr;
use iced::Task;

use crate::preview::{
    VideoPreviewDetails, VideoPreviewPlayback, VideoPreviewPlaybackStatus, VideoSound,
    VideoSubtitle, VideoTrackMenu,
};
use crate::preview_message::PreviewMessage;
use crate::video_preview::{default_subtitle, prepare_subtitle, SubtitleBurn};

impl super::PreviewEngine {
    /// 打开时：标了「默认」的音轨（没有就第一条），按规则挑字幕并开始准备。
    pub(super) fn choose_default_video_tracks(
        &mut self,
        details: &VideoPreviewDetails,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        playback.audio_track = details
            .probe
            .audio_tracks
            .iter()
            .find(|track| track.is_default)
            .or(details.probe.audio_tracks.first())
            .map_or(0, |track| track.ordinal);
        let choice = default_subtitle(&details.subtitles, bennu_localization::current_language());
        self.select_video_subtitle(choice)
    }

    pub fn toggle_video_info(&mut self) -> Task<PreviewMessage> {
        if let Some(playback) = self.video_preview.as_mut() {
            playback.info_visible = !playback.info_visible;
        }
        Task::none()
    }

    pub fn toggle_video_track_menu(&mut self, menu: VideoTrackMenu) -> Task<PreviewMessage> {
        if let Some(playback) = self.video_preview.as_mut() {
            playback.track_menu = (playback.track_menu != Some(menu)).then_some(menu);
        }
        Task::none()
    }

    /// 选字幕（None 是关掉）。只对这次预览有效。
    pub fn select_video_subtitle(&mut self, choice: Option<usize>) -> Task<PreviewMessage> {
        let Some(path) = self
            .video_preview
            .as_ref()
            .map(|playback| playback.path.clone())
        else {
            return Task::none();
        };
        let source = choice.and_then(|index| {
            self.active_video_details(&path)
                .and_then(|details| details.subtitles.get(index).cloned())
        });
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        playback.track_menu = None;
        match (choice, source) {
            (Some(index), Some(source)) => {
                if playback.subtitle.choice() == Some(index) {
                    return Task::none();
                }
                let ready_path = path.clone();
                let (task, work) = Task::perform(prepare_subtitle(path, source), move |outcome| {
                    PreviewMessage::VideoPreviewSubtitleReady(
                        ready_path.clone(),
                        index,
                        outcome.map(Arc::new),
                    )
                })
                .abortable();
                playback.subtitle = VideoSubtitle::Preparing {
                    choice: index,
                    work: work.abort_on_drop(),
                };
                task
            }
            _ => {
                let was_showing = matches!(playback.subtitle, VideoSubtitle::Showing { .. });
                playback.subtitle = VideoSubtitle::Off;
                if was_showing {
                    self.restart_video_segment_in_place()
                } else {
                    Task::none()
                }
            }
        }
    }

    pub fn accept_video_subtitle(
        &mut self,
        path: &Path,
        choice: usize,
        outcome: Result<Arc<SubtitleBurn>, String>,
    ) -> Task<PreviewMessage> {
        let Some(playback) = self
            .video_preview
            .as_mut()
            .filter(|playback| playback.path == path)
        else {
            return Task::none();
        };
        if !matches!(&playback.subtitle, VideoSubtitle::Preparing { choice: preparing, .. } if *preparing == choice)
        {
            return Task::none();
        }
        match outcome {
            Ok(burn) => {
                playback.subtitle = VideoSubtitle::Showing { choice, burn };
                self.restart_video_segment_in_place()
            }
            Err(error) => {
                playback.subtitle = VideoSubtitle::Off;
                playback.error = Some(tr!("Subtitle unavailable: %1", error));
                Task::none()
            }
        }
    }

    /// 换音轨：声卡在放就让它换流接着放（不重开声卡），还没起来就等起来时再换。
    pub fn select_video_audio_track(&mut self, track: usize) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_mut() else {
            return Task::none();
        };
        playback.track_menu = None;
        playback.audio_track = track;
        if let VideoSound::Running(runtime) = &playback.sound {
            if let Err(error) = runtime.switch_track(track, playback.clock.position()) {
                playback.error = Some(tr!("Audio unavailable: %1", error));
            }
            playback.clock.follow_audio(runtime.clone());
        }
        Task::none()
    }

    /// 字幕换了：正在解码的那一段从当前位置重开（播放/暂停状态不变）；没有在解码就等下次开段。
    fn restart_video_segment_in_place(&mut self) -> Task<PreviewMessage> {
        let Some(playback) = self.video_preview.as_ref() else {
            return Task::none();
        };
        if playback.segment_start.is_none() {
            return Task::none();
        }
        let status = continuing_status(playback);
        let position = playback.clock.position();
        self.start_video_segment(position, status)
    }
}

fn continuing_status(playback: &VideoPreviewPlayback) -> VideoPreviewPlaybackStatus {
    if playback.status == VideoPreviewPlaybackStatus::Playing {
        VideoPreviewPlaybackStatus::Playing
    } else {
        VideoPreviewPlaybackStatus::Paused
    }
}
