//! 新建压缩包：zip、7z 交给系统 7z，tar.gz 交给系统 tar + gzip。

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use tokio_util::sync::CancellationToken;

use self::seven_zip::create_archive_with_seven_zip;
use self::tar_gz::create_tar_gz_archive;
use crate::{ArchiveProgress, FileError, FileOperationControls};

mod seven_zip;
mod tar_gz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZip,
    TarGz,
}

impl ArchiveFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZip => "7z",
            Self::TarGz => "tar.gz",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveCompressionLevel {
    Store,
    Fast,
    Balanced,
    Maximum,
}

impl ArchiveCompressionLevel {
    /// gzip 没有不压缩的档位，「存储」按最快的压。
    fn gzip_level(self) -> &'static str {
        match self {
            Self::Store | Self::Fast => "-1",
            Self::Balanced => "-6",
            Self::Maximum => "-9",
        }
    }

    fn seven_zip_level(self) -> &'static str {
        match self {
            Self::Store => "-mx=0",
            Self::Fast => "-mx=1",
            Self::Balanced => "-mx=5",
            Self::Maximum => "-mx=9",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ArchivePassword(String);

impl ArchivePassword {
    pub fn new(password: impl Into<String>) -> Option<Self> {
        let password = password.into();
        (!password.is_empty()).then_some(Self(password))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ArchivePassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArchivePassword(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveCreationRequest {
    pub sources: Vec<PathBuf>,
    pub target: PathBuf,
    pub format: ArchiveFormat,
    pub compression_level: ArchiveCompressionLevel,
    pub password: Option<ArchivePassword>,
}

#[derive(Debug, Clone)]
struct ArchiveEntry {
    relative_path: PathBuf,
    is_directory: bool,
    source_bytes: u64,
}

/// 压缩前先把来源扫一遍：外部命令只报百分比或字节流，总字节和条目数要自己算才能写进文字。
struct ArchiveSources {
    base_directory: PathBuf,
    /// 相对 `base_directory`，按请求里的顺序，交给外部命令。
    relative_sources: Vec<PathBuf>,
    /// 来源和它们底下的全部条目，目录内按名字排序。
    entries: Vec<ArchiveEntry>,
    total_bytes: u64,
}

pub async fn create_archive_with_progress(
    request: ArchiveCreationRequest,
    cancel: CancellationToken,
    progress: impl FnMut(ArchiveProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    create_archive_with_controls_and_progress(
        request,
        FileOperationControls::running(cancel),
        progress,
    )
    .await
}

pub async fn create_archive_with_controls_and_progress(
    request: ArchiveCreationRequest,
    mut controls: FileOperationControls,
    progress: impl FnMut(ArchiveProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    validate_archive_request(&request)?;
    controls.wait_until_running().await?;
    let sources = scan_sources(&request, &controls).await?;
    if request.format == ArchiveFormat::Zip {
        ensure_utf8_zip_names(&sources)?;
    }
    match request.format {
        ArchiveFormat::Zip | ArchiveFormat::SevenZip => {
            create_archive_with_seven_zip(request, sources, controls, progress).await
        }
        ArchiveFormat::TarGz => create_tar_gz_archive(request, sources, controls, progress).await,
    }
}

fn validate_archive_request(request: &ArchiveCreationRequest) -> Result<(), FileError> {
    if request.sources.is_empty() {
        return Err(FileError::InvalidInput {
            path: request.target.clone(),
            message: "archive must contain at least one source".to_owned(),
        });
    }
    if request.format == ArchiveFormat::TarGz && request.password.is_some() {
        return Err(FileError::InvalidInput {
            path: request.target.clone(),
            message: "tar.gz archives do not support passwords".to_owned(),
        });
    }
    if request.target.exists() {
        return Err(FileError::CreateFile {
            path: request.target.clone(),
            source: io::Error::new(io::ErrorKind::AlreadyExists, "target already exists"),
        });
    }
    Ok(())
}

async fn scan_sources(
    request: &ArchiveCreationRequest,
    controls: &FileOperationControls,
) -> Result<ArchiveSources, FileError> {
    let sources = request.sources.clone();
    let target = request.target.clone();
    let cancel = controls.cancellation_token();
    tokio::task::spawn_blocking(move || {
        let base_directory = common_source_parent(&sources)?;
        let mut relative_sources = Vec::with_capacity(sources.len());
        let mut entries = Vec::new();
        for source in &sources {
            let relative_path = archive_relative_path(source, &base_directory)?;
            relative_sources.push(relative_path.clone());
            collect_archive_entry(source, relative_path, &cancel, &mut entries)?;
        }
        let total_bytes = archive_source_bytes(&entries, &target)?;
        Ok(ArchiveSources {
            base_directory,
            relative_sources,
            entries,
            total_bytes,
        })
    })
    .await
    .map_err(|error| FileError::Archive {
        path: request.target.clone(),
        message: error.to_string(),
    })?
}

/// 跟着符号链接走（压进去的是链接指向的内容），和外部命令的设置一致。
fn collect_archive_entry(
    path: &Path,
    relative_path: PathBuf,
    cancel: &CancellationToken,
    entries: &mut Vec<ArchiveEntry>,
) -> Result<(), FileError> {
    if cancel.is_cancelled() {
        return Err(FileError::Cancelled);
    }
    let metadata = fs::metadata(path).map_err(|source| FileError::Metadata {
        path: path.to_path_buf(),
        source,
    })?;
    let is_directory = metadata.is_dir();
    entries.push(ArchiveEntry {
        relative_path: relative_path.clone(),
        is_directory,
        source_bytes: if is_directory { 0 } else { metadata.len() },
    });
    if !is_directory {
        return Ok(());
    }

    let mut children = fs::read_dir(path)
        .map_err(|source| FileError::ReadDirectory {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| FileError::ReadDirectory {
            path: path.to_path_buf(),
            source,
        })?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let child_relative_path = relative_path.join(child.file_name());
        collect_archive_entry(&child.path(), child_relative_path, cancel, entries)?;
    }
    Ok(())
}

fn archive_source_bytes(entries: &[ArchiveEntry], target: &Path) -> Result<u64, FileError> {
    entries.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(entry.source_bytes)
            .ok_or_else(|| FileError::InvalidInput {
                path: target.to_path_buf(),
                message: "archive source byte total exceeds the supported range".to_owned(),
            })
    })
}

fn create_archive_file(path: &Path) -> Result<File, FileError> {
    File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| FileError::CreateFile {
            path: path.to_path_buf(),
            source,
        })
}

fn common_source_parent(sources: &[PathBuf]) -> Result<PathBuf, FileError> {
    let Some(first_parent) = sources.first().and_then(|source| source.parent()) else {
        return Err(FileError::InvalidInput {
            path: PathBuf::new(),
            message: "archive source has no parent directory".to_owned(),
        });
    };
    let mut common = first_parent.to_path_buf();
    for source in sources.iter().skip(1) {
        let Some(parent) = source.parent() else {
            return Err(FileError::InvalidInput {
                path: source.clone(),
                message: "archive source has no parent directory".to_owned(),
            });
        };
        while !parent.starts_with(&common) {
            if !common.pop() {
                return Err(FileError::InvalidInput {
                    path: source.clone(),
                    message: "archive sources do not share a parent directory".to_owned(),
                });
            }
        }
    }
    Ok(common)
}

fn archive_relative_path(path: &Path, base_directory: &Path) -> Result<PathBuf, FileError> {
    let relative = path
        .strip_prefix(base_directory)
        .map_err(|_| FileError::InvalidInput {
            path: path.to_path_buf(),
            message: "archive source is outside the shared parent directory".to_owned(),
        })?;
    if relative.as_os_str().is_empty() || path_has_parent_reference(relative) {
        return Err(FileError::InvalidInput {
            path: path.to_path_buf(),
            message: "archive source name is not safe".to_owned(),
        });
    }
    Ok(relative.to_path_buf())
}

fn path_has_parent_reference(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// zip 的名字按 UTF-8 写（7z 加 `-mcu=on`）：动手前确认每个名字都是 UTF-8，不是就一个
/// 文件都不建、7z 也不启动——否则 7z 会把原始字节标成 UTF-8 写进去。
fn ensure_utf8_zip_names(sources: &ArchiveSources) -> Result<(), FileError> {
    match sources
        .entries
        .iter()
        .find(|entry| entry.relative_path.to_str().is_none())
    {
        Some(entry) => Err(FileError::InvalidInput {
            path: sources.base_directory.join(&entry.relative_path),
            message: "ZIP archive entry names must be valid UTF-8".to_owned(),
        }),
        None => Ok(()),
    }
}
