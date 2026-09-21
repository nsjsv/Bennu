use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::commands::bounded_child_output::{read_bounded_child_output, BoundedChildOutput};

const EXTERNAL_DIAGNOSTIC_CHAR_LIMIT: usize = 512;
const PIPE_READER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) struct DocumentToolCommand<'a> {
    pub(super) program: &'a Path,
    pub(super) arguments: &'a [OsString],
    pub(super) environment: &'a [(OsString, OsString)],
    pub(super) label: &'a str,
    pub(super) timeout: Duration,
    pub(super) stdout_limit: usize,
    pub(super) stderr_limit: usize,
    pub(super) document_cancellation: &'a CancellationToken,
    pub(super) render_cancellation: Option<&'a CancellationToken>,
}

#[derive(Debug)]
pub(super) struct DocumentToolOutput {
    pub(super) stdout: BoundedChildOutput,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DocumentToolError {
    NotFound,
    Failed(String),
}

impl DocumentToolError {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::NotFound => "document preview tool was not found".to_owned(),
            Self::Failed(message) => message,
        }
    }
}

enum DocumentToolCompletion {
    Finished {
        status: ExitStatus,
    },
    Cancelled,
    TimedOut,
    PipeReadFailed {
        stream_name: &'static str,
        message: String,
    },
    WaitFailed(String),
}

pub(super) async fn run_document_tool_command(
    request: DocumentToolCommand<'_>,
) -> Result<Option<DocumentToolOutput>, DocumentToolError> {
    if request.document_cancellation.is_cancelled()
        || request
            .render_cancellation
            .is_some_and(CancellationToken::is_cancelled)
    {
        return Ok(None);
    }
    let mut command = Command::new(request.program);
    command
        .args(request.arguments)
        .envs(request.environment.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // 子进程可能留下继承 pipe 的后代；独立进程组让所有异常出口都能一次终止完整命令树。
    command.as_std_mut().process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(DocumentToolError::NotFound)
        }
        Err(error) => {
            return Err(DocumentToolError::Failed(format!(
                "Could not start {}: {error}",
                request.label
            )))
        }
    };
    let process_group_id = child.id();
    let Some(stdout) = child.stdout.take() else {
        terminate_spawned_process(&mut child, process_group_id, request.label).await?;
        return Err(DocumentToolError::Failed(format!(
            "Could not capture {} stdout",
            request.label
        )));
    };
    let Some(stderr) = child.stderr.take() else {
        drop(stdout);
        terminate_spawned_process(&mut child, process_group_id, request.label).await?;
        return Err(DocumentToolError::Failed(format!(
            "Could not capture {} stderr",
            request.label
        )));
    };

    let mut stdout_task = tokio::spawn(read_bounded_child_output(stdout, request.stdout_limit));
    let mut stderr_task = tokio::spawn(read_bounded_child_output(stderr, request.stderr_limit));
    let mut stdout_result: Option<Result<BoundedChildOutput, String>> = None;
    let mut stderr_result: Option<Result<BoundedChildOutput, String>> = None;
    let deadline = time::sleep(request.timeout);
    tokio::pin!(deadline);
    let cancellation = wait_for_document_tool_cancellation(
        request.document_cancellation,
        request.render_cancellation,
    );
    tokio::pin!(cancellation);

    let completion = loop {
        tokio::select! {
            biased;
            _ = &mut cancellation => break DocumentToolCompletion::Cancelled,
            _ = &mut deadline => break DocumentToolCompletion::TimedOut,
            task_result = &mut stdout_task, if stdout_result.is_none() => {
                let pipe_result = normalize_pipe_task_result(task_result, "stdout");
                if let Err(message) = &pipe_result {
                    let message = message.clone();
                    stdout_result = Some(pipe_result);
                    break DocumentToolCompletion::PipeReadFailed {
                        stream_name: "stdout",
                        message,
                    };
                }
                stdout_result = Some(pipe_result);
            }
            task_result = &mut stderr_task, if stderr_result.is_none() => {
                let pipe_result = normalize_pipe_task_result(task_result, "stderr");
                if let Err(message) = &pipe_result {
                    let message = message.clone();
                    stderr_result = Some(pipe_result);
                    break DocumentToolCompletion::PipeReadFailed {
                        stream_name: "stderr",
                        message,
                    };
                }
                stderr_result = Some(pipe_result);
            }
            wait_result = child.wait(), if stdout_result.is_some() && stderr_result.is_some() => {
                break match wait_result {
                    Ok(status) => DocumentToolCompletion::Finished { status },
                    Err(error) => DocumentToolCompletion::WaitFailed(error.to_string()),
                };
            }
        }
    };

    if let DocumentToolCompletion::Finished { status } = &completion {
        return completed_document_tool_output(
            *status,
            stdout_result,
            stderr_result,
            request.label,
        );
    }

    let termination_result = terminate_document_tool_process(&mut child, process_group_id).await;
    let stdout_cleanup =
        finish_uncompleted_pipe_task(&mut stdout_task, &mut stdout_result, "stdout").await;
    let stderr_cleanup =
        finish_uncompleted_pipe_task(&mut stderr_task, &mut stderr_result, "stderr").await;
    if let Err(error) = termination_result {
        return Err(DocumentToolError::Failed(format!(
            "Could not terminate {} process group: {error}",
            request.label
        )));
    }
    if let Err(message) = stdout_cleanup.or(stderr_cleanup) {
        return Err(DocumentToolError::Failed(format!(
            "Could not finish {} pipe cleanup: {message}",
            request.label
        )));
    }

    match completion {
        DocumentToolCompletion::Cancelled => Ok(None),
        DocumentToolCompletion::TimedOut => Err(DocumentToolError::Failed(format!(
            "{} timed out",
            request.label
        ))),
        DocumentToolCompletion::PipeReadFailed {
            stream_name,
            message,
        } => Err(DocumentToolError::Failed(format!(
            "Could not read {} {stream_name}: {message}",
            request.label
        ))),
        DocumentToolCompletion::WaitFailed(message) => Err(DocumentToolError::Failed(format!(
            "Could not wait for {}: {message}",
            request.label
        ))),
        DocumentToolCompletion::Finished { .. } => unreachable!("handled above"),
    }
}

