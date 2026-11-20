//! zip / 7z / rar 整包解压：只走系统 7z。入队前的检查只列目录、最多试解一个
//! 文件；任务里只解一遍。整包 `7z t` 曾让一个 3 GB 的包被完整处理三遍。

use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{extraction_target, ArchiveExtractionRequest, ExtractionTarget};
use crate::archive_listing::{list_seven_zip_entries, SevenZipListedEntry};
use crate::seven_zip_job::{
    locate_seven_zip, run_seven_zip_job, seven_zip_failure, SevenZipJobFailure,
};
use crate::{ArchiveProgress, FileError, FileOperationControls};

mod display_names;

/// 入队前的密码检查。头部加密的包列目录时就会因为没密码或密码错而失败；
/// 只加密内容的包列得出来，再只试解代价最小的一个加密文件——错密码十几
/// 毫秒就以 "Wrong password?" 失败，对的密码只解这一个文件。
pub(super) async fn inspect_archive_with_seven_zip(
    request: ArchiveExtractionRequest,
    cancel: CancellationToken,
) -> Result<(), FileError> {
    let entries =
        list_seven_zip_entries(&request.archive, request.password.as_ref(), &cancel).await?;
    let Some(probe) = password_probe_entry(&entries) else {
        return Ok(());
    };
    let Some(password) = &request.password else {
        return Err(FileError::ArchivePasswordRequired {
            path: request.archive.clone(),
        });
    };
    // -spd：文件名按字面匹配，不当通配符。
    let mut command = Command::new(locate_seven_zip()?);
    command
        .arg("t")
        .arg("-y")
        .arg("-bd")
        .arg("-bsp0")
        .arg(format!("-p{}", password.as_str()))
        .arg("-spd")
        .arg("--")
        .arg(&request.archive)
        .arg(std::ffi::OsStr::from_bytes(&probe.member.stored_path))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::select! {
        output = command.output() => output.map_err(|source| FileError::Archive {
            path: request.archive.clone(),
            message: format!("could not run 7z: {source}"),
        })?,
        _ = cancel.cancelled() => return Err(FileError::Cancelled),
    };
    if output.status.success() {
        return Ok(());
    }
    Err(seven_zip_failure(
        &request.archive,
        true,
        output.status,
        &output.stdout,
        &output.stderr,
    ))
}

pub(super) async fn extract_archive_with_seven_zip(
    request: &ArchiveExtractionRequest,
    mut controls: FileOperationControls,
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<ExtractionTarget, FileError> {
    let program = locate_seven_zip()?;
    let entries = list_seven_zip_entries(
        &request.archive,
        request.password.as_ref(),
        &controls.cancellation_token(),
    )
    .await?;
    let total_entries = entries.len();
    let total_bytes = entries
        .iter()
        .filter(|entry| !entry.is_directory())
        .map(|entry| entry.member.len)
        .sum::<u64>();
    let members = entries.iter().map(|entry| entry.member.clone()).collect();
    let target = extraction_target(request, async { Some(members) }).await?;
    let outcome = run_seven_zip_job(
        seven_zip_extract_command(&program, request, &target),
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
    match outcome {
        Ok(()) => {}
        Err(SevenZipJobFailure::Interrupted(error)) => return Err(error),
        Err(SevenZipJobFailure::Spawn(source)) => {
            return Err(FileError::Archive {
                path: request.archive.clone(),
                message: format!("could not run 7z: {source}"),
            })
        }
        Err(SevenZipJobFailure::Exited { status, stderr }) => {
            return Err(seven_zip_failure(
                &request.archive,
                request.password.is_some(),
                status,
                &[],
                &stderr,
            ))
        }
    }
    // 7z 把不是 UTF-8 的名字原样写到磁盘上，解完改成显示用的名字。
    display_names::rename_to_display_names(target.directory(), &entries)?;
    Ok(target)
}

/// 试解代价最小的加密文件。solid block 里的文件只能从块头按顺序解，
/// 代价是块内它以及它之前所有文件的大小；没有块信息（zip、rar 等）时只算它自己。
fn password_probe_entry(entries: &[SevenZipListedEntry]) -> Option<&SevenZipListedEntry> {
    let mut block_bytes = std::collections::HashMap::<u64, u64>::new();
    entries
        .iter()
        .filter(|entry| !entry.is_directory())
        .map(|entry| {
            let cost = match entry.block {
                Some(block) => {
                    let bytes = block_bytes.entry(block).or_default();
                    *bytes += entry.member.len;
                    *bytes
                }
                None => entry.member.len,
            };
            (entry, cost)
        })
        .filter(|(entry, _)| entry.encrypted)
        .min_by_key(|(_, cost)| *cost)
        .map(|(entry, _)| entry)
}

pub(super) fn seven_zip_extract_command(
    program: &Path,
    request: &ArchiveExtractionRequest,
    target: &ExtractionTarget,
) -> Command {
    let mut command = Command::new(program);
    command.arg("x").arg("-y").arg("-bsp1").arg("-bso0");
    if let ExtractionTarget::BesideArchive { .. } = target {
        // 解进已有的文件夹时，遇到同名文件给解出来的那个改名（`a_1.jpg`），原有的不动。
        command.arg("-aou");
    }
    if let Some(password) = &request.password {
        command.arg(format!("-p{}", password.as_str()));
    }
    // `--` 之后都当文件名，`-o` 必须写在它前面。
    command
        .arg(seven_zip_output_directory_switch(target.directory()))
        .arg("--")
        .arg(&request.archive);
    command
}

pub(super) fn seven_zip_output_directory_switch(destination: &Path) -> OsString {
    let mut output_switch = OsString::from("-o");
    output_switch.push(destination.as_os_str());
    output_switch
}

#[cfg(test)]
mod tests;
