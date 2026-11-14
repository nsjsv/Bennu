//! 7z 把包里不是 UTF-8 的名字原样写到磁盘上（中文 Windows 打的 zip 多是 GBK）；解完按
//! 列目录时识别出的显示名改过来，和预览、包内浏览看到的名字一致。

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::archive_listing::{decode_member_names, SevenZipListedEntry};
use crate::ops::{move_without_replacing, NoReplaceRenameError};
use crate::{entry_name_candidates, EntryNameKind, FileError};

pub(super) fn rename_to_display_names(
    destination: &Path,
    entries: &[SevenZipListedEntry],
) -> Result<(), FileError> {
    let stored_paths = entries.iter().map(|entry| &entry.member.stored_path[..]);
    if stored_paths
        .clone()
        .all(|stored| std::str::from_utf8(stored).is_ok())
    {
        return Ok(());
    }
    let display_paths = decode_member_names(stored_paths.clone());
    for (stored_prefix, display_name) in renamed_components(stored_paths, &display_paths) {
        let from = destination.join(OsStr::from_bytes(&stored_prefix));
        rename_without_replacing(&from, &display_name)?;
    }
    Ok(())
}

/// 要改名的每一层：磁盘上 7z 写出的路径（包里存的前缀）→ 这一层的显示名。没有单独条目的
/// 中间目录也在里面。深的在前：先改上层目录，下层的路径就对不上了。
fn renamed_components<'a>(
    stored_paths: impl Iterator<Item = &'a [u8]>,
    display_paths: &[String],
) -> Vec<(Vec<u8>, String)> {
    let mut renames = BTreeMap::new();
    for (stored, display) in stored_paths.zip(display_paths) {
        let stored_parts = stored
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty());
        let display_parts = display.split('/').filter(|part| !part.is_empty());
        let mut prefix = Vec::new();
        for (stored_part, display_part) in stored_parts.zip(display_parts) {
            if !prefix.is_empty() {
                prefix.push(b'/');
            }
            prefix.extend_from_slice(stored_part);
            if stored_part != display_part.as_bytes() {
                renames
                    .entry(prefix.clone())
                    .or_insert_with(|| display_part.to_owned());
            }
        }
    }
    let mut renames = renames.into_iter().collect::<Vec<_>>();
    renames.sort_by_key(|(prefix, _)| Reverse(prefix.iter().filter(|byte| **byte == b'/').count()));
    renames
}

/// 显示名已被占用（解进了已有内容的文件夹）时按「名字 2」顺延，不覆盖任何东西。
/// 7z 没写出来的条目（被它当成不安全路径跳过）没有可改的，跳过。
fn rename_without_replacing(from: &Path, display_name: &str) -> Result<(), FileError> {
    let kind = match std::fs::symlink_metadata(from) {
        Ok(metadata) => EntryNameKind::of_metadata(&metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(FileError::Metadata {
                path: from.to_path_buf(),
                source,
            })
        }
    };
    for candidate in entry_name_candidates(OsStr::new(display_name), kind) {
        let to = from.with_file_name(candidate);
        match move_without_replacing(from, &to) {
            Ok(()) => return Ok(()),
            Err(NoReplaceRenameError::TargetExists) => continue,
            Err(error) => {
                return Err(FileError::Rename {
                    from: from.to_path_buf(),
                    to,
                    source: error.into_io_error(),
                })
            }
        }
    }
    Err(FileError::EntryNameCandidatesExhausted {
        directory: from.parent().unwrap_or(from).to_path_buf(),
        base: display_name.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_that_differs_is_renamed_deepest_first() {
        let stored: [&[u8]; 3] = [b"\xce\xc4/", b"\xce\xc4/b\xb1.txt", b"plain/\xb2/c.txt"];
        let display = [
            "文/".to_owned(),
            "文/b报.txt".to_owned(),
            "plain/档/c.txt".to_owned(),
        ];

        let renames = renamed_components(stored.into_iter(), &display);

        assert_eq!(
            renames,
            vec![
                (b"plain/\xb2".to_vec(), "档".to_owned()),
                (b"\xce\xc4/b\xb1.txt".to_vec(), "b报.txt".to_owned()),
                (b"\xce\xc4".to_vec(), "文".to_owned()),
            ]
        );
    }

    #[test]
    fn taken_display_names_get_numbered_instead_of_replaced() {
        let directory = tempfile::tempdir().expect("temp dir");
        let raw = directory.path().join(OsStr::from_bytes(b"\xb1\xa8.txt"));
        std::fs::write(&raw, b"new").expect("write raw");
        std::fs::write(directory.path().join("报.txt"), b"old").expect("write existing");

        rename_without_replacing(&raw, "报.txt").expect("rename");

        assert_eq!(
            std::fs::read(directory.path().join("报.txt")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(directory.path().join("报 2.txt")).unwrap(),
            b"new"
        );
        assert!(!raw.exists());
        // 没解出来的条目不报错。
        rename_without_replacing(&raw, "报.txt").expect("missing entry is skipped");
    }
}
