//! 图片信息层：按钮或 I 键开关；开着时每张图到了就在后台读它的文件头（拍摄参数等），
//! 换图后信息层留着，接着显示下一张的。

use std::sync::Arc;

use iced::Task;

use crate::commands::preview::image_info_command;
use crate::preview::{ImageInfo, PreviewContent, PreviewState};
use crate::preview_message::PreviewMessage;

impl super::PreviewEngine {
    pub(crate) fn toggle_image_info(&mut self) -> Task<PreviewMessage> {
        self.image_info_visible = !self.image_info_visible;
        self.request_image_info()
    }

    /// 信息层开着、显示的这张还没读过就去读。
    pub(crate) fn request_image_info(&mut self) -> Task<PreviewMessage> {
        if !self.image_info_visible {
            return Task::none();
        }
        let Some(PreviewState::Ready(PreviewContent::Image(content))) = &self.preview else {
            return Task::none();
        };
        let Some((path, kind)) = content.shown_source() else {
            return Task::none();
        };
        if self
            .image_info
            .as_ref()
            .is_some_and(|info| info.path == path)
        {
            return Task::none();
        }
        image_info_command(path.to_path_buf(), kind)
    }

    pub(crate) fn accept_image_info(&mut self, info: Arc<ImageInfo>) -> Task<PreviewMessage> {
        if self.shown_image_path() == Some(info.path.as_path()) {
            self.image_info = Some(info);
        }
        Task::none()
    }

    /// 视图用：信息层开着并且读到的是显示的这张。
    pub fn visible_image_info(&self) -> Option<&ImageInfo> {
        let info = self.image_info.as_deref()?;
        (self.image_info_visible && self.shown_image_path() == Some(info.path.as_path()))
            .then_some(info)
    }

    fn shown_image_path(&self) -> Option<&std::path::Path> {
        match &self.preview {
            Some(PreviewState::Ready(PreviewContent::Image(content))) => {
                content.shown_source().map(|(path, _)| path)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::engine::{
        default_preview_size, PreviewEngine, PreviewEngineConfig, PreviewWindowIdentity,
    };
    use crate::preview::{
        ImageInfo, ImagePreviewContent, PreviewContent, PreviewState, PreviewWindowProfile,
    };
    use crate::preview_config::PreviewFileSizeLimits;

    fn engine_showing(path: &str) -> PreviewEngine {
        let mut engine = PreviewEngine::new(
            PreviewEngineConfig {
                directory_expand_levels: 1,
                extension_rules: Default::default(),
                size_limits: PreviewFileSizeLimits::with_default_limits(),
            },
            default_preview_size(PreviewWindowProfile::Regular),
            PreviewWindowIdentity {
                app_id: "bennu-preview-tests".to_owned(),
                icon: None,
            },
        );
        engine.preview = Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::OriginalSvg {
                path: PathBuf::from(path),
                handle: iced::widget::svg::Handle::from_memory(Vec::new()),
                width: 10,
                height: 10,
            },
        )));
        engine
    }

    fn info(path: &str) -> Arc<ImageInfo> {
        Arc::new(ImageInfo {
            path: PathBuf::from(path),
            facts: Default::default(),
            byte_len: 7,
        })
    }

    #[test]
    fn info_shows_only_for_the_image_it_was_read_from() {
        let mut engine = engine_showing("/workspace/a.svg");
        assert_eq!(engine.toggle_image_info().units(), 1);
        drop(engine.accept_image_info(info("/workspace/other.svg")));
        assert!(engine.visible_image_info().is_none());
        drop(engine.accept_image_info(info("/workspace/a.svg")));
        assert_eq!(
            engine.visible_image_info().map(|info| info.byte_len),
            Some(7)
        );
        // 读过的不再读；关掉后不显示。
        assert_eq!(engine.request_image_info().units(), 0);
        drop(engine.toggle_image_info());
        assert!(engine.visible_image_info().is_none());
    }
}
