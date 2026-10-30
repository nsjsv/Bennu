use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use file_core::{
    copy_path_with_options, CopiedTarget, FileOperationVerification, FileTransferOptions,
    ProgressSender, TransferConflictStrategy,
};
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::preview_loading::file_too_large_to_preview;

const NETWORK_PREVIEW_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
/// 磁盘侧容量预算：缓存总量不允许越过该值，防止长期使用无限膨胀。
const NETWORK_PREVIEW_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RemotePreviewCacheRequest {
    pub source_path: PathBuf,
    pub cache_dir: PathBuf,
    pub max_file_bytes: u64,
    pub cancel: CancellationToken,
}

impl RemotePreviewCacheRequest {
    pub fn new(
        source_path: PathBuf,
        cache_dir: PathBuf,
        max_file_bytes: u64,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            source_path,
            cache_dir,
            max_file_bytes,
            cancel,
        }
    }
}

pub fn default_remote_preview_cache_dir() -> PathBuf {
    file_core::app_dirs::app_cache_dir().join("network-preview")
}

pub async fn cache_remote_preview_file(
    request: RemotePreviewCacheRequest,
    progress: ProgressSender,
) -> Result<PathBuf, String> {
    fs::create_dir_all(&request.cache_dir)
        .await
        .map_err(|error| {
            format!(
                "could not create remote preview cache {:?}: {error}",
                request.cache_dir
            )
        })?;

    let now = SystemTime::now();
    log_cache_cleanup_failure(
        &request.cache_dir,
        "remove expired files",
        remove_expired_remote_preview_files(&request.cache_dir, now).await,
    );
    // 每次缓存流程先做容量预算清理，覆盖会话首次预览与长期未预览后的再次进入。
    log_cache_cleanup_failure(
        &request.cache_dir,
        "enforce size budget",
        enforce_remote_preview_cache_budget_with_limit(
            &request.cache_dir,
            NETWORK_PREVIEW_CACHE_MAX_BYTES,
        )
        .await,
    );

    let source_metadata = fs::metadata(&request.source_path).await.map_err(|error| {
        format!(
            "could not read remote preview source {:?}: {error}",
            request.source_path
        )
    })?;
    if !source_metadata.is_file() {
        return Err(format!(
            "remote preview source is not a regular file: {:?}",
            request.source_path
        ));
    }
    if request.max_file_bytes != 0 && source_metadata.len() > request.max_file_bytes {
        return Err(file_too_large_to_preview(
            source_metadata.len(),
            request.max_file_bytes,
        ));
    }

    let cache_path =
        remote_preview_cache_path(&request.cache_dir, &request.source_path, &source_metadata);
    if cached_file_is_fresh(&cache_path, now).await {
        return Ok(cache_path);
    }

    let transfer_options = FileTransferOptions::running(request.cancel)
        .with_progress_sender(progress)
        .with_conflict_strategy(TransferConflictStrategy::Replace)
        .with_verification(FileOperationVerification::BasicMetadata);
    // 预览缓存只需要文件内容:权限/扩展属性/ACL 与缓存语义无关,所以不采用
    // metadata_loss;修改时间没能恢复仍然作为错误返回。
    let CopiedTarget {
        target: _,
        metadata_loss: _,
    } = copy_path_with_options(&request.source_path, &cache_path, transfer_options)
        .await
        .map_err(|error| format!("could not download remote preview file: {error}"))?;
    // 下载新增文件后再次清理，保证总量始终在预算内。
    log_cache_cleanup_failure(
        &request.cache_dir,
        "enforce size budget",
        enforce_remote_preview_cache_budget_with_limit(
            &request.cache_dir,
            NETWORK_PREVIEW_CACHE_MAX_BYTES,
        )
        .await,
    );
    Ok(cache_path)
}

/// 清理只为控制缓存占用，失败不影响这次预览；但要留下记录，否则缓存会悄悄越积越多。
fn log_cache_cleanup_failure(cache_dir: &Path, step: &'static str, outcome: io::Result<()>) {
    if let Err(error) = outcome {
        tracing::warn!(
            event = "remote_preview_cache_cleanup_failed",
            step,
            cache_dir = %cache_dir.display(),
            %error,
            "remote preview cache cleanup failed"
        );
    }
}

fn remote_preview_cache_path(
    cache_dir: &Path,
    source_path: &Path,
    source_metadata: &std::fs::Metadata,
) -> PathBuf {
    let signature = remote_preview_cache_signature(source_path, source_metadata);
    let mut file_name = OsString::from(format!("network-preview-{signature}"));
    if let Some(extension) = source_path.extension() {
        file_name.push(".");
        file_name.push(extension);
    }
    cache_dir.join(file_name)
}

