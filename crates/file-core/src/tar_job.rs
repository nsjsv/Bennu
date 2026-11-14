//! 系统 tar 子进程：列目录、整包解压、单文件解出、压缩共用。需要 GNU tar（主流发行版默认）。
//! 解压和压缩时数据都经本进程转一手：进度按转过的字节算，暂停就是停止转送——tar 读不到
//! 数据或写不出去，自然停在那里。

use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;

use crate::{FileError, FileOperationControls};

const PUMP_BUFFER_SIZE: usize = 256 * 1024;

pub(crate) fn tar_command() -> Command {
    let mut command = Command::new("tar");
    // C 区域设置下名字里的非 ASCII 字节一律写成 \ooo（UTF-8 区域设置下非法序列只转义一半，
    // 还原不回原来的字节）；时间按 UTC 打印，解析时不必猜本地时区。
    command
        .env("LC_ALL", "C")
        .env("TZ", "UTC0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// tar 的报错原样给用户（解压缩器没装时也是 tar 说明白缺什么）。
pub(crate) fn tar_failure(archive: &Path, status: ExitStatus, stderr: &[u8]) -> FileError {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    FileError::Archive {
        path: archive.to_path_buf(),
        message: if stderr.is_empty() {
            format!("tar exited with status {status}")
        } else {
            stderr
        },
    }
}

#[derive(Debug)]
pub(crate) enum PumpFailure {
    /// 取消或关机。
    Interrupted(FileError),
    Read(io::Error),
    /// 对面的进程不再读了：出错退出（原因在它的 stderr 里），或数据已经够了提前结束
    /// （tar 读到包尾的结束块就退出，后面的补齐字节送不进去）。看它的退出码定成败。
    Write,
}

/// 把 `reader` 读完、全部写进 `writer`。每块之前等运行状态；写的时候被卡住（对面处理不过来）
/// 也能被取消打断。`on_progress` 收到累计字节，最多每 `PROGRESS_REPORT_INTERVAL` 一次。
pub(crate) async fn pump_with_controls(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    controls: &mut FileOperationControls,
    mut on_progress: impl FnMut(u64),
) -> Result<u64, PumpFailure> {
    let cancel = controls.cancellation_token();
    let mut buffer = vec![0_u8; PUMP_BUFFER_SIZE];
    let mut pumped = 0_u64;
    let mut last_sent_at: Option<Instant> = None;
    loop {
        controls
            .wait_until_running()
            .await
            .map_err(PumpFailure::Interrupted)?;
        let read = reader.read(&mut buffer).await.map_err(PumpFailure::Read)?;
        if read == 0 {
            break;
        }
        tokio::select! {
            written = writer.write_all(&buffer[..read]) => written.map_err(|_| PumpFailure::Write)?,
            _ = cancel.cancelled() => {
                return Err(PumpFailure::Interrupted(
                    controls.interruption().unwrap_or(FileError::Cancelled),
                ));
            }
        }
        pumped += read as u64;
        let now = Instant::now();
        if last_sent_at
            .is_none_or(|sent| now.duration_since(sent) >= crate::PROGRESS_REPORT_INTERVAL)
        {
            last_sent_at = Some(now);
            on_progress(pumped);
        }
    }
    writer.flush().await.map_err(|_| PumpFailure::Write)?;
    Ok(pumped)
}

/// stderr 必须并发读尽：管道塞满时子进程会卡在写 stderr 上，和这边的 wait 互相等死。
pub(crate) fn collect_output(
    mut stream: impl AsyncRead + Unpin + Send + 'static,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes).await;
        bytes
    })
}
