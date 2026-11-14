//! 压缩成 zip / 7z：走系统 7z。总进度按 7z 报的百分比走，7z 不报单个文件的字节进度。

use std::fs;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::{ArchiveCreationRequest, ArchiveFormat, ArchiveSources};
use crate::seven_zip_job::{locate_seven_zip, run_seven_zip_job, SevenZipJobFailure};
use crate::{ArchiveProgress, FileError, FileOperationControls};

pub(super) async fn create_archive_with_seven_zip(
    request: ArchiveCreationRequest,
    sources: ArchiveSources,
    mut controls: FileOperationControls,
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    let command = seven_zip_archive_command(&locate_seven_zip()?, &request, &sources);
    let total_bytes = sources.total_bytes;
    let total_entries = sources.entries.len();
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
        Err(SevenZipJobFailure::Interrupted(error)) => error,
        Err(SevenZipJobFailure::Spawn(source)) => FileError::Archive {
            path: request.target.clone(),
            message: format!("could not run 7z: {source}"),
        },
        Err(SevenZipJobFailure::Exited { status, stderr }) => FileError::Archive {
            path: request.target.clone(),
            message: match String::from_utf8_lossy(&stderr).trim() {
                "" => format!("7z exited with status {status}"),
                stderr => stderr.to_owned(),
            },
        },
    };
    // 半成品包是本次新建的那一个文件：坏了却顶着正常包的名字，
    // 留着会被当成好包用（用户确认要删）。
    let _ = fs::remove_file(&request.target);
    Err(error)
}

fn seven_zip_archive_command(
    program: &Path,
    request: &ArchiveCreationRequest,
    sources: &ArchiveSources,
) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(&sources.base_directory)
        .arg("a")
        .arg(match request.format {
            ArchiveFormat::Zip => "-tzip",
            ArchiveFormat::SevenZip => "-t7z",
            ArchiveFormat::TarGz => unreachable!("tar.gz archives are created with tar"),
        })
        .arg(request.compression_level.seven_zip_level())
        .arg("-y")
        .arg("-bsp1")
        .arg("-bso0");
    if request.format == ArchiveFormat::Zip {
        // 非 ASCII 的名字按 UTF-8 写并打上标记，别的系统解出来不乱码。
        command.arg("-mcu=on");
    }
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
    for source in &sources.relative_sources {
        command.arg(source);
    }
    command
}
