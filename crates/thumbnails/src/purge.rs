//! 照 GNOME 的 gsd-housekeeping 清理共享缩略图目录：四个尺寸目录里的 `.png`
//! 按修改时间，太旧的删掉，总量超了从最旧的删起。别的程序写的和旧版本留下的文件
//! 一视同仁，同 GNOME。

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::cache_format::ThumbnailBucket;

/// GNOME `org.gnome.desktop.thumbnail-cache` 的默认值：180 天、512 MB。
const THUMBNAIL_MAX_AGE: Duration = Duration::from_secs(180 * 24 * 60 * 60);
const THUMBNAIL_CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// 同 gsd-housekeeping：启动 2 分钟后一次，之后每天一次。
const FIRST_PURGE_DELAY: Duration = Duration::from_secs(2 * 60);
const PURGE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Default, PartialEq, Eq)]
struct ThumbnailCachePurge {
    removed: usize,
    kept_bytes: u64,
    failures: usize,
    /// 第一个失败（路径和原因）；单个文件失败不中断清理。
    first_failure: Option<String>,
}

impl ThumbnailCachePurge {
    fn record_failure(&mut self, path: &Path, error: io::Error) {
        self.failures += 1;
        self.first_failure
            .get_or_insert_with(|| format!("{}: {error}", path.display()));
    }

    /// 文件已经不在了（别的程序刚删掉）也算删掉了。
    fn remove(&mut self, path: &Path) -> bool {
        match fs::remove_file(path) {
            Ok(()) => {
                self.removed += 1;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) => {
                self.record_failure(path, error);
                false
            }
        }
    }
}

fn purge_thumbnail_cache(
    cache_dir: &Path,
    now: SystemTime,
    max_age: Duration,
    max_bytes: u64,
) -> ThumbnailCachePurge {
    let mut purge = ThumbnailCachePurge::default();
    let mut kept = Vec::new();
    for bucket in ThumbnailBucket::ALL {
        let dir = cache_dir.join(bucket.dir_name());
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                purge.record_failure(&dir, error);
                continue;
            }
        };
        for entry in entries {
            // DirEntry::metadata 不跟随链接：链接不是普通文件，不碰。
            let (path, metadata) =
                match entry.and_then(|entry| Ok((entry.path(), entry.metadata()?))) {
                    Ok(found) => found,
                    Err(error) => {
                        purge.record_failure(&dir, error);
                        continue;
                    }
                };
            if !metadata.is_file() || path.extension() != Some(OsStr::new("png")) {
                continue;
            }
            let modified = metadata.modified().unwrap_or(now);
            let too_old = now.duration_since(modified).is_ok_and(|age| age > max_age);
            if !too_old || !purge.remove(&path) {
                kept.push((modified, metadata.len(), path));
            }
        }
    }

    let mut kept_bytes = kept.iter().map(|(_, len, _)| len).sum::<u64>();
    if kept_bytes > max_bytes {
        kept.sort_by_key(|(modified, _, _)| *modified);
        for (_, len, path) in &kept {
            if kept_bytes <= max_bytes {
                break;
            }
            if purge.remove(path) {
                kept_bytes -= len;
            }
        }
    }
    purge.kept_bytes = kept_bytes;
    purge
}

/// 不会结束：由主程序当作后台任务一直跑着。
pub async fn keep_thumbnail_cache_trimmed(cache_dir: PathBuf) {
    let mut delay = FIRST_PURGE_DELAY;
    loop {
        tokio::time::sleep(delay).await;
        delay = PURGE_INTERVAL;
        let dir = cache_dir.clone();
        let purge = tokio::task::spawn_blocking(move || {
            purge_thumbnail_cache(
                &dir,
                SystemTime::now(),
                THUMBNAIL_MAX_AGE,
                THUMBNAIL_CACHE_MAX_BYTES,
            )
        })
        .await;
        match purge {
            Ok(purge) if purge.failures == 0 => tracing::debug!(
                target: "thumbnails",
                cache_dir = ?cache_dir,
                removed = purge.removed,
                kept_bytes = purge.kept_bytes,
                "thumbnail cache trimmed"
            ),
            Ok(purge) => tracing::warn!(
                target: "thumbnails",
                event = "thumbnail_cache_purge_failed",
                cache_dir = ?cache_dir,
                removed = purge.removed,
                failures = purge.failures,
                first_failure = purge.first_failure.as_deref().unwrap_or_default(),
                "some thumbnails could not be removed while trimming the cache"
            ),
            Err(error) => tracing::warn!(
                target: "thumbnails",
                event = "thumbnail_cache_purge_failed",
                cache_dir = ?cache_dir,
                %error,
                "thumbnail cache trimming stopped unexpectedly"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::unix::fs::{symlink, PermissionsExt};

    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn write_aged(path: &Path, bytes: usize, age: Duration, now: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0; bytes]).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(now - age)
            .unwrap();
    }

    #[test]
    fn old_thumbnails_go_and_everything_else_stays() {
        let cache = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = cache.path().join("normal/old.png");
        let fresh = cache.path().join("large/fresh.png");
        let not_png = cache.path().join("normal/notes.txt");
        let outside_buckets = cache.path().join("fail/gnome-thumbnail-factory/old.png");
        let link_target = cache.path().join("target.png");
        for path in [&old, &not_png, &outside_buckets, &link_target] {
            write_aged(path, 10, 200 * DAY, now);
        }
        write_aged(&fresh, 10, DAY, now);
        let link = cache.path().join("x-large/link.png");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&link_target, &link).unwrap();

        let purge = purge_thumbnail_cache(cache.path(), now, 180 * DAY, u64::MAX);

        assert_eq!(purge.removed, 1);
        assert_eq!(purge.failures, 0);
        assert_eq!(purge.kept_bytes, 10);
        assert!(!old.exists());
        for path in [&fresh, &not_png, &outside_buckets, &link_target, &link] {
            assert!(fs::symlink_metadata(path).is_ok(), "{}", path.display());
        }
    }

    #[test]
    fn an_oversized_cache_loses_its_oldest_thumbnails_first() {
        let cache = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let oldest = cache.path().join("normal/a.png");
        let older = cache.path().join("xx-large/b.png");
        let newest = cache.path().join("large/c.png");
        write_aged(&oldest, 40, 3 * DAY, now);
        write_aged(&older, 40, 2 * DAY, now);
        write_aged(&newest, 40, DAY, now);

        let purge = purge_thumbnail_cache(cache.path(), now, 180 * DAY, 50);

        assert_eq!(purge.removed, 2);
        assert_eq!(purge.kept_bytes, 40);
        assert!(!oldest.exists());
        assert!(!older.exists());
        assert!(newest.exists());
    }

    #[test]
    fn a_failed_removal_is_reported_and_the_rest_is_still_trimmed() {
        let cache = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let stuck = cache.path().join("normal/stuck.png");
        let removable = cache.path().join("large/old.png");
        write_aged(&stuck, 10, 200 * DAY, now);
        write_aged(&removable, 10, 200 * DAY, now);
        let locked_dir = cache.path().join("normal");
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o555)).unwrap();

        let purge = purge_thumbnail_cache(cache.path(), now, 180 * DAY, u64::MAX);

        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(purge.removed, 1);
        assert_eq!(purge.failures, 1);
        assert!(purge
            .first_failure
            .is_some_and(|failure| failure.contains("normal/stuck.png")));
        assert!(stuck.exists());
        assert!(!removable.exists());
    }
}