async fn wait_for_document_tool_cancellation(
    document_cancellation: &CancellationToken,
    render_cancellation: Option<&CancellationToken>,
) {
    if let Some(render_cancellation) = render_cancellation {
        tokio::select! {
            _ = document_cancellation.cancelled() => {}
            _ = render_cancellation.cancelled() => {}
        }
    } else {
        document_cancellation.cancelled().await;
    }
}

fn completed_document_tool_output(
    status: ExitStatus,
    stdout: Option<Result<BoundedChildOutput, String>>,
    stderr: Option<Result<BoundedChildOutput, String>>,
    label: &str,
) -> Result<Option<DocumentToolOutput>, DocumentToolError> {
    let stdout = completed_pipe_output(stdout, "stdout", label)?;
    let stderr = completed_pipe_output(stderr, "stderr", label)?;
    if stdout.exceeded_limit {
        return Err(DocumentToolError::Failed(format!(
            "{label} stdout exceeded the safety limit"
        )));
    }
    if stderr.exceeded_limit {
        return Err(DocumentToolError::Failed(format!(
            "{label} stderr exceeded the safety limit"
        )));
    }
    if !status.success() {
        let detail = external_diagnostic(&stderr.bytes)
            .or_else(|| external_diagnostic(&stdout.bytes))
            .unwrap_or_else(|| status.to_string());
        return Err(DocumentToolError::Failed(format!(
            "{label} failed: {detail}"
        )));
    }
    Ok(Some(DocumentToolOutput { stdout }))
}

fn normalize_pipe_task_result(
    task_result: Result<io::Result<BoundedChildOutput>, tokio::task::JoinError>,
    stream_name: &'static str,
) -> Result<BoundedChildOutput, String> {
    match task_result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(format!("{stream_name} reader task failed: {error}")),
    }
}

fn completed_pipe_output(
    output: Option<Result<BoundedChildOutput, String>>,
    stream_name: &'static str,
    label: &str,
) -> Result<BoundedChildOutput, DocumentToolError> {
    match output.expect("finished document tool must have completed pipe readers") {
        Ok(output) => Ok(output),
        Err(message) => Err(DocumentToolError::Failed(format!(
            "Could not read {label} {stream_name}: {message}"
        ))),
    }
}

async fn terminate_spawned_process(
    child: &mut Child,
    process_group_id: Option<u32>,
    label: &str,
) -> Result<(), DocumentToolError> {
    terminate_document_tool_process(child, process_group_id)
        .await
        .map_err(|error| {
            DocumentToolError::Failed(format!(
                "Could not terminate {label} process group: {error}"
            ))
        })
}

async fn terminate_document_tool_process(
    child: &mut Child,
    process_group_id: Option<u32>,
) -> io::Result<()> {
    let Some(process_group_id) = process_group_id else {
        return child.kill().await;
    };
    let process_group_id = i32::try_from(process_group_id).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "document tool process ID does not fit in a Unix process-group ID",
        )
    })?;
    let signal_result = unsafe { libc::kill(-process_group_id, libc::SIGKILL) };
    let signal_error = if signal_result == 0 {
        None
    } else {
        let error = io::Error::last_os_error();
        (error.raw_os_error() != Some(libc::ESRCH)).then_some(error)
    };

    if signal_error.is_some() {
        child.kill().await?;
    } else {
        child.wait().await?;
    }
    match signal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn finish_uncompleted_pipe_task(
    task: &mut JoinHandle<io::Result<BoundedChildOutput>>,
    output: &mut Option<Result<BoundedChildOutput, String>>,
    stream_name: &'static str,
) -> Result<(), String> {
    if output.is_none() {
        let task_result = match time::timeout(PIPE_READER_CLEANUP_TIMEOUT, &mut *task).await {
            Ok(task_result) => task_result,
            Err(_) => {
                task.abort();
                let _ = task.await;
                return Err(format!(
                    "{stream_name} reader did not stop after process group termination"
                ));
            }
        };
        *output = Some(normalize_pipe_task_result(task_result, stream_name));
    }
    Ok(())
}

fn external_diagnostic(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut sanitized = text
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    sanitized = sanitized.trim().to_owned();
    if sanitized.is_empty() {
        return None;
    }
    if sanitized.chars().count() > EXTERNAL_DIAGNOSTIC_CHAR_LIMIT {
        sanitized = sanitized
            .chars()
            .take(EXTERNAL_DIAGNOSTIC_CHAR_LIMIT - 1)
            .collect();
        sanitized.push('…');
    }
    Some(sanitized)
}

#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
