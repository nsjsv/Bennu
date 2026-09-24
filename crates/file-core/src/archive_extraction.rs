use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use zip::result::ZipError;

use crate::archive_listing::split_volume_hint;
use crate::{ArchivePassword, FileError, FileOperationControls, SEVEN_ZIP_COMMAND_NAMES};

const ARCHIVE_EXTRACTION_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveExtractionFormat {
    Zip,
    Tar,
    TarGz,
    SevenZip,
    Rar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveExtractionProgress {
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub completed_entries: usize,
    pub total_entries: usize,
}

#[derive(Debug)]
struct ZipExtractionWorkload {
    entry_bytes: Vec<u64>,
    total_bytes: u64,
}

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

pub fn archive_extraction_format_for_path(
    path: impl AsRef<Path>,
) -> Option<ArchiveExtractionFormat> {
    let path = path.as_ref();
    let file_name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
        return Some(ArchiveExtractionFormat::TarGz);
    }

    match path.extension().and_then(OsStr::to_str) {
        Some(extension) if extension.eq_ignore_ascii_case("zip") => {
            Some(ArchiveExtractionFormat::Zip)
        }
        Some(extension) if extension.eq_ignore_ascii_case("tar") => {
            Some(ArchiveExtractionFormat::Tar)
        }
        Some(extension) if extension.eq_ignore_ascii_case("7z") => {
            Some(ArchiveExtractionFormat::SevenZip)
        }
        Some(extension) if extension.eq_ignore_ascii_case("rar") => {
            Some(ArchiveExtractionFormat::Rar)
        }
        _ => None,
    }
}

pub fn is_supported_archive_path(path: impl AsRef<Path>) -> bool {
    archive_extraction_format_for_path(path).is_some()
}

