use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use zip::result::ZipError;

use self::seven_zip::{extract_archive_with_seven_zip, inspect_archive_with_seven_zip};
use crate::{
    ArchiveEntryProgress, ArchivePassword, ArchiveProgress, FileError, FileOperationControls,
};

mod seven_zip;

const ARCHIVE_EXTRACTION_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveExtractionFormat {
    Zip,
    Tar,
    TarGz,
    SevenZip,
    Rar,
}

#[derive(Debug)]
struct ZipExtractionWorkload {
    entry_bytes: Vec<u64>,
    total_bytes: u64,
    /// 目录不计：总进度按文件个数走。
    total_files: usize,
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
    let format = archive_extraction_format_for_path(&request.archive).ok_or(
        FileError::Unsupported("archive format is not supported for extraction"),
    )?;
    controls.wait_until_running().await?;

    match format {
        ArchiveExtractionFormat::Zip => extract_zip_archive(request, controls, progress).await,
        ArchiveExtractionFormat::Tar => {
            reject_tar_password(&request)?;
            extract_tar_archive(request, TarCompression::Plain, controls, progress).await
        }
        ArchiveExtractionFormat::TarGz => {
            reject_tar_password(&request)?;
            extract_tar_archive(request, TarCompression::Gzip, controls, progress).await
        }
        ArchiveExtractionFormat::SevenZip | ArchiveExtractionFormat::Rar => {
            extract_archive_with_seven_zip(request, controls, progress).await
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
    progress: impl FnMut(ArchiveProgress) + Send + 'static,
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
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    let cancel = controls.cancellation_token();
    let workload = inspect_zip_entries(&request, &cancel)?;
    archive_control_checkpoint(&mut controls, &runtime)?;
    progress(workload.progress(0, 0, None));
    // 出错或取消时不删已经解出来的东西：清理一旦有 bug 会多删用户文件。
    ensure_destination_directory(&request.destination)?;
    extract_zip_entries(&request, &mut controls, &runtime, &workload, &mut progress)?;
    Ok(request.destination)
}

impl ZipExtractionWorkload {
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

fn inspect_zip_entries(
    request: &ArchiveExtractionRequest,
    cancel: &CancellationToken,
) -> Result<ZipExtractionWorkload, FileError> {
    let file = open_archive_file(&request.archive)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| zip_error(&request.archive, error))?;
    let mut entry_bytes = Vec::with_capacity(archive.len());
    let mut total_bytes = 0_u64;
    let mut total_files = 0;

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
        total_files += usize::from(!entry.is_dir());
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
        total_files,
    })
}

