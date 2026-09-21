use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, UNIX_EPOCH};

use file_search::SearchRuntimeIdentity;
use iced::Task;
use serde_json::{Map, Value};
use tokio::process::Command;

use super::bounded_child_output::{read_bounded_child_output, BoundedChildOutput};
use crate::model::{
    bounded_application_log_message, ApplicationLogEntry, ApplicationLogLevel,
    ApplicationLogRequest, ApplicationLogSource, Message, APPLICATION_LOG_ENTRY_LIMIT,
    APP_JOURNAL_IDENTIFIER, SEARCH_JOURNAL_IDENTIFIER,
};

const APPLICATION_LOG_QUERY_TIMEOUT: Duration = Duration::from_secs(1);
const APPLICATION_LOG_STDOUT_BYTE_LIMIT: usize = 1_048_576;
const APPLICATION_LOG_STDERR_BYTE_LIMIT: usize = 65_536;
const JOURNAL_ERROR_FIELD: &str = "F_ERROR";

pub(crate) fn application_logs_command(request: ApplicationLogRequest) -> Task<Message> {
    Task::perform(read_application_logs(request), move |outcome| {
        Message::ApplicationLogsLoaded(request, outcome)
    })
}

async fn read_application_logs(
    request: ApplicationLogRequest,
) -> Result<Vec<ApplicationLogEntry>, String> {
    let runtime_identity =
        SearchRuntimeIdentity::from_environment().map_err(|error| error.to_string())?;
    read_application_logs_with(
        Path::new("journalctl"),
        request.threshold,
        APPLICATION_LOG_QUERY_TIMEOUT,
        runtime_identity,
    )
    .await
}

async fn read_application_logs_with(
    journalctl_executable: &Path,
    threshold: ApplicationLogLevel,
    query_timeout: Duration,
    runtime_identity: SearchRuntimeIdentity,
) -> Result<Vec<ApplicationLogEntry>, String> {
    let mut command = Command::new(journalctl_executable);
    command
        .args(journalctl_arguments(threshold, runtime_identity))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not execute journalctl: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not capture journalctl stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "could not capture journalctl stderr".to_owned())?;
    let (stdout, stderr, status) = tokio::time::timeout(query_timeout, async {
        tokio::join!(
            read_bounded_child_output(stdout, APPLICATION_LOG_STDOUT_BYTE_LIMIT),
            read_bounded_child_output(stderr, APPLICATION_LOG_STDERR_BYTE_LIMIT),
            child.wait(),
        )
    })
    .await
    .map_err(|_| {
        format!(
            "application log query timed out after {} ms",
            query_timeout.as_millis()
        )
    })?;
    let stdout = stdout.map_err(|error| format!("could not read journalctl stdout: {error}"))?;
    let stderr = stderr.map_err(|error| format!("could not read journalctl stderr: {error}"))?;
    let status = status.map_err(|error| format!("could not wait for journalctl: {error}"))?;
    let stdout = journal_output_within_limit(stdout, APPLICATION_LOG_STDOUT_BYTE_LIMIT, "stdout")?;
    let stderr = journal_output_within_limit(stderr, APPLICATION_LOG_STDERR_BYTE_LIMIT, "stderr")?;

    if !status.success() {
        let stderr = bounded_application_log_message(String::from_utf8_lossy(&stderr).trim());
        let detail = if stderr.is_empty() {
            status.to_string()
        } else {
            stderr
        };
        return Err(format!("journalctl failed: {detail}"));
    }

    let stdout = String::from_utf8(stdout)
        .map_err(|error| format!("journalctl returned non-UTF-8 output: {error}"))?;
    parse_journal_output(&stdout, runtime_identity)
}

fn journal_output_within_limit(
    output: BoundedChildOutput,
    byte_limit: usize,
    stream_name: &'static str,
) -> Result<Vec<u8>, String> {
    if output.exceeded_limit {
        return Err(format!(
            "journalctl {stream_name} exceeded the {byte_limit}-byte limit"
        ));
    }
    Ok(output.bytes)
}

