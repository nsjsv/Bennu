use std::env;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const TRACE_ENV: &str = "FILE_MANAGER_STARTUP_TRACE";
const PRE_LOGGING_MILESTONE_CAPACITY: usize = 8;
const LOGGING_READY_BATCH_CAPACITY: usize = PRE_LOGGING_MILESTONE_CAPACITY + 1;

static STARTUP_TRACE: OnceLock<Option<StartupTrace>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopActivationClaimOutcome {
    Primary,
    Forwarded,
    Detached,
    Failed,
}

impl DesktopActivationClaimOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Forwarded => "forwarded",
            Self::Detached => "detached",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StartupMilestone {
    label: &'static str,
    elapsed_ms: f64,
    classification: Option<&'static str>,
}

struct StartupTraceState {
    emitted_once_labels: Vec<&'static str>,
    pre_logging_milestones: [Option<StartupMilestone>; PRE_LOGGING_MILESTONE_CAPACITY],
    pre_logging_milestone_count: usize,
    logging_ready: bool,
}

struct StartupTrace {
    started_at: Instant,
    state: Mutex<StartupTraceState>,
}

struct StartupMilestoneBatch {
    milestones: [Option<StartupMilestone>; LOGGING_READY_BATCH_CAPACITY],
    count: usize,
}

pub(crate) fn begin_launch_from_env() {
    let _ = STARTUP_TRACE.set(trace_from_env());
    mark("launch_trace_started");
}

pub(crate) fn mark(label: &'static str) {
    let Some(trace) = current_trace() else {
        return;
    };
    if let Some(milestone) = trace.record(label, None, false) {
        emit(milestone);
    }
}

pub(crate) fn mark_once(label: &'static str) {
    let Some(trace) = current_trace() else {
        return;
    };
    if let Some(milestone) = trace.record(label, None, true) {
        emit(milestone);
    }
}

pub(crate) fn mark_desktop_activation_claim_finished(outcome: DesktopActivationClaimOutcome) {
    let Some(trace) = current_trace() else {
        return;
    };
    if let Some(milestone) = trace.record(
        "desktop_activation_claim_finished",
        Some(outcome.as_str()),
        true,
    ) {
        emit(milestone);
    }
}

pub(crate) fn mark_runtime_logging_ready() {
    let Some(trace) = current_trace() else {
        return;
    };
    for milestone in trace.logging_ready().iter() {
        emit(milestone);
    }
}

pub(crate) fn record_rendering_backend_selected(backend: &'static str) {
    if current_trace().is_none() {
        return;
    }
    tracing::info!(
        target: "app_ui::startup",
        event = "rendering_backend_selected",
        backend,
        "startup rendering backend selected"
    );
}

pub(crate) fn record_directory_collection_hint_counts(
    produced: usize,
    accepted: usize,
    dropped: usize,
) {
    if current_trace().is_none() {
        return;
    }
    tracing::info!(
        target: "app_ui::startup",
        event = "directory_collection_transport",
        produced_hint_count = produced,
        accepted_hint_count = accepted,
        dropped_hint_count = dropped,
        "directory collection transport counters"
    );
}

pub(crate) fn record_directory_collection_ready(entry_count: usize) {
    if current_trace().is_none() {
        return;
    }
    tracing::info!(
        target: "app_ui::startup",
        event = "directory_collection_ready",
        entry_count,
        snapshot_commit_count = 1,
        order_sort_count = 1,
        "authoritative directory collection committed"
    );
}

pub(crate) fn record_directory_metadata_resolution(
    requirement: &'static str,
    requested_target_count: usize,
    resolved_target_count: usize,
    warning_count: usize,
) {
    if current_trace().is_none() {
        return;
    }
    tracing::info!(
        target: "app_ui::startup",
        event = "directory_metadata_resolution",
        requirement,
        requested_target_count,
        resolved_target_count,
        warning_count,
        "directory metadata demand resolved"
    );
}

pub(crate) fn record_application_shutdown_plan(
    waiting_operation_count: usize,
    stopping_signal_count: usize,
    journal_read_count: usize,
    interrupted_recoverable_count: usize,
    transient_task_count: usize,
) {
    if current_trace().is_none() {
        return;
    }
    tracing::info!(
        target: "app_ui::startup",
        event = "application_shutdown_plan",
        waiting_operation_count,
        stopping_signal_count,
        journal_read_count,
        interrupted_recoverable_count,
        transient_task_count,
        "application shutdown plan captured"
    );
}

fn current_trace() -> Option<&'static StartupTrace> {
    STARTUP_TRACE.get().and_then(Option::as_ref)
}

fn trace_from_env() -> Option<StartupTrace> {
    let raw_value = env::var_os(TRACE_ENV)?;
    if !env_value_enables_trace(raw_value) {
        return None;
    }

    Some(StartupTrace::new())
}

fn env_value_enables_trace(raw_value: OsString) -> bool {
    let normalized_value = raw_value.to_string_lossy().trim().to_ascii_lowercase();
    !matches!(normalized_value.as_str(), "" | "0" | "false" | "no" | "off")
}

