//! 压缩成 .7z、或带密码压缩成 zip：走系统 7z。总进度按 7z 报的百分比走，
//! 7z 不报单个文件的字节进度。

use std::fs;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::{
    archive_relative_path, archive_source_bytes, collect_archive_entries, common_source_parent,
    zip_archive_names, ArchiveCreationRequest, ArchiveEntry, ArchiveFormat,
};
use crate::seven_zip_job::{run_seven_zip_job, SevenZipJobFailure};
use crate::{ArchiveProgress, FileError, FileOperationControls, SEVEN_ZIP_COMMAND_NAMES};

pub(super) async fn create_archive_with_seven_zip(
    request: ArchiveCreationRequest,
    mut controls: FileOperationControls,
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    let base_directory = common_source_parent(&request.sources)?;
    let entries = scan_sources(&request, &base_directory, &controls).await?;
    if request.format == ArchiveFormat::Zip {
        zip_archive_names(&entries)?;
    }
    let total_bytes = archive_source_bytes(&entries, &request.target)?;
    let total_entries = entries.len();
    let mut relative_sources = Vec::with_capacity(request.sources.len());
    for source in &request.sources {
        relative_sources.push(archive_relative_path(source, &base_directory)?);
    }

    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        let command =
            seven_zip_archive_command(command_name, &request, &base_directory, &relative_sources);
        let outcome = run_seven_zip_job(command, &mut controls, |sample| {
            progress(ArchiveProgress::Bytes {
                completed_bytes: total_bytes * u64::from(sample.percent) / 100,
                total_bytes,
                completed_entries: sample.completed_items.min(total_entries),
                total_entries: Some(total_entries),
            });
        })
        .await;
        let error = match outcome {
            Ok(()) => return Ok(request.target),
            Err(SevenZipJobFailure::CommandNotFound) => continue,
            Err(SevenZipJobFailure::Interrupted(error)) => error,
            Err(SevenZipJobFailure::Spawn(source)) => FileError::Archive {
                path: request.target.clone(),
                message: source.to_string(),
            },
            Err(SevenZipJobFailure::Exited { status, stderr }) => FileError::Archive {
                path: request.target.clone(),
                message: if stderr.is_empty() {
                    format!("7z exited with status {status}")
                } else {
                    stderr
                },
            },
        };
        // 半成品包是本次新建的那一个文件：坏了却顶着正常包的名字，
        // 留着会被当成好包用（用户确认要删）。
        let _ = fs::remove_file(&request.target);
        return Err(error);
    }

    Err(FileError::Unsupported("7z, 7zz or 7za command is required"))
}

/// 先扫一遍来源：7z 只报百分比，总字节和条目数要自己算才能写进文字；
/// zip 还要在动手前检查条目名能不能用 UTF-8 写进去。
async fn scan_sources(
    request: &ArchiveCreationRequest,
    base_directory: &Path,
    controls: &FileOperationControls,
) -> Result<Vec<ArchiveEntry>, FileError> {
    let sources = request.sources.clone();
    let base_directory = base_directory.to_path_buf();
    let cancel = controls.cancellation_token();
    let join_target = request.target.clone();
    tokio::task::spawn_blocking(move || collect_archive_entries(&sources, &base_directory, &cancel))
        .await
        .map_err(|error| FileError::Archive {
            path: join_target,
            message: error.to_string(),
        })?
}

fn seven_zip_archive_command(
    command_name: &str,
    request: &ArchiveCreationRequest,
    base_directory: &Path,
    relative_sources: &[PathBuf],
) -> Command {
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
        .arg("-bsp1")
        .arg("-bso0");
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
    command
}
