//! 整包解压：zip、7z、rar 交给系统 7z，tar 一类交给系统 tar。

use std::ffi::{OsStr, OsString};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::archive_listing::StoredMember;

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

/// 整包解压落在哪。两种都只在压缩包所在的文件夹里放一个原来没有的东西，从不解进已经
/// 存在的文件夹：解进去的话，包里的同名文件会盖掉用户原有的文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveExtractionPlacement {
    /// 包里只有一个顶层文件夹、而且这个名字在压缩包旁边还空着，就直接放在压缩包旁边；
    /// 否则同 [`Self::ArchiveFolder`]。
    Smart,
    /// 在压缩包旁边新建一个以包名命名的文件夹解进去，名字被占用时按"名字 2"续编号。
    ArchiveFolder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveExtractionRequest {
    pub archive: PathBuf,
    /// 压缩包所在的文件夹，解出来的东西放在这里面。
    pub landing_directory: PathBuf,
    pub placement: ArchiveExtractionPlacement,
    pub password: Option<ArchivePassword>,
}

/// 解压放出来的那一项：直接放在压缩包旁边的顶层文件夹，或者新建的包名文件夹。
/// 撤销解压就是把它移到回收站。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedArchive {
    pub placed: PathBuf,
}

impl ArchiveExtractionRequest {
    /// 压缩包要有上级目录、名字能当文件夹名，在这里查一次。
    pub fn new(
        archive: impl Into<PathBuf>,
        placement: ArchiveExtractionPlacement,
    ) -> Result<Self, FileError> {
        let archive = archive.into();
        let landing_directory = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .ok_or_else(|| FileError::InvalidInput {
                path: archive.clone(),
                message: "archive path has no parent directory".to_owned(),
            })?;
        if archive_folder_name(&archive).is_none() {
            return Err(FileError::InvalidInput {
                path: archive,
                message: "archive name is not usable as a directory name".to_owned(),
            });
        }
        Ok(Self {
            archive,
            landing_directory,
            placement,
            password: None,
        })
    }

    pub fn with_password(&self, password: Option<ArchivePassword>) -> Self {
        Self {
            password,
            ..self.clone()
        }
    }

    pub fn without_password(&self) -> Self {
        self.with_password(None)
    }
}

/// 这一次解进哪里，在解压真正开始时才定：排队期间同一个包可能已经被解过一次，
/// 入队时看到的空位到这时不一定还空着。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtractionTarget {
    /// 直接解进压缩包所在的文件夹：包里唯一的顶层文件夹 `root` 在那里还不存在。解压命令
    /// 要用不覆盖的写法，万一这期间别处建了同名的东西，也不会被盖掉。
    BesideArchive { landing: PathBuf, root: OsString },
    /// 解进刚新建的空文件夹。
    NewFolder(PathBuf),
}

impl ExtractionTarget {
    pub(crate) fn directory(&self) -> &Path {
        match self {
            Self::BesideArchive { landing, .. } => landing,
            Self::NewFolder(folder) => folder,
        }
    }

    fn placed(self) -> PathBuf {
        match self {
            Self::BesideArchive { landing, root } => landing.join(root),
            Self::NewFolder(folder) => folder,
        }
    }
}

/// `members` 是这个包列出的成员，只在智能解压时才会被等待：整包解进包名文件夹用不着它，
/// tar 也就不用为此多读一遍包。
pub(crate) async fn extraction_target(
    request: &ArchiveExtractionRequest,
    members: impl Future<Output = Option<Vec<StoredMember>>>,
) -> Result<ExtractionTarget, FileError> {
    if request.placement == ArchiveExtractionPlacement::Smart {
        if let Some(root) = free_single_root(request, members.await) {
            return Ok(ExtractionTarget::BesideArchive {
                landing: request.landing_directory.clone(),
                root,
            });
        }
    }
    let name = archive_folder_name(&request.archive).ok_or_else(|| FileError::InvalidInput {
        path: request.archive.clone(),
        message: "archive name is not usable as a directory name".to_owned(),
    })?;
    let folder =
        crate::create_directory_at_available_name(request.landing_directory.join(name)).await?;
    Ok(ExtractionTarget::NewFolder(folder))
}

/// 包里唯一的顶层文件夹（显示名，和包内浏览看到的一样），并且这个名字在压缩包旁边还空着。
/// 列不出成员（坏包、头部加密又没给密码）时按多根处理，让解压本身报出具体错误。
fn free_single_root(
    request: &ArchiveExtractionRequest,
    members: Option<Vec<StoredMember>>,
) -> Option<OsString> {
    let root = crate::archive_vfs::single_root_folder(members?)?;
    let free = matches!(
        fs::symlink_metadata(request.landing_directory.join(&root)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    );
    free.then_some(root)
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
) -> Result<ExtractedArchive, FileError> {
    extract_archive_with_progress(request, cancel, |_| {}).await
}

pub async fn extract_archive_with_progress(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
    progress: impl FnMut(ArchiveProgress) + Send + 'static,
) -> Result<ExtractedArchive, FileError> {
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
) -> Result<ExtractedArchive, FileError> {
    let format = extraction_format(&request)?;
    controls.wait_until_running().await?;
    // 出错或取消时不删已经解出来的东西，也不删新建的文件夹：清理一旦有 bug 会多删用户文件。
    let target = match format {
        ArchiveExtractionFormat::Tar => extract_tar_archive(&request, controls, progress).await?,
        ArchiveExtractionFormat::Zip
        | ArchiveExtractionFormat::SevenZip
        | ArchiveExtractionFormat::Rar => {
            extract_archive_with_seven_zip(&request, controls, progress).await?
        }
    };
    Ok(ExtractedArchive {
        placed: target.placed(),
    })
}

fn extraction_format(
    request: &ArchiveExtractionRequest,
) -> Result<ArchiveExtractionFormat, FileError> {
    archive_extraction_format_for_path(&request.archive).ok_or(FileError::Unsupported(
        "archive format is not supported for extraction",
    ))
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
                archive_folder_name(&path).unwrap(),
                OsStr::new(folder),
                "{name}"
            );
        }
        assert_eq!(archive_extraction_format_for_path("/tmp/a.gz"), None);
        assert_eq!(archive_extraction_format_for_path("/tmp/a.txt"), None);
    }
}
