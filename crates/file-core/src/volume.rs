//! 路径所在的磁盘：挂在哪、什么文件系统、多大、还剩多少。

use std::io;
use std::path::{Path, PathBuf};

use crate::mount_table::{
    parse_mount_table, MountTableEntry, MountTableSnapshot, PROC_SELF_MOUNTINFO,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeUsage {
    pub mount_point: PathBuf,
    pub fs_type: String,
    pub total_bytes: u64,
    /// 普通用户还能写多少：不含只留给 root 的保留块，同 `df` 的 Avail。
    pub available_bytes: u64,
}

/// `path` 要存在；链接按指向的位置量。
pub fn volume_usage(path: &Path) -> io::Result<VolumeUsage> {
    let path = std::fs::canonicalize(path)?;
    let table = parse_mount_table(&std::fs::read(PROC_SELF_MOUNTINFO)?);
    let mount = mount_containing(&table, &path)
        .ok_or_else(|| io::Error::other("no mount point contains this path"))?;
    let stats = rustix::fs::statvfs(&path)?;
    Ok(VolumeUsage {
        mount_point: mount.mount_point.clone(),
        fs_type: mount.fs_type.to_string_lossy().into_owned(),
        total_bytes: stats.f_blocks.saturating_mul(stats.f_frsize),
        available_bytes: stats.f_bavail.saturating_mul(stats.f_frsize),
    })
}

/// 嵌套挂载取最里层：前缀最长的那个挂载点。
fn mount_containing<'a>(table: &'a MountTableSnapshot, path: &Path) -> Option<&'a MountTableEntry> {
    table
        .mounts
        .iter()
        .filter(|mount| path.starts_with(&mount.mount_point))
        .max_by_key(|mount| mount.mount_point.components().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_innermost_mount_point_holds_the_path() {
        let table = parse_mount_table(
            b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n\
              2 1 8:2 / /home rw - btrfs /dev/sda2 rw\n\
              3 2 0:40 / /home/me/remote\\040share rw - fuse.sshfs host: rw\n",
        );
        let mount_of = |path: &str| {
            mount_containing(&table, Path::new(path))
                .map(|mount| mount.mount_point.to_string_lossy().into_owned())
        };

        assert_eq!(mount_of("/etc/fstab").as_deref(), Some("/"));
        assert_eq!(mount_of("/home").as_deref(), Some("/home"));
        assert_eq!(mount_of("/home/me/notes").as_deref(), Some("/home"));
        assert_eq!(
            mount_of("/home/me/remote share/a").as_deref(),
            Some("/home/me/remote share")
        );
        // 只是名字开头一样，不在那个挂载点里面。
        assert_eq!(mount_of("/homework").as_deref(), Some("/"));
    }

    #[test]
    fn a_real_directory_has_a_size_and_free_space_within_it() {
        let directory = tempfile::tempdir().unwrap();
        let usage = volume_usage(directory.path()).unwrap();

        assert!(usage.total_bytes > 0);
        assert!(usage.available_bytes <= usage.total_bytes);
        assert!(directory
            .path()
            .canonicalize()
            .unwrap()
            .starts_with(&usage.mount_point));
        assert!(!usage.fs_type.is_empty());
    }
}