fn remote_preview_cache_signature(
    source_path: &Path,
    source_metadata: &std::fs::Metadata,
) -> String {
    let mut hasher = blake3::Hasher::new();
    update_signature_field(&mut hasher, b"bennu-network-preview");
    #[cfg(unix)]
    {
        update_signature_field(&mut hasher, source_path.as_os_str().as_bytes());
    }
    #[cfg(not(unix))]
    {
        let path_text = source_path.to_string_lossy();
        update_signature_field(&mut hasher, path_text.as_bytes());
    }
    update_signature_field(&mut hasher, &source_metadata.len().to_le_bytes());
    update_signature_field(
        &mut hasher,
        &modified_signature(source_metadata).to_le_bytes(),
    );
    hasher.finalize().to_hex().to_string()
}

fn modified_signature(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn update_signature_field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

async fn cached_file_is_fresh(path: &Path, now: SystemTime) -> bool {
    let Ok(metadata) = fs::metadata(path).await else {
        return false;
    };
    metadata.is_file()
        && metadata
            .modified()
            .is_ok_and(|modified| !cache_entry_is_expired(modified, now))
}

async fn remove_expired_remote_preview_files(cache_dir: &Path, now: SystemTime) -> io::Result<()> {
    let mut entries = match fs::read_dir(cache_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let Some(metadata) = entry_metadata_unless_gone(&entry).await? else {
            continue;
        };
        let expired = metadata
            .modified()
            .map(|modified| cache_entry_is_expired(modified, now))
            .unwrap_or(true);
        if !expired {
            continue;
        }

        let outcome = if metadata.is_dir() {
            fs::remove_dir_all(&path).await
        } else {
            fs::remove_file(&path).await
        };
        if let Err(error) = outcome {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    }

    Ok(())
}

fn cache_entry_is_expired(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .map(|age| age > NETWORK_PREVIEW_CACHE_TTL)
        .unwrap_or(false)
}

async fn enforce_remote_preview_cache_budget_with_limit(
    cache_dir: &Path,
    max_total_bytes: u64,
) -> io::Result<()> {
    let mut entries = match fs::read_dir(cache_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let Some(metadata) = entry_metadata_unless_gone(&entry).await? else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        files.push((
            entry.path(),
            metadata.len(),
            metadata.modified().unwrap_or(UNIX_EPOCH),
        ));
    }

    let mut total_bytes: u64 = files.iter().map(|(_, size, _)| size).sum();
    if total_bytes <= max_total_bytes {
        return Ok(());
    }
    // 超预算按 modified 最旧优先删除，等价 LRU：刚下载的文件 modified 最新，必被保留。
    // 某个删不掉就接着删下一个，尽量回到预算内，最后报第一个失败。
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut first_failure = None;
    for (path, size, _) in files {
        if total_bytes <= max_total_bytes {
            break;
        }
        match fs::remove_file(&path).await {
            Ok(()) => total_bytes = total_bytes.saturating_sub(size),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                total_bytes = total_bytes.saturating_sub(size);
            }
            Err(error) => {
                first_failure.get_or_insert(error);
            }
        }
    }
    first_failure.map_or(Ok(()), Err)
}

/// 条目在列目录和取元数据之间被别处删掉不算失败；其余读不到的情况如实返回。
async fn entry_metadata_unless_gone(entry: &fs::DirEntry) -> io::Result<Option<std::fs::Metadata>> {
    match entry.metadata().await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;

    #[tokio::test]
    async fn cache_remote_preview_file_reuses_fresh_cached_file() {
        let temp_dir = tempdir().expect("temp dir");
        let source = temp_dir.path().join("remote.txt");
        let cache_dir = temp_dir.path().join("cache");
        tokio::fs::write(&source, b"remote")
            .await
            .expect("write source");

        let cached = cache_once(&source, &cache_dir).await;
        tokio::fs::write(&cached, b"cached")
            .await
            .expect("mark cache");

        let reused = cache_once(&source, &cache_dir).await;

        assert_eq!(reused, cached);
        assert_eq!(
            tokio::fs::read(reused).await.expect("read reused cache"),
            b"cached"
        );
    }

    #[tokio::test]
    async fn cache_remote_preview_file_uses_new_path_when_source_size_changes() {
        let temp_dir = tempdir().expect("temp dir");
        let source = temp_dir.path().join("remote.txt");
        let cache_dir = temp_dir.path().join("cache");
        tokio::fs::write(&source, b"one")
            .await
            .expect("write source");
        let first = cache_once(&source, &cache_dir).await;

        tokio::fs::write(&source, b"one plus more")
            .await
            .expect("rewrite source");
        let second = cache_once(&source, &cache_dir).await;

        assert_ne!(first, second);
        assert_eq!(
            tokio::fs::read(second).await.expect("read changed cache"),
            b"one plus more"
        );
    }

    #[tokio::test]
    async fn remove_expired_remote_preview_files_clears_old_entries() {
        let temp_dir = tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("cache");
        tokio::fs::create_dir(&cache_dir)
            .await
            .expect("create cache");
        let entry = cache_dir.join("network-preview-old.txt");
        tokio::fs::write(&entry, b"old").await.expect("write old");

        let future_now = SystemTime::now() + Duration::from_secs(60 * 60);
        remove_expired_remote_preview_files(&cache_dir, future_now)
            .await
            .expect("cleanup");

        assert!(!entry.exists());
    }

    #[tokio::test]
    async fn cache_remote_preview_file_rejects_source_over_limit_before_copy() {
        let temp_dir = tempdir().expect("temp dir");
        let source = temp_dir.path().join("remote.txt");
        let cache_dir = temp_dir.path().join("cache");
        tokio::fs::write(&source, b"remote")
            .await
            .expect("write source");

        let error = cache_once_with_limit(&source, &cache_dir, 3)
            .await
            .expect_err("oversized preview");
        let mut entries = tokio::fs::read_dir(&cache_dir)
            .await
            .expect("read cache dir");

        assert!(error.contains("File is too large to preview"));
        assert!(entries.next_entry().await.expect("cache entry").is_none());
    }

    async fn cache_once(source: &Path, cache_dir: &Path) -> PathBuf {
        cache_once_with_limit(source, cache_dir, u64::MAX)
            .await
            .expect("cache preview")
    }

    async fn cache_once_with_limit(
        source: &Path,
        cache_dir: &Path,
        max_file_bytes: u64,
    ) -> Result<PathBuf, String> {
        let (progress, _receiver) = mpsc::unbounded_channel();
        let request = RemotePreviewCacheRequest::new(
            source.to_path_buf(),
            cache_dir.to_path_buf(),
            max_file_bytes,
            CancellationToken::new(),
        );
        cache_remote_preview_file(request, progress).await
    }

    /// 清理失败不拦预览，但要留下日志事件。
    #[cfg(unix)]
    #[tokio::test]
    async fn a_cache_entry_that_cannot_be_removed_is_logged_and_the_preview_still_works() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tracing_subscriber::layer::{Context, SubscriberExt};

        struct CleanupFailureCounter(Arc<AtomicUsize>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CleanupFailureCounter {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                struct EventName(bool);
                impl tracing::field::Visit for EventName {
                    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                        if field.name() == "event" && value == "remote_preview_cache_cleanup_failed"
                        {
                            self.0 = true;
                        }
                    }
                    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {
                    }
                }
                let mut name = EventName(false);
                event.record(&mut name);
                if name.0 {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        let temp_dir = tempdir().expect("temp dir");
        let source = temp_dir.path().join("remote.txt");
        let cache_dir = temp_dir.path().join("cache");
        let stale = cache_dir.join("network-preview-stale");
        tokio::fs::write(&source, b"remote")
            .await
            .expect("write source");
        std::fs::create_dir_all(&stale).expect("create stale entry");
        std::fs::write(stale.join("inner"), b"stale").expect("write stale content");
        std::fs::File::open(&stale)
            .and_then(|directory| directory.set_modified(UNIX_EPOCH + Duration::from_secs(1)))
            .expect("age stale entry");
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o555))
            .expect("lock stale entry");
        // root 不受目录权限限制，删得掉就测不出这条路径。
        if std::fs::write(stale.join("probe"), b"").is_ok() {
            std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let failures = Arc::new(AtomicUsize::new(0));
        let subscriber =
            tracing_subscriber::registry().with(CleanupFailureCounter(Arc::clone(&failures)));
        let cached = {
            // 当前线程运行时：清理与日志都在这个线程上，线程内订阅器看得到。
            let _subscriber = tracing::subscriber::set_default(subscriber);
            cache_once(&source, &cache_dir).await
        };
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(tokio::fs::read(&cached).await.unwrap(), b"remote");
        assert!(failures.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn budget_cleanup_removes_oldest_until_under_limit() {
        let temp_dir = tempdir().expect("temp dir");
        let cache_dir = temp_dir.path().join("cache");
        tokio::fs::create_dir(&cache_dir)
            .await
            .expect("create cache");
        for (name, size) in [
            ("network-preview-a.bin", 40usize),
            ("network-preview-b.bin", 40usize),
        ] {
            tokio::fs::write(cache_dir.join(name), vec![0u8; size])
                .await
                .expect("write entry");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        tokio::fs::write(cache_dir.join("network-preview-c.bin"), vec![0u8; 40])
            .await
            .expect("write newest");

        enforce_remote_preview_cache_budget_with_limit(&cache_dir, 100)
            .await
            .expect("cleanup");

        let oldest_gone = !tokio::fs::try_exists(cache_dir.join("network-preview-a.bin"))
            .await
            .unwrap_or(true);
        let newest_kept = tokio::fs::try_exists(cache_dir.join("network-preview-c.bin"))
            .await
            .unwrap_or(false);
        assert!(oldest_gone, "最旧文件应按 LRU 顺序被删除");
        assert!(newest_kept, "最新文件必须保留");
    }
}
