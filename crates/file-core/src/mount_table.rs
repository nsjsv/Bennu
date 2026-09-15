use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

pub const PROC_SELF_MOUNTINFO: &str = "/proc/self/mountinfo";

#[derive(Debug, Eq, PartialEq)]
pub struct MountTableWarning {
    pub line_number: usize,
    pub message: &'static str,
}

#[derive(Debug, Eq, PartialEq)]
pub struct MountTableEntry {
    pub mount_point: PathBuf,
    pub fs_type: OsString,
    pub device: OsString,
}

#[derive(Debug, Eq, PartialEq)]
pub struct MountTableSnapshot {
    pub mounts: Vec<MountTableEntry>,
    pub warnings: Vec<MountTableWarning>,
}

pub fn parse_mount_table(content: &[u8]) -> MountTableSnapshot {
    let mut mounts = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = HashSet::new();

    for (line_index, line) in content.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields = line
            .split(|byte| *byte == b' ')
            .filter(|field| !field.is_empty())
            .collect::<Vec<_>>();
        let Some(separator_index) = fields.iter().position(|field| *field == b"-") else {
            warnings.push(mount_table_warning(line_index, "invalid mountinfo record"));
            continue;
        };
        if fields.len() < 10 || separator_index < 6 || fields.len() < separator_index + 4 {
            warnings.push(mount_table_warning(line_index, "invalid mountinfo record"));
            continue;
        }

        let decoded = match decode_mount_path(fields[4]) {
            Ok(decoded) => decoded,
            Err(message) => {
                warnings.push(mount_table_warning(line_index, message));
                continue;
            }
        };
        let mount_point = path_from_bytes(decoded);
        if !mount_point.is_absolute() {
            warnings.push(mount_table_warning(
                line_index,
                "mount point is not an absolute path",
            ));
            continue;
        }
        if seen.insert(path_bytes(&mount_point)) {
            // Like glib's mountinfo parsing, fs_type and device are kept as raw
            // fields: only the path fields carry \040-style escapes.
            mounts.push(MountTableEntry {
                mount_point,
                fs_type: os_string_from_bytes(fields[separator_index + 1].to_vec()),
                device: os_string_from_bytes(fields[separator_index + 2].to_vec()),
            });
        }
    }

    MountTableSnapshot { mounts, warnings }
}

fn decode_mount_path(encoded: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] != b'\\' {
            if encoded[index] == 0 {
                return Err("mount point contains a NUL byte");
            }
            decoded.push(encoded[index]);
            index += 1;
            continue;
        }
        let escape = encoded
            .get(index + 1..index + 4)
            .ok_or("mount point has a truncated escape")?;
        let value = match escape {
            b"040" => b' ',
            b"011" => b'\t',
            b"012" => b'\n',
            b"134" => b'\\',
            _ => return Err("mount point has an unknown escape"),
        };
        decoded.push(value);
        index += 4;
    }
    Ok(decoded)
}

fn mount_table_warning(line_index: usize, message: &'static str) -> MountTableWarning {
    MountTableWarning {
        line_number: line_index + 1,
        message,
    }
}

#[cfg(unix)]
fn os_string_from_bytes(bytes: Vec<u8>) -> OsString {
    OsString::from_vec(bytes)
}

#[cfg(not(unix))]
fn os_string_from_bytes(bytes: Vec<u8>) -> OsString {
    OsString::from(String::from_utf8_lossy(&bytes).into_owned())
}

fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(os_string_from_bytes(bytes))
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().to_string_lossy().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_preserves_bytes_order_and_distinct_mounts() {
        let snapshot = parse_mount_table(
            b"1 0 8:1 / / rw - ext4 /dev/root rw\n\
              2 1 8:2 / /media/My\\040Disk rw - ext4 /dev/sdb rw\n\
              3 1 8:2 / /media/My\\040Disk rw - ext4 /dev/sdb rw\n\
              4 1 8:3 / /media/tab\\011line\\012slash\\134 rw - ext4 /dev/sdc rw\n",
        );

        let mount_points = snapshot
            .mounts
            .iter()
            .map(|mount| mount.mount_point.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            mount_points,
            vec![
                PathBuf::from("/"),
                PathBuf::from("/media/My Disk"),
                PathBuf::from("/media/tab\tline\nslash\\"),
            ]
        );
        assert!(snapshot.warnings.is_empty());
    }

    #[test]
    fn parser_captures_fs_type_and_device_per_mount() {
        let snapshot = parse_mount_table(
            b"1 0 8:1 / / rw - btrfs /dev/nvme0n1p2 rw,compress=zstd\n\
              2 1 0:75 / /mnt/cloud rw - fuse.rclone TG: rw\n",
        );

        assert_eq!(snapshot.mounts.len(), 2);
        assert_eq!(snapshot.mounts[0].fs_type, OsString::from("btrfs"));
        assert_eq!(snapshot.mounts[0].device, OsString::from("/dev/nvme0n1p2"));
        assert_eq!(snapshot.mounts[1].fs_type, OsString::from("fuse.rclone"));
        assert_eq!(snapshot.mounts[1].device, OsString::from("TG:"));
        assert!(snapshot.warnings.is_empty());
    }

    #[test]
    fn malformed_records_are_reported_without_hiding_valid_mounts() {
        let snapshot = parse_mount_table(
            b"broken\n\
              2 1 8:2 / relative rw - ext4 /dev/sdb rw\n\
              3 1 8:3 / /bad\\999 rw - ext4 /dev/sdc rw\n\
              4 1 8:4 / /valid rw - ext4 /dev/sdd rw\n",
        );

        let mount_points = snapshot
            .mounts
            .iter()
            .map(|mount| mount.mount_point.clone())
            .collect::<Vec<_>>();
        assert_eq!(mount_points, vec![PathBuf::from("/valid")]);
        assert_eq!(snapshot.warnings.len(), 3);
        assert_eq!(snapshot.warnings[0].line_number, 1);
    }
}
