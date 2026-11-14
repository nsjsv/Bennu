//! tar.gz：`tar -c` 打包，输出经本进程转送给 `gzip`，gzip 直接写目标文件。暂停就停送，
//! 两个进程都被管道卡住；取消就杀掉。进度按转送的字节对照估算的 tar 流大小算——tar 流
//! 每个条目带 512 字节的头、数据按 512 对齐，小文件多时比源文件大好几倍，不能直接拿源字节比。

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Command;

use super::{create_archive_file, ArchiveCreationRequest, ArchiveEntry, ArchiveSources};
use crate::tar_job::{collect_output, pump_with_controls, tar_command, tar_failure, PumpFailure};
use crate::{ArchiveProgress, FileError, FileOperationControls};

const TAR_BLOCK: u64 = 512;
/// GNU tar 默认每 20 块写一次，结尾补齐到整记录。
const TAR_RECORD: u64 = 20 * TAR_BLOCK;
/// gnu 格式的头里名字只放得下这么长，再长要多一个长名头。
const TAR_HEADER_NAME_BYTES: usize = 100;

pub(super) async fn create_tar_gz_archive(
    request: ArchiveCreationRequest,
    sources: ArchiveSources,
    controls: FileOperationControls,
    progress: impl FnMut(ArchiveProgress),
) -> Result<PathBuf, FileError> {
    let target = create_archive_file(&request.target)?;
    let outcome = run_tar_gzip(&request, &sources, target, controls, progress).await;
    if outcome.is_err() {
        // 半成品包是本次 create_new 建出的那一个文件：坏了却顶着正常包的名字，
        // 留着会被当成好包用（用户确认要删）。
        let _ = fs::remove_file(&request.target);
    }
    outcome.map(|()| request.target)
}

async fn run_tar_gzip(
    request: &ArchiveCreationRequest,
    sources: &ArchiveSources,
    target: fs::File,
    mut controls: FileOperationControls,
    mut progress: impl FnMut(ArchiveProgress),
) -> Result<(), FileError> {
    let spawn_error = |program: &str, source: std::io::Error| FileError::Archive {
        path: request.target.clone(),
        message: format!("could not run {program}: {source}"),
    };
    let mut gzip = Command::new("gzip")
        .arg("-c")
        .arg(request.compression_level.gzip_level())
        .stdin(Stdio::piped())
        .stdout(Stdio::from(target))
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| spawn_error("gzip", source))?;
    // --dereference / --hard-dereference：压进去的是链接指向的内容，和扫描时的大小一致；
    // --sort=name：目录内按名字排，和扫描顺序一致，条目数才对得上。
    let mut tar = tar_command();
    tar.current_dir(&sources.base_directory)
        .arg("-c")
        .arg("--dereference")
        .arg("--hard-dereference")
        .arg("--sort=name")
        .arg("-f")
        .arg("-")
        .arg("--")
        .args(&sources.relative_sources);
    let mut tar = tar.spawn().map_err(|source| spawn_error("tar", source))?;
    let mut archive_stream = tar.stdout.take().expect("tar stdout is piped");
    let tar_stderr = collect_output(tar.stderr.take().expect("tar stderr is piped"));
    let gzip_stderr = collect_output(gzip.stderr.take().expect("gzip stderr is piped"));
    let mut gzip_stdin = gzip.stdin.take().expect("gzip stdin is piped");

    let layout = TarStreamLayout::of(&sources.entries);
    let total_bytes = sources.total_bytes;
    progress(layout.progress(0, total_bytes));
    let pumped = pump_with_controls(
        &mut archive_stream,
        &mut gzip_stdin,
        &mut controls,
        |bytes| progress(layout.progress(bytes, total_bytes)),
    )
    .await;
    // 关掉 gzip 的输入，让它写完结尾。
    drop(gzip_stdin);
    match pumped {
        Ok(_) => {}
        Err(PumpFailure::Interrupted(error)) => {
            let _ = tar.kill().await;
            let _ = gzip.kill().await;
            return Err(error);
        }
        Err(PumpFailure::Read(source)) => {
            let _ = tar.kill().await;
            let _ = gzip.kill().await;
            return Err(FileError::Archive {
                path: request.target.clone(),
                message: source.to_string(),
            });
        }
        // gzip 提前退出了：原因在它的 stderr 里。
        Err(PumpFailure::Write) => {
            let _ = tar.kill().await;
        }
    }
    let wait_error = |source: std::io::Error| FileError::Archive {
        path: request.target.clone(),
        message: source.to_string(),
    };
    let tar_status = tar.wait().await.map_err(wait_error)?;
    let gzip_status = gzip.wait().await.map_err(wait_error)?;
    let gzip_stderr = gzip_stderr.await.unwrap_or_default();
    if !gzip_status.success() {
        return Err(FileError::Archive {
            path: request.target.clone(),
            message: match String::from_utf8_lossy(&gzip_stderr).trim() {
                "" => format!("gzip exited with status {gzip_status}"),
                stderr => stderr.to_owned(),
            },
        });
    }
    // 源文件在压缩途中变了（tar 报 "file changed as we read it"）也算失败：包里的不是完整的那一份。
    if !tar_status.success() {
        let tar_stderr = tar_stderr.await.unwrap_or_default();
        return Err(tar_failure(&request.target, tar_status, &tar_stderr));
    }
    progress(layout.finished(total_bytes));
    Ok(())
}