pub async fn inspect_archive_extraction(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<(), FileError> {
    let format = archive_extraction_format_for_path(&request.archive).ok_or(
        FileError::Unsupported("archive format is not supported for extraction"),
    )?;

    match format {
        ArchiveExtractionFormat::Zip => inspect_zip_archive(request, cancel).await,
        ArchiveExtractionFormat::Tar | ArchiveExtractionFormat::TarGz => {
            reject_tar_password(&request)?;
            Ok(())
        }
        ArchiveExtractionFormat::SevenZip | ArchiveExtractionFormat::Rar => {
            inspect_archive_with_seven_zip(request, cancel).await
        }
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
    progress: impl FnMut(ArchiveExtractionProgress) + Send + 'static,
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
    progress: impl FnMut(ArchiveExtractionProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    let format = archive_extraction_format_for_path(&request.archive).ok_or(
        FileError::Unsupported("archive format is not supported for extraction"),
    )?;
    controls.wait_until_running().await?;

    match format {
        ArchiveExtractionFormat::Zip => extract_zip_archive(request, controls, progress).await,
        ArchiveExtractionFormat::Tar => {
            reject_tar_password(&request)?;
            extract_tar_archive(request, TarCompression::Plain, controls).await
        }
        ArchiveExtractionFormat::TarGz => {
            reject_tar_password(&request)?;
            extract_tar_archive(request, TarCompression::Gzip, controls).await
        }
        ArchiveExtractionFormat::SevenZip | ArchiveExtractionFormat::Rar => {
            extract_archive_with_seven_zip(request, controls.cancellation_token()).await
        }
    }
}

async fn inspect_zip_archive(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<(), FileError> {
    let archive = request.archive.clone();
    tokio::task::spawn_blocking(move || inspect_zip_entries(&request, &cancel).map(|_| ()))
        .await
        .map_err(|error| FileError::Archive {
            path: archive,
            message: error.to_string(),
        })?
}

async fn extract_zip_archive(
    request: ArchiveExtractionRequest,
    controls: FileOperationControls,
    progress: impl FnMut(ArchiveExtractionProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    let destination = request.destination.clone();
    let join_destination = destination.clone();
    let runtime = Handle::current();
    tokio::task::spawn_blocking(move || {
        extract_zip_archive_blocking(request, controls, runtime, progress)
    })
    .await
    .map_err(|error| FileError::Archive {
        path: join_destination,
        message: error.to_string(),
    })?
}

fn extract_zip_archive_blocking(
    request: ArchiveExtractionRequest,
    mut controls: FileOperationControls,
    runtime: Handle,
    mut progress: impl FnMut(ArchiveExtractionProgress),
) -> Result<PathBuf, FileError> {
    let cancel = controls.cancellation_token();
    let workload = inspect_zip_entries(&request, &cancel)?;
    archive_control_checkpoint(&mut controls, &runtime)?;
    progress(ArchiveExtractionProgress {
        completed_bytes: 0,
        total_bytes: workload.total_bytes,
        completed_entries: 0,
        total_entries: workload.entry_bytes.len(),
    });
    let destination_created = ensure_destination_directory(&request.destination)?;
    let outcome = extract_zip_entries(&request, &mut controls, &runtime, &workload, &mut progress);
    if outcome.is_err() && destination_created {
        let _ = fs::remove_dir_all(&request.destination);
    }
    outcome.map(|_| request.destination)
}

fn inspect_zip_entries(
    request: &ArchiveExtractionRequest,
    cancel: &CancellationToken,
) -> Result<ZipExtractionWorkload, FileError> {
    let file = open_archive_file(&request.archive)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| zip_error(&request.archive, error))?;
    let mut entry_bytes = Vec::with_capacity(archive.len());
    let mut total_bytes = 0_u64;

    for index in 0..archive.len() {
        cancel_if_requested(cancel)?;
        let entry = open_zip_entry(
            &mut archive,
            index,
            request.password.as_ref(),
            &request.archive,
        )?;
        validate_zip_entry_path(&request.archive, &entry)?;
        if entry.is_symlink() {
            return Err(FileError::Unsupported(
                "zip symlink extraction is not supported",
            ));
        }
        let size = if entry.is_dir() { 0 } else { entry.size() };
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or_else(|| FileError::InvalidInput {
                path: request.archive.clone(),
                message: "ZIP uncompressed byte total exceeds the supported range".to_owned(),
            })?;
        entry_bytes.push(size);
    }
    Ok(ZipExtractionWorkload {
        entry_bytes,
        total_bytes,
    })
}

fn extract_zip_entries(
    request: &ArchiveExtractionRequest,
    controls: &mut FileOperationControls,
    runtime: &Handle,
    workload: &ZipExtractionWorkload,
    progress: &mut impl FnMut(ArchiveExtractionProgress),
) -> Result<(), FileError> {
    let file = open_archive_file(&request.archive)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| zip_error(&request.archive, error))?;
    if archive.len() != workload.entry_bytes.len() {
        return Err(archive_changed_after_inspection(&request.archive));
    }
    let total_entries = workload.entry_bytes.len();
    let mut completed_bytes = 0_u64;
    let mut buffer = vec![0_u8; ARCHIVE_EXTRACTION_BUFFER_SIZE];

    for (index, expected_entry_bytes) in workload.entry_bytes.iter().copied().enumerate() {
        archive_control_checkpoint(controls, runtime)?;
        let mut entry = open_zip_entry(
            &mut archive,
            index,
            request.password.as_ref(),
            &request.archive,
        )?;
        let relative_path = validate_zip_entry_path(&request.archive, &entry)?;
        if entry.size() != expected_entry_bytes {
            return Err(archive_changed_after_inspection(&request.archive));
        }
        let target = request.destination.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|source| FileError::CreateDirectory {
                path: target,
                source,
            })?;
            archive_control_checkpoint(controls, runtime)?;
            progress(ArchiveExtractionProgress {
                completed_bytes,
                total_bytes: workload.total_bytes,
                completed_entries: index + 1,
                total_entries,
            });
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| FileError::CreateDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut output = File::options()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|source| FileError::CreateFile {
                path: target.clone(),
                source,
            })?;
        let mut completed_entry_bytes = 0_u64;
        loop {
            archive_control_checkpoint(controls, runtime)?;
            let read = entry
                .read(&mut buffer)
                .map_err(|source| FileError::Archive {
                    path: request.archive.clone(),
                    message: source.to_string(),
                })?;
            if read == 0 {
                break;
            }
            completed_entry_bytes = completed_entry_bytes
                .checked_add(read as u64)
                .ok_or_else(|| archive_changed_after_inspection(&request.archive))?;
            if completed_entry_bytes > expected_entry_bytes {
                return Err(archive_changed_after_inspection(&request.archive));
            }
            output
                .write_all(&buffer[..read])
                .map_err(|source| FileError::Archive {
                    path: target.clone(),
                    message: source.to_string(),
                })?;
            completed_bytes += read as u64;
            archive_control_checkpoint(controls, runtime)?;
            progress(ArchiveExtractionProgress {
                completed_bytes,
                total_bytes: workload.total_bytes,
                completed_entries: index,
                total_entries,
            });
        }
        if completed_entry_bytes != expected_entry_bytes {
            return Err(archive_changed_after_inspection(&request.archive));
        }
        output.flush().map_err(|source| FileError::Archive {
            path: target,
            message: source.to_string(),
        })?;
        archive_control_checkpoint(controls, runtime)?;
        progress(ArchiveExtractionProgress {
            completed_bytes,
            total_bytes: workload.total_bytes,
            completed_entries: index + 1,
            total_entries,
        });
    }
    Ok(())
}

fn open_zip_entry<'a, R: io::Read + io::Seek>(
    archive: &'a mut zip::ZipArchive<R>,
    index: usize,
    password: Option<&ArchivePassword>,
    archive_path: &Path,
) -> Result<zip::read::ZipFile<'a, R>, FileError> {
    match password {
        Some(password) => archive
            .by_index_decrypt(index, password.as_str().as_bytes())
            .map_err(|error| zip_error(archive_path, error)),
        None => {
            let entry = archive
                .by_index(index)
                .map_err(|error| zip_error(archive_path, error))?;
            if entry.encrypted() {
                return Err(FileError::ArchivePasswordRequired {
                    path: archive_path.to_path_buf(),
                });
            }
            Ok(entry)
        }
    }
}

