//! 图片预览缩放/平移交互已迁 bennu-preview 的 PreviewEngine
//! （engine/image_viewport.rs，逐字节保真）；薄转发注入宿主侧呈现面
//! 视口尺寸（预览窗口/右侧面板两种表面的几何归宿主）。

use iced::Task;

use super::FileBrowser;
use crate::model::{Message, PreviewImageViewportMessage};

impl FileBrowser {
    pub(in crate::app) fn update_preview_image_viewport(
        &mut self,
        message: PreviewImageViewportMessage,
    ) -> Task<Message> {
        // 先取几何快照再借用引擎：方法实参求值借用整个 &self，与接收者
        // 的 &mut preview_engine 字段借用重叠，先绑定局部值隔开两者。
        let panel_size = self.preview_surface_viewport();
        self.preview_engine
            .update_preview_image_viewport(message, panel_size)
            .map(Message::Preview)
    }
}
