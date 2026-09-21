use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::error::{SearchError, SearchResult};

mod zip_xml_text;

use zip_xml_text::{extract_zipped_xml_text, zipped_xml_document_kind, ZippedXmlDocumentKind};

const DEFAULT_EXTRACTION_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_ADDRESS_SPACE_BYTES: u64 = 44_000_000;
const STDERR_CAPTURE_LIMIT_BYTES: u64 = 64 * 1024;
const PIPE_READ_CHUNK_BYTES: usize = 8 * 1024;
const PIPE_READER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
struct ExtractionProcessLimits {
    wall_clock_timeout: Duration,
    max_stdout_bytes: u64,
    max_stderr_bytes: u64,
    max_address_space_bytes: u64,
}

impl ExtractionProcessLimits {
    fn production(wall_clock_timeout: Duration, max_stdout_bytes: u64) -> Self {
        Self {
            wall_clock_timeout,
            max_stdout_bytes,
            max_stderr_bytes: STDERR_CAPTURE_LIMIT_BYTES,
            max_address_space_bytes: DEFAULT_MAX_ADDRESS_SPACE_BYTES,
        }
    }
}

#[derive(Debug)]
struct CapturedPipeOutput {
    bytes: Vec<u8>,
    limit_exceeded: bool,
}

#[derive(Debug)]
enum ExtractionProcessCompletion {
    Finished {
        status: ExitStatus,
    },
    TimedOut,
    Cancelled,
    StdoutLimitExceeded,
    PipeReadFailed {
        stream_name: &'static str,
        message: String,
    },
    WaitFailed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionOutcome {
    pub status: ExtractionStatus,
    pub text: Option<String>,
}

impl ExtractionOutcome {
    pub fn text(text: String) -> Self {
        Self {
            status: ExtractionStatus::Indexed,
            text: Some(text),
        }
    }

