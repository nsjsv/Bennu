use std::path::PathBuf;

use file_core::FileKind;
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::app::FileBrowser;
use crate::commands::remote_preview_cache_command;
use crate::model::{
    Message, PreviewState, PreviewWindowProfile, RemotePreviewCacheFinished,
    RemotePreviewCacheMessage, RemotePreviewCacheProgress, RemotePreviewDownload,
};
use crate::remote_preview_cache::default_remote_preview_cache_dir;

impl FileBrowser {
    pub(in crate::app) fn start_remote_preview_download(
        &mut self,
        source_path: PathBuf,
    ) -> Task<Message> {
        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.clear_preview();

        self.remote_preview_download_generation =
            self.remote_preview_download_generation.wrapping_add(1);
        let generation = self.remote_preview_download_generation;
        let cancel = CancellationToken::new();
        self.remote_preview_download_cancel = Some(cancel.clone());
        self.preview = Some(PreviewState::DownloadingRemoteFile(
            RemotePreviewDownload::new(source_path.clone(), generation),
        ));
        self.clear_global_error();

        let max_file_bytes = self.preview_file_size_limit_for(&source_path);
        Task::batch([
            window_command,
            remote_preview_cache_command(
                source_path,
                generation,
                default_remote_preview_cache_dir(),
                max_file_bytes,
                cancel,
            ),
        ])
    }

    pub(in crate::app) fn accept_remote_preview_cache_message(
        &mut self,
        message: RemotePreviewCacheMessage,
    ) -> Task<Message> {
        match message {
            RemotePreviewCacheMessage::Progress(progress) => {
                self.accept_remote_preview_cache_progress(progress);
                Task::none()
            }
            RemotePreviewCacheMessage::Finished(finished) => {
                self.accept_remote_preview_cache_finished(finished)
            }
        }
    }

    pub(super) fn cancel_remote_preview_download(&mut self) {
        if let Some(cancel) = self.remote_preview_download_cancel.take() {
            cancel.cancel();
        }
    }

    fn accept_remote_preview_cache_progress(&mut self, progress: RemotePreviewCacheProgress) {
        let Some(PreviewState::DownloadingRemoteFile(download)) = self.preview.as_mut() else {
            return;
        };
        if download.source_path != progress.source_path
            || download.generation != progress.generation
        {
            return;
        }

        download.accept_progress(&progress);
    }

    fn accept_remote_preview_cache_finished(
        &mut self,
        finished: RemotePreviewCacheFinished,
    ) -> Task<Message> {
        if !self.remote_preview_download_matches(&finished.source_path, finished.generation) {
            return Task::none();
        }

        self.remote_preview_download_cancel = None;
        match finished.outcome {
            Ok(cache_path) => self.open_preview_for_resolved_path(cache_path, FileKind::File),
            Err(error) => {
                self.text_preview_document = None;
                self.preview = Some(PreviewState::Error(format!(
                    "Could not download remote preview: {error}"
                )));
                // 面板会话不弹独立窗口;错误态直接呈现在面板里。
                self.ensure_preview_window_for_standalone_load(PreviewWindowProfile::Regular)
            }
        }
    }

    fn remote_preview_download_matches(&self, source_path: &PathBuf, generation: u64) -> bool {
        matches!(
            &self.preview,
            Some(PreviewState::DownloadingRemoteFile(download))
                if download.source_path == *source_path && download.generation == generation
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::config;
    use crate::model::{RemotePreviewCacheFinished, RemotePreviewCacheProgress};

    use super::*;

    #[test]
    fn stale_remote_preview_progress_does_not_update_active_download() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let active_path = PathBuf::from("/run/user/1000/gvfs/dav/active.txt");
        browser.preview = Some(PreviewState::DownloadingRemoteFile(
            RemotePreviewDownload::new(active_path.clone(), 2),
        ));

        browser.accept_remote_preview_cache_progress(RemotePreviewCacheProgress {
            source_path: active_path,
            generation: 1,
            bytes_done: 50,
            bytes_total: 100,
        });

        let Some(PreviewState::DownloadingRemoteFile(download)) = browser.preview else {
            panic!("expected download state");
        };
        assert_eq!(download.bytes_done, 0);
        assert_eq!(download.bytes_total, None);
    }

    #[test]
    fn stale_remote_preview_finished_does_not_replace_active_download() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let active_path = PathBuf::from("/run/user/1000/gvfs/dav/active.txt");
        browser.preview = Some(PreviewState::DownloadingRemoteFile(
            RemotePreviewDownload::new(active_path.clone(), 2),
        ));

        let _ = browser.accept_remote_preview_cache_finished(RemotePreviewCacheFinished {
            source_path: active_path,
            generation: 1,
            outcome: Ok(PathBuf::from("/tmp/cache.txt")),
        });

        let Some(PreviewState::DownloadingRemoteFile(download)) = browser.preview else {
            panic!("expected download state");
        };
        assert_eq!(download.generation, 2);
    }

    #[test]
    fn completed_remote_office_cache_reuses_local_document_dispatch() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let source = PathBuf::from("/run/user/1000/gvfs/dav/report.docx");
        let cache_path = PathBuf::from("/tmp/bennu-preview/report.docx");
        browser.preview = Some(PreviewState::DownloadingRemoteFile(
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
            browser.preview,
            Some(PreviewState::Loading(ref path)) if path == &cache_path
        ));
        assert_eq!(
            browser
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
        browser.preview = Some(PreviewState::DownloadingRemoteFile(
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
            browser.preview,
            Some(PreviewState::Loading(ref path)) if path == &cache_path
        ));
        assert_eq!(
            browser
                .pending_document_preview
                .as_ref()
                .unwrap()
                .key
                .source_path,
            cache_path
        );
    }
}
