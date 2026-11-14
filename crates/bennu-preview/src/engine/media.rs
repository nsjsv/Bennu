//! 音频、视频预览共用的部分：播放快捷键分给当前的音频或视频，「播下一个」的排队，进度刷新节拍。

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::{Subscription, Task};

use crate::preview_message::{MediaKeyCommand, PreviewMessage};

use super::video_fullscreen::VideoFullscreen;

/// 「播下一个」时宿主排进来的下一个：打开它时右侧面板也直接播，全屏着的接着全屏。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMedia {
    pub path: PathBuf,
    pub fullscreen: Option<VideoFullscreen>,
}

impl super::PreviewEngine {
    /// 宿主找到了下一个、即将把选中移过去：打开它时直接开播，全屏状态带过去。
    pub fn queue_next_media(&mut self, path: PathBuf) {
        self.queued_media = Some(QueuedMedia {
            path,
            fullscreen: self.video_fullscreen,
        });
    }

    /// 打开 `path` 时取走排队记录（只认一次；排的不是它就作废）。
    pub(crate) fn take_queued_media(&mut self, path: &Path) -> Option<QueuedMedia> {
        self.queued_media
            .take()
            .filter(|queued| queued.path == path)
    }

    /// 视频铺满主窗口时，下一个视频正在打开（中间有一段没有视频的加载）也算还在全屏，
    /// 宿主不该在这时把主窗口退回去再切回来。
    pub fn keeps_video_fullscreen(&self) -> bool {
        self.video_preview.is_some()
            || self
                .queued_media
                .as_ref()
                .is_some_and(|queued| queued.fullscreen.is_some())
    }

    pub(super) fn handle_media_key(&mut self, command: MediaKeyCommand) -> Task<PreviewMessage> {
        let video_is_active = self.video_preview.is_some();
        match command {
            MediaKeyCommand::TogglePlayback if video_is_active => {
                self.toggle_video_preview_playback()
            }
            MediaKeyCommand::TogglePlayback => self.toggle_audio_preview_playback(),
            MediaKeyCommand::SeekForward(step) | MediaKeyCommand::SeekBackward(step) => {
                let forward = matches!(command, MediaKeyCommand::SeekForward(_));
                if video_is_active {
                    let Some(playback) = self.video_preview.as_ref() else {
                        return Task::none();
                    };
                    let target = step_position(playback.position, step, forward, playback.duration);
                    return self.seek_video_to(target);
                }
                let duration = self.active_audio_duration();
                let Some(playback) = self.audio_preview.as_mut() else {
                    return Task::none();
                };
                playback.position = step_position(playback.position, step, forward, duration);
                self.commit_audio_preview_seek()
            }
            MediaKeyCommand::ChangeVolume(delta) => Task::done(PreviewMessage::MediaVolumeChanged(
                (self.media_settings.volume + delta).clamp(0.0, 1.0),
            )),
            MediaKeyCommand::ToggleFullscreen => self.toggle_video_fullscreen(),
            MediaKeyCommand::ToggleInfo => self.toggle_video_info(),
        }
    }
}

/// 快捷键前进/后退一步，停在开头和结尾之间。
fn step_position(
    position: Duration,
    step: Duration,
    forward: bool,
    duration: Option<Duration>,
) -> Duration {
    let target = if forward {
        position.saturating_add(step)
    } else {
        position.saturating_sub(step)
    };
    match duration {
        Some(duration) => target.min(duration),
        None => target,
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
