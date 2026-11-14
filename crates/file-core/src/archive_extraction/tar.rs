//! tar 一类整包解压：本进程读包文件、送进 `tar -x -f -` 的 stdin。进度按送进去的包字节算，
//! 对任何压缩方式都准；暂停就停送，取消就杀掉 tar。tar 自己挡住 `..` 和绝对路径的条目。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use super::{ensure_destination_directory, ArchiveExtractionRequest};
use crate::tar_job::{collect_output, pump_with_controls, tar_command, tar_failure, PumpFailure};
use crate::{ArchiveProgress, FileError, FileOperationControls};

pub(super) async fn extract_tar_archive(
    request: ArchiveExtractionRequest,
    mut controls: FileOperationControls,
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    let archive_error = |source: std::io::Error| FileError::Archive {
        path: request.archive.clone(),
        message: source.to_string(),
    };
    let mut archive = tokio::fs::File::open(&request.archive)
        .await
        .map_err(archive_error)?;
    let total_bytes = archive.metadata().await.map_err(archive_error)?.len();
    // tar 从管道读时不自己认压缩方式，要告诉它。
    let decompress = decompress_option(&mut archive)
        .await
        .map_err(archive_error)?;
    ensure_destination_directory(&request.destination)?;

    let mut command = tar_command();
    command.arg("-x").arg("-v").arg("--no-same-owner");
    command.args(decompress);
    let mut child = command
        .arg("-f")
        .arg("-")
        .arg("-C")
        .arg(&request.destination)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|source| FileError::Archive {
            path: request.archive.clone(),
            message: format!("could not run tar: {source}"),
        })?;
    let mut stdin = child.stdin.take().expect("tar stdin is piped");
    let stderr = collect_output(child.stderr.take().expect("tar stderr is piped"));
    let completed_files = Arc::new(AtomicUsize::new(0));
    let listing = count_extracted_files(
        child.stdout.take().expect("tar stdout is piped"),
        Arc::clone(&completed_files),
    );
    let report = |progress: &mut dyn FnMut(ArchiveProgress), completed_bytes: u64| {
        progress(ArchiveProgress::Bytes {
            completed_bytes,
            total_bytes,
            completed_entries: completed_files.load(Ordering::Relaxed),
            total_entries: None,
        })
    };

    report(&mut progress, 0);
    let pumped = pump_with_controls(&mut archive, &mut stdin, &mut controls, |bytes| {
        report(&mut progress, bytes.min(total_bytes))
    })
    .await;
    // 关掉 stdin 让 tar 读到结尾；tar 提前出错退出时写会失败，原因看它的 stderr。
    drop(stdin);
    match pumped {
        Err(PumpFailure::Interrupted(error)) => {
            let _ = child.kill().await;
            return Err(error);
        }
        Err(PumpFailure::Read(source)) => {
            let _ = child.kill().await;
            return Err(archive_error(source));
        }
        Ok(_) | Err(PumpFailure::Write) => {}
    }
    let cancel = controls.cancellation_token();
    let status = tokio::select! {
        status = child.wait() => status.map_err(archive_error)?,
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(controls.interruption().unwrap_or(FileError::Cancelled));
        }
    };
    let _ = listing.await;
    let stderr = stderr.await.unwrap_or_default();
    if !status.success() {
        return Err(tar_failure(&request.archive, status, &stderr));
    }
    report(&mut progress, total_bytes);
    Ok(request.destination)
}

/// `-v` 每解一个条目在 stdout 打一行名字（与错误分开，错误在 stderr）；目录以 `/` 结尾，不计。
fn count_extracted_files(
    stdout: tokio::process::ChildStdout,
    completed_files: Arc<AtomicUsize>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).split(b'\n');
        while let Ok(Some(line)) = lines.next_segment().await {
            if !line.ends_with(b"/") {
                completed_files.fetch_add(1, Ordering::Relaxed);
            }
        }
    })
}

/// 看文件头决定解压选项，读完把位置拨回开头。不认识的头当作没压缩，交给 tar 判断。
async fn decompress_option(archive: &mut tokio::fs::File) -> std::io::Result<Option<&'static str>> {
    use std::io::SeekFrom;
    use tokio::io::AsyncSeekExt;

    let mut header = [0_u8; 6];
    let read = archive.read(&mut header).await?;
    archive.seek(SeekFrom::Start(0)).await?;
    Ok(decompress_option_for_header(&header[..read]))
}

fn decompress_option_for_header(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(&[0x1f, 0x8b]) {
        Some("--gzip")
    } else if header.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Some("--xz")
    } else if header.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some("--zstd")
    } else if header.starts_with(b"BZh") {
        Some("--bzip2")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_is_read_from_the_header() {
        assert_eq!(
            decompress_option_for_header(b"\x1f\x8b\x08"),
            Some("--gzip")
        );
        assert_eq!(decompress_option_for_header(b"\xfd7zXZ\x00"), Some("--xz"));
        assert_eq!(
            decompress_option_for_header(b"\x28\xb5\x2f\xfd"),
            Some("--zstd")
        );
        assert_eq!(decompress_option_for_header(b"BZh91AY"), Some("--bzip2"));
        assert_eq!(decompress_option_for_header(b"ustar"), None);
        assert_eq!(decompress_option_for_header(b""), None);
    }
}
