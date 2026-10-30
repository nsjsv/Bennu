//! 预览会话/音视频播放状态机在 bennu-preview 的 PreviewEngine
//! （engine/session.rs + engine/media.rs）；宿主在这里接预览消息：
//! [`FileBrowser::handle_preview_message`] 是唯一入口，引擎自足的变体由
//! 引擎处理，其余变体依赖宿主的扫描选项、视口几何、滚动管线或通知域，
//! 在本模块及子模块里各有一个处理者。宿主域差异以参数/返回值补齐——
//! 全局错误通知（accept_preview 就绪分支与音频跳转失败）经验收结论/错误串
//! 回传，目录展开的扫描选项与展开层数实时读取 user_config 传入（引擎配置
//! 快照只随持久化刷新）。

use std::path::PathBuf;

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::model::{Message, PreviewContent, PreviewMessage, ScrollbarRegion};

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
    pub(super) fn handle_preview_message(&mut self, message: PreviewMessage) -> Task<Message> {
        match self.preview_engine.handle(message) {
            Ok(task) => task.map(Message::Preview),
            Err(host_message) => self.handle_host_preview_message(*host_message),
        }
    }

    /// 引擎经 `Err` 还回的变体。与 `PreviewEngine::handle` 的列表互补：
    /// 引擎已处理的变体不会到这里，所以没有兜底分支掩盖遗漏。
    fn handle_host_preview_message(&mut self, message: PreviewMessage) -> Task<Message> {
        match message {
            PreviewMessage::PreviewLoaded(path, outcome) => self.accept_preview(path, outcome),
            PreviewMessage::DocumentPreview(message) => {
                self.handle_document_preview_message(message)
            }
            PreviewMessage::SqliteTablesResizeStarted => self.start_sqlite_tables_resize_drag(),
            PreviewMessage::RemotePreviewCache(message) => {
                self.accept_remote_preview_cache_message(message)
            }
            PreviewMessage::AnimatedImagePreviewLoaded(path, generation, outcome) => {
                self.accept_animated_image_preview_loaded(path, generation, outcome)
            }
            PreviewMessage::OriginalImagePreviewLoaded(path, generation, outcome) => {
                self.accept_original_image_preview(path, generation, outcome)
            }
            PreviewMessage::RetryImagePreview(path) => self.retry_image_preview(path),
            PreviewMessage::PreviewImageViewport(message) => {
                self.update_preview_image_viewport(message)
            }
            PreviewMessage::PreviewDirectoryChildrenLoaded(parent_path, outcome) => {
                self.accept_preview_directory_children(parent_path, outcome)
            }
            PreviewMessage::TextPreviewViewerScrolled {
                lines,
                offset_y,
                viewport_height,
            } => self.handle_text_preview_viewer_scrolled(lines, offset_y, viewport_height),
            PreviewMessage::TextPreviewViewportSynced {
                offset_y,
                viewport_height,
            } => self.handle_text_preview_viewport_synced(offset_y, viewport_height),
            PreviewMessage::TextPreviewContentHeightChanged(content_height) => {
                self.preview_engine.text_preview_content_height = content_height;
                Task::none()
            }
            PreviewMessage::MarkdownPreviewScrolled {
                offset_y,
                viewport_height,
                content_height,
            } => Task::batch([
                self.show_scrollbars_temporarily(ScrollbarRegion::MarkdownPreview),
                self.preview_engine
                    .handle_markdown_preview_scrolled(offset_y, viewport_height, content_height)
                    .map(Message::Preview),
            ]),
            PreviewMessage::MarkdownPreviewModeSelected(mode) => {
                if let Some(document) = self.preview_engine.text_preview_document.as_mut() {
                    document.select_markdown_preview_mode(mode);
                }
                Task::none()
            }
            PreviewMessage::ImagePreviewDimensionsLoaded(path, generation, outcome) => {
                self.accept_image_preview_dimensions(path, generation, outcome)
            }
            PreviewMessage::AudioPreviewSeekRequested(position) => {
                self.seek_audio_preview_playback(position)
            }
            PreviewMessage::PreviewTreeDirectoryToggled(entry_id) => {
                self.toggle_preview_tree_directory(entry_id)
            }
            PreviewMessage::RightPreviewPanelInfoLoaded { path, snapshot } => {
                self.accept_right_preview_panel_info(&path, snapshot);
                Task::none()
            }
            PreviewMessage::SqlitePreviewTablesScrolled => {
                self.show_scrollbars_temporarily(ScrollbarRegion::PreviewSqliteTables)
            }
            PreviewMessage::SqlitePreviewDataScrolled => {
                self.show_scrollbars_temporarily(ScrollbarRegion::PreviewSqliteData)
            }
            PreviewMessage::PreviewDirectoryScrolled => {
                self.show_scrollbars_temporarily(ScrollbarRegion::PreviewDirectory)
            }
            PreviewMessage::PreviewArchiveScrolled => {
                self.show_scrollbars_temporarily(ScrollbarRegion::PreviewArchive)
            }
            // 焦点簿记归宿主；窗口在消息回到宿主前已被关掉时，不能让它
            // 指向已死窗口。
            PreviewMessage::PreviewWindowPresented(window) => {
                if self.preview_engine.preview_window == Some(window) {
                    self.focused_window = window;
                }
                Task::none()
            }
            engine_message => {
                unreachable!("{engine_message:?} 由 PreviewEngine::handle 处理，不会还给宿主")
            }
        }
    }

    fn accept_preview(
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
            self.retract_global_error(GlobalErrorSource::Preview);
        }
        command.map(Message::Preview)
    }

    fn seek_audio_preview_playback(&mut self, position_seconds: f32) -> Task<Message> {
        if let Some(error) = self
            .preview_engine
            .seek_audio_preview_playback(position_seconds)
        {
            self.show_global_error_from(GlobalErrorSource::Preview, error);
        }
        Task::none()
    }
}
