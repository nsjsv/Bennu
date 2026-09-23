//! 动图预览状态机已迁 bennu-preview 的 PreviewEngine
//! （engine/animated_image.rs，逐字节保真）；此处保留返回宿主 Task 的
//! 同名薄转发。就绪分支的全局错误清除是宿主通知域，经验收结论回传；
//! 帧到达/结束/失败/seek 等纯引擎消息已由 engine.handle 直达处理
//! （update.rs 的散装分支保留作 From 回退路径）。

use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::animated_image_preview::AnimatedImagePreview;
use crate::model::Message;

impl FileBrowser {
    pub(in crate::app) fn accept_animated_image_preview_loaded(
        &mut self,
        path: PathBuf,
        generation: u64,
        preview_outcome: Result<AnimatedImagePreview, String>,
    ) -> Task<Message> {
        let (acceptance, command) = self.preview_engine.accept_animated_image_preview_loaded(
            path,
            generation,
            preview_outcome,
        );
        if matches!(
            acceptance,
            bennu_preview::engine::PreviewAcceptance::ContentReady
        ) {
            self.clear_global_error();
        }
        self.sync_preview_window_focus();
        command.map(Message::Preview)
    }

    pub(in crate::app) fn accept_animated_image_frame(
        &mut self,
        frame: crate::animated_image_preview::AnimatedImageFrame,
    ) -> Task<Message> {
        self.preview_engine
            .accept_animated_image_frame(frame)
            .map(Message::Preview)
    }

    pub(in crate::app) fn accept_animated_image_preview_finished(
        &mut self,
        path: PathBuf,
        generation: u64,
    ) -> Task<Message> {
        self.preview_engine
            .accept_animated_image_preview_finished(path, generation)
            .map(Message::Preview)
    }

    pub(in crate::app) fn accept_animated_image_preview_error(
        &mut self,
        path: PathBuf,
        generation: u64,
        error: String,
    ) -> Task<Message> {
        let command = self
            .preview_engine
            .accept_animated_image_preview_error(path, generation, error);
        self.sync_preview_window_focus();
        command.map(Message::Preview)
    }

    pub(in crate::app) fn seek_animated_image_preview(
        &mut self,
        position_seconds: f32,
    ) -> Task<Message> {
        self.preview_engine
            .seek_animated_image_preview(position_seconds)
            .map(Message::Preview)
    }

    pub(in crate::app) fn commit_animated_image_preview_seek(&mut self) -> Task<Message> {
        self.preview_engine
            .commit_animated_image_preview_seek()
            .map(Message::Preview)
    }
}
