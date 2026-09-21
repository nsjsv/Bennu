use std::ffi::OsStr;
use std::path::PathBuf;

use crate::mount_table::{parse_mount_table, MountTableEntry, PROC_SELF_MOUNTINFO};
use crate::ScanWarning;

pub(super) const MOUNTINFO_PATH: &str = PROC_SELF_MOUNTINFO;

#[derive(Debug)]
pub(super) struct MountInfoSnapshot {
    pub mounts: Vec<MountTableEntry>,
    pub warnings: Vec<ScanWarning>,
}

pub(super) fn parse_mountinfo(content: &[u8]) -> MountInfoSnapshot {
    let snapshot = parse_mount_table(content);
    MountInfoSnapshot {
        mounts: snapshot.mounts,
        warnings: snapshot
            .warnings
            .into_iter()
            .map(|warning| ScanWarning {
                path: PathBuf::from(MOUNTINFO_PATH),
                message: format!("line {}: {}", warning.line_number, warning.message),
            })
            .collect(),
    }
}

/// Filesystem types that never host a trash directory. Mirrors glib's
/// `ignore_fs` list plus virtual filesystems observed on modern kernels.
const SYSTEM_FS_TYPES: &[&str] = &[
    "auto",
    "autofs",
    "autofs4",
    "bdev",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devfs",
    "devpts",
    "efivarfs",
    "fusectl",
    "hugetlbfs",
    "kernfs",
    "linprocfs",
    "mqueue",
    "nsfs",
    "overlay",
    "proc",
    "procfs",
    "pstore",
    "ptyfs",
    "ramfs",
    "rootfs",
    "selinuxfs",
    "squashfs",
    "sysfs",
    "tmpfs",
    "tracefs",
    "usbfs",
];

fn is_system_fs_type(fs_type: &OsStr) -> bool {
    SYSTEM_FS_TYPES
        .iter()
        .any(|candidate| fs_type == OsStr::new(candidate))
}

fn device_has_block_prefix(device: &OsStr) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        device.as_bytes().starts_with(b"/dev/")
    }
    #[cfg(not(unix))]
    {
        device.to_string_lossy().starts_with("/dev/")
    }
}

/// Whether a mount is worth probing for a trash directory at all.
///
/// Mounts whose device is not a `/dev/*` block path (network filesystems,
/// FUSE bridges like rclone or gvfs, ZFS datasets) are skipped: probing them
/// happens on this scan's synchronous critical path, and one network stat can
/// block for seconds. This matches glib's
/// `g_unix_mount_entry_is_system_internal` behavior. The file manager never
/// trashes files on remote mounts anyway (they are deleted permanently), so
/// this filter cannot hide recoverable data.
pub(super) fn mounts_candidate_for_trash_probing(entry: &MountTableEntry) -> bool {
    !is_system_fs_type(entry.fs_type.as_os_str())
        && device_has_block_prefix(entry.device.as_os_str())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn trash_snapshot_preserves_shared_parser_warnings() {
        let snapshot = parse_mountinfo(
            b"broken\n\
              2 1 8:2 / relative rw - ext4 /dev/sdb rw\n\
              3 1 8:3 / /bad\\999 rw - ext4 /dev/sdc rw\n\
              4 1 8:4 / /valid rw - ext4 /dev/sdd rw\n",
        );

        assert_eq!(
            snapshot
                .mounts
                .iter()
                .map(|mount| mount.mount_point.clone())
                .collect::<Vec<_>>(),
            vec![PathBuf::from("/valid")]
        );
        assert_eq!(snapshot.warnings.len(), 3);
        assert!(snapshot
            .warnings
            .iter()
            .all(|warning| warning.path == Path::new(MOUNTINFO_PATH)));
    }

    #[test]
    fn trash_snapshot_filters_non_local_mounts() {
        let snapshot = parse_mountinfo(
            b"1 0 8:1 / / rw - btrfs /dev/nvme0n1p2 rw\n\
              2 1 0:75 / /mnt/TG rw - fuse.rclone TG: rw\n\
              3 1 0:76 / /mnt/123 rw - fuse.rclone webdev: rw\n\
              4 1 0:77 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw\n\
              5 1 0:78 /mnt/net rw - nfs server:/export rw\n\
              6 1 0:79 /mnt/autodir rw - autofs systemd-1 rw\n",
        );

        let probe_candidates = snapshot
            .mounts
            .iter()
            .filter(|mount| mounts_candidate_for_trash_probing(mount))
            .map(|mount| mount.mount_point.clone())
            .collect::<Vec<_>>();
        assert_eq!(probe_candidates, vec![PathBuf::from("/")]);
    }

    #[test]
    fn trash_snapshot_keeps_local_volume_mounts() {
        let snapshot = parse_mountinfo(
            b"1 0 8:1 /@ / rw - btrfs /dev/nvme0n1p2 rw\n\
              2 1 8:1 /@home /home rw - btrfs /dev/nvme0n1p2 rw\n\
              3 1 8:2 /boot rw - vfat /dev/nvme0n1p1 rw\n",
        );

        assert!(snapshot
            .mounts
            .iter()
            .all(mounts_candidate_for_trash_probing));
    }
}