fn journalctl_arguments(
    threshold: ApplicationLogLevel,
    runtime_identity: SearchRuntimeIdentity,
) -> Vec<OsString> {
    [
        "--user".to_owned(),
        "--boot=0".to_owned(),
        "--no-pager".to_owned(),
        "--quiet".to_owned(),
        "--reverse".to_owned(),
        format!("--lines={APPLICATION_LOG_ENTRY_LIMIT}"),
        "--output=json".to_owned(),
        format!(
            "--output-fields=PRIORITY,__REALTIME_TIMESTAMP,SYSLOG_IDENTIFIER,MESSAGE,_SYSTEMD_USER_UNIT,{JOURNAL_ERROR_FIELD}"
        ),
        format!("--priority={}", threshold.journal_priority_range()),
        format!("SYSLOG_IDENTIFIER={APP_JOURNAL_IDENTIFIER}"),
        "+".to_owned(),
        format!("SYSLOG_IDENTIFIER={SEARCH_JOURNAL_IDENTIFIER}"),
        format!("_SYSTEMD_USER_UNIT={}", runtime_identity.systemd_unit()),
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

fn parse_journal_output(
    stdout: &str,
    runtime_identity: SearchRuntimeIdentity,
) -> Result<Vec<ApplicationLogEntry>, String> {
    let mut entries = Vec::new();
    for (line_index, journal_line) in stdout.lines().enumerate() {
        if journal_line.trim().is_empty() {
            continue;
        }
        let journal_fields = serde_json::from_str::<Value>(journal_line)
            .map_err(|error| format!("journal line {} is invalid JSON: {error}", line_index + 1))?;
        let journal_fields = journal_fields
            .as_object()
            .ok_or_else(|| format!("journal line {} must be a JSON object", line_index + 1))?;
        entries.push(
            parse_journal_fields(journal_fields, runtime_identity)
                .map_err(|error| format!("journal line {} is invalid: {error}", line_index + 1))?,
        );
        if entries.len() == APPLICATION_LOG_ENTRY_LIMIT {
            break;
        }
    }
    Ok(entries)
}

fn parse_journal_fields(
    journal_fields: &Map<String, Value>,
    runtime_identity: SearchRuntimeIdentity,
) -> Result<ApplicationLogEntry, String> {
    let priority = required_journal_string(journal_fields, "PRIORITY")?
        .parse::<u8>()
        .map_err(|error| format!("PRIORITY is not an integer: {error}"))?;
    let level = match priority {
        0..=3 => ApplicationLogLevel::Error,
        4 => ApplicationLogLevel::Warning,
        5..=6 => ApplicationLogLevel::Info,
        7 => ApplicationLogLevel::Debug,
        _ => return Err(format!("PRIORITY={priority} is outside 0..=7")),
    };

    let timestamp_micros = required_journal_string(journal_fields, "__REALTIME_TIMESTAMP")?
        .parse::<u64>()
        .map_err(|error| format!("__REALTIME_TIMESTAMP is not an integer: {error}"))?;
    let timestamp = UNIX_EPOCH
        .checked_add(Duration::from_micros(timestamp_micros))
        .ok_or_else(|| "__REALTIME_TIMESTAMP is outside SystemTime range".to_owned())?;

    let identifier = required_journal_string(journal_fields, "SYSLOG_IDENTIFIER")?;
    let trusted_search_unit = runtime_identity.systemd_unit();
    let source = match identifier {
        APP_JOURNAL_IDENTIFIER => ApplicationLogSource::App,
        SEARCH_JOURNAL_IDENTIFIER
            if optional_journal_string(journal_fields, "_SYSTEMD_USER_UNIT")?
                == Some(trusted_search_unit) =>
        {
            ApplicationLogSource::SearchService
        }
        SEARCH_JOURNAL_IDENTIFIER => {
            return Err(format!(
                "{SEARCH_JOURNAL_IDENTIFIER} is not owned by {trusted_search_unit}"
            ));
        }
        _ => return Err(format!("unknown SYSLOG_IDENTIFIER={identifier}")),
    };

    let mut message = required_journal_string(journal_fields, "MESSAGE")?.to_owned();
    if let Some(error) = optional_journal_string(journal_fields, JOURNAL_ERROR_FIELD)? {
        if !error.is_empty() {
            message.push_str(": ");
            message.push_str(error);
        }
    }

    Ok(ApplicationLogEntry {
        timestamp,
        level,
        source,
        message: bounded_application_log_message(&message),
    })
}

fn required_journal_string<'a>(
    journal_fields: &'a Map<String, Value>,
    field_name: &str,
) -> Result<&'a str, String> {
    optional_journal_string(journal_fields, field_name)?
        .ok_or_else(|| format!("missing {field_name}"))
}

