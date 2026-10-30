use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::tempdir;
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::error::SearchError;

use super::{
    classify_plain_text_read_error, extract_content, extract_with_system_command,
    extract_with_system_command_cancelled, plan_content_extraction, CommandSpec,
    DurableContentStageState, ExtractionExecutionMode, ExtractionProcessLimits, ExtractionStatus,
    ZippedXmlDocumentKind, DEFAULT_EXTRACTION_TIMEOUT, DEFAULT_MAX_ADDRESS_SPACE_BYTES,
    STDERR_CAPTURE_LIMIT_BYTES,
};

#[test]
fn permission_denied_plain_text_read_is_inaccessible() {
    let path = Path::new("/tmp/private.txt");
    let error =
        classify_plain_text_read_error(path, io::Error::from(io::ErrorKind::PermissionDenied))
            .unwrap_err();

    assert!(matches!(
        error,
        SearchError::Inaccessible { path: observed_path, .. } if observed_path == path
    ));
}

#[test]
fn plans_plain_text_and_pdf_backends_explicitly() {
    let plain_text_plan = plan_content_extraction(Path::new("/tmp/note.txt"), 12, 1024, true);

    assert_eq!(
        plain_text_plan.execution_mode,
        ExtractionExecutionMode::PlainTextInProcess
    );

    let extraction_plan = plan_content_extraction(Path::new("/tmp/report.pdf"), 12, 1024, true);
    assert_eq!(
        extraction_plan.execution_mode,
        ExtractionExecutionMode::IsolatedSubprocess {
            command: CommandSpec {
                program: "pdftotext".to_owned(),
                args: vec!["{}".to_owned(), "-".to_owned()],
            },
            timeout: DEFAULT_EXTRACTION_TIMEOUT,
        }
    );
}

#[test]
fn office_documents_are_extracted_in_process_by_format() {
    let planned_kinds = [
        ("/tmp/report.docx", ZippedXmlDocumentKind::WordDocument),
        ("/tmp/workbook.xlsx", ZippedXmlDocumentKind::Spreadsheet),
        ("/tmp/slides.pptx", ZippedXmlDocumentKind::Presentation),
        ("/tmp/notes.odt", ZippedXmlDocumentKind::OpenDocumentText),
        ("/tmp/UPPERCASE.XLSX", ZippedXmlDocumentKind::Spreadsheet),
    ];
    for (document_path, document_kind) in planned_kinds {
        let extraction_plan = plan_content_extraction(Path::new(document_path), 12, 1024, true);
        assert_eq!(
            extraction_plan.execution_mode,
            ExtractionExecutionMode::ZippedXmlTextInProcess { document_kind },
            "{document_path} must be extracted in process"
        );
    }
}

#[test]
fn unsupported_office_formats_stay_unsupported() {
    for file_name in [
        "legacy.doc",
        "legacy.xls",
        "legacy.ppt",
        "spreadsheet.ods",
        "presentation.odp",
    ] {
        let extraction_plan = plan_content_extraction(Path::new(file_name), 12, 1024, true);
        assert_eq!(
            extraction_plan.execution_mode,
            ExtractionExecutionMode::SkipNow {
                skip_reason: ExtractionStatus::Unsupported,
            },
            "{file_name} must not be routed to a zipped-xml extractor"
        );
    }
}

#[test]
fn failed_statuses_map_to_durable_skipped_state() {
    assert_eq!(
        ExtractionStatus::Indexed.durable_content_stage_state(),
        DurableContentStageState::Complete
    );

    let skipped_statuses = [
        ExtractionStatus::Disabled,
        ExtractionStatus::Unsupported,
        ExtractionStatus::TooLarge,
        ExtractionStatus::NonUtf8,
        ExtractionStatus::ReadFailed {
            message: "permission denied".to_owned(),
        },
        ExtractionStatus::ToolUnavailable {
            tool: "pandoc".to_owned(),
        },
        ExtractionStatus::ToolFailed {
            tool: "pandoc".to_owned(),
            message: "boom".to_owned(),
        },
        ExtractionStatus::TimedOut {
            tool: "pdftotext".to_owned(),
        },
        ExtractionStatus::ResourceBudgetExceeded {
            tool: "pandoc".to_owned(),
        },
    ];

    for skipped_status in skipped_statuses {
        assert_eq!(
            skipped_status.durable_content_stage_state(),
            DurableContentStageState::Skipped
        );
    }
}

#[tokio::test]
async fn extracts_plain_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("note.txt");
    fs::write(&path, "hello search").unwrap();

    let outcome = extract_content(&path, 12, 1024, true).await.unwrap();

    assert_eq!(outcome.status, ExtractionStatus::Indexed);
    assert_eq!(outcome.text.as_deref(), Some("hello search"));
}

