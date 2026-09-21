use std::path::{Path, PathBuf};

use iced::Task;

use super::FileBrowser;
use crate::commands::text_preview_chunk_command;
use crate::model::{
    Message, PreviewContent, PreviewState, ScrollbarRegion, TextPreviewChunk, TextPreviewDocument,
    TextPreviewFormat,
};

impl FileBrowser {
    pub(in crate::app) fn handle_text_preview_content_scrolled(
        &mut self,
        lines: i32,
        viewport_height: f32,
    ) -> Task<Message> {
        // 滚动几何宿主是输入源（滚轮动画/滚动条），查看器从动；
        // 这里只镜像文档副本以预取分块，不回推宿主，避免滞后回拉振荡。
        self.active_text_preview_document_mut()
            .map(|document| {
                document
                    .scroll_by(lines, viewport_height)
                    .map(text_preview_chunk_command)
                    .unwrap_or_else(Task::none)
            })
            .unwrap_or_else(Task::none)
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
            .active_text_preview_document_mut()
            .map(|document| {
                document
                    .scroll_by(lines, viewport_height)
                    .map(text_preview_chunk_command)
                    .unwrap_or_else(Task::none)
            })
            .unwrap_or_else(Task::none);
        Task::batch([chunk_task, self.scroll_text_preview_geometry(offset_y)])
    }

    pub(in crate::app) fn handle_text_preview_viewport_synced(
        &mut self,
        offset_y: f32,
        _viewport_height: f32,
    ) -> Task<Message> {
        // 滚动几何宿主变化（滚轮动画/滚动条拖动）驱动查看器像素滚动。
        iced::widget::operation::scroll_to(
            iced::widget::Id::new(crate::text_preview_viewer::TEXT_PREVIEW_VIEWER_ID),
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
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };

        document
            .request_next_chunk_for_rendered_markdown(offset_y, viewport_height, content_height)
            .map(text_preview_chunk_command)
            .unwrap_or_else(Task::none)
    }

    pub(in crate::app) fn accept_text_preview_chunk(
        &mut self,
        path: PathBuf,
        generation: u64,
        start_offset: u64,
        outcome: Result<TextPreviewChunk, String>,
    ) -> Task<Message> {
        let Some(format) = active_text_preview_format(self.preview.as_ref()) else {
            return Task::none();
        };
        let preview_height = self.preview_size.height;
        let Some(document) = self.active_text_preview_document_mut().filter(|document| {
            document.path() == path.as_path() && document.generation() == generation
        }) else {
            return Task::none();
        };

        match outcome {
            Ok(chunk) => {
                if chunk.start_offset != start_offset {
                    return Task::none();
                }
                if !document.append_chunk(chunk, preview_height) {
                    return Task::none();
                }
                self.preview = Some(text_preview_state_from_document(path, format, document));
            }
            Err(error) => {
                document.accept_chunk_error(start_offset, error);
            }
        }

        Task::none()
    }

    fn active_text_preview_document_mut(&mut self) -> Option<&mut TextPreviewDocument> {
        let path = active_text_preview_path(self.preview.as_ref())?;
        self.text_preview_document
            .as_mut()
            .filter(|document| document.path() == path)
    }
}

fn text_preview_state_from_document(
    path: PathBuf,
    format: TextPreviewFormat,
    document: &TextPreviewDocument,
) -> PreviewState {
    PreviewState::Ready(PreviewContent::Text {
        path,
        rendered: document.shared_content(),
        format,
        next_offset: document.next_offset(),
        loaded_line_count: document.loaded_line_count(),
        line_limit_notice: document.line_limit_notice(),
    })
}

fn active_text_preview_path(preview: Option<&PreviewState>) -> Option<&Path> {
    match preview? {
        PreviewState::Ready(PreviewContent::Text { path, .. }) => Some(path.as_path()),
        _ => None,
    }
}

fn active_text_preview_format(preview: Option<&PreviewState>) -> Option<TextPreviewFormat> {
    match preview? {
        PreviewState::Ready(PreviewContent::Text { format, .. }) => Some(*format),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
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
