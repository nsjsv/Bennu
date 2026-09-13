use std::path::{Path, PathBuf};

use std::os::unix::fs::MetadataExt;
use tokio::fs;

use super::RecoverableTransferError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileObjectKind {
    RegularFile,
    Directory,
    SymbolicLink,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub object_kind: FileObjectKind,
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub changed_seconds: i64,
    pub changed_nanoseconds: i64,
    #[serde(with = "super::path_codec::optional")]
    pub symbolic_link_target: Option<PathBuf>,
}

/// 主流本地文件系统中最粗的修改时间粒度:exFAT/FAT 为 2 秒且无亚秒字段。
/// staging 副本落盘时 mtime 会被目标文件系统截断,因此快照比对必须按该粒度量化,
/// 否则跨文件系统传输(如移动到 exFAT 的 U 盘)会把截断误判为"源被修改"。
const STAGING_MTIME_GRANULARITY_SECONDS: i64 = 2;

impl FileIdentity {
    pub fn same_object(&self, other: &Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.object_kind == other.object_kind
    }

    /// 判断副本身份是否仍对应源快照:kind/size/符号链接精确相等,
    /// mtime 按 2 秒桶量化后相等(见 STAGING_MTIME_GRANULARITY_SECONDS)。
    pub fn matches_staging_snapshot(&self, snapshot: &Self) -> bool {
        self.object_kind == snapshot.object_kind
            && self.size == snapshot.size
            && self.modified_seconds.div_euclid(STAGING_MTIME_GRANULARITY_SECONDS)
                == snapshot.modified_seconds.div_euclid(STAGING_MTIME_GRANULARITY_SECONDS)
            && self.symbolic_link_target == snapshot.symbolic_link_target
    }
}

pub async fn inspect_file_identity(path: &Path) -> Result<FileIdentity, RecoverableTransferError> {
    let metadata = fs::symlink_metadata(path).await.map_err(|source| {
        RecoverableTransferError::file_system("read metadata for", path, source)
    })?;
    let file_type = metadata.file_type();
    let (object_kind, symbolic_link_target) = if file_type.is_file() {
        (FileObjectKind::RegularFile, None)
    } else if file_type.is_dir() {
        (FileObjectKind::Directory, None)
    } else if file_type.is_symlink() {
        let target = fs::read_link(path).await.map_err(|source| {
            RecoverableTransferError::file_system("read symbolic link", path, source)
        })?;
        (FileObjectKind::SymbolicLink, Some(target))
    } else {
        return Err(RecoverableTransferError::UnsupportedObject {
            path: path.to_path_buf(),
        });
    };

    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        object_kind,
        size: metadata.size(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
        symbolic_link_target,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{FileIdentity, FileObjectKind};

    // 快照比对只依赖 kind/size/mtime/symlink target,helper 只参数化 mtime,
    // 其余字段对每组用例都是无关的固定合法值。
    fn identity(modified_seconds: i64, modified_nanoseconds: i64) -> FileIdentity {
        FileIdentity {
            device: 0,
            inode: 0,
            object_kind: FileObjectKind::RegularFile,
            size: 1,
            modified_seconds,
            modified_nanoseconds,
            changed_seconds: 0,
            changed_nanoseconds: 0,
            symbolic_link_target: None,
        }
    }

    // 真实事故样本:源 mtime 21:45:25.257776066 被 exFAT 截断为 21:45:24.000000000
    // (奇数秒 -> 偶数秒、纳秒归零),量化后必须仍认定为同一快照。
    #[test]
    fn exfat_truncated_mtime_matches_snapshot() {
        let source = identity(1_787_060_725, 257_776_066);
        let payload = identity(1_787_060_724, 0);
        assert!(payload.matches_staging_snapshot(&source));
    }

    // 已落在偶数秒、但亚秒字段被目标文件系统丢掉的形态,同样不能误判为源被修改。
    #[test]
    fn even_second_with_lost_nanoseconds_matches_snapshot() {
        let source = identity(1_787_060_724, 999_999_999);
        let payload = identity(1_787_060_724, 0);
        assert!(payload.matches_staging_snapshot(&source));
    }

    #[test]
    fn size_mismatch_rejects_snapshot() {
        let mut payload = identity(100, 0);
        payload.size = 2;
        assert!(!payload.matches_staging_snapshot(&identity(100, 0)));
    }

    #[test]
    fn object_kind_mismatch_rejects_snapshot() {
        let mut payload = identity(100, 0);
        payload.object_kind = FileObjectKind::Directory;
        assert!(!payload.matches_staging_snapshot(&identity(100, 0)));
    }

    #[test]
    fn symlink_target_mismatch_rejects_snapshot() {
        let mut snapshot = identity(100, 0);
        snapshot.object_kind = FileObjectKind::SymbolicLink;
        snapshot.symbolic_link_target = Some(PathBuf::from("old-target"));
        let mut payload = snapshot.clone();
        payload.symbolic_link_target = Some(PathBuf::from("new-target"));
        assert!(!payload.matches_staging_snapshot(&snapshot));
    }

    #[test]
    fn mtime_within_same_bucket_matches_snapshot() {
        let source = identity(1_787_060_725, 0);
        let payload = identity(1_787_060_724, 0);
        assert!(payload.matches_staging_snapshot(&source));
    }

    #[test]
    fn mtime_across_buckets_rejects_snapshot() {
        let source = identity(1_787_060_726, 0);
        let payload = identity(1_787_060_724, 0);
        assert!(!payload.matches_staging_snapshot(&source));
    }

    // div_euclid 向 -∞ 取整:pre-epoch 的负秒值 (-3, nsec>0) 被截断成 (-4, 0)
    // 时仍落同一 2 秒桶,与 exFAT 对负时间戳的截断方向一致。
    #[test]
    fn negative_pre_epoch_seconds_floor_to_same_bucket() {
        let source = identity(-3, 500_000_000);
        let payload = identity(-4, 0);
        assert!(payload.matches_staging_snapshot(&source));
    }
}
