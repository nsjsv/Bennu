use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use file_core::{
    copy_path_with_options, FileOperationVerification, FileTransferOptions, ProgressSender,
    TransferConflictStrategy,
};
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::formatting::format_file_size;

const NETWORK_PREVIEW_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
/// 磁盘侧容量预算：缓存总量不允许越过该值，防止长期使用无限膨胀。
const NETWORK_PREVIEW_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct RemotePreviewCacheRequest {
    pub(crate) source_path: PathBuf,
    pub(crate) cache_dir: PathBuf,
    pub(crate) max_file_bytes: u64,
    pub(crate) cancel: CancellationToken,
}

impl RemotePreviewCacheRequest {
    pub(crate) fn new(
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

pub(crate) fn default_remote_preview_cache_dir() -> PathBuf {
    let fallback_base = dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    dirs::cache_dir()
        .unwrap_or(fallback_base)
        .join("bennu")
        .join("network-preview")
}

pub(crate) async fn cache_remote_preview_file(
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
    let _ = remove_expired_remote_preview_files(&request.cache_dir, now).await;
    // 每次缓存流程先做容量预算清理，覆盖会话首次预览与长期未预览后的再次进入。
    let _ = enforce_remote_preview_cache_budget_with_limit(
        &request.cache_dir,
        NETWORK_PREVIEW_CACHE_MAX_BYTES,
    )
    .await;

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
        return Err(format!(
            "File is too large to preview ({}). Maximum preview size is {}.",
            format_file_size(source_metadata.len()),
            format_file_size(request.max_file_bytes)
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
    copy_path_with_options(&request.source_path, &cache_path, transfer_options)
        .await
        .map_err(|error| format!("could not download remote preview file: {error}"))?;
    // 下载新增文件后再次清理，保证总量始终在预算内。
    let _ = enforce_remote_preview_cache_budget_with_limit(
        &request.cache_dir,
        NETWORK_PREVIEW_CACHE_MAX_BYTES,
    )
    .await;
    Ok(cache_path)
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
        let Ok(metadata) = entry.metadata().await else {
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
        let Ok(metadata) = entry.metadata().await else {
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
    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, size, _) in files {
        if total_bytes <= max_total_bytes {
            break;
        }
        if fs::remove_file(&path).await.is_ok() {
            total_bytes = total_bytes.saturating_sub(size);
        }
    }
    Ok(())
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