    pub fn skipped(status: ExtractionStatus) -> Self {
        Self { status, text: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtractionStatus {
    Indexed,
    Disabled,
    Unsupported,
    TooLarge,
    NonUtf8,
    ReadFailed {
        message: String,
    },
    ToolUnavailable {
        tool: String,
    },
    ToolFailed {
        tool: String,
        message: String,
    },
    TimedOut {
        tool: String,
    },
    /// 已没有生产路径；保留是为了让旧索引库里存量行 JSON 仍能反序列化，
    /// 移除会让 daemon 打开旧库时把这些行的状态读成错误。
    ResourceBudgetExceeded {
        tool: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableContentStageState {
    Complete,
    Skipped,
}

impl ExtractionStatus {
    pub fn durable_content_stage_state(&self) -> DurableContentStageState {
        match self {
            Self::Indexed => DurableContentStageState::Complete,
            Self::Disabled
            | Self::Unsupported
            | Self::TooLarge
            | Self::NonUtf8
            | Self::ReadFailed { .. }
            | Self::ToolUnavailable { .. }
            | Self::ToolFailed { .. }
            | Self::TimedOut { .. }
            | Self::ResourceBudgetExceeded { .. } => DurableContentStageState::Skipped,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionPlan {
    pub max_output_bytes: u64,
    pub execution_mode: ExtractionExecutionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractionExecutionMode {
    PlainTextInProcess,
    IsolatedSubprocess {
        command: CommandSpec,
        timeout: Duration,
    },
    ZippedXmlTextInProcess {
        document_kind: ZippedXmlDocumentKind,
    },
    SkipNow {
        skip_reason: ExtractionStatus,
    },
}

pub fn plan_content_extraction(
    path: &Path,
    len: u64,
    max_extract_bytes: u64,
    content_indexing_enabled: bool,
) -> ExtractionPlan {
    if !content_indexing_enabled {
        return ExtractionPlan {
            max_output_bytes: max_extract_bytes,
            execution_mode: ExtractionExecutionMode::SkipNow {
                skip_reason: ExtractionStatus::Disabled,
            },
        };
    }

    if len > max_extract_bytes {
        return ExtractionPlan {
            max_output_bytes: max_extract_bytes,
            execution_mode: ExtractionExecutionMode::SkipNow {
                skip_reason: ExtractionStatus::TooLarge,
            },
        };
    }

    if is_plain_text_path(path) {
        return ExtractionPlan {
            max_output_bytes: max_extract_bytes,
            execution_mode: ExtractionExecutionMode::PlainTextInProcess,
        };
    }

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    let execution_mode = match extension.as_deref() {
        Some("pdf") => ExtractionExecutionMode::IsolatedSubprocess {
            command: CommandSpec {
                program: "pdftotext".to_owned(),
                args: vec!["{}".to_owned(), "-".to_owned()],
            },
            timeout: DEFAULT_EXTRACTION_TIMEOUT,
        },
        Some(extension) => match zipped_xml_document_kind(extension) {
            Some(document_kind) => {
                ExtractionExecutionMode::ZippedXmlTextInProcess { document_kind }
            }
            None => ExtractionExecutionMode::SkipNow {
                skip_reason: ExtractionStatus::Unsupported,
            },
        },
        None => ExtractionExecutionMode::SkipNow {
            skip_reason: ExtractionStatus::Unsupported,
        },
    };
    ExtractionPlan {
        max_output_bytes: max_extract_bytes,
        execution_mode,
    }
}

pub async fn execute_extraction_plan(
    path: &Path,
    extraction_plan: &ExtractionPlan,
) -> SearchResult<ExtractionOutcome> {
    let cancellation = CancellationToken::new();
    execute_extraction_plan_cancelled(path, extraction_plan, &cancellation).await
}

pub(crate) async fn execute_extraction_plan_cancelled(
    path: &Path,
    extraction_plan: &ExtractionPlan,
    cancellation: &CancellationToken,
) -> SearchResult<ExtractionOutcome> {
    if cancellation.is_cancelled() {
        return Err(SearchError::Cancelled);
    }

    match &extraction_plan.execution_mode {
        ExtractionExecutionMode::PlainTextInProcess => {
            extract_plain_text(path, extraction_plan.max_output_bytes).await
        }
        ExtractionExecutionMode::ZippedXmlTextInProcess { document_kind } => {
            // 与纯文本一致的进程内提取；输入文件已受 max_extract_bytes 上限保护，
            // 输出受 BoundedText 预算保护，无需子进程隔离。
            Ok(extract_zipped_xml_text(
                path,
                *document_kind,
                extraction_plan.max_output_bytes,
            )?)
        }
        ExtractionExecutionMode::IsolatedSubprocess { command, timeout } => {
            extract_with_system_command_cancelled(
                path,
                command.clone(),
                ExtractionProcessLimits::production(*timeout, extraction_plan.max_output_bytes),
                cancellation,
            )
            .await
        }
        ExtractionExecutionMode::SkipNow { skip_reason } => {
            Ok(ExtractionOutcome::skipped(skip_reason.clone()))
        }
    }
}

pub async fn extract_content(
    path: &Path,
    len: u64,
    max_extract_bytes: u64,
    content_indexing_enabled: bool,
) -> SearchResult<ExtractionOutcome> {
    let extraction_plan =
        plan_content_extraction(path, len, max_extract_bytes, content_indexing_enabled);
    execute_extraction_plan(path, &extraction_plan).await
}

pub async fn extract_with_system_command(
    path: &Path,
    command: CommandSpec,
    timeout: Duration,
    max_output_bytes: u64,
) -> SearchResult<ExtractionOutcome> {
    let cancellation = CancellationToken::new();
    extract_with_system_command_cancelled(
        path,
        command,
        ExtractionProcessLimits::production(timeout, max_output_bytes),
        &cancellation,
    )
    .await
}

async fn extract_with_system_command_cancelled(
    path: &Path,
    command: CommandSpec,
    limits: ExtractionProcessLimits,
    cancellation: &CancellationToken,
) -> SearchResult<ExtractionOutcome> {
    let tool_name = command.program.clone();
    let mut process = Command::new(&command.program);
    for arg in &command.args {
        process.arg(if arg == "{}" {
            path.as_os_str().to_owned()
        } else {
            arg.into()
        });
    }
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_extraction_process(&mut process, limits.max_address_space_bytes);

    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ExtractionOutcome::skipped(
                ExtractionStatus::ToolUnavailable { tool: tool_name },
            ));
        }
        Err(source) => {
            return Err(SearchError::WorkerFailed(format!(
                "could not start {tool_name} extraction process: {source}"
            )));
        }
    };
    let process_group_id = child.id();

    let Some(stdout) = child.stdout.take() else {
        terminate_spawned_process(&mut child, process_group_id, &tool_name).await?;
        return Err(SearchError::WorkerFailed(format!(
            "{tool_name} extraction process did not expose stdout"
        )));
    };
    let Some(stderr) = child.stderr.take() else {
        drop(stdout);
        terminate_spawned_process(&mut child, process_group_id, &tool_name).await?;
        return Err(SearchError::WorkerFailed(format!(
            "{tool_name} extraction process did not expose stderr"
        )));
    };

    let mut stdout_task = tokio::spawn(read_stdout_limited(stdout, limits.max_stdout_bytes));
    let mut stderr_task = tokio::spawn(read_stderr_limited(stderr, limits.max_stderr_bytes));
    let mut stdout_result: Option<Result<CapturedPipeOutput, String>> = None;
    let mut stderr_result: Option<Result<CapturedPipeOutput, String>> = None;
    let deadline = time::sleep(limits.wall_clock_timeout);
    tokio::pin!(deadline);

    let completion = loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                break ExtractionProcessCompletion::Cancelled;
            }
            _ = &mut deadline => {
                break ExtractionProcessCompletion::TimedOut;
            }
            task_result = &mut stdout_task, if stdout_result.is_none() => {
                let pipe_result = normalize_pipe_task_result(task_result, "stdout");
                match &pipe_result {
                    Ok(output) if output.limit_exceeded => {
                        stdout_result = Some(pipe_result);
                        break ExtractionProcessCompletion::StdoutLimitExceeded;
                    }
                    Ok(_) => stdout_result = Some(pipe_result),
                    Err(message) => {
                        let message = message.clone();
                        stdout_result = Some(pipe_result);
                        break ExtractionProcessCompletion::PipeReadFailed {
                            stream_name: "stdout",
                            message,
                        };
                    }
                }
            }
            task_result = &mut stderr_task, if stderr_result.is_none() => {
                let pipe_result = normalize_pipe_task_result(task_result, "stderr");
                match &pipe_result {
                    Ok(_) => stderr_result = Some(pipe_result),
                    Err(message) => {
                        let message = message.clone();
                        stderr_result = Some(pipe_result);
                        break ExtractionProcessCompletion::PipeReadFailed {
                            stream_name: "stderr",
                            message,
                        };
                    }
                }
            }
            wait_result = child.wait(), if stdout_result.is_some() && stderr_result.is_some() => {
                match wait_result {
                    Ok(status) => {
                        break ExtractionProcessCompletion::Finished { status };
                    }
                    Err(source) => {
                        break ExtractionProcessCompletion::WaitFailed {
                            message: source.to_string(),
                        };
                    }
                }
            }
        }
    };

    if let ExtractionProcessCompletion::Finished { status } = &completion {
        let stdout_output = completed_pipe_output(stdout_result, "stdout", &tool_name)?;
        let stderr_output = completed_pipe_output(stderr_result, "stderr", &tool_name)?;
        return Ok(outcome_from_completed_process(
            tool_name,
            *status,
            stdout_output,
            stderr_output,
            limits.max_stderr_bytes,
        ));
    }

    let cleanup_result = terminate_extraction_process(&mut child, process_group_id).await;
    if let Err(source) = cleanup_result {
        abort_unfinished_pipe_task(&mut stdout_task, stdout_result.is_none()).await;
        abort_unfinished_pipe_task(&mut stderr_task, stderr_result.is_none()).await;
        return Err(SearchError::WorkerFailed(format!(
            "could not terminate {tool_name} extraction process: {source}"
        )));
    }

    let stdout_cleanup_result =
        finish_uncompleted_pipe_task(&mut stdout_task, &mut stdout_result, "stdout").await;
    let stderr_cleanup_result =
        finish_uncompleted_pipe_task(&mut stderr_task, &mut stderr_result, "stderr").await;
    if let Err(message) = stdout_cleanup_result.or(stderr_cleanup_result) {
        return Err(SearchError::WorkerFailed(format!(
            "could not finish {tool_name} extraction pipe cleanup: {message}"
        )));
    }

    match completion {
        ExtractionProcessCompletion::TimedOut => {
            Ok(ExtractionOutcome::skipped(ExtractionStatus::TimedOut {
                tool: tool_name,
            }))
        }
        ExtractionProcessCompletion::Cancelled => Err(SearchError::Cancelled),
        ExtractionProcessCompletion::StdoutLimitExceeded => {
            Ok(ExtractionOutcome::skipped(ExtractionStatus::TooLarge))
        }
        ExtractionProcessCompletion::PipeReadFailed {
            stream_name,
            message,
        } => Err(SearchError::WorkerFailed(format!(
            "could not read {stream_name} from {tool_name}: {message}"
        ))),
        ExtractionProcessCompletion::WaitFailed { message } => Err(SearchError::WorkerFailed(
            format!("could not wait for {tool_name} extraction process: {message}"),
        )),
        ExtractionProcessCompletion::Finished { .. } => unreachable!("handled above"),
    }
}

fn configure_extraction_process(process: &mut Command, max_address_space_bytes: u64) {
    process.as_std_mut().process_group(0);

    let address_space_limit = max_address_space_bytes as libc::rlim_t;
    let expected_parent_process_id = unsafe { libc::getpid() };
    // The closure runs after fork and before exec, so it must only call async-signal-safe libc APIs.
    unsafe {
        process.as_std_mut().pre_exec(move || {
            let resource_limit = libc::rlimit {
                rlim_cur: address_space_limit,
                rlim_max: address_space_limit,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &resource_limit) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != expected_parent_process_id {
                return Err(io::Error::from_raw_os_error(libc::EPIPE));
            }
            Ok(())
        });
    }
}

async fn read_stdout_limited(
    mut stdout: ChildStdout,
    max_output_bytes: u64,
) -> io::Result<CapturedPipeOutput> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; PIPE_READ_CHUNK_BYTES];
    loop {
        let read_count = stdout.read(&mut chunk).await?;
        if read_count == 0 {
            return Ok(CapturedPipeOutput {
                bytes,
                limit_exceeded: false,
            });
        }

        let remaining_bytes = max_output_bytes.saturating_sub(bytes.len() as u64);
        let retained_bytes = (read_count as u64).min(remaining_bytes) as usize;
        bytes.extend_from_slice(&chunk[..retained_bytes]);
        if retained_bytes < read_count {
            return Ok(CapturedPipeOutput {
                bytes,
                limit_exceeded: true,
            });
        }
    }
}

async fn read_stderr_limited(
    mut stderr: ChildStderr,
    max_output_bytes: u64,
) -> io::Result<CapturedPipeOutput> {
    let mut bytes = Vec::new();
    let mut limit_exceeded = false;
    let mut chunk = [0_u8; PIPE_READ_CHUNK_BYTES];
    loop {
        let read_count = stderr.read(&mut chunk).await?;
        if read_count == 0 {
            return Ok(CapturedPipeOutput {
                bytes,
                limit_exceeded,
            });
        }

        let remaining_bytes = max_output_bytes.saturating_sub(bytes.len() as u64);
        let retained_bytes = (read_count as u64).min(remaining_bytes) as usize;
        bytes.extend_from_slice(&chunk[..retained_bytes]);
        limit_exceeded |= retained_bytes < read_count;
    }
}

fn normalize_pipe_task_result(
    task_result: Result<io::Result<CapturedPipeOutput>, tokio::task::JoinError>,
    stream_name: &'static str,
) -> Result<CapturedPipeOutput, String> {
    match task_result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(source)) => Err(source.to_string()),
        Err(source) => Err(format!("{stream_name} reader task failed: {source}")),
    }
}

