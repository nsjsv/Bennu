//! 条目命名规则的唯一来源：文件名合法性、带后缀的派生名、"名字 N" 候选序列。
//! 复制引擎、回收站还原与界面都从这里取规则，同一个名字在不同入口不会编出不同结果。

use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::entry::FileKind;
use crate::ops::FileObjectKind;

/// 候选序列在首选名之后编号到的最大值。超过后由调用方报告"没有可用名字"，
/// 不能退回冲突的原名。
pub const ENTRY_NAME_LAST_CANDIDATE_NUMBER: usize = 999;

/// 编号和派生后缀插在哪里取决于条目是不是目录：文件按扩展名拆分，保住扩展名
/// 才不影响格式识别；目录名里的点不是扩展名，整体当主名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryNameKind {
    File,
    Directory,
}

impl EntryNameKind {
    pub fn of_metadata(metadata: &std::fs::Metadata) -> Self {
        if metadata.is_dir() {
            Self::Directory
        } else {
            Self::File
        }
    }
}

impl From<FileKind> for EntryNameKind {
    fn from(kind: FileKind) -> Self {
        match kind {
            FileKind::Directory => Self::Directory,
            FileKind::File | FileKind::Symlink | FileKind::Other => Self::File,
        }
    }
}

impl From<FileObjectKind> for EntryNameKind {
    fn from(kind: FileObjectKind) -> Self {
        match kind {
            FileObjectKind::Directory => Self::Directory,
            FileObjectKind::RegularFile | FileObjectKind::SymbolicLink => Self::File,
        }
    }
}

/// 只拦文件系统层面非法、或会让 `parent.join(name)` 离开父目录的名字；
/// `\` 等在 Linux 上合法的字符不在此列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidEntryName {
    #[error("name is empty")]
    Empty,
    #[error("name cannot be \".\"")]
    CurrentDirectory,
    #[error("name cannot be \"..\"")]
    ParentDirectory,
    #[error("name cannot contain \"/\"")]
    ContainsSlash,
    #[error("name cannot contain a NUL character")]
    ContainsNul,
}

pub fn validate_entry_name(name: &OsStr) -> Result<(), InvalidEntryName> {
    let bytes = name.as_encoded_bytes();
    if bytes.is_empty() {
        Err(InvalidEntryName::Empty)
    } else if bytes == b"." {
        Err(InvalidEntryName::CurrentDirectory)
    } else if bytes == b".." {
        Err(InvalidEntryName::ParentDirectory)
    } else if bytes.contains(&b'/') {
        Err(InvalidEntryName::ContainsSlash)
    } else if bytes.contains(&0) {
        Err(InvalidEntryName::ContainsNul)
    } else {
        Ok(())
    }
}

/// 后缀插在主名与扩展名之间（`report.pdf` + "副本" → `report副本.pdf`）。
pub fn entry_name_with_suffix(original: &OsStr, kind: EntryNameKind, suffix: &str) -> OsString {
    let (stem, extension) = split_entry_name(original, kind);
    composed_entry_name(&stem, suffix, extension.as_deref())
}

/// 首选名本身，然后是 `主名 2.扩展名` … `主名 999.扩展名`。
pub fn entry_name_candidates(base: &OsStr, kind: EntryNameKind) -> EntryNameCandidates {
    let (stem, extension) = split_entry_name(base, kind);
    EntryNameCandidates {
        base: base.to_os_string(),
        stem,
        extension,
        next_number: 1,
    }
}

#[derive(Debug, Clone)]
pub struct EntryNameCandidates {
    base: OsString,
    stem: OsString,
    extension: Option<OsString>,
    next_number: usize,
}

impl Iterator for EntryNameCandidates {
    type Item = OsString;

    fn next(&mut self) -> Option<OsString> {
        let number = self.next_number;
        if number > ENTRY_NAME_LAST_CANDIDATE_NUMBER {
            return None;
        }
        self.next_number += 1;
        if number == 1 {
            return Some(self.base.clone());
        }
        Some(composed_entry_name(
            &self.stem,
            &format!(" {number}"),
            self.extension.as_deref(),
        ))
    }
}

/// 以点开头且没有扩展名的隐藏文件（`.bashrc`）由 `Path::extension` 判为无扩展名，
/// 整体当主名。
pub(crate) fn split_entry_name(name: &OsStr, kind: EntryNameKind) -> (OsString, Option<OsString>) {
    if kind == EntryNameKind::Directory {
        return (name.to_os_string(), None);
    }
    let path = Path::new(name);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) => (stem.to_os_string(), Some(extension.to_os_string())),
        _ => (name.to_os_string(), None),
    }
}

