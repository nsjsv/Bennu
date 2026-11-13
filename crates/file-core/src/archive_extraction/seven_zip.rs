//! .7z / .rar 整包解压：只走系统 7z。入队前的检查只列目录、最多试解一个
//! 文件；任务里只解一遍。整包 `7z t` 曾让一个 3 GB 的包被完整处理三遍。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{ensure_destination_directory, ArchiveExtractionRequest};
use crate::archive_listing::{parse_seven_zip_entries, split_volume_hint, SevenZipListedEntry};
use crate::seven_zip_job::{run_seven_zip_job, SevenZipJobFailure};
use crate::seven_zip_password::seven_zip_password_error;
use crate::{ArchiveProgress, FileError, FileOperationControls, SEVEN_ZIP_COMMAND_NAMES};

const SEVEN_ZIP_REQUIRED: FileError =
    FileError::Unsupported("7z, 7zz or 7za command is required to extract this archive");

/// 入队前的密码检查。头部加密的包列目录时就会因为没密码或密码错而失败；
/// 只加密内容的包列得出来，再只试解代价最小的一个加密文件——错密码十几
/// 毫秒就以 "Wrong password?" 失败，对的密码只解这一个文件。
pub(super) async fn inspect_archive_with_seven_zip(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<(), FileError> {
    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        let entries = match list_entries(command_name, &request, &cancel).await {
            Err(ListingFailure::CommandNotFound) => continue,
            Err(ListingFailure::Failed(error)) => return Err(error),
            Ok(entries) => entries,
        };
        let Some(probe) = password_probe_entry(&entries) else {
            return Ok(());
        };
        if request.password.is_none() {
            return Err(FileError::ArchivePasswordRequired {
                path: request.archive.clone(),
            });
        }
        let mut command = seven_zip_command(command_name, "t", &request);
        // -spd：文件名按字面匹配，不当通配符。
        command
            .arg("-spd")
            .arg("--")
            .arg(&request.archive)
            .arg(&probe.path);
        let output = run_to_completion(command, &request, &cancel).await?;
        if output.status.success() {
            return Ok(());
        }
        return Err(seven_zip_error(
            &request,
            output.status,
            output.stdout,
            output.stderr,
        ));
    }
    Err(SEVEN_ZIP_REQUIRED)
}

pub(super) async fn extract_archive_with_seven_zip(
    request: ArchiveExtractionRequest,
    mut controls: FileOperationControls,
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    let cancel = controls.cancellation_token();
    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        let entries = match list_entries(command_name, &request, &cancel).await {
            Err(ListingFailure::CommandNotFound) => continue,
            Err(ListingFailure::Failed(error)) => return Err(error),
            Ok(entries) => entries,
        };
        let total_entries = entries.len();
        let total_bytes = entries
            .iter()
            .filter(|entry| !entry.is_directory)
            .map(|entry| entry.len)
            .sum::<u64>();
        // 出错或取消时不删已经解出来的东西：清理一旦有 bug 会多删用户文件。
        ensure_destination_directory(&request.destination)?;
        let outcome = run_seven_zip_job(
            seven_zip_extract_command(command_name, &request),
            &mut controls,
            |sample| {
                progress(ArchiveProgress::Bytes {
                    completed_bytes: total_bytes * u64::from(sample.percent) / 100,
                    total_bytes,
                    completed_entries: sample.completed_items.min(total_entries),
                    total_entries: Some(total_entries),
                });
            },
        )
        .await;
        return match outcome {
            Ok(()) => Ok(request.destination),
            Err(SevenZipJobFailure::CommandNotFound) => continue,
            Err(SevenZipJobFailure::Interrupted(error)) => Err(error),
            Err(SevenZipJobFailure::Spawn(source)) => Err(FileError::Archive {
                path: request.archive.clone(),
                message: source.to_string(),
            }),
            Err(SevenZipJobFailure::Exited { status, stderr }) => {
                Err(seven_zip_error(&request, status, String::new(), stderr))
            }
        };
    }
    Err(SEVEN_ZIP_REQUIRED)
}

