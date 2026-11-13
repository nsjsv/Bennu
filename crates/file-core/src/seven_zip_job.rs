//! 7z 子进程的唯一运行入口：整包解压与走 7z 的压缩共用。进度、暂停、取消
//! 都在这里——子进程不经过本进程的检查点，只能由监督方替它响应运行状态。

use std::io;
use std::process::{ExitStatus, Stdio};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::{FileError, FileOperationControls, FileOperationRunState};

/// `-bsp1` 进度流里的一次更新。`completed_items` 是 7z 报的已完成条目数
/// （目录也算），为 0 时 7z 不写这个数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SevenZipProgress {
    pub(crate) percent: u8,
    pub(crate) completed_items: usize,
}

#[derive(Debug)]
pub(crate) enum SevenZipJobFailure {
    /// 找不到这个命令名，调用方换下一个（7z / 7zz / 7za）。
    CommandNotFound,
    Spawn(io::Error),
    /// 进程跑完但失败；stderr 原文交给调用方分类（密码错误等）。
    Exited {
        status: ExitStatus,
        stderr: String,
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
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                SevenZipJobFailure::CommandNotFound
            } else {
                SevenZipJobFailure::Spawn(error)
            }
        })?;
    let mut stdout = child.stdout.take().expect("7z stdout is piped");
    let mut stderr = child.stderr.take().expect("7z stderr is piped");
    // stderr 必须并发读尽：管道塞满时 7z 会卡在写 stderr 上，和这边的 wait 互相等死。
    let stderr_text = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes).await;
        String::from_utf8_lossy(&bytes).trim().to_owned()
    });
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
