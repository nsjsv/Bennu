//! `tar -tv` 列 tar 一类（tar 读文件时自己认 gzip/xz/zstd/bzip2）。

use std::path::Path;
use std::time::SystemTime;

use super::StoredMember;
use crate::tar_job::{tar_command, tar_failure};
use crate::{FileError, FileKind};

pub(super) async fn list_tar_members(archive: &Path) -> Result<Vec<StoredMember>, FileError> {
    let output = tar_command()
        .arg("-t")
        .arg("-v")
        .arg("--full-time")
        .arg("--numeric-owner")
        .arg("--quoting-style=escape")
        .arg("-f")
        .arg(archive)
        .output()
        .await
        .map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: format!("could not run tar: {source}"),
        })?;
    if !output.status.success() {
        return Err(tar_failure(archive, output.status, &output.stderr));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(parse_tar_listing_line)
        .collect())
}

/// `-rw-r--r-- 0/0   5 2026-11-14 09:00:00 dir/name`：权限、属主、大小、日期、时间、名字。
/// 名字前恰好一个空格（名字本身可以以空格开头）；符号链接后面跟 ` -> 目标`，硬链接跟 ` link to 目标`。
fn parse_tar_listing_line(line: &[u8]) -> Option<StoredMember> {
    let line = std::str::from_utf8(line).ok()?;
    let (mode, rest) = next_field(line)?;
    let (_owner, rest) = next_field(rest)?;
    let (size, rest) = next_field(rest)?;
    let (date, rest) = next_field(rest)?;
    let (time, rest) = next_field(rest)?;
    let name = rest.strip_prefix(' ')?;
    let (kind, name) = match mode.as_bytes().first()? {
        b'd' => (FileKind::Directory, name),
        b'l' => (
            FileKind::Symlink,
            name.split_once(" -> ").map_or(name, |(name, _)| name),
        ),
        b'h' => (
            FileKind::File,
            name.split_once(" link to ").map_or(name, |(name, _)| name),
        ),
        b'-' | b'C' => (FileKind::File, name),
        _ => (FileKind::Other, name),
    };
    let modified =
        chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|time| SystemTime::from(time.and_utc()));
    Some(StoredMember {
        stored_path: unescape(name),
        kind,
        // 设备文件这一栏是 `主,次` 设备号，不是大小。
        len: size.parse().unwrap_or(0),
        modified,
    })
}

fn next_field(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches(' ');
    let end = text.find(' ')?;
    Some((&text[..end], &text[end..]))
}

/// `--quoting-style=escape` 的反转义：`\ooo` 八进制字节、`\n` 之类的控制字符、`\\`。
fn unescape(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut unescaped = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte != b'\\' || index == bytes.len() {
            unescaped.push(byte);
            continue;
        }
        let escaped = bytes[index];
        index += 1;
        unescaped.push(match escaped {
            b'0'..=b'7' => {
                let mut value = u32::from(escaped - b'0');
                for _ in 0..2 {
                    match bytes.get(index) {
                        Some(digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            index += 1;
                        }
                        _ => break,
                    }
                }
                value as u8
            }
            b'a' => 0x07,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            other => other,
        });
    }
    unescaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn listing_lines_give_kind_size_time_and_raw_name() {
        let file = parse_tar_listing_line(
            b"-rw-r--r-- 0/0               5 1970-01-02 00:00:01 \\344\\270\\255/a\\\\b \\n.txt",
        )
        .expect("file line");
        assert_eq!(file.kind, FileKind::File);
        assert_eq!(file.len, 5);
        assert_eq!(
            file.modified,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(86_401))
        );
        assert_eq!(file.stored_path, "中/a\\b \n.txt".as_bytes());

        let directory =
            parse_tar_listing_line(b"drwxr-xr-x 1000/1000 0 2026-11-14 09:00:00  leading space/")
                .expect("directory line");
        assert_eq!(directory.kind, FileKind::Directory);
        assert_eq!(directory.stored_path, b" leading space/");

        let link = parse_tar_listing_line(b"lrwxrwxrwx 0/0 0 2026-11-14 09:00:00 link -> target")
            .expect("symlink line");
        assert_eq!(link.kind, FileKind::Symlink);
        assert_eq!(link.stored_path, b"link");

        let hard_link =
            parse_tar_listing_line(b"hrw-r--r-- 0/0 0 2026-11-14 09:00:00 copy link to original")
                .expect("hard link line");
        assert_eq!(hard_link.kind, FileKind::File);
        assert_eq!(hard_link.stored_path, b"copy");

        let device = parse_tar_listing_line(b"crw-r--r-- 0/0 1,3 2026-11-14 09:00:00 dev/null")
            .expect("device line");
        assert_eq!(device.kind, FileKind::Other);
        assert_eq!(device.len, 0);

        assert!(parse_tar_listing_line(b"").is_none());
    }
}