fn completed_pipe_output(
    output: Option<Result<CapturedPipeOutput, String>>,
    stream_name: &'static str,
    tool_name: &str,
) -> SearchResult<CapturedPipeOutput> {
    match output.expect("finished extraction must have completed pipe readers") {
        Ok(output) => Ok(output),
        Err(message) => Err(SearchError::WorkerFailed(format!(
            "could not read {stream_name} from {tool_name}: {message}"
        ))),
    }
}

fn outcome_from_completed_process(
    tool_name: String,
    status: ExitStatus,
    stdout: CapturedPipeOutput,
    stderr: CapturedPipeOutput,
    max_stderr_bytes: u64,
) -> ExtractionOutcome {
    if stdout.limit_exceeded {
        return ExtractionOutcome::skipped(ExtractionStatus::TooLarge);
    }
    if !status.success() {
        return ExtractionOutcome::skipped(ExtractionStatus::ToolFailed {
            tool: tool_name,
            message: tool_failure_message(status, stderr, max_stderr_bytes),
        });
    }
    match String::from_utf8(stdout.bytes) {
        Ok(text) => ExtractionOutcome::text(text),
        Err(_) => ExtractionOutcome::skipped(ExtractionStatus::NonUtf8),
    }
}

fn tool_failure_message(
    status: ExitStatus,
    stderr: CapturedPipeOutput,
    max_stderr_bytes: u64,
) -> String {
    let stderr_text = String::from_utf8_lossy(&stderr.bytes).trim().to_owned();
    let mut message = if stderr_text.is_empty() {
        format!("process exited with {status}")
    } else {
        stderr_text
    };
    if stderr.limit_exceeded {
        message.push_str(&format!(
            "\n[stderr truncated after {max_stderr_bytes} bytes]"
        ));
    }
    message
}

