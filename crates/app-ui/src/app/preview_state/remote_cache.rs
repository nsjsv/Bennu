//! 远程预览缓存下载状态机已迁 bennu-preview 的 PreviewEngine
//! （engine/remote_cache.rs，逐字节保真；过期进度/完成结果的守卫测试随迁
//! 引擎侧）。此处保留返回宿主 Task 的同名薄转发：大小上限实时读取
//! user_config 传入，缓存命中后的本地打开走宿主入口（分类/门禁/会话
//! 保存是宿主语义），启动分支的全局错误清除是宿主通知域。

use std::path::PathBuf;

use file_core::FileKind;
use iced::Task;

use crate::app::FileBrowser;
use crate::model::{Message, RemotePreviewCacheMessage};
use bennu_preview::engine::RemotePreviewCacheCompletion;

impl FileBrowser {
    pub(in crate::app) fn start_remote_preview_download(
        &mut self,
        source_path: PathBuf,
    ) -> Task<Message> {
        let max_file_bytes = self.preview_file_size_limit_for(&source_path);
        let command = self
            .preview_engine
            .start_remote_preview_download(source_path, max_file_bytes);
        self.clear_global_error();
        self.sync_preview_window_focus();
        command.map(Message::Preview)
    }

    pub(in crate::app) fn accept_remote_preview_cache_message(
        &mut self,
        message: RemotePreviewCacheMessage,
    ) -> Task<Message> {
        match message {
            RemotePreviewCacheMessage::Progress(progress) => {
                self.preview_engine
                    .accept_remote_preview_cache_progress(progress);
                Task::none()
            }
            RemotePreviewCacheMessage::Finished(finished) => {
                self.accept_remote_preview_cache_finished(finished)
            }
        }
    }

    fn accept_remote_preview_cache_finished(
        &mut self,
        finished: crate::model::RemotePreviewCacheFinished,
    ) -> Task<Message> {
        match self
            .preview_engine
            .accept_remote_preview_cache_finished(finished)
        {
            None => Task::none(),
            Some(RemotePreviewCacheCompletion::OpenLocalCache(cache_path)) => {
                self.open_preview_for_resolved_path(cache_path, FileKind::File)
            }
            Some(RemotePreviewCacheCompletion::Failed(command)) => {
                self.sync_preview_window_focus();
                command.map(Message::Preview)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config;
    use crate::model::{PreviewState, RemotePreviewCacheFinished};
    use bennu_preview::preview::RemotePreviewDownload;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn completed_remote_office_cache_reuses_local_document_dispatch() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let source = PathBuf::from("/run/user/1000/gvfs/dav/report.docx");
        let cache_path = PathBuf::from("/tmp/bennu-preview/report.docx");
        browser.preview_engine.preview = Some(PreviewState::DownloadingRemoteFile(
            RemotePreviewDownload::new(source.clone(), 5),
        ));

        drop(
            browser.accept_remote_preview_cache_finished(RemotePreviewCacheFinished {
                source_path: source,
                generation: 5,
                outcome: Ok(cache_path.clone()),
            }),
        );

        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Loading(ref path)) if path == &cache_path
        ));
        assert_eq!(
            browser
                .preview_engine
                .pending_document_preview
                .as_ref()
                .unwrap()
                .key
                .source_path,
            cache_path
        );
    }

    #[test]
    fn completed_remote_pdf_cache_reuses_local_document_dispatch() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let source = PathBuf::from("/run/user/1000/gvfs/dav/report.pdf");
        let cache_path = PathBuf::from("/tmp/bennu-preview/report.pdf");
        browser.preview_engine.preview = Some(PreviewState::DownloadingRemoteFile(
            RemotePreviewDownload::new(source.clone(), 4),
        ));

        drop(
            browser.accept_remote_preview_cache_finished(RemotePreviewCacheFinished {
                source_path: source,
                generation: 4,
                outcome: Ok(cache_path.clone()),
            }),
        );

        assert!(matches!(
            browser.preview_engine.preview,
            Some(PreviewState::Loading(ref path)) if path == &cache_path
        ));
        assert_eq!(
            browser
                .preview_engine
                .pending_document_preview
                .as_ref()
                .unwrap()
                .key
                .source_path,
            cache_path
        );
    }
}