/// 试解代价最小的加密文件。solid block 里的文件只能从块头按顺序解，
/// 代价是块内它以及它之前所有文件的大小；没有块信息（rar 等）时只算它自己。
fn password_probe_entry(entries: &[SevenZipListedEntry]) -> Option<&SevenZipListedEntry> {
    let mut block_bytes = std::collections::HashMap::<u64, u64>::new();
    entries
        .iter()
        .filter(|entry| !entry.is_directory)
        .map(|entry| {
            let cost = match entry.block {
                Some(block) => {
                    let bytes = block_bytes.entry(block).or_default();
                    *bytes += entry.len;
                    *bytes
                }
                None => entry.len,
            };
            (entry, cost)
        })
        .filter(|(entry, _)| entry.encrypted)
        .min_by_key(|(_, cost)| *cost)
        .map(|(entry, _)| entry)
}

enum ListingFailure {
    CommandNotFound,
    Failed(FileError),
}

async fn list_entries(
    command_name: &str,
    request: &ArchiveExtractionRequest,
    cancel: &CancellationToken,
) -> Result<Vec<SevenZipListedEntry>, ListingFailure> {
    let mut command = seven_zip_command(command_name, "l", request);
    command.arg("-slt");
    append_seven_zip_archive_operand(&mut command, &request.archive);
    let output = match run_to_completion(command, request, cancel).await {
        Ok(output) => output,
        Err(FileError::Archive { message, .. }) if message == command_not_found_marker() => {
            return Err(ListingFailure::CommandNotFound);
        }
        Err(error) => return Err(ListingFailure::Failed(error)),
    };
    if !output.status.success() {
        return Err(ListingFailure::Failed(seven_zip_error(
            request,
            output.status,
            output.stdout,
            output.stderr,
        )));
    }
    Ok(parse_seven_zip_entries(&output.stdout))
}

struct SevenZipOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

async fn run_to_completion(
    mut command: Command,
    request: &ArchiveExtractionRequest,
    cancel: &CancellationToken,
) -> Result<SevenZipOutput, FileError> {
    let output = tokio::select! {
        output = command.output() => output.map_err(|source| FileError::Archive {
            path: request.archive.clone(),
            message: if source.kind() == std::io::ErrorKind::NotFound {
                command_not_found_marker().to_owned()
            } else {
                source.to_string()
            },
        })?,
        _ = cancel.cancelled() => return Err(FileError::Cancelled),
    };
    Ok(SevenZipOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

pub(super) fn seven_zip_extract_command(
    command_name: &str,
    request: &ArchiveExtractionRequest,
) -> Command {
    let mut command = Command::new(command_name);
    command.arg("x").arg("-y").arg("-bsp1").arg("-bso0");
    append_password(&mut command, request);
    command.arg(seven_zip_output_directory_switch(&request.destination));
    append_seven_zip_archive_operand(&mut command, &request.archive);
    command
}

pub(super) fn seven_zip_output_directory_switch(destination: &Path) -> OsString {
    let mut output_switch = OsString::from("-o");
    output_switch.push(destination.as_os_str());
    output_switch
}

/// 列目录和试解用：一次性收集输出，不要进度。
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
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    append_password(&mut command, request);
    command
}

fn append_password(command: &mut Command, request: &ArchiveExtractionRequest) {
    if let Some(password) = &request.password {
        command.arg(format!("-p{}", password.as_str()));
    }
}

fn append_seven_zip_archive_operand(command: &mut Command, archive: &Path) {
    // 7z treats arguments after `--` as operands, so switches such as `-o` must be finalized first.
    command.arg("--").arg(archive);
}

pub(super) fn seven_zip_error(
    request: &ArchiveExtractionRequest,
    status: ExitStatus,
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
    if let Some(error) = seven_zip_password_error(
        &combined_output,
        request.password.is_some(),
        &request.archive,
    ) {
        return error;
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

fn command_not_found_marker() -> &'static str {
    "__7z_command_not_found__"
}

#[cfg(test)]
mod tests;