/// 估算的 tar 流：每个条目在流里的结束位置（按扫描顺序）和总长度。
struct TarStreamLayout {
    entry_ends: Vec<u64>,
    total: u64,
}

impl TarStreamLayout {
    fn of(entries: &[ArchiveEntry]) -> Self {
        let mut position = 0_u64;
        let entry_ends = entries
            .iter()
            .map(|entry| {
                position += entry_stream_bytes(entry);
                position
            })
            .collect();
        // 结尾两个空块，再补齐到整记录。
        let total = (position + 2 * TAR_BLOCK).div_ceil(TAR_RECORD) * TAR_RECORD;
        Self { entry_ends, total }
    }

    /// 已转送 `streamed` 字节时的进度；显示用的字节按比例折回源文件字节。
    fn progress(&self, streamed: u64, total_bytes: u64) -> ArchiveProgress {
        let streamed = streamed.min(self.total);
        ArchiveProgress::Bytes {
            completed_bytes: (u128::from(total_bytes) * u128::from(streamed)
                / u128::from(self.total.max(1))) as u64,
            total_bytes,
            completed_entries: self.entry_ends.partition_point(|end| *end <= streamed),
            total_entries: Some(self.entry_ends.len()),
        }
    }

    fn finished(&self, total_bytes: u64) -> ArchiveProgress {
        self.progress(self.total, total_bytes)
    }
}

fn entry_stream_bytes(entry: &ArchiveEntry) -> u64 {
    // 目录名在包里带结尾的 `/`。
    let name_bytes =
        entry.relative_path.as_os_str().as_bytes().len() + usize::from(entry.is_directory);
    let long_name = if name_bytes > TAR_HEADER_NAME_BYTES {
        TAR_BLOCK + (name_bytes as u64 + 1).div_ceil(TAR_BLOCK) * TAR_BLOCK
    } else {
        0
    };
    long_name + TAR_BLOCK + entry.source_bytes.div_ceil(TAR_BLOCK) * TAR_BLOCK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_directory: bool, source_bytes: u64) -> ArchiveEntry {
        ArchiveEntry {
            relative_path: PathBuf::from(path),
            is_directory,
            source_bytes,
        }
    }

    #[test]
    fn layout_counts_headers_padding_and_long_names() {
        let long = "x".repeat(150);
        let layout = TarStreamLayout::of(&[
            entry("dir", true, 0),
            entry("dir/a.txt", false, 100),
            entry(&long, false, 513),
        ]);

        // 目录 512；a.txt 512 + 512；长名 512 + 512（151 字节的名字）+ 512 + 1024。
        assert_eq!(layout.entry_ends, vec![512, 1536, 4096]);
        assert_eq!(layout.total, TAR_RECORD);

        let halfway = layout.progress(1536, 613);
        assert_eq!(
            halfway,
            ArchiveProgress::Bytes {
                completed_bytes: 613 * 1536 / TAR_RECORD,
                total_bytes: 613,
                completed_entries: 2,
                total_entries: Some(3),
            }
        );
        assert_eq!(
            layout.finished(613),
            ArchiveProgress::Bytes {
                completed_bytes: 613,
                total_bytes: 613,
                completed_entries: 3,
                total_entries: Some(3),
            }
        );
        // tar 实际写出的比估算多（比如 PAX 头）时不越过终点。
        assert_eq!(layout.progress(u64::MAX, 613), layout.finished(613));
    }
}
