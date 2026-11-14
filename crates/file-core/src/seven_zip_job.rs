//! 7z 子进程的唯一运行入口：整包解压与走 7z 的压缩共用。进度、暂停、取消
//! 都在这里——子进程不经过本进程的检查点，只能由监督方替它响应运行状态。

use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::archive_listing::split_volume_hint;
use crate::seven_zip_password::seven_zip_password_error;
use crate::tar_job::collect_output;
use crate::{ExternalTool, FileError, FileOperationControls, FileOperationRunState};

/// 7-Zip 是可选依赖：没装就是 `ToolMissing`，界面据此给安装命令。
pub(crate) fn locate_seven_zip() -> Result<PathBuf, FileError> {
    ExternalTool::SevenZip
        .locate()
        .ok_or(FileError::ToolMissing(ExternalTool::SevenZip))
}

/// 7z 失败的统一分类：先认密码问题，再补分卷后续卷的提示，其余把 7z 的原话给用户。
pub(crate) fn seven_zip_failure(
    archive: &Path,
    password_provided: bool,
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> FileError {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let combined_output = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(error) = seven_zip_password_error(&combined_output, password_provided, archive) {
        return error;
    }
    let message = if combined_output.is_empty() {
        format!("7z exited with status {status}")
    } else {
        combined_output
    };
    // 分卷后续卷 7z 只报裸的 Headers Error：按文件名补一句「请打开第一个卷」。
    let message = match split_volume_hint(archive) {
        Some(hint) => format!("{message}; {hint}"),
        None => message,
    };
    FileError::Archive {
        path: archive.to_path_buf(),
        message,
    }
}

/// `-bsp1` 进度流里的一次更新。`completed_items` 是 7z 报的已完成条目数
/// （目录也算），为 0 时 7z 不写这个数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SevenZipProgress {
    pub(crate) percent: u8,
    pub(crate) completed_items: usize,
}

#[derive(Debug)]
pub(crate) enum SevenZipJobFailure {
    Spawn(io::Error),
    /// 进程跑完但失败；stderr 原文交给调用方分类（密码错误等）。
    Exited {
        status: ExitStatus,
        stderr: Vec<u8>,
    },
    /// 取消或关机：进程已被杀掉。
    Interrupted(FileError),
}

/// 跑一个已经写好参数的 7z 命令直到结束。参数里必须带 `-bsp1 -bso0`：
/// 进度写到 stdout，错误照旧写到 stderr（`-bd` 会把进度整个关掉）。
pub(crate) async fn run_seven_zip_job(
    mut command: Command,
    controls: &mut FileOperationControls,
    mut on_progress: impl FnMut(SevenZipProgress),
) -> Result<(), SevenZipJobFailure> {
    controls
        .wait_until_running()
        .await
        .map_err(SevenZipJobFailure::Interrupted)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(SevenZipJobFailure::Spawn)?;
    let mut stdout = child.stdout.take().expect("7z stdout is piped");
    let stderr_text = collect_output(child.stderr.take().expect("7z stderr is piped"));
    let pid = child.id();
    let cancel = controls.cancellation_token();
    let mut progress_stream = SevenZipProgressStream::default();
    let mut buffer = [0_u8; 4096];
    let mut stdout_open = true;
    let mut run_state_open = true;

    let status = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(SevenZipJobFailure::Spawn)?,
            read = stdout.read(&mut buffer), if stdout_open => match read {
                Ok(0) | Err(_) => stdout_open = false,
                Ok(read) => progress_stream.push(&buffer[..read], &mut on_progress),
            },
            state = controls.run_state_changed(), if run_state_open => match state {
                Some(FileOperationRunState::Paused) => signal(pid, libc::SIGSTOP),
                Some(FileOperationRunState::Running) => signal(pid, libc::SIGCONT),
                Some(FileOperationRunState::ApplicationStopping) => {
                    let _ = child.kill().await;
                    return Err(SevenZipJobFailure::Interrupted(FileError::ApplicationStopping));
                }
                None => run_state_open = false,
            },
            _ = cancel.cancelled() => {
                // 被 SIGSTOP 停住的进程照样能被 SIGKILL 杀掉。
                let _ = child.kill().await;
                let error = controls.interruption().unwrap_or(FileError::Cancelled);
                return Err(SevenZipJobFailure::Interrupted(error));
            }
        }
    };
    let stderr = stderr_text.await.unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(SevenZipJobFailure::Exited { status, stderr })
    }
}

fn signal(pid: Option<u32>, signal: libc::c_int) {
    let Some(pid) = pid.and_then(|pid| libc::pid_t::try_from(pid).ok()) else {
        return;
    };
    // 进程可能刚好已经退出：kill 失败就是没有可停的进程，下一轮 wait 会收尾。
    unsafe {
        libc::kill(pid, signal);
    }
}

/// 管道里的进度流是“文字 + 一串 `\b` 擦除”，没有换行。按 `\b`、`\r`、`\n`
/// 切段；读到一半的段留到下一次。
#[derive(Default)]
struct SevenZipProgressStream {
    pending: Vec<u8>,
}

impl SevenZipProgressStream {
    fn push(&mut self, bytes: &[u8], on_progress: &mut impl FnMut(SevenZipProgress)) {
        for &byte in bytes {
            if matches!(byte, b'\x08' | b'\r' | b'\n') {
                if let Some(progress) =
                    parse_seven_zip_progress(&String::from_utf8_lossy(&self.pending))
                {
                    on_progress(progress);
                }
                self.pending.clear();
            } else {
                self.pending.push(byte);
            }
        }
    }
}

/// 解析一段进度：解压是 `  24% 2 - dir/file`，压缩是 `  41% 1 + dir/file`，
/// 还没完成任何条目时没有数字（`   8% + dir/file`）。扫描阶段的 `0M Scan`
/// 之类不带百分号的段返回 None。
pub(crate) fn parse_seven_zip_progress(segment: &str) -> Option<SevenZipProgress> {
    let (percent, rest) = segment.trim_start().split_once('%')?;
    let percent = percent
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|percent| *percent <= 100)?;
    let completed_items = rest
        .split_whitespace()
        .next()
        .and_then(|token| token.parse().ok())
        .unwrap_or(0);
    Some(SevenZipProgress {
        percent,
        completed_items,
    })
}

#[cfg(test)]
mod tests;
