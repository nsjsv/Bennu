use std::io::Write;
use std::sync::OnceLock;

use tracing_subscriber::prelude::*;

use crate::model::{sanitized_application_log_detail, APP_JOURNAL_IDENTIFIER};

const DEFAULT_LOG_FILTER: &str = "app_ui=info,desktop_linux=info,thumbnails=info,file_search=info";

static JOURNALD_INITIALIZATION_WARNING: OnceLock<Option<String>> = OnceLock::new();

pub(crate) fn init() {
    let (filter, invalid_filter_warning) = runtime_filter(std::env::var("RUST_LOG"));
    let journald_warning = match tracing_journald::layer() {
        Ok(layer) => {
            let subscriber_outcome = tracing_subscriber::registry()
                .with(filter)
                .with(
                    layer
                        .with_syslog_identifier(APP_JOURNAL_IDENTIFIER.to_owned())
                        .with_priority_mappings(journal_priority_mappings()),
                )
                .try_init();
            subscriber_outcome
                .err()
                .map(|error| format!("could not initialize application logging: {error}"))
        }
        Err(error) => {
            let journald_warning = format!(
                "journald logging is unavailable; application events are using stderr: {error}"
            );
            let fallback_outcome = tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_target(true)
                        .compact()
                        .with_writer(std::io::stderr),
                )
                .try_init();
            let warning = match fallback_outcome {
                Ok(()) => journald_warning,
                Err(fallback_error) => format!(
                    "{journald_warning}; stderr subscriber initialization failed: {fallback_error}"
                ),
            };
            let _ = writeln!(std::io::stderr(), "{warning}");
            Some(warning)
        }
    };

    if let Some(warning) = invalid_filter_warning {
        tracing::warn!(
            target: "app_ui::runtime_logging",
            event = "invalid_log_filter",
            "{warning}"
        );
    }
    if let Some(warning) = journald_warning.as_deref() {
        let log_error = sanitized_application_log_detail(warning);
        tracing::warn!(
            target: "app_ui::runtime_logging",
            event = "journald_fallback",
            error = %log_error,
            "journald logging fallback activated"
        );
    }
    let _ = JOURNALD_INITIALIZATION_WARNING.set(journald_warning);
}

pub(crate) fn journald_initialization_warning() -> Option<String> {
    JOURNALD_INITIALIZATION_WARNING
        .get()
        .and_then(|warning| warning.clone())
}

fn runtime_filter(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_filter_defaults_to_scoped_info() {
        let (filter, warning) = runtime_filter(Err(std::env::VarError::NotPresent));
        let filter = filter.to_string();

        for directive in [
            "app_ui=info",
            "desktop_linux=info",
            "thumbnails=info",
            "file_search=info",
        ] {
            assert!(filter.contains(directive));
        }
        assert_eq!(warning, None);
    }

    #[test]
    fn runtime_filter_accepts_override_and_rejects_invalid_value() {
        let (override_filter, override_warning) = runtime_filter(Ok("app_ui=debug".to_owned()));
        let (fallback_filter, fallback_warning) = runtime_filter(Ok("app_ui=[invalid".to_owned()));

        assert_eq!(override_filter.to_string(), "app_ui=debug");
        assert_eq!(override_warning, None);
        let fallback_filter = fallback_filter.to_string();
        for directive in [
            "app_ui=info",
            "desktop_linux=info",
            "thumbnails=info",
            "file_search=info",
        ] {
            assert!(fallback_filter.contains(directive));
        }
        assert!(fallback_warning.is_some());
    }

    #[test]
    fn runtime_filter_rejects_non_unicode_without_exposing_its_value() {
        use std::os::unix::ffi::OsStringExt;

        let (_, warning) = runtime_filter(Err(std::env::VarError::NotUnicode(
            std::ffi::OsString::from_vec(vec![0xff]),
        )));

        assert_eq!(
            warning,
            Some("RUST_LOG is not valid Unicode; using the default Info filter")
        );
    }

    #[test]
    fn journal_priorities_keep_debug_out_of_the_info_threshold() {
        let mappings = journal_priority_mappings();

        assert_eq!(mappings.error, tracing_journald::Priority::Error);
        assert_eq!(mappings.warn, tracing_journald::Priority::Warning);
        assert_eq!(mappings.info, tracing_journald::Priority::Notice);
        assert_eq!(mappings.debug, tracing_journald::Priority::Debug);
        assert_eq!(mappings.trace, tracing_journald::Priority::Debug);
    }
}