fn validate_zip_entry_path<R: io::Read>(
    archive_path: &Path,
    entry: &zip::read::ZipFile<'_, R>,
) -> Result<PathBuf, FileError> {
    entry
        .enclosed_name()
        .ok_or_else(|| FileError::InvalidInput {
            path: archive_path.to_path_buf(),
            message: format!("archive entry path is not safe: {}", entry.name()),
        })
}

async fn extract_tar_archive(
    request: ArchiveExtractionRequest,
    compression: TarCompression,
    controls: FileOperationControls,
) -> Result<PathBuf, FileError> {
    let destination = request.destination.clone();
    let join_destination = destination.clone();
    let runtime = Handle::current();
    tokio::task::spawn_blocking(move || {
        extract_tar_archive_blocking(request, compression, controls, runtime)
    })
    .await
    .map_err(|error| FileError::Archive {
        path: join_destination,
        message: error.to_string(),
    })?
}

fn extract_tar_archive_blocking(
    request: ArchiveExtractionRequest,
    compression: TarCompression,
    mut controls: FileOperationControls,
    runtime: Handle,
) -> Result<PathBuf, FileError> {
    archive_control_checkpoint(&mut controls, &runtime)?;
    let destination_created = ensure_destination_directory(&request.destination)?;
    let outcome = match compression {
        TarCompression::Plain => {
            let file = open_archive_file(&request.archive)?;
            extract_tar_entries(file, &request, &mut controls, &runtime)
        }
        TarCompression::Gzip => {
            let file = open_archive_file(&request.archive)?;
            extract_tar_entries(
                flate2::read::GzDecoder::new(file),
                &request,
                &mut controls,
                &runtime,
            )
        }
    };
    if outcome.is_err() && destination_created {
        let _ = fs::remove_dir_all(&request.destination);
    }
    outcome.map(|_| request.destination)
}