#[tokio::test]
async fn reports_non_utf8_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.txt");
    fs::write(&path, [0xff, 0xfe]).unwrap();

    let outcome = extract_content(&path, 2, 1024, true).await.unwrap();

    assert_eq!(outcome.status, ExtractionStatus::NonUtf8);
    assert!(outcome.text.is_none());
}

#[tokio::test]
async fn oversized_file_is_skipped_before_reading() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("large.txt");
    fs::write(&path, "hello").unwrap();

    let outcome = extract_content(&path, 5, 4, true).await.unwrap();

    assert_eq!(outcome.status, ExtractionStatus::TooLarge);
}

#[tokio::test]
async fn plain_text_read_stops_at_the_extraction_limit_after_file_growth() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("grew-after-stat.txt");
    fs::write(&path, "0123456789").unwrap();

    let outcome = extract_content(&path, 4, 4, true).await.unwrap();

    assert_eq!(outcome.status, ExtractionStatus::TooLarge);
    assert!(outcome.text.is_none());
}

#[tokio::test]
async fn missing_plain_text_file_is_skipped_instead_of_aborting_crawl() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("gone.txt");
    fs::write(&path, "hello").unwrap();
    fs::remove_file(&path).unwrap();

    let outcome = extract_content(&path, 5, 1024, true).await.unwrap();

    assert!(matches!(
        outcome.status,
        ExtractionStatus::ReadFailed { .. }
    ));
    assert!(outcome.text.is_none());
}

#[tokio::test]
async fn system_command_output_is_indexed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("document.pdf");
    fs::write(&path, b"unused").unwrap();

    let outcome = extract_with_system_command(
        &path,
        CommandSpec {
            program: "printf".to_owned(),
            args: vec!["document text".to_owned()],
        },
        Duration::from_secs(1),
        1024,
    )
    .await
    .unwrap();

    assert_eq!(outcome.status, ExtractionStatus::Indexed);
    assert_eq!(outcome.text.as_deref(), Some("document text"));
}

#[tokio::test]
async fn timeout_reaps_process_groups_before_returning() {
    let directory = tempdir().unwrap();
    let cancellation = CancellationToken::new();
    let limits = ExtractionProcessLimits::production(Duration::from_millis(500), 1024);
    let mut observed_process_ids = Vec::new();

    for sequence_number in 0..2 {
        let parent_pid_path = directory
            .path()
            .join(format!("timeout-{sequence_number}.pid"));
        let child_pid_path = PathBuf::from(format!("{}.child", parent_pid_path.display()));
        let outcome = extract_with_system_command_cancelled(
            &parent_pid_path,
            shell_command(
                "printf '%s' \"$$\" > \"$1\"; sleep 30 & printf '%s' \"$!\" > \"${1}.child\"; wait",
            ),
            limits,
            &cancellation,
        )
        .await
        .unwrap();

        assert_eq!(
            outcome.status,
            ExtractionStatus::TimedOut {
                tool: "sh".to_owned(),
            }
        );
        observed_process_ids.push(read_process_id(&parent_pid_path));
        observed_process_ids.push(read_process_id(&child_pid_path));
    }

    for process_id in observed_process_ids {
        assert!(
            !process_is_running(process_id),
            "timed-out extractor process {process_id} is still running"
        );
    }
}

#[tokio::test]
async fn timeout_keeps_group_identity_until_descendants_are_stopped() {
    let directory = tempdir().unwrap();
    let parent_pid_path = directory.path().join("exited-leader.pid");
    let child_pid_path = PathBuf::from(format!("{}.child", parent_pid_path.display()));
    let outcome = extract_with_system_command_cancelled(
        &parent_pid_path,
        shell_command(
            "printf '%s' \"$$\" > \"$1\"; sleep 30 & printf '%s' \"$!\" > \"${1}.child\"; exit 0",
        ),
        ExtractionProcessLimits::production(Duration::from_millis(500), 1024),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        outcome.status,
        ExtractionStatus::TimedOut {
            tool: "sh".to_owned(),
        }
    );
    for process_id in [
        read_process_id(&parent_pid_path),
        read_process_id(&child_pid_path),
    ] {
        assert!(
            !process_is_running(process_id),
            "extractor process {process_id} is still running after group cleanup"
        );
    }
}