async fn terminate_spawned_process(
    child: &mut Child,
    process_group_id: Option<u32>,
    tool_name: &str,
) -> SearchResult<()> {
    terminate_extraction_process(child, process_group_id)
        .await
        .map_err(|source| {
            SearchError::WorkerFailed(format!(
                "could not terminate {tool_name} extraction process: {source}"
            ))
        })
}

async fn terminate_extraction_process(
    child: &mut Child,
    process_group_id: Option<u32>,
) -> io::Result<()> {
    let Some(process_group_id) = process_group_id else {
        return child.kill().await;
    };

    let process_group_id = i32::try_from(process_group_id).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "extractor process ID does not fit in a Unix process-group ID",
        )
    })?;
    let signal_result = unsafe { libc::kill(-process_group_id, libc::SIGKILL) };
    let signal_error = if signal_result == 0 {
        None
    } else {
        let source = io::Error::last_os_error();
        (source.raw_os_error() != Some(libc::ESRCH)).then_some(source)
    };

    if signal_error.is_some() {
        child.kill().await?;
    } else {
        child.wait().await?;
    }

    match signal_error {
        Some(source) => Err(source),
        None => Ok(()),
    }
}

async fn finish_uncompleted_pipe_task(
    task: &mut JoinHandle<io::Result<CapturedPipeOutput>>,
    output: &mut Option<Result<CapturedPipeOutput, String>>,
    stream_name: &'static str,
) -> Result<(), String> {
    if output.is_none() {
        let task_result = match time::timeout(PIPE_READER_CLEANUP_TIMEOUT, &mut *task).await {
            Ok(task_result) => task_result,
            Err(_) => {
                task.abort();
                let _ = task.await;
                return Err(format!(
                    "{stream_name} reader did not stop after process exit"
                ));
            }
        };
        *output = Some(normalize_pipe_task_result(task_result, stream_name));
    }
    Ok(())
}

