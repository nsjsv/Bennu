//! 一张接一张看图（方向键、右侧面板跟着选中走）：新图加载时上一张留在屏幕上当底，新图第一帧
//! 到了才换，不闪「加载中」；独立窗口已经是图片窗口时不再按新图改大小（用户调过的大小留着）。

use std::path::PathBuf;

use iced::Task;

use crate::preview::{
    ImagePreviewContent, ImageView, PreviewContent, PreviewState, PreviewWindowProfile,
};
use crate::preview_message::PreviewMessage;

impl super::PreviewEngine {
    /// 开始加载一张图（静态或动图）：显示进入加载中，正显示的图留作底。
    pub fn show_image_loading(&mut self, path: PathBuf) {
        let backdrop = match self.preview.take() {
            Some(PreviewState::Ready(PreviewContent::Image(content))) => Some(content),
            Some(PreviewState::Ready(PreviewContent::AnimatedImage(animated))) => {
                Some(ImagePreviewContent::Thumbnail {
                    path: animated.path().to_path_buf(),
                    handle: animated.current_frame_handle().clone(),
                    width: animated.width(),
                    height: animated.height(),
                    max_edge: 0,
                })
            }
            // 连按方向键：没来得及出来的那张跳过，底留着最后真正显示过的那张。
            Some(PreviewState::Loading(_)) => {
                self.loading_backdrop.take().map(|(_, content)| content)
            }
            _ => None,
        };
        self.loading_backdrop = backdrop.map(|content| (path.clone(), content));
        self.preview = Some(PreviewState::Loading(path));
    }

    pub fn image_view(&self) -> ImageView<'_> {
        ImageView {
            viewport: &self.preview_image_viewport,
            info: self.visible_image_info(),
            loading_backdrop: self
                .loading_backdrop
                .as_ref()
                .and_then(|(loading, content)| {
                    matches!(&self.preview, Some(PreviewState::Loading(path)) if path == loading)
                        .then_some(content)
                }),
        }
    }

    /// 图片尺寸知道了：独立窗口按它开窗；窗口已经是图片窗口（在里面换图）就不动它的大小。
    /// `None` 是没有固有尺寸的 SVG，按默认大小。
    pub fn fit_image_preview_window(&mut self, size: Option<(u32, u32)>) -> Task<PreviewMessage> {
        if self.preview_load_surface != super::PreviewLoadSurface::StandaloneWindow
            || self.shows_image_window()
        {
            return Task::none();
        }
        match size {
            Some((width, height)) => self.open_image_preview_window_for_dimensions(width, height),
            None => self.open_image_preview_window_with_default_size(),
        }
    }

    pub(crate) fn shows_image_window(&self) -> bool {
        self.preview_window.is_some() && self.preview_window_profile == PreviewWindowProfile::Image
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use iced::widget::image;

    use crate::engine::test_support::test_engine;
    use crate::engine::PreviewLoadSurface;
    use crate::preview::{ImagePreviewContent, PreviewContent, PreviewState, PreviewWindowProfile};

    fn photo(path: &str) -> PreviewState {
        PreviewState::Ready(PreviewContent::Image(ImagePreviewContent::Thumbnail {
            path: PathBuf::from(path),
            handle: image::Handle::from_rgba(1, 1, vec![0; 4]),
            width: 1,
            height: 1,
            max_edge: 1,
        }))
    }

    #[test]
    fn the_last_shown_image_stays_until_the_next_one_arrives() {
        let mut engine = test_engine(PreviewLoadSurface::RightDockedPanel);
        engine.preview = Some(photo("/p/a.jpg"));
        engine.show_image_loading(PathBuf::from("/p/b.jpg"));
        assert!(engine.image_view().loading_backdrop.is_some());
        // 连按：还没出来的 b 跳过，底还是 a。
        engine.show_image_loading(PathBuf::from("/p/c.jpg"));
        assert!(matches!(
            engine.image_view().loading_backdrop,
            Some(ImagePreviewContent::Thumbnail { path, .. }) if path == &PathBuf::from("/p/a.jpg")
        ));
        // 别的类型直接进加载中（视频）：不垫旧图。
        engine.preview = Some(PreviewState::Loading(PathBuf::from("/p/clip.mp4")));
        assert!(engine.image_view().loading_backdrop.is_none());
        engine.clear_preview();
        assert!(engine.loading_backdrop.is_none());
    }

    #[test]
    fn an_image_window_keeps_its_size_for_the_next_image() {
        let mut engine = test_engine(PreviewLoadSurface::StandaloneWindow);
        engine.preview_window = Some(iced::window::Id::unique());
        engine.preview_window_profile = PreviewWindowProfile::Regular;
        assert!(engine.fit_image_preview_window(Some((400, 300))).units() > 0);
        assert_eq!(engine.preview_window_profile, PreviewWindowProfile::Image);
        assert_eq!(engine.fit_image_preview_window(Some((300, 400))).units(), 0);
        // 右侧面板从不动窗口。
        let mut panel = test_engine(PreviewLoadSurface::RightDockedPanel);
        assert_eq!(panel.fit_image_preview_window(Some((400, 300))).units(), 0);
    }
}
