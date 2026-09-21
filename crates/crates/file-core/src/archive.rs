use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::{FileError, FileOperationControls, SEVEN_ZIP_COMMAND_NAMES};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveCreationProgress {
    pub completed_source_bytes: u64,
    pub total_source_bytes: u64,
    pub completed_entries: usize,
    pub total_entries: usize,
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
    progress: impl FnMut(ArchiveCreationProgress) + Send + 'static,
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
    progress: impl FnMut(ArchiveCreationProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    validate_archive_request(&request)?;
    if request.format == ArchiveFormat::TarGz && request.password.is_some() {
        return Err(FileError::InvalidInput {
            path: request.target.clone(),
            message: "tar.gz archives do not support passwords".to_owned(),
        });
    }

    if request.format == ArchiveFormat::SevenZip || request.password.is_some() {
        controls.wait_until_running().await?;
        return create_archive_with_seven_zip(request, controls.cancellation_token()).await;
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
    mut progress: impl FnMut(ArchiveCreationProgress),
) -> Result<PathBuf, FileError> {
    archive_control_checkpoint(&mut controls, &runtime)?;
    let cancel = controls.cancellation_token();
    let base_directory = common_source_parent(&request.sources)?;
    let entries = collect_archive_entries(&request.sources, &base_directory, &cancel)?;
    let total_source_bytes = archive_source_bytes(&entries, &request.target)?;
    archive_control_checkpoint(&mut controls, &runtime)?;
    progress(ArchiveCreationProgress {
        completed_source_bytes: 0,
        total_source_bytes,
        completed_entries: 0,
        total_entries: entries.len(),
    });
    let outcome = match request.format {
        ArchiveFormat::Zip => write_zip_archive(
            &request,
            &entries,
            total_source_bytes,
            &mut controls,
            &runtime,
            &mut progress,
        ),
        ArchiveFormat::TarGz => write_tar_gz_archive(
            &request,
            &entries,
            total_source_bytes,
            &mut controls,
            &runtime,
            &mut progress,
        ),
        ArchiveFormat::SevenZip => Err(FileError::Unsupported("7z requires system 7z backend")),
    };
    if outcome.is_err() {
        let _ = fs::remove_file(&request.target);
    }
    outcome.map(|_| request.target)
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

fn validate_archive_entry_source_size(entry: &ArchiveEntry) -> Result<(), FileError> {
    let metadata = fs::metadata(&entry.path).map_err(|source| FileError::Metadata {
        path: entry.path.clone(),
        source,
    })?;
    if metadata.len() == entry.source_bytes {
        Ok(())
    } else {
        Err(archive_source_size_changed(&entry.path))
    }
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
    total_source_bytes: u64,
    controls: &mut FileOperationControls,
    runtime: &Handle,
    progress: &mut impl FnMut(ArchiveCreationProgress),
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
    let total_entries = entries.len();
    let mut completed_source_bytes = 0_u64;
    let mut buffer = vec![0_u8; ARCHIVE_COPY_BUFFER_SIZE];

    for (index, (entry, archive_name)) in entries.iter().zip(archive_names).enumerate() {
        archive_control_checkpoint(controls, runtime)?;
        if entry.is_directory {
            archive
                .add_directory(archive_name, options)
                .map_err(|source| archive_write_error(&request.target, source))?;
        } else {
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
                progress(ArchiveCreationProgress {
                    completed_source_bytes,
                    total_source_bytes,
                    completed_entries: index,
                    total_entries,
                });
            }
            if completed_entry_bytes != entry.source_bytes {
                return Err(archive_source_size_changed(&entry.path));
            }
        }
        archive_control_checkpoint(controls, runtime)?;
        progress(ArchiveCreationProgress {
            completed_source_bytes,
            total_source_bytes,
            completed_entries: index + 1,
            total_entries,
        });
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
    total_source_bytes: u64,
    controls: &mut FileOperationControls,
    runtime: &Handle,
    progress: &mut impl FnMut(ArchiveCreationProgress),
) -> Result<(), FileError> {
    let file = create_archive_file(&request.target)?;
    let encoder = flate2::write::GzEncoder::new(file, request.compression_level.gzip_level());
    let mut archive = tar::Builder::new(encoder);
    let total_entries = entries.len();
    let mut completed_source_bytes = 0_u64;

    for (index, entry) in entries.iter().enumerate() {
        archive_control_checkpoint(controls, runtime)?;
        if entry.is_directory {
            archive
                .append_dir(&entry.relative_path, &entry.path)
                .map_err(|source| archive_io_error(&request.target, source))?;
        } else {
            validate_archive_entry_source_size(entry)?;
            archive
                .append_path_with_name(&entry.path, &entry.relative_path)
                .map_err(|source| archive_io_error(&request.target, source))?;
            validate_archive_entry_source_size(entry)?;
            completed_source_bytes += entry.source_bytes;
        }
        archive_control_checkpoint(controls, runtime)?;
        progress(ArchiveCreationProgress {
            completed_source_bytes,
            total_source_bytes,
            completed_entries: index + 1,
            total_entries,
        });
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

async fn create_archive_with_seven_zip(
    request: ArchiveCreationRequest,
    cancel: CancellationToken,
) -> Result<PathBuf, FileError> {
    let base_directory = common_source_parent(&request.sources)?;
    if request.format == ArchiveFormat::Zip {
        validate_external_zip_entry_encoding(&request, &base_directory, &cancel).await?;
    }
    let mut relative_sources = Vec::with_capacity(request.sources.len());
    for source in &request.sources {
        relative_sources.push(archive_relative_path(source, &base_directory)?);
    }

    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        match spawn_seven_zip_archive(command_name, &request, &base_directory, &relative_sources) {
            Ok(mut child) => {
                let status = tokio::select! {
                    status = child.wait() => status.map_err(|source| FileError::Archive {
                        path: request.target.clone(),
                        message: source.to_string(),
                    })?,
                    _ = cancel.cancelled() => {
                        let _ = child.kill().await;
                        let _ = fs::remove_file(&request.target);
                        return Err(FileError::Cancelled);
                    }
                };
                if status.success() {
                    return Ok(request.target);
                }
                let _ = fs::remove_file(&request.target);
                return Err(FileError::Archive {
                    path: request.target,
                    message: format!("7z exited with status {status}"),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(FileError::Archive {
                    path: request.target,
                    message: error.to_string(),
                });
            }
        }
    }

    Err(FileError::Unsupported("7z, 7zz or 7za command is required"))
}

async fn validate_external_zip_entry_encoding(
    request: &ArchiveCreationRequest,
    base_directory: &Path,
    cancel: &CancellationToken,
) -> Result<(), FileError> {
    let sources = request.sources.clone();
    let base_directory = base_directory.to_path_buf();
    let cancel = cancel.clone();
    let join_target = request.target.clone();
    tokio::task::spawn_blocking(move || {
        let entries = collect_archive_entries(&sources, &base_directory, &cancel)?;
        zip_archive_names(&entries).map(|_| ())
    })
    .await
    .map_err(|error| FileError::Archive {
        path: join_target,
        message: error.to_string(),
    })?
}

fn spawn_seven_zip_archive(
    command_name: &str,
    request: &ArchiveCreationRequest,
    base_directory: &Path,
    relative_sources: &[PathBuf],
) -> io::Result<tokio::process::Child> {
    let mut command = Command::new(command_name);
    command
        .current_dir(base_directory)
        .arg("a")
        .arg(match request.format {
            ArchiveFormat::Zip => "-tzip",
            ArchiveFormat::SevenZip => "-t7z",
            ArchiveFormat::TarGz => "-ttar",
        })
        .arg(request.compression_level.seven_zip_level())
        .arg("-y")
        .arg("-bd")
        .arg("-bso0")
        .arg("-bsp0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(password) = &request.password {
        command.arg(format!("-p{}", password.as_str()));
        if request.format == ArchiveFormat::SevenZip {
            command.arg("-mhe=on");
        }
        if request.format == ArchiveFormat::Zip {
            command.arg("-mem=AES256");
        }
    }
    command.arg("--").arg(&request.target);
    for source in relative_sources {
        command.arg(source);
    }
    command.spawn()
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