fn emit(milestone: StartupMilestone) {
    if let Some(classification) = milestone.classification {
        tracing::info!(
            target: "app_ui::startup",
            event = "startup_milestone",
            label = milestone.label,
            elapsed_ms = milestone.elapsed_ms,
            classification,
            "startup milestone reached"
        );
    } else {
        tracing::info!(
            target: "app_ui::startup",
            event = "startup_milestone",
            label = milestone.label,
            elapsed_ms = milestone.elapsed_ms,
            "startup milestone reached"
        );
    }
}

impl StartupTrace {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            state: Mutex::new(StartupTraceState {
                emitted_once_labels: Vec::new(),
                pre_logging_milestones: [None; PRE_LOGGING_MILESTONE_CAPACITY],
                pre_logging_milestone_count: 0,
                logging_ready: false,
            }),
        }
    }

    fn record(
        &self,
        label: &'static str,
        classification: Option<&'static str>,
        once: bool,
    ) -> Option<StartupMilestone> {
        let milestone = StartupMilestone {
            label,
            elapsed_ms: self.started_at.elapsed().as_secs_f64() * 1000.0,
            classification,
        };
        let mut state = self.state.lock().expect("startup trace state lock");
        if once {
            if state.emitted_once_labels.contains(&label) {
                return None;
            }
            state.emitted_once_labels.push(label);
        }
        if state.logging_ready {
            return Some(milestone);
        }
        assert!(
            state.pre_logging_milestone_count < PRE_LOGGING_MILESTONE_CAPACITY,
            "startup trace pre-logging milestone capacity exceeded"
        );
        let index = state.pre_logging_milestone_count;
        state.pre_logging_milestones[index] = Some(milestone);
        state.pre_logging_milestone_count += 1;
        None
    }

    fn logging_ready(&self) -> StartupMilestoneBatch {
        let ready = StartupMilestone {
            label: "runtime_logging_ready",
            elapsed_ms: self.started_at.elapsed().as_secs_f64() * 1000.0,
            classification: None,
        };
        let mut state = self.state.lock().expect("startup trace state lock");
        if state.logging_ready {
            return StartupMilestoneBatch::empty();
        }
        let mut batch = StartupMilestoneBatch::empty();
        for index in 0..state.pre_logging_milestone_count {
            batch.push(
                state.pre_logging_milestones[index]
                    .take()
                    .expect("buffered startup milestone"),
            );
        }
        state.pre_logging_milestone_count = 0;
        state.logging_ready = true;
        batch.push(ready);
        batch
    }
}

impl StartupMilestoneBatch {
    fn empty() -> Self {
        Self {
            milestones: [None; LOGGING_READY_BATCH_CAPACITY],
            count: 0,
        }
    }

    fn push(&mut self, milestone: StartupMilestone) {
        self.milestones[self.count] = Some(milestone);
        self.count += 1;
    }

    fn iter(&self) -> impl Iterator<Item = StartupMilestone> + '_ {
        self.milestones[..self.count].iter().flatten().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_env_accepts_common_truthy_values() {
        assert!(env_value_enables_trace(OsString::from("1")));
        assert!(env_value_enables_trace(OsString::from("true")));
        assert!(env_value_enables_trace(OsString::from("yes")));
    }

    #[test]
    fn trace_env_rejects_common_disabled_values() {
        assert!(!env_value_enables_trace(OsString::from("")));
        assert!(!env_value_enables_trace(OsString::from("0")));
        assert!(!env_value_enables_trace(OsString::from("false")));
        assert!(!env_value_enables_trace(OsString::from("no")));
        assert!(!env_value_enables_trace(OsString::from("off")));
    }

    #[test]
    fn pre_logging_milestones_flush_once_in_timestamp_order() {
        let trace = StartupTrace::new();
        assert_eq!(trace.record("launch_trace_started", None, false), None);
        assert_eq!(
            trace.record("desktop_activation_claim_started", None, false),
            None
        );
        assert_eq!(
            trace.record("desktop_activation_claim_finished", Some("primary"), true,),
            None
        );

        let first_flush = trace.logging_ready().iter().collect::<Vec<_>>();
        let second_flush = trace.logging_ready().iter().collect::<Vec<_>>();
        let labels = first_flush
            .iter()
            .map(|milestone| milestone.label)
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            [
                "launch_trace_started",
                "desktop_activation_claim_started",
                "desktop_activation_claim_finished",
                "runtime_logging_ready",
            ]
        );
        assert!(first_flush
            .windows(2)
            .all(|pair| pair[0].elapsed_ms <= pair[1].elapsed_ms));
        assert_eq!(first_flush[2].classification, Some("primary"));
        assert!(second_flush.is_empty());
    }

    #[test]
    fn once_milestones_are_deduplicated_before_logging_is_ready() {
        let trace = StartupTrace::new();

        assert_eq!(trace.record("first_view", None, true), None);
        assert_eq!(trace.record("first_view", None, true), None);

        let labels = trace
            .logging_ready()
            .iter()
            .map(|milestone| milestone.label)
            .collect::<Vec<_>>();
        assert_eq!(labels, ["first_view", "runtime_logging_ready"]);
    }
}
