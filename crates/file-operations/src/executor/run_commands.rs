//! 动作里的"执行命令"：一条接一条执行外部命令，不经过 shell。
//!
//! 每条命令放进它自己的进程组，暂停 / 继续 / 取消对整组发信号，命令自己再起的子进程也跟着停：
//! 暂停 SIGSTOP、继续 SIGCONT；取消先 SIGTERM，给一点时间收尾，再 SIGKILL 整组。

use std::ffi::OsStr;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use file_core::{FileError, FileOperationControls, FileOperationRunState};
use tokio::process::Command;

use crate::{
    CommandInvocation, FileOperationEvent, FileOperationEventSender, FileOperationOutcome,
    FileOperationProgressUpdate,
};

/// 失败说明里保留的输出末尾。
const OUTPUT_TAIL_BYTES: usize = 4 * 1024;
/// 失败说明里最多放几行输出。
const OUTPUT_TAIL_LINES: usize = 5;
/// SIGTERM 之后等它自己退出多久，再 SIGKILL 整组。
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

pub(super) async fn run_queued_commands(
    invocations: Vec<CommandInvocation>,
    mut controls: FileOperationControls,
    task_id: u64,
    output: &FileOperationEventSender,
) -> Result<FileOperationOutcome, String> {
    let total = invocations.len();
    for (index, invocation) in invocations.iter().enumerate() {
        run_command(invocation, &mut controls)
            .await
            .map_err(|failure| failure.to_string())?;
        let _ = output
            .send(FileOperationEvent::Progressed {
                task_id,
                update: FileOperationProgressUpdate::IndeterminateItems {
                    completed: index + 1,
                    total,
                },
                entries: Vec::new(),
            })
            .await;
    }
    Ok(FileOperationOutcome::NoHistory)
}

#[derive(Debug)]
enum CommandFailure {
    Interrupted(FileError),
    Start {
        program: String,
        error: std::io::Error,
    },
    Exited {
        program: String,
        status: ExitStatus,
        output: Vec<u8>,
    },
}

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Interrupted(error) => error.to_string(),
            Self::Start { program, error } => tr!("Could not start %1: %2", program, error),
            Self::Exited {
                program,
                status,
                output,
            } => {
                let detail = last_lines(output);
                let ended = match (
                    status.code(),
                    std::os::unix::process::ExitStatusExt::signal(status),
                ) {
                    (Some(code), _) => tr!("%1 exited with status %2", program, code),
                    (None, Some(signal)) => tr!("%1 was stopped by signal %2", program, signal),
                    (None, None) => tr!("%1 failed", program),
                };
                if detail.is_empty() {
                    ended
                } else {
                    format!("{ended}: {detail}")
                }
            }
        };
        formatter.write_str(&text)
    }
}

async fn run_command(
    invocation: &CommandInvocation,
    controls: &mut FileOperationControls,
) -> Result<(), CommandFailure> {
    controls
        .wait_until_running()
        .await
        .map_err(CommandFailure::Interrupted)?;
    let program_name = display_name(&invocation.program);
    let mut child = Command::new(&invocation.program)
        .args(&invocation.arguments)
        .current_dir(&invocation.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| CommandFailure::Start {
            program: program_name.clone(),
            error,
        })?;
    let group = child.id().map(|pid| pid as libc::pid_t);
    let stdout = tokio::spawn(bennu_subprocess::read_child_output_tail(
        child.stdout.take().expect("stdout is piped"),
        OUTPUT_TAIL_BYTES,
    ));
    let stderr = tokio::spawn(bennu_subprocess::read_child_output_tail(
        child.stderr.take().expect("stderr is piped"),
        OUTPUT_TAIL_BYTES,
    ));
    let cancel = controls.cancellation_token();
    let mut run_state_open = true;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(|error| CommandFailure::Start {
                program: program_name.clone(),
                error,
            })?,
            state = controls.run_state_changed(), if run_state_open => match state {
                Some(FileOperationRunState::Paused) => signal_group(group, libc::SIGSTOP),
                Some(FileOperationRunState::Running) => signal_group(group, libc::SIGCONT),
                Some(FileOperationRunState::ApplicationStopping) => {
                    terminate_group(group, &mut child).await;
                    return Err(CommandFailure::Interrupted(FileError::ApplicationStopping));
                }
                None => run_state_open = false,
            },
            _ = cancel.cancelled() => {
                terminate_group(group, &mut child).await;
                let error = controls.interruption().unwrap_or(FileError::Cancelled);
                return Err(CommandFailure::Interrupted(error));
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    // 出错信息一般写在 stderr；没写时看 stdout。
    let stderr = stderr.await.ok().and_then(Result::ok).unwrap_or_default();
    let output = if stderr.iter().any(|byte| !byte.is_ascii_whitespace()) {
        stderr
    } else {
        stdout.await.ok().and_then(Result::ok).unwrap_or_default()
    };
    Err(CommandFailure::Exited {
        program: program_name,
        status,
        output,
    })
}

fn signal_group(group: Option<libc::pid_t>, signal: libc::c_int) {
    if let Some(group) = group {
        // 进程组已经没了（ESRCH）不算错：命令刚好结束。
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

/// 停住的进程收不到 SIGTERM，先让它继续；等它自己退出一会儿，再把整组杀掉，
/// 命令留下的后台子进程也不放过。
async fn terminate_group(group: Option<libc::pid_t>, child: &mut tokio::process::Child) {
    signal_group(group, libc::SIGCONT);
    signal_group(group, libc::SIGTERM);
    let _ = tokio::time::timeout(TERMINATION_GRACE, child.wait()).await;
    signal_group(group, libc::SIGKILL);
    let _ = child.wait().await;
}

fn display_name(program: &OsStr) -> String {
    std::path::Path::new(program)
        .file_name()
        .unwrap_or(program)
        .to_string_lossy()
        .into_owned()
}

fn last_lines(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    lines[lines.len().saturating_sub(OUTPUT_TAIL_LINES)..].join(" / ")
}

#[cfg(test)]
#[path = "run_commands_tests.rs"]
mod tests;
