//! `7z l -slt` 列 zip / 7z / rar。浏览只用路径、类型、大小、修改时间；整包解压的密码检查还要
//! `Encrypted` 和 `Block`（同一 solid block 里的文件要按顺序解）。

use std::path::Path;
use std::process::Stdio;
use std::time::SystemTime;

use tokio_util::sync::CancellationToken;

use super::StoredMember;
use crate::seven_zip_job::{locate_seven_zip, seven_zip_failure};
use crate::{ArchivePassword, FileError, FileKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SevenZipListedEntry {
    pub(crate) member: StoredMember,
    pub(crate) encrypted: bool,
    /// 7z 格式才有；rar 等没有时为 None，按各文件独立解码对待。
    pub(crate) block: Option<u64>,
}

impl SevenZipListedEntry {
    pub(crate) fn is_directory(&self) -> bool {
        self.member.kind == FileKind::Directory
    }
}

/// 头部加密的包没给密码（或给错）时这里就失败，按密码错误分类。
pub(crate) async fn list_seven_zip_entries(
    archive: &Path,
    password: Option<&ArchivePassword>,
    cancel: &CancellationToken,
) -> Result<Vec<SevenZipListedEntry>, FileError> {
    let mut command = tokio::process::Command::new(locate_seven_zip()?);
    command.arg("l").arg("-slt").arg("-bd").arg("-bsp0");
    if let Some(password) = password {
        command.arg(format!("-p{}", password.as_str()));
    }
    // 时间按 UTC 打印，解析时不必猜本地时区。
    command
        .env("TZ", "UTC0")
        .arg("--")
        .arg(archive)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::select! {
        output = command.output() => output.map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: format!("could not run 7z: {source}"),
        })?,
        _ = cancel.cancelled() => return Err(FileError::Cancelled),
    };
    if !output.status.success() {
        return Err(seven_zip_failure(
            archive,
            password.is_some(),
            output.status,
            &output.stdout,
            &output.stderr,
        ));
    }
    Ok(parse_seven_zip_entries(&output.stdout))
}

/// 按字节解析：包里存的名字不一定是 UTF-8（见 `member_names`），路径要原样留下。
pub(crate) fn parse_seven_zip_entries(technical_listing: &[u8]) -> Vec<SevenZipListedEntry> {
    let mut entries = Vec::new();
    let mut in_archive_entries = false;
    let mut current: Option<SevenZipListedEntry> = None;

    for line in technical_listing.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line == b"----------" {
            in_archive_entries = true;
            continue;
        }
        if !in_archive_entries {
            continue;
        }
        if line.is_empty() {
            entries.extend(current.take());
            continue;
        }
        if let Some(path) = line.strip_prefix(b"Path = ") {
            entries.extend(current.take());
            current = Some(SevenZipListedEntry {
                member: StoredMember {
                    stored_path: path.to_vec(),
                    kind: if path.ends_with(b"/") || path.ends_with(b"\\") {
                        FileKind::Directory
                    } else {
                        FileKind::File
                    },
                    len: 0,
                    modified: None,
                },
                encrypted: false,
                block: None,
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = String::from_utf8_lossy(line)
            .split_once(" = ")
            .map(|(key, value)| (key.to_owned(), value.trim().to_owned()))
        else {
            continue;
        };
        match key.as_str() {
            "Folder" if value == "+" || value.eq_ignore_ascii_case("true") => {
                entry.member.kind = FileKind::Directory;
            }
            "Attributes" => apply_attributes(&mut entry.member, &value),
            "Size" => entry.member.len = value.parse().unwrap_or(0),
            "Modified" => entry.member.modified = parse_modified(&value),
            "Encrypted" => entry.encrypted = value == "+",
            "Block" => entry.block = value.parse().ok(),
            _ => {}
        }
    }

    entries.extend(current);
    entries.retain(|entry| !entry.member.stored_path.is_empty());
    entries
}

/// `D drwxr-xr-x`、`A -rw-r--r--`、` lrwxrwxrwx`：开头是 Windows 属性字母，后面是 Unix 权限串。
fn apply_attributes(member: &mut StoredMember, attributes: &str) {
    if attributes.starts_with('D') {
        member.kind = FileKind::Directory;
    } else if member.kind == FileKind::File
        && attributes
            .split_whitespace()
            .any(|token| token.len() == 10 && token.starts_with('l'))
    {
        member.kind = FileKind::Symlink;
    }
}

/// `2026-11-14 09:06:26.7436731` 或不带小数；列目录时已让 7z 按 UTC 打印。
fn parse_modified(value: &str) -> Option<SystemTime> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .ok()
        .map(|time| SystemTime::from(time.and_utc()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn listing_keeps_raw_names_and_reads_kinds_sizes_and_times() {
        let mut listing = b"Path = archive.zip\nType = zip\n\n----------\n".to_vec();
        listing.extend_from_slice(b"Path = src\nFolder = +\nAttributes = D drwxr-xr-x\n\n");
        listing.extend_from_slice(
            b"Path = src/main.rs\nSize = 12\nModified = 2026-11-14 09:06:26.5\nAttributes = A -rw-r--r--\nEncrypted = +\nBlock = 3\n\n",
        );
        listing
            .extend_from_slice(b"Path = \xce\xc4\xb5\xb5.txt\r\nSize = 5\r\nModified = \r\n\r\n");
        listing.extend_from_slice(b"Path = link\nAttributes =  lrwxrwxrwx\n");

        let entries = parse_seven_zip_entries(&listing);

        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].member.stored_path, b"src");
        assert_eq!(entries[0].member.kind, FileKind::Directory);
        assert_eq!(entries[1].member.kind, FileKind::File);
        assert_eq!(entries[1].member.len, 12);
        assert_eq!(
            entries[1].member.modified,
            Some(SystemTime::UNIX_EPOCH + Duration::from_millis(1_794_647_186_500))
        );
        assert!(entries[1].encrypted);
        assert_eq!(entries[1].block, Some(3));
        assert_eq!(entries[2].member.stored_path, b"\xce\xc4\xb5\xb5.txt");
        assert_eq!(entries[2].member.modified, None);
        assert!(!entries[2].encrypted);
        assert_eq!(entries[3].member.kind, FileKind::Symlink);
    }
}
