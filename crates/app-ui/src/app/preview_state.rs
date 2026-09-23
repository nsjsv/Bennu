//! 预览会话/音视频播放状态机已迁 bennu-preview 的 PreviewEngine
//! （engine/session.rs + engine/media.rs，逐字节保真）；此处保留同名
//! 薄转发：Task 经 `.map(Message::Preview)` 包装，update.rs 调用点零
//! 改动。宿主域差异以转发层补齐——全局错误通知（accept_preview 就绪
//! 分支与音频跳转失败）经验收结论/错误串回传，目录展开的扫描选项与
//! 展开层数实时读取 user_config 传入（引擎配置快照只随持久化刷新）。
//! clear_preview / clear_audio_preview / active_video_preview_stream /
//! audio_preview_is_active / video_preview_is_active 的签名不含宿主
//! 消息，经 FileBrowser 的 Deref 垫片直接命中引擎同名方法。

use std::path::PathBuf;
use std::time::Duration;

use iced::Task;

use super::FileBrowser;
use crate::model::{Message, PreviewContent};

mod animated_image;
mod document;
mod image_viewport;
mod original_image;
mod remote_cache;
mod sqlite;
#[cfg(test)]
mod tests;
mod text;
mod tree;

impl FileBrowser {
    pub(super) fn accept_preview(
        &mut self,
        path: PathBuf,
        preview_outcome: Result<PreviewContent, String>,
    ) -> Task<Message> {
        let options = self.options.clone();
        let expand_levels = self.preview_directory_expand_levels();
        let (acceptance, command) =
            self.preview_engine
                .accept_preview(path, preview_outcome, options, expand_levels);
        if matches!(
            acceptance,
            bennu_preview::engine::PreviewAcceptance::ContentReady
        ) {
            self.clear_global_error();
        }
        command.map(Message::Preview)
    }

    pub(super) fn toggle_audio_preview_playback(&mut self) -> Task<Message> {
        self.preview_engine
            .toggle_audio_preview_playback()
            .map(Message::Preview)
    }

    pub(super) fn accept_audio_preview_started(
        &mut self,
        path: PathBuf,
        playback_outcome: Result<crate::audio_preview::AudioPreviewRuntime, String>,
    ) -> Task<Message> {
        self.preview_engine
            .accept_audio_preview_started(path, playback_outcome)
            .map(Message::Preview)
    }

    pub(super) fn seek_audio_preview_playback(&mut self, position_seconds: f32) -> Task<Message> {
        if let Some(error) = self
            .preview_engine
            .seek_audio_preview_playback(position_seconds)
        {
            self.show_global_error(error);
        }
        Task::none()
    }

    pub(super) fn change_audio_preview_volume(&mut self, volume: f32) -> Task<Message> {
        self.preview_engine
            .change_audio_preview_volume(volume)
            .map(Message::Preview)
    }

    pub(super) fn update_audio_preview_playback(&mut self) -> Task<Message> {
        self.preview_engine
            .update_audio_preview_playback()
            .map(Message::Preview)
    }

    pub(super) fn toggle_video_preview_playback(&mut self) -> Task<Message> {
        self.preview_engine
            .toggle_video_preview_playback()
            .map(Message::Preview)
    }

    pub(super) fn accept_video_preview_audio_started(
        &mut self,
        path: PathBuf,
        generation: u64,
        audio_outcome: Result<crate::audio_preview::AudioPreviewRuntime, String>,
    ) -> Task<Message> {
        self.preview_engine
            .accept_video_preview_audio_started(path, generation, audio_outcome)
            .map(Message::Preview)
    }

    pub(super) fn accept_video_preview_metadata(
        &mut self,
        path: PathBuf,
        metadata_outcome: Result<Option<Duration>, String>,
    ) -> Task<Message> {
        self.preview_engine
            .accept_video_preview_metadata(path, metadata_outcome)
            .map(Message::Preview)
    }

    pub(super) fn seek_video_preview_playback(&mut self, position_seconds: f32) -> Task<Message> {
        self.preview_engine
            .seek_video_preview_playback(position_seconds)
            .map(Message::Preview)
    }

    pub(super) fn commit_video_preview_seek(&mut self) -> Task<Message> {
        self.preview_engine
            .commit_video_preview_seek()
            .map(Message::Preview)
    }

    pub(super) fn change_video_preview_volume(&mut self, volume: f32) -> Task<Message> {
        self.preview_engine
            .change_video_preview_volume(volume)
            .map(Message::Preview)
    }

    pub(super) fn update_video_preview_playback(&mut self) -> Task<Message> {
        self.preview_engine
            .update_video_preview_playback()
            .map(Message::Preview)
    }

    pub(super) fn accept_video_preview_frame(
        &mut self,
        video_frame: crate::model::VideoPreviewFrame,
    ) -> Task<Message> {
        self.preview_engine
            .accept_video_preview_frame(video_frame)
            .map(Message::Preview)
    }

    pub(super) fn accept_video_preview_seek_frame_error(
        &mut self,
        path: PathBuf,
        generation: u64,
        position: Duration,
        error: String,
    ) -> Task<Message> {
        self.preview_engine
            .accept_video_preview_seek_frame_error(path, generation, position, error)
            .map(Message::Preview)
    }

    pub(super) fn accept_video_preview_finished(
        &mut self,
        path: PathBuf,
        generation: u64,
    ) -> Task<Message> {
        self.preview_engine
            .accept_video_preview_finished(path, generation)
            .map(Message::Preview)
    }

    pub(super) fn accept_video_preview_error(
        &mut self,
        path: PathBuf,
        generation: u64,
        error: String,
    ) -> Task<Message> {
        self.preview_engine
            .accept_video_preview_error(path, generation, error)
            .map(Message::Preview)
    }
}