fn extract_tar_entries<R: io::Read>(
    reader: R,
    request: &ArchiveExtractionRequest,
    controls: &mut FileOperationControls,
    runtime: &Handle,
) -> Result<(), FileError> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|source| FileError::Archive {
        path: request.archive.clone(),
        message: source.to_string(),
    })?;

    for entry in entries {
        archive_control_checkpoint(controls, runtime)?;
        let mut entry = entry.map_err(|source| FileError::Archive {
            path: request.archive.clone(),
            message: source.to_string(),
        })?;
        let unpacked =
            entry
                .unpack_in(&request.destination)
                .map_err(|source| FileError::Archive {
                    path: request.archive.clone(),
                    message: source.to_string(),
                })?;
        if !unpacked {
            return Err(FileError::InvalidInput {
                path: request.archive.clone(),
                message: "archive entry path is not safe".to_owned(),
            });
        }
        archive_control_checkpoint(controls, runtime)?;
    }
    Ok(())
}

async fn extract_archive_with_seven_zip(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<PathBuf, FileError> {
    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        match test_seven_zip_archive(command_name, &request, &cancel).await {
            Ok(()) => {
                let destination_created = ensure_destination_directory(&request.destination)?;
                let outcome = run_seven_zip_extract(command_name, &request, &cancel).await;
                if outcome.is_err() && destination_created {
                    let _ = fs::remove_dir_all(&request.destination);
                }
                return outcome.map(|_| request.destination);
            }
            Err(FileError::Archive { message, .. }) if message == command_not_found_marker() => {
                continue;
            }
            Err(error) => return Err(error),
        }
    }

    Err(FileError::Unsupported(
        "7z, 7zz or 7za command is required to extract this archive",
    ))
}

async fn inspect_archive_with_seven_zip(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<(), FileError> {
    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        match test_seven_zip_archive(command_name, &request, &cancel).await {
            Ok(()) => return Ok(()),
            Err(FileError::Archive { message, .. }) if message == command_not_found_marker() => {
                continue;
            }
            Err(error) => return Err(error),
        }
    }

    Err(FileError::Unsupported(
        "7z, 7zz or 7za command is required to extract this archive",
    ))
}

async fn test_seven_zip_archive(
    command_name: &str,
    request: &ArchiveExtractionRequest,
    cancel: &CancellationToken,
) -> Result<(), FileError> {
    let mut command = seven_zip_command(command_name, "t", request);
    append_seven_zip_archive_operand(&mut command, &request.archive);
    run_seven_zip_command(&mut command, request, cancel).await
}

async fn run_seven_zip_extract(
    command_name: &str,
    request: &ArchiveExtractionRequest,
    cancel: &CancellationToken,
) -> Result<(), FileError> {
    let mut command = seven_zip_extract_command(command_name, request);
    run_seven_zip_command(&mut command, request, cancel).await
}

fn seven_zip_extract_command(command_name: &str, request: &ArchiveExtractionRequest) -> Command {
    let mut command = seven_zip_command(command_name, "x", request);
    command.arg(seven_zip_output_directory_switch(&request.destination));
    append_seven_zip_archive_operand(&mut command, &request.archive);
    command
}

fn seven_zip_output_directory_switch(destination: &Path) -> OsString {
    let mut output_switch = OsString::from("-o");
    output_switch.push(destination.as_os_str());
    output_switch
}

fn seven_zip_command(
    command_name: &str,
    action: &str,
    request: &ArchiveExtractionRequest,
) -> Command {
    let mut command = Command::new(command_name);
    command
        .arg(action)
        .arg("-y")
        .arg("-bd")
        .arg("-bsp0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(password) = &request.password {
        command.arg(format!("-p{}", password.as_str()));
    }
    command
}

fn append_seven_zip_archive_operand(command: &mut Command, archive: &Path) {
    // 7z treats arguments after `--` as operands, so switches such as `-o` must be finalized first.
    command.arg("--").arg(archive);
}

