//! 文本/Markdown 预览内容与分块加载处理：自 app-ui 的
//! preview_state/text.rs 迁入（impl FileBrowser → impl PreviewEngine，
//! 逐字节保真；分块命令直连本 crate 命令层）。
//! 查看器内部滚动/滚动几何宿主同步（smooth_scroll、text_preview_viewer
//! 的部件 id）依赖宿主视图层，留在宿主侧同名方法。

use std::path::{Path, PathBuf};

use iced::Task;

use encoding_rs::Encoding;

use crate::commands::preview::text_preview_chunk_command;
use crate::preview::{PreviewContent, PreviewState};
use crate::preview_message::PreviewMessage;
use crate::text_preview::{TextPreviewChunk, TextPreviewDocument, TextPreviewFormat};
use crate::text_preview_loading::load_reencoded_text_preview;

impl super::PreviewEngine {
    pub fn handle_text_preview_content_scrolled(
        &mut self,
        lines: i32,
        viewport_height: f32,
    ) -> Task<PreviewMessage> {
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

    pub fn handle_markdown_preview_scrolled(
        &mut self,
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    ) -> Task<PreviewMessage> {
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };

        document
            .request_next_chunk_for_rendered_markdown(offset_y, viewport_height, content_height)
            .map(text_preview_chunk_command)
            .unwrap_or_else(Task::none)
    }

    pub fn accept_text_preview_chunk(
        &mut self,
        path: PathBuf,
        generation: u64,
        start_offset: u64,
        outcome: Result<TextPreviewChunk, String>,
    ) -> Task<PreviewMessage> {
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

    /// 按用户指定的编码重读当前文本预览。代次在发起时推进：期间又改编码或换了文件，
    /// 先发出的结果到达时代次已过期，直接丢弃。
    pub fn select_text_preview_encoding(
        &mut self,
        encoding: &'static Encoding,
    ) -> Task<PreviewMessage> {
        let Some(format) = active_text_preview_format(self.preview.as_ref()) else {
            return Task::none();
        };
        let Some(path) = self
            .active_text_preview_document_mut()
            .map(|document| document.path().to_path_buf())
        else {
            return Task::none();
        };
        self.text_preview_generation = self.text_preview_generation.wrapping_add(1);
        let generation = self.text_preview_generation;
        Task::perform(
            load_reencoded_text_preview(path.clone(), encoding, format),
            move |outcome| PreviewMessage::TextPreviewReencoded {
                path: path.clone(),
                generation,
                outcome,
            },
        )
    }

    /// 重读结果替换文档内容；Markdown 原文/渲染模式是用户当前的选择，保留。
    pub fn accept_reencoded_text_preview(
        &mut self,
        path: PathBuf,
        generation: u64,
        outcome: Result<PreviewContent, String>,
    ) -> Task<PreviewMessage> {
        if generation != self.text_preview_generation {
            return Task::none();
        }
        let Some(mode) = self
            .active_text_preview_document_mut()
            .filter(|document| document.path() == path.as_path())
            .map(|document| document.markdown_preview_mode())
        else {
            return Task::none();
        };
        match outcome {
            Ok(content) => {
                self.text_preview_document = TextPreviewDocument::from_loaded(&content, generation)
                    .map(|mut document| {
                        document.select_markdown_preview_mode(mode);
                        document
                    });
                self.preview = Some(PreviewState::Ready(content));
            }
            Err(error) => {
                self.text_preview_document = None;
                self.preview = Some(PreviewState::Error(error));
            }
        }
        Task::none()
    }

    pub fn active_text_preview_document_mut(&mut self) -> Option<&mut TextPreviewDocument> {
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
        encoding: document.encoding(),
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
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use encoding_rs::{Encoding, GBK, UTF_8};

    use super::*;
    use crate::engine::{PreviewEngine, PreviewEngineConfig, PreviewWindowIdentity};
    use crate::preview::PreviewWindowProfile;
    use crate::preview_config::PreviewFileSizeLimits;
    use crate::text_preview::MarkdownPreviewMode;

    fn markdown_content(path: &Path, encoding: &'static Encoding, text: &str) -> PreviewContent {
        PreviewContent::Text {
            path: path.to_path_buf(),
            rendered: Arc::from(text),
            format: TextPreviewFormat::Markdown,
            encoding,
            next_offset: None,
            loaded_line_count: 1,
            line_limit_notice: None,
        }
    }

    fn engine_showing(content: PreviewContent) -> PreviewEngine {
        let mut engine = PreviewEngine::new(
            PreviewEngineConfig {
                directory_expand_levels: 1,
                extension_rules: Default::default(),
                size_limits: PreviewFileSizeLimits::with_default_limits(),
            },
            crate::engine::default_preview_size(PreviewWindowProfile::Regular),
            PreviewWindowIdentity {
                app_id: "bennu-preview-tests".to_owned(),
                icon: None,
            },
        );
        engine.text_preview_generation = 1;
        engine.text_preview_document = TextPreviewDocument::from_loaded(&content, 1);
        engine.preview = Some(PreviewState::Ready(content));
        engine
    }

    #[test]
    fn only_the_latest_reencode_replaces_the_document_and_keeps_the_mode() {
        let path = PathBuf::from("/tmp/notes.md");
        let mut engine = engine_showing(markdown_content(&path, UTF_8, "original"));
        engine
            .text_preview_document
            .as_mut()
            .expect("document")
            .select_markdown_preview_mode(MarkdownPreviewMode::Raw);

        let _ = engine.select_text_preview_encoding(GBK);
        let _ = engine.select_text_preview_encoding(GBK);
        let latest = engine.text_preview_generation;

        // 先发出的那次重读晚到：代次已过期，丢弃。
        let _ = engine.accept_reencoded_text_preview(
            path.clone(),
            latest - 1,
            Ok(markdown_content(&path, GBK, "stale")),
        );
        let document = engine.text_preview_document.as_ref().expect("document");
        assert_eq!(document.encoding(), UTF_8);
        assert_eq!(document.content(), "original");

        let _ = engine.accept_reencoded_text_preview(
            path.clone(),
            latest,
            Ok(markdown_content(&path, GBK, "reencoded")),
        );
        let document = engine.text_preview_document.as_ref().expect("document");
        assert_eq!(document.encoding(), GBK);
        assert_eq!(document.content(), "reencoded");
        assert_eq!(document.markdown_preview_mode(), MarkdownPreviewMode::Raw);
    }
}
