//! 预览会话/音视频播放状态机在 bennu-preview 的 PreviewEngine
//! （engine/session.rs + engine/media.rs）；宿主在这里接预览消息：
//! [`FileBrowser::handle_preview_message`] 是唯一入口，引擎自足的变体由
//! 引擎处理，其余变体依赖宿主的扫描选项、视口几何、滚动管线或通知域，
//! 在本模块及子模块里各有一个处理者。宿主域差异以参数/返回值补齐——
//! 全局错误通知（accept_preview 就绪分支与音频跳转失败）经验收结论/错误串
//! 回传，目录展开的扫描选项与展开层数实时读取 user_config 传入（引擎配置
//! 快照只随持久化刷新）。

use bennu_preview::text_document::TextViewerEvent;
use std::path::PathBuf;

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::model::{Message, PreviewContent, PreviewMessage};

mod animated_image;
mod document;
mod image_viewport;
mod original_image;
mod remote_cache;
mod sqlite;
#[cfg(test)]
mod tests;
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
            PreviewMessage::ImagePreviewDimensionsLoaded(path, generation, outcome) => {
                self.accept_image_preview_dimensions(path, generation, outcome)
            }
            PreviewMessage::PreviewTreeDirectoryToggled(entry_id) => {
                self.toggle_preview_tree_directory(entry_id)
            }
            PreviewMessage::RightPreviewPanelInfoLoaded { path, snapshot } => {
                self.accept_right_preview_panel_info(&path, snapshot);
                Task::none()
            }
            // 焦点簿记归宿主；窗口在消息回到宿主前已被关掉时，不能让它
            // 指向已死窗口。
            PreviewMessage::PreviewWindowPresented(window) => {
                if self.preview_engine.preview_window == Some(window) {
                    self.focused_window = window;
                }
                Task::none()
            }
            // 字号与换行是主程序设置：改了就记住。
            PreviewMessage::TextViewer(TextViewerEvent::ChangeView(change)) => {
                let view = self.user_config.text_preview_view.apply(change);
                if view == self.user_config.text_preview_view {
                    return Task::none();
                }
                self.user_config.text_preview_view = view;
                self.persist_user_preferences_command()
            }
            // 音频、视频共用的音量是主程序设置：改了就记住（落盘已合并，拖动不会狂写）。
            PreviewMessage::MediaVolumeChanged(volume) => {
                let volume = volume.clamp(0.0, 1.0);
                if volume == self.user_config.media_preview_volume {
                    return Task::none();
                }
                self.user_config.media_preview_volume = volume;
                self.preview_engine.set_media_volume(volume);
                self.persist_user_preferences_command()
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
        if !matches!(
            acceptance,
            bennu_preview::engine::PreviewAcceptance::ContentReady
        ) {
            return command.map(Message::Preview);
        }
        self.retract_global_error(GlobalErrorSource::Preview);
        // 独立预览窗口一打开键盘就归预览内容（文本查看器、Markdown 渲染内容、音视频）；
        // 右侧面板不抢，点一下内容才归它。
        let focus_viewer = self.preview_engine.preview_load_surface
            == bennu_preview::engine::PreviewLoadSurface::StandaloneWindow
            && self.preview_engine.preview_takes_keyboard();
        if focus_viewer {
            return Task::batch([
                command.map(Message::Preview),
                bennu_preview::text_preview_viewer::focus_preview_keyboard(),
            ]);
        }
        command.map(Message::Preview)
    }
}
