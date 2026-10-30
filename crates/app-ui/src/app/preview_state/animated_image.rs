//! 动图预览状态机在 bennu-preview 的 PreviewEngine（engine/animated_image.rs）；
//! 就绪分支的全局错误清除是宿主通知域，经验收结论回传。帧到达/结束/失败/
//! seek 等纯引擎消息由 `PreviewEngine::handle` 直接处理。

use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::app::global_error::GlobalErrorSource;
use crate::model::Message;
use bennu_preview::animated_image_preview::AnimatedImagePreview;

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
            self.retract_global_error(GlobalErrorSource::Preview);
        }
        command.map(Message::Preview)
    }
}
