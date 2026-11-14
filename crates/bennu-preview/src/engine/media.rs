//! 音频、视频预览共用的部分：播放快捷键分给当前的音频或视频，进度刷新节拍。

use std::time::Duration;

use iced::{Subscription, Task};

use crate::preview_message::{MediaKeyCommand, PreviewMessage};

impl super::PreviewEngine {
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