pub(crate) fn composed_entry_name(
    stem: &OsStr,
    middle: &str,
    extension: Option<&OsStr>,
) -> OsString {
    let mut name = stem.to_os_string();
    name.push(middle);
    if let Some(extension) = extension {
        name.push(".");
        name.push(extension);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(candidates: impl Iterator<Item = OsString>, count: usize) -> Vec<String> {
        candidates
            .take(count)
            .map(|name| name.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn rejects_names_that_are_illegal_or_leave_the_parent() {
        assert_eq!(
            validate_entry_name(OsStr::new("")),
            Err(InvalidEntryName::Empty)
        );
        assert_eq!(
            validate_entry_name(OsStr::new(".")),
            Err(InvalidEntryName::CurrentDirectory)
        );
        assert_eq!(
            validate_entry_name(OsStr::new("..")),
            Err(InvalidEntryName::ParentDirectory)
        );
        assert_eq!(
            validate_entry_name(OsStr::new("a/b")),
            Err(InvalidEntryName::ContainsSlash)
        );
        assert_eq!(
            validate_entry_name(OsStr::new("../escape")),
            Err(InvalidEntryName::ContainsSlash)
        );
        assert_eq!(
            validate_entry_name(OsStr::new("a\0b")),
            Err(InvalidEntryName::ContainsNul)
        );
    }

    #[test]
    fn accepts_backslashes_dots_and_hidden_names() {
        for name in ["a\\b", "...", ".hidden", "report.pdf", "名字 2"] {
            assert_eq!(validate_entry_name(OsStr::new(name)), Ok(()), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn accepts_non_utf8_names() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(validate_entry_name(OsStr::from_bytes(b"caf\xe9")), Ok(()));
    }

    #[test]
    fn suffix_goes_between_stem_and_extension_for_files_only() {
        let cases = [
            ("report.pdf", EntryNameKind::File, "report副本.pdf"),
            ("archive.tar.gz", EntryNameKind::File, "archive.tar副本.gz"),
            (".bashrc", EntryNameKind::File, ".bashrc副本"),
            ("notes", EntryNameKind::File, "notes副本"),
            ("photos.2024", EntryNameKind::Directory, "photos.2024副本"),
        ];
        for (original, kind, expected) in cases {
            assert_eq!(
                entry_name_with_suffix(OsStr::new(original), kind, "副本"),
                OsString::from(expected),
                "{original}"
            );
        }
    }

    #[test]
    fn candidates_start_with_the_base_and_number_before_the_extension() {
        assert_eq!(
            names(
                entry_name_candidates(OsStr::new("a.txt"), EntryNameKind::File),
                3
            ),
            ["a.txt", "a 2.txt", "a 3.txt"]
        );
        assert_eq!(
            names(
                entry_name_candidates(OsStr::new(".bashrc"), EntryNameKind::File),
                2
            ),
            [".bashrc", ".bashrc 2"]
        );
    }

    #[test]
    fn directory_candidates_number_after_the_whole_name() {
        assert_eq!(
            names(
                entry_name_candidates(OsStr::new("photos.2024"), EntryNameKind::Directory),
                2
            ),
            ["photos.2024", "photos.2024 2"]
        );
    }

    #[test]
    fn duplicate_names_continue_the_historical_copy_sequence() {
        let base = entry_name_with_suffix(OsStr::new("report.pdf"), EntryNameKind::File, "副本");
        assert_eq!(
            names(entry_name_candidates(&base, EntryNameKind::File), 3),
            ["report副本.pdf", "report副本 2.pdf", "report副本 3.pdf"]
        );
    }

    #[test]
    fn candidate_sequence_is_bounded() {
        let candidates = entry_name_candidates(OsStr::new("a.txt"), EntryNameKind::File);
        let all = candidates.collect::<Vec<_>>();
        assert_eq!(all.len(), ENTRY_NAME_LAST_CANDIDATE_NUMBER);
        assert_eq!(all.last(), Some(&OsString::from("a 999.txt")));
    }

    #[test]
    fn kinds_follow_whether_the_entry_is_a_directory() {
        assert_eq!(
            EntryNameKind::from(FileKind::Directory),
            EntryNameKind::Directory
        );
        assert_eq!(EntryNameKind::from(FileKind::Symlink), EntryNameKind::File);
        assert_eq!(
            EntryNameKind::from(FileObjectKind::Directory),
            EntryNameKind::Directory
        );
        assert_eq!(
            EntryNameKind::from(FileObjectKind::SymbolicLink),
            EntryNameKind::File
        );
    }
}