#[tokio::test]
async fn cancellation_reaps_the_child_before_returning() {
    let directory = tempdir().unwrap();
    let pid_path = directory.path().join("cancelled.pid");
    let cancellation = CancellationToken::new();
    let cancellation_trigger = cancellation.clone();
    let observed_pid_path = pid_path.clone();
    let cancellation_task = tokio::spawn(async move {
        let process_id = wait_for_process_id(&observed_pid_path).await;
        cancellation_trigger.cancel();
        process_id
    });

    let result = extract_with_system_command_cancelled(
        &pid_path,
        shell_command("printf '%s' \"$$\" > \"$1\"; exec sleep 30"),
        ExtractionProcessLimits::production(Duration::from_secs(5), 1024),
        &cancellation,
    )
    .await;
    let process_id = cancellation_task.await.unwrap();

    assert!(matches!(result, Err(SearchError::Cancelled)));
    assert!(
        !process_is_running(process_id),
        "cancelled extractor process {process_id} is still running"
    );
}

#[tokio::test]
async fn stdout_overflow_stops_and_reaps_the_process() {
    let directory = tempdir().unwrap();
    let pid_path = directory.path().join("stdout-overflow.pid");
    let outcome = extract_with_system_command_cancelled(
        &pid_path,
        shell_command(
            "printf '%s' \"$$\" > \"$1\"; while true; do printf '0123456789abcdef'; done",
        ),
        ExtractionProcessLimits::production(Duration::from_secs(5), 1024),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let process_id = read_process_id(&pid_path);

    assert_eq!(outcome.status, ExtractionStatus::TooLarge);
    assert!(
        !process_is_running(process_id),
        "oversized-output extractor process {process_id} is still running"
    );
}

#[tokio::test]
async fn stderr_diagnostics_are_bounded_and_marked_as_truncated() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("stderr.txt");
    let outcome = extract_with_system_command_cancelled(
        &path,
        shell_command("yes diagnostic-line | head -c 70000 >&2; exit 7"),
        ExtractionProcessLimits::production(Duration::from_secs(5), 1024),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    let ExtractionStatus::ToolFailed { message, .. } = outcome.status else {
        panic!("expected bounded tool failure diagnostics");
    };
    assert!(message.contains("[stderr truncated after 65536 bytes]"));
    assert!(
        message.len() <= STDERR_CAPTURE_LIMIT_BYTES as usize + 64,
        "stderr diagnostic retained {} bytes",
        message.len()
    );
}

#[tokio::test]
async fn child_receives_the_configured_address_space_limit() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("limits.txt");
    let test_address_space_limit = 32_000_000;
    let outcome = extract_with_system_command_cancelled(
        &path,
        shell_command("ulimit -v"),
        ExtractionProcessLimits {
            wall_clock_timeout: Duration::from_secs(2),
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
            max_address_space_bytes: test_address_space_limit,
        },
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.status, ExtractionStatus::Indexed);
    let reported_limit_kibibytes = outcome
        .text
        .expect("shell should report its address-space limit")
        .trim()
        .parse::<u64>()
        .expect("address-space limit should be numeric");
    assert_eq!(reported_limit_kibibytes, test_address_space_limit / 1024);
    assert!(test_address_space_limit < DEFAULT_MAX_ADDRESS_SPACE_BYTES);
}

fn shell_command(script: &str) -> CommandSpec {
    CommandSpec {
        program: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            script.to_owned(),
            "file-search-extractor-test".to_owned(),
            "{}".to_owned(),
        ],
    }
}

async fn wait_for_process_id(path: &Path) -> libc::pid_t {
    for attempt_number in 0..200 {
        if path.is_file() {
            return read_process_id(path);
        }
        time::sleep(Duration::from_millis(10)).await;
        assert!(attempt_number < 199, "process ID file was not created");
    }
    unreachable!("loop either returns or fails its final assertion")
}

fn read_process_id(path: &Path) -> libc::pid_t {
    fs::read_to_string(path)
        .unwrap_or_else(|source| panic!("could not read process ID from {path:?}: {source}"))
        .parse()
        .unwrap_or_else(|source| panic!("invalid process ID in {path:?}: {source}"))
}

fn process_is_running(process_id: libc::pid_t) -> bool {
    let process_status_path = PathBuf::from(format!("/proc/{process_id}/stat"));
    if let Ok(process_status) = fs::read_to_string(process_status_path) {
        let process_state = process_status
            .rfind(')')
            .and_then(|name_end| process_status.get(name_end + 2..))
            .and_then(|remaining_status| remaining_status.chars().next());
        if matches!(process_state, Some('Z' | 'X')) {
            return false;
        }
    }

    let signal_result = unsafe { libc::kill(process_id, 0) };
    if signal_result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
