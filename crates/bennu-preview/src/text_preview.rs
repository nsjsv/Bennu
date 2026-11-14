//! 文本预览的格式与 Markdown 模式；文档本身是按需读取的窗口模型（`text_document`）。

pub use crate::text_document::TextPreviewDocument;

pub const TEXT_PREVIEW_TEXT_SIZE: f32 = 16.0;
pub const TEXT_PREVIEW_LINE_HEIGHT: f32 = 1.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextPreviewFormat {
    Plain,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownPreviewMode {
    Rendered,
    Raw,
}

/// Markdown 模式切换行的样子：能切换（当前选中哪个），或文件太大只能看原文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownModeChoice {
    Switchable(MarkdownPreviewMode),
    RawOnly,
}

impl MarkdownModeChoice {
    pub fn for_document(document: &TextPreviewDocument) -> Self {
        match document.markdown() {
            Some(_) => Self::Switchable(document.markdown_preview_mode()),
            None => Self::RawOnly,
        }
    }
}
