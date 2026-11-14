//! 整包解压：zip、7z、rar 交给系统 7z，tar 一类交给系统 tar。

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use self::seven_zip::{extract_archive_with_seven_zip, inspect_archive_with_seven_zip};
use self::tar::extract_tar_archive;
use crate::{ArchivePassword, ArchiveProgress, FileError, FileOperationControls};

mod seven_zip;
mod tar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveExtractionFormat {
    Zip,
    SevenZip,
    Rar,
    /// 不压缩或 gzip / xz / zstd / bzip2 压缩的 tar：tar 读文件时自己认压缩方式。
    Tar,
}

/// 后缀 → 格式。多段后缀写在前面，先于单段的 `.tar` 匹配。
const ARCHIVE_SUFFIXES: &[(&str, ArchiveExtractionFormat)] = &[
    (".tar.gz", ArchiveExtractionFormat::Tar),
    (".tar.xz", ArchiveExtractionFormat::Tar),
    (".tar.zst", ArchiveExtractionFormat::Tar),
    (".tar.bz2", ArchiveExtractionFormat::Tar),
    (".tgz", ArchiveExtractionFormat::Tar),
    (".txz", ArchiveExtractionFormat::Tar),
    (".tzst", ArchiveExtractionFormat::Tar),
    (".tbz2", ArchiveExtractionFormat::Tar),
    (".tbz", ArchiveExtractionFormat::Tar),
    (".tar", ArchiveExtractionFormat::Tar),
    (".zip", ArchiveExtractionFormat::Zip),
    (".7z", ArchiveExtractionFormat::SevenZip),
    (".rar", ArchiveExtractionFormat::Rar),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveExtractionRequest {
    pub archive: PathBuf,
    pub destination: PathBuf,
    pub password: Option<ArchivePassword>,
}

impl ArchiveExtractionRequest {
    pub fn from_archive_path(
        archive: impl Into<PathBuf>,
        password: Option<ArchivePassword>,
    ) -> Result<Self, FileError> {
        let archive = archive.into();
        let destination = default_archive_extraction_directory(&archive)?;
        Ok(Self {
            archive,
            destination,
            password,
        })
    }

    /// 右键解压语义：目的地由调用方计算（智能解压/包名文件夹），
    /// 不再固定为归档同级目录。
    pub fn with_destination(self, destination: PathBuf) -> Self {
        Self {
            destination,
            ..self
        }
    }

    pub fn with_password(&self, password: Option<ArchivePassword>) -> Self {
        Self {
            archive: self.archive.clone(),
            destination: self.destination.clone(),
            password,
        }
    }

    pub fn without_password(&self) -> Self {
        self.with_password(None)
    }
}

/// 名字里的压缩包后缀（含点，保留原来的大小写）和格式，按 [`ARCHIVE_SUFFIXES`] 认。
pub(crate) fn archive_suffix(file_name: &str) -> Option<(&str, ArchiveExtractionFormat)> {
    let lower = file_name.to_ascii_lowercase();
    ARCHIVE_SUFFIXES
        .iter()
        .find(|(suffix, _)| lower.ends_with(suffix))
        .map(|(suffix, format)| (&file_name[file_name.len() - suffix.len()..], *format))
}

pub fn archive_extraction_format_for_path(
    path: impl AsRef<Path>,
) -> Option<ArchiveExtractionFormat> {
    let file_name = path.as_ref().file_name()?.to_string_lossy();
    archive_suffix(&file_name).map(|(_, format)| format)
}

pub fn is_supported_archive_path(path: impl AsRef<Path>) -> bool {
    archive_extraction_format_for_path(path).is_some()
}

pub async fn inspect_archive_extraction(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<(), FileError> {
    match extraction_format(&request)? {
        ArchiveExtractionFormat::Tar => reject_tar_password(&request),
        ArchiveExtractionFormat::Zip
        | ArchiveExtractionFormat::SevenZip
        | ArchiveExtractionFormat::Rar => inspect_archive_with_seven_zip(request, cancel).await,
    }
}

pub async fn extract_archive(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<PathBuf, FileError> {
    extract_archive_with_progress(request, cancel, |_| {}).await
}

pub async fn extract_archive_with_progress(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
    progress: impl FnMut(ArchiveProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    extract_archive_with_controls_and_progress(
        request,
        FileOperationControls::running(cancel),
        progress,
    )
    .await
}

pub async fn extract_archive_with_controls_and_progress(
    request: ArchiveExtractionRequest,
    mut controls: FileOperationControls,
    progress: impl FnMut(ArchiveProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    let format = extraction_format(&request)?;
    controls.wait_until_running().await?;
    match format {
        ArchiveExtractionFormat::Tar => {
            reject_tar_password(&request)?;
            extract_tar_archive(request, controls, progress).await
        }
        ArchiveExtractionFormat::Zip
        | ArchiveExtractionFormat::SevenZip
        | ArchiveExtractionFormat::Rar => {
            extract_archive_with_seven_zip(request, controls, progress).await
        }
    }
}

fn extraction_format(
    request: &ArchiveExtractionRequest,
) -> Result<ArchiveExtractionFormat, FileError> {
    archive_extraction_format_for_path(&request.archive).ok_or(FileError::Unsupported(
        "archive format is not supported for extraction",
    ))
}

fn default_archive_extraction_directory(archive: &Path) -> Result<PathBuf, FileError> {
    let parent = archive
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Err(FileError::InvalidInput {
            path: archive.to_path_buf(),
            message: "archive path has no parent directory".to_owned(),
        });
    };
    let Some(name) = archive_folder_name(archive) else {
        return Err(FileError::InvalidInput {
            path: archive.to_path_buf(),
            message: "archive name is not usable as a directory name".to_owned(),
        });
    };
    Ok(parent.join(name))
}

/// 去掉压缩包后缀作为解压出来的文件夹名：`a.tar.gz` → `a`。
pub fn archive_folder_name(archive: &Path) -> Option<OsString> {
    let file_name = archive.file_name()?;
    if let Some(text) = file_name.to_str() {
        if let Some((suffix, _)) = archive_suffix(text) {
            let stem = &text[..text.len() - suffix.len()];
            if !stem.is_empty() {
                return Some(OsString::from(stem));
            }
        }
    }
    archive.file_stem().map(OsStr::to_os_string)
}

/// 目的地允许已存在：右键智能解压的单根语义直接解入压缩包所在文件夹。
/// 出错或取消时不删已经解出来的东西：清理一旦有 bug 会多删用户文件。
fn ensure_destination_directory(destination: &Path) -> Result<(), FileError> {
    if destination.is_dir() {
        return Ok(());
    }
    fs::create_dir(destination).map_err(|source| FileError::CreateDirectory {
        path: destination.to_path_buf(),
        source,
    })
}

fn reject_tar_password(request: &ArchiveExtractionRequest) -> Result<(), FileError> {
    if request.password.is_some() {
        return Err(FileError::InvalidInput {
            path: request.archive.clone(),
            message: "tar archives do not support passwords".to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes_pick_the_format_and_the_folder_name() {
        for (name, format, folder) in [
            ("a.zip", ArchiveExtractionFormat::Zip, "a"),
            ("a.7z", ArchiveExtractionFormat::SevenZip, "a"),
            ("a.part1.rar", ArchiveExtractionFormat::Rar, "a.part1"),
            ("a.tar", ArchiveExtractionFormat::Tar, "a"),
            ("A.TAR.GZ", ArchiveExtractionFormat::Tar, "A"),
            ("a.tgz", ArchiveExtractionFormat::Tar, "a"),
            ("a.tar.xz", ArchiveExtractionFormat::Tar, "a"),
            ("a.txz", ArchiveExtractionFormat::Tar, "a"),
            ("a.tar.zst", ArchiveExtractionFormat::Tar, "a"),
            ("a.tzst", ArchiveExtractionFormat::Tar, "a"),
            ("a.tar.bz2", ArchiveExtractionFormat::Tar, "a"),
            ("a.tbz2", ArchiveExtractionFormat::Tar, "a"),
            ("a.tbz", ArchiveExtractionFormat::Tar, "a"),
        ] {
            let path = Path::new("/tmp").join(name);
            assert_eq!(
                archive_extraction_format_for_path(&path),
                Some(format),
                "{name}"
            );
            assert_eq!(
                default_archive_extraction_directory(&path).unwrap(),
                Path::new("/tmp").join(folder),
                "{name}"
            );
        }
        assert_eq!(archive_extraction_format_for_path("/tmp/a.gz"), None);
        assert_eq!(archive_extraction_format_for_path("/tmp/a.txt"), None);
    }
}