fn extract_zip_entries(
    request: &ArchiveExtractionRequest,
    controls: &mut FileOperationControls,
    runtime: &Handle,
    workload: &ZipExtractionWorkload,
    progress: &mut impl FnMut(ArchiveProgress),
) -> Result<(), FileError> {
    let file = open_archive_file(&request.archive)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| zip_error(&request.archive, error))?;
    if archive.len() != workload.entry_bytes.len() {
        return Err(archive_changed_after_inspection(&request.archive));
    }
    let mut completed_bytes = 0_u64;
    let mut completed_files = 0;
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
        let entry_name = relative_path
            .file_name()
            .unwrap_or(relative_path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let target = request.destination.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|source| FileError::CreateDirectory {
                path: target,
                source,
            })?;
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
            progress(workload.progress(
                completed_files,
                completed_bytes,
                Some(ArchiveEntryProgress {
                    name: entry_name.clone(),
                    completed_bytes: completed_entry_bytes,
                    total_bytes: expected_entry_bytes,
                }),
            ));
        }
        if completed_entry_bytes != expected_entry_bytes {
            return Err(archive_changed_after_inspection(&request.archive));
        }
        output.flush().map_err(|source| FileError::Archive {
            path: target,
            message: source.to_string(),
        })?;
        completed_files += 1;
        archive_control_checkpoint(controls, runtime)?;
        progress(workload.progress(completed_files, completed_bytes, None));
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
    mut progress: impl FnMut(ArchiveProgress) + Send + 'static,
) -> Result<PathBuf, FileError> {
    let destination = request.destination.clone();
    let join_destination = destination.clone();
    let runtime = Handle::current();
    tokio::task::spawn_blocking(move || {
        extract_tar_archive_blocking(request, compression, controls, runtime, &mut progress)
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
    progress: &mut dyn FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    archive_control_checkpoint(&mut controls, &runtime)?;
    // 出错或取消时不删已经解出来的东西：清理一旦有 bug 会多删用户文件。
    ensure_destination_directory(&request.destination)?;
    let file = open_archive_file(&request.archive)?;
    let total_bytes = file
        .metadata()
        .map_err(|source| FileError::Metadata {
            path: request.archive.clone(),
            source,
        })?
        .len();
    let completed_entries = std::rc::Rc::new(std::cell::Cell::new(0));
    let reader = TarReadProgress {
        inner: file,
        controls: controls.clone(),
        runtime: runtime.clone(),
        read_bytes: 0,
        total_bytes,
        completed_entries: completed_entries.clone(),
        last_sent_at: None,
        progress,
    };
    let reader = match compression {
        TarCompression::Plain => {
            extract_tar_entries(reader, &request, &controls, &completed_entries)?
        }
        TarCompression::Gzip => extract_tar_entries(
            flate2::read::GzDecoder::new(reader),
            &request,
            &controls,
            &completed_entries,
        )?
        .into_inner(),
    };
    reader.report_finished();
    Ok(request.destination)
}

/// tar 没有目录表，只能边读边解：进度按读过的压缩包原始字节算（gzip 解码
/// 在它外面），事先不知道条目总数。单个大条目解到一半时也要有进度，所以
/// 在读的地方按间隔报，而不是每解完一个条目才报；暂停也在读的地方等，
/// 大条目解到一半也能停住。
struct TarReadProgress<'p, R> {
    inner: R,
    controls: FileOperationControls,
    runtime: Handle,
    read_bytes: u64,
    total_bytes: u64,
    completed_entries: std::rc::Rc<std::cell::Cell<usize>>,
    last_sent_at: Option<Instant>,
    progress: &'p mut dyn FnMut(ArchiveProgress),
}

impl<R> TarReadProgress<'_, R> {
    fn report(&mut self, completed_bytes: u64) {
        (self.progress)(ArchiveProgress::Bytes {
            completed_bytes,
            total_bytes: self.total_bytes,
            completed_entries: self.completed_entries.get(),
            total_entries: None,
        });
    }

    /// 解完了：gzip 尾部和 tar 结尾的空块不一定会被读到，直接报满，并带上
    /// 最终条目数（读完最后一块数据时，最后一个条目还没解完）。
    fn report_finished(mut self) {
        self.report(self.total_bytes);
    }
}

impl<R: io::Read> io::Read for TarReadProgress<'_, R> {
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
            self.report(self.read_bytes.min(self.total_bytes));
        }
        Ok(read)
    }
}

fn extract_tar_entries<R: io::Read>(
    reader: R,
    request: &ArchiveExtractionRequest,
    controls: &FileOperationControls,
    completed_entries: &std::cell::Cell<usize>,
) -> Result<R, FileError> {
    let mut archive = tar::Archive::new(reader);
    // 读数据时被取消或关机打断：tar 会把它包成自己的读错误，按运行状态还原。
    let tar_error = |source: io::Error| {
        controls
            .interruption()
            .unwrap_or_else(|| FileError::Archive {
                path: request.archive.clone(),
                message: source.to_string(),
            })
    };
    let entries = archive.entries().map_err(tar_error)?;

    // 暂停/取消在 reader 里每次读数据时检查（读条目头也算），这里不用再查。
    for entry in entries {
        let mut entry = entry.map_err(tar_error)?;
        let unpacked = entry.unpack_in(&request.destination).map_err(tar_error)?;
        if !unpacked {
            return Err(FileError::InvalidInput {
                path: request.archive.clone(),
                message: "archive entry path is not safe".to_owned(),
            });
        }
        if !entry.header().entry_type().is_dir() {
            completed_entries.set(completed_entries.get() + 1);
        }
    }
    Ok(archive.into_inner())
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

#[derive(Debug, Clone, Copy)]
enum TarCompression {
    Plain,
    Gzip,
}