async fn run_seven_zip_command(
    command: &mut Command,
    request: &ArchiveExtractionRequest,
    cancel: &CancellationToken,
) -> Result<(), FileError> {
    let output = tokio::select! {
        output = command.output() => output.map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                FileError::Archive {
                    path: request.archive.clone(),
                    message: command_not_found_marker().to_owned(),
                }
            } else {
                FileError::Archive {
                    path: request.archive.clone(),
                    message: source.to_string(),
                }
            }
        })?,
        _ = cancel.cancelled() => return Err(FileError::Cancelled),
    };
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(seven_zip_error(request, output.status, stdout, stderr))
}

fn seven_zip_error(
    request: &ArchiveExtractionRequest,
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
) -> FileError {
    let combined_output = if stdout.is_empty() {
        stderr.clone()
    } else if stderr.is_empty() {
        stdout.clone()
    } else {
        format!("{stdout}\n{stderr}")
    };
    let lower = combined_output.to_ascii_lowercase();
    if lower.contains("password") || lower.contains("encrypted") {
        if request.password.is_some() {
            return FileError::ArchiveInvalidPassword {
                path: request.archive.clone(),
            };
        }
        return FileError::ArchivePasswordRequired {
            path: request.archive.clone(),
        };
    }
    let message = if combined_output.is_empty() {
        format!("7z exited with status {status}")
    } else {
        combined_output
    };
    // 分卷后续卷在这里只报裸的 Headers Error:与列表失败同一套文件名
    // 启发式,补一句「请打开第一个卷」的提示。
    let message = match split_volume_hint(&request.archive) {
        Some(hint) => format!("{message}; {hint}"),
        None => message,
    };
    FileError::Archive {
        path: request.archive.clone(),
        message,
    }
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
    let Some(name) = archive_extraction_directory_name(archive) else {
        return Err(FileError::InvalidInput {
            path: archive.to_path_buf(),
            message: "archive name is not usable as a directory name".to_owned(),
        });
    };
    Ok(parent.join(name))
}

fn archive_extraction_directory_name(archive: &Path) -> Option<OsString> {
    let file_name = archive.file_name()?;
    if let Some(text) = file_name.to_str() {
        let lower = text.to_ascii_lowercase();
        for suffix in [".tar.gz", ".tgz", ".zip", ".tar", ".7z", ".rar"] {
            if lower.ends_with(suffix) {
                let stem = &text[..text.len() - suffix.len()];
                if !stem.is_empty() {
                    return Some(OsString::from(stem));
                }
            }
        }
    }
    archive.file_stem().map(OsStr::to_os_string)
}

/// 目的地允许已存在：右键智能解压的单根语义直接解入压缩包所在文件夹。
/// 返回本次是否新建了目录——失败清理只移除自建目录，
/// 解入已存在目录失败时必须原样保留用户数据。
fn ensure_destination_directory(destination: &Path) -> Result<bool, FileError> {
    if destination.is_dir() {
        return Ok(false);
    }
    fs::create_dir(destination).map_err(|source| FileError::CreateDirectory {
        path: destination.to_path_buf(),
        source,
    })?;
    Ok(true)
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

fn open_archive_file(path: &Path) -> Result<File, FileError> {
    File::open(path).map_err(|source| FileError::Archive {
        path: path.to_path_buf(),
        message: source.to_string(),
    })
}

fn zip_error(path: &Path, error: ZipError) -> FileError {
    match error {
        ZipError::UnsupportedArchive(message) if message == ZipError::PASSWORD_REQUIRED => {
            FileError::ArchivePasswordRequired {
                path: path.to_path_buf(),
            }
        }
        ZipError::InvalidPassword => FileError::ArchiveInvalidPassword {
            path: path.to_path_buf(),
        },
        error => FileError::Archive {
            path: path.to_path_buf(),
            message: error.to_string(),
        },
    }
}

fn archive_changed_after_inspection(path: &Path) -> FileError {
    FileError::Archive {
        path: path.to_path_buf(),
        message: "ZIP archive changed after progress workload inspection".to_owned(),
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

fn command_not_found_marker() -> &'static str {
    "__7z_command_not_found__"
}

#[derive(Debug, Clone, Copy)]
enum TarCompression {
    Plain,
    Gzip,
}

#[cfg(test)]
mod tests;
