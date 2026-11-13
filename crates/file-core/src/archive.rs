use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use self::seven_zip::create_archive_with_seven_zip;
use crate::{ArchiveEntryProgress, ArchiveProgress, FileError, FileOperationControls};

mod seven_zip;

const ARCHIVE_COPY_BUFFER_SIZE: usize = 1024 * 1024;

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
    fn zip_level(self) -> Option<i64> {
        match self {
            Self::Store => None,
            Self::Fast => Some(1),
            Self::Balanced => Some(6),
            Self::Maximum => Some(9),
        }
    }

    fn gzip_level(self) -> flate2::Compression {
        match self {
            Self::Store => flate2::Compression::none(),
            Self::Fast => flate2::Compression::fast(),
            Self::Balanced => flate2::Compression::new(6),
            Self::Maximum => flate2::Compression::best(),
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
    path: PathBuf,
    relative_path: PathBuf,
    is_directory: bool,
    source_bytes: u64,
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
    if request.format == ArchiveFormat::TarGz && request.password.is_some() {
        return Err(FileError::InvalidInput {
            path: request.target.clone(),
            message: "tar.gz archives do not support passwords".to_owned(),
        });
    }

    if request.format == ArchiveFormat::SevenZip || request.password.is_some() {
        return create_archive_with_seven_zip(request, controls, progress).await;
    }

    controls.wait_until_running().await?;
    let target = request.target.clone();
    let join_target = target.clone();
    let runtime = Handle::current();
    tokio::task::spawn_blocking(move || create_rust_archive(request, controls, runtime, progress))
        .await
        .map_err(|error| FileError::Archive {
            path: join_target,
            message: error.to_string(),
        })?
}

fn validate_archive_request(request: &ArchiveCreationRequest) -> Result<(), FileError> {
    if request.sources.is_empty() {
        return Err(FileError::InvalidInput {
            path: request.target.clone(),
            message: "archive must contain at least one source".to_owned(),
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

fn create_rust_archive(
    request: ArchiveCreationRequest,
    mut controls: FileOperationControls,
    runtime: Handle,
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    archive_control_checkpoint(&mut controls, &runtime)?;
    let cancel = controls.cancellation_token();
    let base_directory = common_source_parent(&request.sources)?;
    let entries = collect_archive_entries(&request.sources, &base_directory, &cancel)?;
    let workload = ArchiveWorkload::of(&entries, &request.target)?;
    archive_control_checkpoint(&mut controls, &runtime)?;
    progress(workload.progress(0, 0, None));
    let outcome = match request.format {
        ArchiveFormat::Zip => write_zip_archive(
            &request,
            &entries,
            &workload,
            &mut controls,
            &runtime,
            &mut progress,
        ),
        ArchiveFormat::TarGz => write_tar_gz_archive(
            &request,
            &entries,
            &workload,
            &mut controls,
            &runtime,
            &mut progress,
        ),
        ArchiveFormat::SevenZip => Err(FileError::Unsupported("7z requires system 7z backend")),
    };
    // 半成品包是本次 create_new 建出的那一个文件：坏了却顶着正常包的名字，
    // 留着会被当成好包用（用户确认要删）。
    if outcome.is_err() {
        let _ = fs::remove_file(&request.target);
    }
    outcome.map(|_| request.target)
}

/// 总进度按文件个数（目录不计），总字节只用于文字。
struct ArchiveWorkload {
    total_files: usize,
    total_bytes: u64,
}

impl ArchiveWorkload {
    fn of(entries: &[ArchiveEntry], target: &Path) -> Result<Self, FileError> {
        Ok(Self {
            total_files: entries.iter().filter(|entry| !entry.is_directory).count(),
            total_bytes: archive_source_bytes(entries, target)?,
        })
    }

    fn progress(
        &self,
        completed_files: usize,
        completed_bytes: u64,
        current_entry: Option<ArchiveEntryProgress>,
    ) -> ArchiveProgress {
        ArchiveProgress::Entries {
            completed_entries: completed_files,
            total_entries: self.total_files,
            bytes: Some((completed_bytes, self.total_bytes)),
            current_entry,
        }
    }
}

fn collect_archive_entries(
    sources: &[PathBuf],
    base_directory: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<ArchiveEntry>, FileError> {
    let mut entries = Vec::new();
    for source in sources {
        let relative_path = archive_relative_path(source, base_directory)?;
        collect_archive_entry(source, relative_path, cancel, &mut entries)?;
    }
    Ok(entries)
}

fn collect_archive_entry(
    path: &Path,
    relative_path: PathBuf,
    cancel: &CancellationToken,
    entries: &mut Vec<ArchiveEntry>,
) -> Result<(), FileError> {
    cancel_if_requested(cancel)?;
    let metadata = fs::metadata(path).map_err(|source| FileError::Metadata {
        path: path.to_path_buf(),
        source,
    })?;
    let is_directory = metadata.is_dir();
    entries.push(ArchiveEntry {
        path: path.to_path_buf(),
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

fn archive_source_size_changed(path: &Path) -> FileError {
    FileError::InvalidInput {
        path: path.to_path_buf(),
        message: "archive source size changed after workload collection".to_owned(),
    }
}

fn write_zip_archive(
    request: &ArchiveCreationRequest,
    entries: &[ArchiveEntry],
    workload: &ArchiveWorkload,
    controls: &mut FileOperationControls,
    runtime: &Handle,
    progress: &mut impl FnMut(ArchiveProgress),
) -> Result<(), FileError> {
    let archive_names = zip_archive_names(entries)?;
    let file = create_archive_file(&request.target)?;
    let mut archive = zip::ZipWriter::new(file);
    let method = if request.compression_level == ArchiveCompressionLevel::Store {
        zip::CompressionMethod::Stored
    } else {
        zip::CompressionMethod::Deflated
    };
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(method)
        .compression_level(request.compression_level.zip_level())
        .large_file(true);
    let mut completed_files = 0;
    let mut completed_source_bytes = 0_u64;
    let mut buffer = vec![0_u8; ARCHIVE_COPY_BUFFER_SIZE];

    for (entry, archive_name) in entries.iter().zip(archive_names) {
        archive_control_checkpoint(controls, runtime)?;
        if entry.is_directory {
            archive
                .add_directory(archive_name, options)
                .map_err(|source| archive_write_error(&request.target, source))?;
            continue;
        }
        archive
            .start_file(archive_name, options)
            .map_err(|source| archive_write_error(&request.target, source))?;
        let mut input = File::open(&entry.path).map_err(|source| FileError::Metadata {
            path: entry.path.clone(),
            source,
        })?;
        let mut completed_entry_bytes = 0_u64;
        loop {
            archive_control_checkpoint(controls, runtime)?;
            let read = input
                .read(&mut buffer)
                .map_err(|source| archive_io_error(&request.target, source))?;
            if read == 0 {
                break;
            }
            let next_entry_bytes = completed_entry_bytes
                .checked_add(read as u64)
                .ok_or_else(|| archive_source_size_changed(&entry.path))?;
            if next_entry_bytes > entry.source_bytes {
                return Err(archive_source_size_changed(&entry.path));
            }
            archive
                .write_all(&buffer[..read])
                .map_err(|source| archive_io_error(&request.target, source))?;
            completed_entry_bytes = next_entry_bytes;
            completed_source_bytes += read as u64;
            archive_control_checkpoint(controls, runtime)?;
            progress(workload.progress(
                completed_files,
                completed_source_bytes,
                Some(entry.progress(completed_entry_bytes)),
            ));
        }
        if completed_entry_bytes != entry.source_bytes {
            return Err(archive_source_size_changed(&entry.path));
        }
        completed_files += 1;
        archive_control_checkpoint(controls, runtime)?;
        progress(workload.progress(completed_files, completed_source_bytes, None));
    }
    archive
        .finish()
        .map_err(|source| archive_write_error(&request.target, source))?;
    archive_control_checkpoint(controls, runtime)?;
    Ok(())
}

fn write_tar_gz_archive(
    request: &ArchiveCreationRequest,
    entries: &[ArchiveEntry],
    workload: &ArchiveWorkload,
    controls: &mut FileOperationControls,
    runtime: &Handle,
    progress: &mut impl FnMut(ArchiveProgress),
) -> Result<(), FileError> {
    let file = create_archive_file(&request.target)?;
    let encoder = flate2::write::GzEncoder::new(file, request.compression_level.gzip_level());
    let mut archive = tar::Builder::new(encoder);
    let mut completed_files = 0;
    let mut completed_source_bytes = 0_u64;

    for entry in entries {
        archive_control_checkpoint(controls, runtime)?;
        if entry.is_directory {
            archive
                .append_dir(&entry.relative_path, &entry.path)
                .map_err(|source| archive_io_error(&request.target, source))?;
            continue;
        }
        let input = File::open(&entry.path).map_err(|source| FileError::Metadata {
            path: entry.path.clone(),
            source,
        })?;
        let metadata = input.metadata().map_err(|source| FileError::Metadata {
            path: entry.path.clone(),
            source,
        })?;
        if metadata.len() != entry.source_bytes {
            return Err(archive_source_size_changed(&entry.path));
        }
        // 和 append_path_with_name 同样从元数据建头；数据经计数 reader 读入，
        // 大文件压到一半也有当前文件进度。最多读声明的长度：文件在读的过程中
        // 变长时，多出来的字节会写坏 tar 结构。
        let mut header = tar::Header::new_gnu();
        header.set_metadata(&metadata);
        let mut reader = EntryReadProgress {
            inner: input.take(entry.source_bytes),
            controls: controls.clone(),
            runtime: runtime.clone(),
            entry,
            read_bytes: 0,
            last_sent_at: None,
            report: &mut |entry_bytes: u64| {
                progress(workload.progress(
                    completed_files,
                    completed_source_bytes + entry_bytes,
                    Some(entry.progress(entry_bytes)),
                ))
            },
        };
        // 读数据时被取消或关机打断：tar 会把它包成自己的写错误，按运行状态还原。
        archive
            .append_data(&mut header, &entry.relative_path, &mut reader)
            .map_err(|source| {
                controls
                    .interruption()
                    .unwrap_or_else(|| archive_io_error(&request.target, source))
            })?;
        if reader.read_bytes != entry.source_bytes {
            return Err(archive_source_size_changed(&entry.path));
        }
        completed_files += 1;
        completed_source_bytes += entry.source_bytes;
        archive_control_checkpoint(controls, runtime)?;
        progress(workload.progress(completed_files, completed_source_bytes, None));
    }
    let encoder = archive
        .into_inner()
        .map_err(|source| archive_io_error(&request.target, source))?;
    encoder
        .finish()
        .map_err(|source| archive_io_error(&request.target, source))?;
    archive_control_checkpoint(controls, runtime)?;
    Ok(())
}

/// 单个源文件的读取计数：按 `PROGRESS_REPORT_INTERVAL` 报当前文件进度；
/// 暂停也在读的地方等，大文件压到一半也能停住。
struct EntryReadProgress<'a, R> {
    inner: R,
    controls: FileOperationControls,
    runtime: Handle,
    entry: &'a ArchiveEntry,
    read_bytes: u64,
    last_sent_at: Option<Instant>,
    report: &'a mut dyn FnMut(u64),
}

impl<R: Read> Read for EntryReadProgress<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.runtime
            .block_on(self.controls.wait_until_running())
            .map_err(io::Error::other)?;
        let read = self.inner.read(buffer)?;
        self.read_bytes += read as u64;
        let now = Instant::now();
        if self
            .last_sent_at
            .is_none_or(|sent| now.duration_since(sent) >= crate::PROGRESS_REPORT_INTERVAL)
        {
            self.last_sent_at = Some(now);
            (self.report)(self.read_bytes.min(self.entry.source_bytes));
        }
        Ok(read)
    }
}

impl ArchiveEntry {
    fn progress(&self, completed_bytes: u64) -> ArchiveEntryProgress {
        ArchiveEntryProgress {
            name: self
                .path
                .file_name()
                .unwrap_or(self.path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            completed_bytes,
            total_bytes: self.source_bytes,
        }
    }
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

fn zip_archive_names(entries: &[ArchiveEntry]) -> Result<Vec<String>, FileError> {
    entries.iter().map(zip_archive_name).collect()
}

fn zip_archive_name(entry: &ArchiveEntry) -> Result<String, FileError> {
    let mut parts = Vec::new();
    for component in entry.relative_path.components() {
        let Component::Normal(part) = component else {
            return Err(FileError::InvalidInput {
                path: entry.path.clone(),
                message: "archive entry path is not safe".to_owned(),
            });
        };
        let component = part.to_str().ok_or_else(|| FileError::InvalidInput {
            path: entry.path.clone(),
            message: "ZIP archive entry names must be valid UTF-8".to_owned(),
        })?;
        parts.push(component);
    }
    if parts.is_empty() {
        return Err(FileError::InvalidInput {
            path: entry.path.clone(),
            message: "archive entry path is empty".to_owned(),
        });
    }
    Ok(parts.join("/"))
}

fn archive_write_error(path: &Path, source: zip::result::ZipError) -> FileError {
    FileError::Archive {
        path: path.to_path_buf(),
        message: source.to_string(),
    }
}

fn archive_io_error(path: &Path, source: io::Error) -> FileError {
    FileError::Archive {
        path: path.to_path_buf(),
        message: source.to_string(),
    }
}

fn archive_control_checkpoint(
    controls: &mut FileOperationControls,
    runtime: &Handle,
) -> Result<(), FileError> {
    runtime.block_on(controls.wait_until_running())
}

fn cancel_if_requested(cancel: &CancellationToken) -> Result<(), FileError> {
    if cancel.is_cancelled() {
        Err(FileError::Cancelled)
    } else {
        Ok(())
    }
}