async fn abort_unfinished_pipe_task(
    task: &mut JoinHandle<io::Result<CapturedPipeOutput>>,
    task_is_unfinished: bool,
) {
    if task_is_unfinished {
        task.abort();
        let _ = task.await;
    }
}

async fn extract_plain_text(path: &Path, max_output_bytes: u64) -> SearchResult<ExtractionOutcome> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(source) => return classify_plain_text_read_error(path, source),
    };
    let mut bytes = Vec::new();
    let mut limited_file = file.take(max_output_bytes.saturating_add(1));
    let read_outcome = limited_file.read_to_end(&mut bytes).await;
    release_plain_text_file_pages(limited_file.get_ref());
    if let Err(source) = read_outcome {
        return classify_plain_text_read_error(path, source);
    }
    if bytes.len() as u64 > max_output_bytes {
        return Ok(ExtractionOutcome::skipped(ExtractionStatus::TooLarge));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(ExtractionOutcome::text(text)),
        Err(_) => Ok(ExtractionOutcome::skipped(ExtractionStatus::NonUtf8)),
    }
}

fn release_plain_text_file_pages(file: &tokio::fs::File) {
    // 正文已经复制进有界 Vec；保留源文件干净页只会抬高 service cgroup 的空闲工作集。
    let _ = unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
}

fn classify_plain_text_read_error(
    path: &Path,
    source: io::Error,
) -> SearchResult<ExtractionOutcome> {
    if source.kind() == io::ErrorKind::PermissionDenied {
        Err(SearchError::Inaccessible {
            path: path.to_path_buf(),
            source,
        })
    } else {
        Ok(ExtractionOutcome::skipped(ExtractionStatus::ReadFailed {
            message: source.to_string(),
        }))
    }
}

fn is_plain_text_path(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "rs"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "html"
            | "css"
            | "js"
            | "ts"
            | "py"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "java"
            | "go"
            | "sh"
            | "log"
    )
}

#[cfg(test)]
#[path = "extractor/tests.rs"]
mod tests;