fn optional_journal_string<'a>(
    journal_fields: &'a Map<String, Value>,
    field_name: &str,
) -> Result<Option<&'a str>, String> {
    journal_fields
        .get(field_name)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{field_name} must be a string"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::SystemTime;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn journal_line(
        priority: &str,
        identifier: &str,
        message: &str,
        user_unit: Option<&str>,
    ) -> String {
        let mut fields = json!({
            "PRIORITY": priority,
            "__REALTIME_TIMESTAMP": "1704067200000000",
            "SYSLOG_IDENTIFIER": identifier,
            "MESSAGE": message,
        });
        if let Some(user_unit) = user_unit {
            fields["_SYSTEMD_USER_UNIT"] = Value::String(user_unit.to_owned());
        }
        fields.to_string()
    }

    fn write_executable(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn journal_json_maps_priority_timestamp_source_message_and_error() {
        let runtime_identity = SearchRuntimeIdentity::Release;
        let trusted_search_unit = runtime_identity.systemd_unit();
        let stdout = [
            journal_line("3", APP_JOURNAL_IDENTIFIER, "app failed", None),
            journal_line(
                "4",
                SEARCH_JOURNAL_IDENTIFIER,
                "watch degraded",
                Some(trusted_search_unit),
            ),
            journal_line("6", APP_JOURNAL_IDENTIFIER, "app ready", None),
            journal_line(
                "7",
                SEARCH_JOURNAL_IDENTIFIER,
                "batch complete",
                Some(trusted_search_unit),
            ),
        ]
        .join("\n");
        let mut lines = stdout.lines().map(str::to_owned).collect::<Vec<_>>();
        let mut warning_fields = serde_json::from_str::<Value>(&lines[1]).unwrap();
        // tracing-journald prefixes user fields with `F_` by default.
        warning_fields[JOURNAL_ERROR_FIELD] = Value::String("watch backend unavailable".to_owned());
        lines[1] = warning_fields.to_string();

        let entries = parse_journal_output(&lines.join("\n"), runtime_identity).unwrap();

        assert_eq!(
            entries.iter().map(|entry| entry.level).collect::<Vec<_>>(),
            vec![
                ApplicationLogLevel::Error,
                ApplicationLogLevel::Warning,
                ApplicationLogLevel::Info,
                ApplicationLogLevel::Debug,
            ]
        );
        assert_eq!(entries[0].source, ApplicationLogSource::App);
        assert_eq!(entries[1].source, ApplicationLogSource::SearchService);
        assert_eq!(
            entries[0].timestamp,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_067_200)
        );
        assert_eq!(
            entries[1].message,
            "watch degraded: watch backend unavailable"
        );
    }

    #[test]
    fn journal_json_rejects_missing_invalid_and_untrusted_fields() {
        for journal_line in [
            "{}".to_owned(),
            journal_line("8", APP_JOURNAL_IDENTIFIER, "bad priority", None),
            json!({
                "PRIORITY": 6,
                "__REALTIME_TIMESTAMP": "1704067200000000",
                "SYSLOG_IDENTIFIER": APP_JOURNAL_IDENTIFIER,
                "MESSAGE": "numeric priority",
            })
            .to_string(),
            json!({
                "PRIORITY": "6",
                "__REALTIME_TIMESTAMP": "invalid",
                "SYSLOG_IDENTIFIER": APP_JOURNAL_IDENTIFIER,
                "MESSAGE": [255],
            })
            .to_string(),
            journal_line("6", SEARCH_JOURNAL_IDENTIFIER, "untrusted daemon", None),
            journal_line("6", "unrelated", "wrong source", None),
            "[]".to_owned(),
            "not-json".to_owned(),
        ] {
            assert!(parse_journal_output(&journal_line, SearchRuntimeIdentity::Release).is_err());
        }
    }

    #[test]
    fn daemon_log_source_follows_the_configured_runtime_identity() {
        let development_line = journal_line(
            "6",
            SEARCH_JOURNAL_IDENTIFIER,
            "development daemon ready",
            Some(SearchRuntimeIdentity::Development.systemd_unit()),
        );

        assert!(
            parse_journal_output(&development_line, SearchRuntimeIdentity::Development).is_ok()
        );
        assert!(parse_journal_output(&development_line, SearchRuntimeIdentity::Release).is_err());
    }

    #[test]
    fn parser_never_returns_more_than_the_entry_limit() {
        let stdout = (0..250)
            .map(|position| journal_line("6", APP_JOURNAL_IDENTIFIER, &position.to_string(), None))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            parse_journal_output(&stdout, SearchRuntimeIdentity::Release)
                .unwrap()
                .len(),
            APPLICATION_LOG_ENTRY_LIMIT
        );
    }

    #[test]
    fn journalctl_arguments_are_fixed_and_threshold_bounded() {
        let runtime_identity = SearchRuntimeIdentity::Development;
        let arguments = journalctl_arguments(ApplicationLogLevel::Info, runtime_identity);
        let arguments = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(arguments.contains(&"--boot=0".into()));
        assert!(arguments.contains(&"--reverse".into()));
        assert!(arguments.contains(&"--lines=200".into()));
        assert!(arguments.iter().any(|argument| {
            argument.starts_with("--output-fields=") && argument.contains(JOURNAL_ERROR_FIELD)
        }));
        assert!(arguments.contains(&"--priority=0..6".into()));
        assert!(arguments.contains(&format!("SYSLOG_IDENTIFIER={APP_JOURNAL_IDENTIFIER}").into()));
        assert!(arguments.contains(&"+".into()));
        assert!(arguments
            .contains(&format!("_SYSTEMD_USER_UNIT={}", runtime_identity.systemd_unit()).into()));
    }

    #[test]
    fn journal_output_limit_error_keeps_stream_context() {
        let error = journal_output_within_limit(
            BoundedChildOutput {
                bytes: b"01234567".to_vec(),
                exceeded_limit: true,
            },
            8,
            "stdout",
        )
        .expect_err("output should be rejected");

        assert_eq!(error, "journalctl stdout exceeded the 8-byte limit");
    }

    #[tokio::test]
    async fn journalctl_failures_are_specific_and_bounded() {
        let temporary_directory = tempdir().unwrap();
        let failing_journalctl = temporary_directory.path().join("failing-journalctl");
        let slow_journalctl = temporary_directory.path().join("slow-journalctl");
        let binary_journalctl = temporary_directory.path().join("binary-journalctl");
        write_executable(
            &failing_journalctl,
            "#!/usr/bin/env bash\nprintf 'journal unavailable' >&2\nexit 7\n",
        );
        write_executable(&slow_journalctl, "#!/usr/bin/env bash\nsleep 1\n");
        write_executable(&binary_journalctl, "#!/usr/bin/env bash\nprintf '\\377'\n");

        let nonzero_error = read_application_logs_with(
            &failing_journalctl,
            ApplicationLogLevel::Info,
            Duration::from_secs(1),
            SearchRuntimeIdentity::Release,
        )
        .await
        .unwrap_err();
        let timeout_error = read_application_logs_with(
            &slow_journalctl,
            ApplicationLogLevel::Info,
            Duration::from_millis(20),
            SearchRuntimeIdentity::Release,
        )
        .await
        .unwrap_err();
        let utf8_error = read_application_logs_with(
            &binary_journalctl,
            ApplicationLogLevel::Info,
            Duration::from_secs(1),
            SearchRuntimeIdentity::Release,
        )
        .await
        .unwrap_err();
        let missing_error = read_application_logs_with(
            &temporary_directory.path().join("missing-journalctl"),
            ApplicationLogLevel::Info,
            Duration::from_secs(1),
            SearchRuntimeIdentity::Release,
        )
        .await
        .unwrap_err();

        assert!(nonzero_error.contains("journalctl failed: journal unavailable"));
        assert!(timeout_error.contains("timed out"));
        assert!(utf8_error.contains("non-UTF-8"));
        assert!(missing_error.contains("could not execute journalctl"));
    }
}
