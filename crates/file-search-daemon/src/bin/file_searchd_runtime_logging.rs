use std::io::{self, Write};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;

const DEFAULT_LOG_FILTER: &str = "file_search=info";
const SEARCH_JOURNAL_IDENTIFIER: &str = "bennu-search";
const DAEMON_LOG_DETAIL_CHAR_LIMIT: usize = 1_000;

pub(super) fn init_runtime_logging() {
    let (filter, invalid_filter_warning) = daemon_runtime_filter(std::env::var("RUST_LOG"));
    let journald_warning = match tracing_journald::layer() {
        Ok(layer) => tracing_subscriber::registry()
            .with(filter)
            .with(
                layer
                    .with_syslog_identifier(SEARCH_JOURNAL_IDENTIFIER.to_owned())
                    .with_priority_mappings(journal_priority_mappings()),
            )
            .try_init()
            .err()
            .map(|error| format!("could not initialize search logging: {error}")),
        Err(error) => {
            let journald_warning =
                format!("journald logging is unavailable; search events are using stderr: {error}");
            let fallback_outcome = tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_target(true)
                        .compact()
                        .with_writer(SystemdPriorityStderr),
                )
                .try_init();
            let warning = match fallback_outcome {
                Ok(()) => journald_warning,
                Err(fallback_error) => format!(
                    "{journald_warning}; stderr subscriber initialization failed: {fallback_error}"
                ),
            };
            let _ = writeln!(std::io::stderr(), "<4>{warning}");
            Some(warning)
        }
    };

    if let Some(warning) = invalid_filter_warning {
        tracing::warn!(
            target: "file_search::runtime_logging",
            event = "invalid_log_filter",
            "{warning}"
        );
    }
    if let Some(warning) = journald_warning {
        let log_error = bounded_daemon_log_detail(&warning);
        tracing::warn!(
            target: "file_search::runtime_logging",
            event = "journald_fallback",
            error = %log_error,
            "journald logging fallback activated"
        );
    }
}

fn daemon_runtime_filter(
    rust_log: Result<String, std::env::VarError>,
) -> (tracing_subscriber::EnvFilter, Option<&'static str>) {
    let rust_log = match rust_log {
        Ok(rust_log) => rust_log,
        Err(std::env::VarError::NotPresent) => {
            return (tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER), None);
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return (
                tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER),
                Some("RUST_LOG is not valid Unicode; using the default Info filter"),
            );
        }
    };
    match tracing_subscriber::EnvFilter::try_new(rust_log) {
        Ok(filter) => (filter, None),
        Err(_) => (
            tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER),
            Some("RUST_LOG is invalid; using the default Info filter"),
        ),
    }
}

fn journal_priority_mappings() -> tracing_journald::PriorityMappings {
    tracing_journald::PriorityMappings {
        debug: tracing_journald::Priority::Debug,
        trace: tracing_journald::Priority::Debug,
        ..tracing_journald::PriorityMappings::new()
    }
}

#[derive(Clone, Copy)]
struct SystemdPriorityStderr;

impl<'writer> MakeWriter<'writer> for SystemdPriorityStderr {
    type Writer = SystemdPriorityStderrLine;

    fn make_writer(&'writer self) -> Self::Writer {
        SystemdPriorityStderrLine::new(tracing::Level::INFO)
    }

    fn make_writer_for(&'writer self, metadata: &tracing::Metadata<'_>) -> Self::Writer {
        SystemdPriorityStderrLine::new(*metadata.level())
    }
}

struct SystemdPriorityStderrLine {
    stderr: std::io::Stderr,
    prefix: &'static [u8],
    prefix_written: bool,
}

impl SystemdPriorityStderrLine {
    fn new(level: tracing::Level) -> Self {
        Self {
            stderr: std::io::stderr(),
            prefix: syslog_priority_prefix(level),
            prefix_written: false,
        }
    }
}

impl Write for SystemdPriorityStderrLine {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if !self.prefix_written {
            self.stderr.write_all(self.prefix)?;
            self.prefix_written = true;
        }
        self.stderr.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stderr.flush()
    }
}

fn syslog_priority_prefix(level: tracing::Level) -> &'static [u8] {
    match level {
        tracing::Level::ERROR => b"<3>",
        tracing::Level::WARN => b"<4>",
        tracing::Level::INFO => b"<5>",
        tracing::Level::DEBUG | tracing::Level::TRACE => b"<7>",
    }
}

pub(super) fn bounded_daemon_log_detail(detail: &str) -> String {
    let mut characters = detail.chars();
    let prefix = characters
        .by_ref()
        .take(DAEMON_LOG_DETAIL_CHAR_LIMIT)
        .collect::<String>();
    if characters.next().is_none() {
        return prefix;
    }

    let mut truncated = prefix
        .chars()
        .take(DAEMON_LOG_DETAIL_CHAR_LIMIT - 1)
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    #[test]
    fn daemon_runtime_filter_defaults_to_info_and_accepts_override() {
        let (default_filter, default_warning) =
            daemon_runtime_filter(Err(std::env::VarError::NotPresent));
        let (override_filter, override_warning) =
            daemon_runtime_filter(Ok("file_search=debug".to_owned()));
        let (fallback_filter, fallback_warning) =
            daemon_runtime_filter(Ok("file_search=[bad".to_owned()));

        assert_eq!(default_filter.to_string(), DEFAULT_LOG_FILTER);
        assert_eq!(default_warning, None);
        assert_eq!(override_filter.to_string(), "file_search=debug");
        assert_eq!(override_warning, None);
        assert_eq!(fallback_filter.to_string(), DEFAULT_LOG_FILTER);
        assert!(fallback_warning.is_some());
    }

    #[test]
    fn daemon_runtime_filter_rejects_non_unicode_without_exposing_its_value() {
        let (_, warning) = daemon_runtime_filter(Err(std::env::VarError::NotUnicode(
            std::ffi::OsString::from_vec(vec![0xff]),
        )));

        assert_eq!(
            warning,
            Some("RUST_LOG is not valid Unicode; using the default Info filter")
        );
    }

    #[test]
    fn daemon_journal_priorities_keep_debug_out_of_the_info_threshold() {
        let mappings = journal_priority_mappings();

        assert_eq!(mappings.error, tracing_journald::Priority::Error);
        assert_eq!(mappings.warn, tracing_journald::Priority::Warning);
        assert_eq!(mappings.info, tracing_journald::Priority::Notice);
        assert_eq!(mappings.debug, tracing_journald::Priority::Debug);
        assert_eq!(mappings.trace, tracing_journald::Priority::Debug);
    }

    #[test]
    fn stderr_fallback_preserves_journal_priorities() {
        assert_eq!(syslog_priority_prefix(tracing::Level::ERROR), b"<3>");
        assert_eq!(syslog_priority_prefix(tracing::Level::WARN), b"<4>");
        assert_eq!(syslog_priority_prefix(tracing::Level::INFO), b"<5>");
        assert_eq!(syslog_priority_prefix(tracing::Level::DEBUG), b"<7>");
        assert_eq!(syslog_priority_prefix(tracing::Level::TRACE), b"<7>");
    }

    #[test]
    fn daemon_log_detail_is_unicode_safe_and_bounded() {
        let detail = "界".repeat(DAEMON_LOG_DETAIL_CHAR_LIMIT + 1);
        let bounded = bounded_daemon_log_detail(&detail);

        assert_eq!(bounded.chars().count(), DAEMON_LOG_DETAIL_CHAR_LIMIT);
        assert!(bounded.ends_with('…'));
    }
}
