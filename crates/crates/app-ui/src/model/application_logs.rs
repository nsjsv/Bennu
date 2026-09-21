use std::sync::OnceLock;
use std::time::SystemTime;

use regex::Regex;

pub(crate) const APPLICATION_LOG_ENTRY_LIMIT: usize = 200;
pub(crate) const APPLICATION_LOG_MESSAGE_CHAR_LIMIT: usize = 1_000;
pub(crate) const APP_JOURNAL_IDENTIFIER: &str = "bennu-ui";
pub(crate) const SEARCH_JOURNAL_IDENTIFIER: &str = "bennu-search";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ApplicationLogLevel {
    Error,
    Warning,
    Info,
    Debug,
}

impl ApplicationLogLevel {
    pub(crate) const ALL: [Self; 4] = [Self::Error, Self::Warning, Self::Info, Self::Debug];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Info => "Info",
            Self::Debug => "Debug",
        }
    }

    pub(crate) fn journal_priority_range(self) -> &'static str {
        match self {
            Self::Error => "0..3",
            Self::Warning => "0..4",
            Self::Info => "0..6",
            Self::Debug => "0..7",
        }
    }

    pub(crate) fn includes(self, event_level: Self) -> bool {
        event_level <= self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplicationLogSource {
    App,
    SearchService,
}

impl ApplicationLogSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::App => "App",
            Self::SearchService => "Search Service",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplicationLogEntry {
    pub(crate) timestamp: SystemTime,
    pub(crate) level: ApplicationLogLevel,
    pub(crate) source: ApplicationLogSource,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplicationLogRequest {
    pub(crate) generation: u64,
    pub(crate) threshold: ApplicationLogLevel,
}

#[derive(Debug, Clone)]
pub(crate) struct ApplicationLogViewState {
    pub(crate) threshold: ApplicationLogLevel,
    pub(crate) entries: Vec<ApplicationLogEntry>,
    pub(crate) load_error: Option<String>,
    pub(crate) journald_warning: Option<String>,
    pending_request: Option<ApplicationLogRequest>,
    refresh_after_pending: bool,
    next_generation: u64,
}

impl ApplicationLogViewState {
    pub(crate) fn new(journald_warning: Option<String>) -> Self {
        Self {
            threshold: ApplicationLogLevel::Info,
            entries: Vec::new(),
            load_error: None,
            journald_warning,
            pending_request: None,
            refresh_after_pending: false,
            next_generation: 0,
        }
    }

    pub(crate) fn request_refresh(&mut self) -> Option<ApplicationLogRequest> {
        if self.pending_request.is_some() {
            return None;
        }
        Some(self.begin_request())
    }

    pub(crate) fn select_threshold(
        &mut self,
        threshold: ApplicationLogLevel,
    ) -> Option<ApplicationLogRequest> {
        if self.threshold == threshold {
            return self.request_refresh();
        }
        self.threshold = threshold;
        self.load_error = None;
        if self.pending_request.is_some() {
            self.refresh_after_pending = true;
            return None;
        }
        Some(self.begin_request())
    }

    pub(crate) fn accept_loaded(
        &mut self,
        request: ApplicationLogRequest,
        outcome: Result<Vec<ApplicationLogEntry>, String>,
    ) -> Option<ApplicationLogRequest> {
        if self.pending_request != Some(request) {
            return None;
        }
        self.pending_request = None;
        let threshold_is_current = self.threshold == request.threshold;
        if threshold_is_current {
            match outcome {
                Ok(mut entries) => {
                    entries.truncate(APPLICATION_LOG_ENTRY_LIMIT);
                    self.entries = entries;
                    self.load_error = None;
                }
                Err(error) => self.load_error = Some(error),
            }
        }
        let refresh_after_pending = self.refresh_after_pending || !threshold_is_current;
        self.refresh_after_pending = false;
        refresh_after_pending.then(|| self.begin_request())
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.pending_request.is_some()
    }

    pub(crate) fn visible_entries(&self) -> impl Iterator<Item = &ApplicationLogEntry> {
        self.entries
            .iter()
            .filter(|entry| self.threshold.includes(entry.level))
    }

    fn begin_request(&mut self) -> ApplicationLogRequest {
        self.next_generation = self.next_generation.wrapping_add(1);
        let request = ApplicationLogRequest {
            generation: self.next_generation,
            threshold: self.threshold,
        };
        self.pending_request = Some(request);
        self.load_error = None;
        request
    }
}

pub(crate) fn bounded_application_log_message(message: &str) -> String {
    let mut characters = message.chars();
    let prefix = characters
        .by_ref()
        .take(APPLICATION_LOG_MESSAGE_CHAR_LIMIT)
        .collect::<String>();
    if characters.next().is_none() {
        return prefix;
    }

    let mut truncated = prefix
        .chars()
        .take(APPLICATION_LOG_MESSAGE_CHAR_LIMIT - 1)
        .collect::<String>();
    truncated.push('…');
    truncated
}

pub(crate) fn sanitized_application_log_detail(detail: &str) -> String {
    static URI_USERINFO: OnceLock<Regex> = OnceLock::new();
    static SENSITIVE_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();

    let uri_userinfo = URI_USERINFO.get_or_init(|| {
        Regex::new(r"(?P<scheme>[A-Za-z][A-Za-z0-9+.-]*://)[^/\s?#@]*@")
            .expect("credential URI regex must compile")
    });
    let sensitive_assignment = SENSITIVE_ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(password|passwd|token|secret|authorization|cookie)\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)"#,
        )
        .expect("sensitive assignment regex must compile")
    });
    let redacted = uri_userinfo.replace_all(detail, "${scheme}[redacted]@");
    let redacted = sensitive_assignment.replace_all(&redacted, "$1=[redacted]");
    bounded_application_log_message(&redacted)
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    fn entry(level: ApplicationLogLevel, message: impl Into<String>) -> ApplicationLogEntry {
        ApplicationLogEntry {
            timestamp: UNIX_EPOCH,
            level,
            source: ApplicationLogSource::App,
            message: message.into(),
        }
    }

    #[test]
    fn view_state_defaults_to_info_and_filters_by_severity() {
        let mut state = ApplicationLogViewState::new(None);
        let request = state.request_refresh().unwrap();
        let _ = state.accept_loaded(
            request,
            Ok(vec![
                entry(ApplicationLogLevel::Error, "error"),
                entry(ApplicationLogLevel::Warning, "warning"),
                entry(ApplicationLogLevel::Info, "info"),
                entry(ApplicationLogLevel::Debug, "debug"),
            ]),
        );

        assert_eq!(state.threshold, ApplicationLogLevel::Info);
        assert_eq!(
            state
                .visible_entries()
                .map(|entry| entry.message.as_str())
                .collect::<Vec<_>>(),
            vec!["error", "warning", "info"]
        );
    }

    #[test]
    fn threshold_change_rejects_stale_outcome() {
        let mut state = ApplicationLogViewState::new(None);
        let stale_request = state.request_refresh().unwrap();
        assert!(state.select_threshold(ApplicationLogLevel::Debug).is_none());

        let current_request = state
            .accept_loaded(
                stale_request,
                Ok(vec![entry(ApplicationLogLevel::Info, "stale")]),
            )
            .unwrap();
        assert!(state
            .accept_loaded(
                current_request,
                Ok(vec![entry(ApplicationLogLevel::Debug, "current")])
            )
            .is_none());
        assert_eq!(state.entries[0].message, "current");
    }

    #[test]
    fn rapid_threshold_changes_queue_only_the_latest_follow_up() {
        let mut state = ApplicationLogViewState::new(None);
        let first_request = state.request_refresh().unwrap();

        assert!(state.select_threshold(ApplicationLogLevel::Debug).is_none());
        assert!(state
            .select_threshold(ApplicationLogLevel::Warning)
            .is_none());
        assert!(state.select_threshold(ApplicationLogLevel::Error).is_none());
        assert!(state.request_refresh().is_none());

        let follow_up = state
            .accept_loaded(
                first_request,
                Ok(vec![entry(ApplicationLogLevel::Info, "stale")]),
            )
            .unwrap();
        assert_eq!(follow_up.threshold, ApplicationLogLevel::Error);
        assert!(state.request_refresh().is_none());
        assert!(state
            .accept_loaded(
                follow_up,
                Ok(vec![entry(ApplicationLogLevel::Error, "latest")])
            )
            .is_none());
        assert_eq!(state.entries[0].message, "latest");
    }

    #[test]
    fn refresh_is_single_flight_and_failure_preserves_snapshot() {
        let mut state = ApplicationLogViewState::new(None);
        let first_request = state.request_refresh().unwrap();
        assert!(state.request_refresh().is_none());
        let _ = state.accept_loaded(
            first_request,
            Ok(vec![entry(ApplicationLogLevel::Info, "existing")]),
        );
        let failed_request = state.request_refresh().unwrap();

        let _ = state.accept_loaded(failed_request, Err("journal unavailable".to_owned()));

        assert_eq!(state.entries[0].message, "existing");
        assert_eq!(state.load_error.as_deref(), Some("journal unavailable"));
        assert!(!state.is_loading());
    }

    #[test]
    fn accepted_snapshot_and_message_length_are_bounded() {
        let mut state = ApplicationLogViewState::new(None);
        let request = state.request_refresh().unwrap();
        let _ = state.accept_loaded(
            request,
            Ok((0..250)
                .map(|position| entry(ApplicationLogLevel::Info, position.to_string()))
                .collect()),
        );
        let long_message = "界".repeat(APPLICATION_LOG_MESSAGE_CHAR_LIMIT + 1);
        let bounded = bounded_application_log_message(&long_message);

        assert_eq!(state.entries.len(), APPLICATION_LOG_ENTRY_LIMIT);
        assert_eq!(bounded.chars().count(), APPLICATION_LOG_MESSAGE_CHAR_LIMIT);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn persisted_log_detail_redacts_uri_and_assignment_credentials() {
        let detail = "mount smb://alice:secret@example.test/share failed; token=abc123";
        let sanitized = sanitized_application_log_detail(detail);

        assert_eq!(
            sanitized,
            "mount smb://[redacted]@example.test/share failed; token=[redacted]"
        );
        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("abc123"));
    }
}
