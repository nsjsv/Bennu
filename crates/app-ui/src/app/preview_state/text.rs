//! 文本/Markdown 预览的内容滚动与分块加载已迁 bennu-preview 的
//! PreviewEngine（engine/text.rs，逐字节保真）；此处保留同名薄转发，
//! 以及两个依赖宿主视图层（smooth_scroll 滚动几何宿主、
//! text_preview_viewer 部件 id）的方法——它们随视图组迁移再下沉。

use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::model::{Message, ScrollbarRegion, TextPreviewChunk};

impl FileBrowser {
    pub(in crate::app) fn handle_text_preview_content_scrolled(
        &mut self,
        lines: i32,
        viewport_height: f32,
    ) -> Task<Message> {
        self.preview_engine
            .handle_text_preview_content_scrolled(lines, viewport_height)
            .map(Message::Preview)
    }

    pub(in crate::app) fn handle_text_preview_viewer_scrolled(
        &mut self,
        lines: i32,
        offset_y: f32,
        viewport_height: f32,
    ) -> Task<Message> {
        // 查看器内部滚动（键盘/光标跟随）镜像到文档副本以预取分块，
        // 并把总偏移同步给滚动几何宿主（宿主此时是静止的，无竞争）。
        let chunk_task = self
            .preview_engine
            .handle_text_preview_content_scrolled(lines, viewport_height)
            .map(Message::Preview);
        Task::batch([chunk_task, self.scroll_text_preview_geometry(offset_y)])
    }

    pub(in crate::app) fn handle_text_preview_viewport_synced(
        &mut self,
        offset_y: f32,
        _viewport_height: f32,
    ) -> Task<Message> {
        // 滚动几何宿主变化（滚轮动画/滚动条拖动）驱动查看器像素滚动。
        iced::widget::operation::scroll_to(
            iced::widget::Id::new(bennu_preview::text_preview_viewer::TEXT_PREVIEW_VIEWER_ID),
            iced::widget::scrollable::AbsoluteOffset {
                x: 0.0,
                y: offset_y,
            },
        )
    }

    fn scroll_text_preview_geometry(&mut self, offset_y: f32) -> Task<Message> {
        iced::widget::operation::scroll_to(
            crate::app::smooth_scroll::smooth_scroll_id(&ScrollbarRegion::TextPreview),
            iced::widget::scrollable::AbsoluteOffset {
                x: 0.0,
                y: offset_y,
            },
        )
    }

    pub(in crate::app) fn handle_markdown_preview_scrolled(
        &mut self,
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    ) -> Task<Message> {
        self.preview_engine
            .handle_markdown_preview_scrolled(offset_y, viewport_height, content_height)
            .map(Message::Preview)
    }

    pub(in crate::app) fn accept_text_preview_chunk(
        &mut self,
        path: PathBuf,
        generation: u64,
        start_offset: u64,
        outcome: Result<TextPreviewChunk, String>,
    ) -> Task<Message> {
        self.preview_engine
            .accept_text_preview_chunk(path, generation, start_offset, outcome)
            .map(Message::Preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::model::{PreviewContent, PreviewState, TextPreviewDocument};
    use crate::text_preview::TextPreviewFormat;
    use std::sync::Arc;

    fn browser_with_loading_text_preview() -> (FileBrowser, PathBuf) {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let path = PathBuf::from("/tmp/large.txt");
        let content = (0..50)
            .map(|line_number| format!("line {line_number}"))
            .collect::<Vec<_>>()
            .join("\n");
        browser.text_preview_generation = 1;
        browser.preview = Some(PreviewState::Ready(PreviewContent::Text {
            path: path.clone(),
            rendered: Arc::from(content.as_str()),
            format: TextPreviewFormat::Plain,
            next_offset: Some(100),
            loaded_line_count: 50,
            line_limit_notice: None,
        }));
        let mut document = TextPreviewDocument::new_initial(
            path.clone(),
            &content,
            TextPreviewFormat::Plain,
            1,
            Some(100),
            50,
            None,
        );
        document.scroll_by(21, 400.0).expect("chunk request");
        browser.text_preview_document = Some(document);
        (browser, path)
    }

    #[test]
    fn stale_text_preview_chunk_generation_is_ignored() {
        let (mut browser, path) = browser_with_loading_text_preview();

        drop(browser.accept_text_preview_chunk(
            path,
            0,
            100,
            Ok(TextPreviewChunk {
                start_offset: 100,
                content: "stale".to_owned(),
                line_count: 1,
                next_offset: None,
                line_limit_notice: None,
            }),
        ));

        let document = browser.text_preview_document.as_ref().expect("document");
        assert!(!document.content().contains("stale"));
    }

    #[test]
    fn stale_text_preview_chunk_path_is_ignored() {
        let (mut browser, _path) = browser_with_loading_text_preview();

        drop(browser.accept_text_preview_chunk(
            PathBuf::from("/tmp/other.txt"),
            1,
            100,
            Ok(TextPreviewChunk {
                start_offset: 100,
                content: "stale".to_owned(),
                line_count: 1,
                next_offset: None,
                line_limit_notice: None,
            }),
        ));

        let document = browser.text_preview_document.as_ref().expect("document");
        assert!(!document.content().contains("stale"));
    }
}
