//! 远程预览缓存下载状态机：自 app-ui 的 preview_state/remote_cache.rs
//! 迁入（impl FileBrowser → impl PreviewEngine，逐字节保真；下载命令
//! 直连本 crate 命令层）。缓存命中后的本地打开入口归宿主（分类/门禁/
//! 会话保存是宿主语义），经完成结论回传；大小上限由宿主实时读取
//! user_config 后传入（引擎配置快照只随持久化刷新）。

use std::path::PathBuf;

use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::commands::preview::remote_preview_cache_command;
use crate::preview::{
    PreviewState, PreviewWindowProfile, RemotePreviewCacheFinished, RemotePreviewCacheProgress,
    RemotePreviewDownload,
};
use crate::preview_message::PreviewMessage;
use crate::remote_preview_cache::default_remote_preview_cache_dir;

/// 远程缓存下载完成后的处置：命中缓存交宿主本地打开入口，失败分支
/// 引擎已置错误态并附带补开独立窗口的任务。
pub enum RemotePreviewCacheCompletion {
    OpenLocalCache(PathBuf),
    Failed(Task<PreviewMessage>),
}

impl super::PreviewEngine {
    pub fn start_remote_preview_download(
        &mut self,
        source_path: PathBuf,
        max_file_bytes: u64,
    ) -> Task<PreviewMessage> {
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

    pub(crate) fn cancel_remote_preview_download(&mut self) {
        if let Some(cancel) = self.remote_preview_download_cancel.take() {
            cancel.cancel();
        }
    }

    pub fn accept_remote_preview_cache_progress(&mut self, progress: RemotePreviewCacheProgress) {
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

    pub fn accept_remote_preview_cache_finished(
        &mut self,
        finished: RemotePreviewCacheFinished,
    ) -> Option<RemotePreviewCacheCompletion> {
        if !self.remote_preview_download_matches(&finished.source_path, finished.generation) {
            return None;
        }

        self.remote_preview_download_cancel = None;
        match finished.outcome {
            Ok(cache_path) => Some(RemotePreviewCacheCompletion::OpenLocalCache(cache_path)),
            Err(error) => {
                self.text_preview_document = None;
                self.preview = Some(PreviewState::Error(format!(
                    "Could not download remote preview: {error}"
                )));
                // 面板会话不弹独立窗口;错误态直接呈现在面板里。
                Some(RemotePreviewCacheCompletion::Failed(
                    self.ensure_preview_window_for_standalone_load(PreviewWindowProfile::Regular),
                ))
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
    use super::*;
    use crate::engine::{
        PreviewEngine, PreviewEngineConfig, PreviewFileSizeLimits, PreviewWindowIdentity,
    };
    use crate::preview::RemotePreviewDownload;

    fn engine() -> PreviewEngine {
        PreviewEngine::new(
            PreviewEngineConfig {
                directory_expand_levels: 1,
                extension_rules: Default::default(),
                size_limits: PreviewFileSizeLimits::with_default_limits(),
            },
            crate::engine::default_preview_size(crate::preview::PreviewWindowProfile::Regular),
            PreviewWindowIdentity {
                app_id: "bennu-preview-tests".to_owned(),
                icon: None,
            },
        )
    }

    #[test]
    fn stale_remote_preview_progress_does_not_update_active_download() {
        let mut engine = engine();
        let active_path = PathBuf::from("/run/user/1000/gvfs/dav/active.txt");
        engine.preview = Some(PreviewState::DownloadingRemoteFile(
            RemotePreviewDownload::new(active_path.clone(), 2),
        ));

        engine.accept_remote_preview_cache_progress(RemotePreviewCacheProgress {
            source_path: active_path,
            generation: 1,
            bytes_done: 50,
            bytes_total: 100,
        });

        let Some(PreviewState::DownloadingRemoteFile(download)) = &engine.preview else {
            panic!("expected download state");
        };
        assert_eq!(download.bytes_done, 0);
        assert_eq!(download.bytes_total, None);
    }

    #[test]
    fn stale_remote_preview_finished_does_not_replace_active_download() {
        let mut engine = engine();
        let active_path = PathBuf::from("/run/user/1000/gvfs/dav/active.txt");
        engine.preview = Some(PreviewState::DownloadingRemoteFile(
            RemotePreviewDownload::new(active_path.clone(), 2),
        ));

        let _ = engine.accept_remote_preview_cache_finished(RemotePreviewCacheFinished {
            source_path: active_path,
            generation: 1,
            outcome: Ok(PathBuf::from("/tmp/cache.txt")),
        });

        let Some(PreviewState::DownloadingRemoteFile(download)) = &engine.preview else {
            panic!("expected download state");
        };
        assert_eq!(download.generation, 2);
    }
}
