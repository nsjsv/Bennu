use file_core::TrashScan;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub(crate) struct TrashRefreshRequest {
    pub(crate) generation: u64,
    pub(crate) cancellation: CancellationToken,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrashRefreshCompletionDecision {
    #[default]
    Apply,
    /// 读的时候回收站又变了：这次的结果照常用上，再读一次。
    ApplyThenRefresh,
    StartReplacement,
    Discard,
}

#[derive(Debug, Default)]
pub(crate) struct TrashRefreshState {
    generation: u64,
    active_generation: Option<u64>,
    cancellation: Option<CancellationToken>,
    active_completion_decision: TrashRefreshCompletionDecision,
    snapshot: Option<TrashScan>,
    last_error: Option<String>,
    warning_details_expanded: bool,
}

impl TrashRefreshState {
    pub(crate) fn begin_if_idle(&mut self) -> Option<TrashRefreshRequest> {
        if self.active_generation.is_some() {
            if self.active_completion_decision == TrashRefreshCompletionDecision::Discard {
                self.active_completion_decision = TrashRefreshCompletionDecision::StartReplacement;
            }
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        let cancellation = CancellationToken::new();
        self.active_generation = Some(self.generation);
        self.cancellation = Some(cancellation.clone());
        self.active_completion_decision = TrashRefreshCompletionDecision::Apply;
        Some(TrashRefreshRequest {
            generation: self.generation,
            cancellation,
        })
    }

    /// 回收站变了：没在读就开读；正在读的不打断，读完用上再补读一次。打断重读的话，变化接连
    /// 不断时（别的程序一个个往回收站里删）每次都读不完，列表一直不动。
    pub(crate) fn begin_after_change(&mut self) -> Option<TrashRefreshRequest> {
        if self.active_generation.is_some()
            && self.active_completion_decision == TrashRefreshCompletionDecision::Apply
        {
            self.active_completion_decision = TrashRefreshCompletionDecision::ApplyThenRefresh;
        }
        self.begin_if_idle()
    }

    pub(crate) fn invalidate_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        let Some(cancellation) = self.cancellation.as_ref() else {
            return;
        };
        cancellation.cancel();
        self.active_completion_decision = TrashRefreshCompletionDecision::StartReplacement;
    }

    pub(crate) fn discard_snapshot(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.snapshot = None;
        self.last_error = None;
        self.warning_details_expanded = false;
        let Some(cancellation) = self.cancellation.as_ref() else {
            return;
        };
        cancellation.cancel();
        self.active_completion_decision = TrashRefreshCompletionDecision::Discard;
    }

    pub(crate) fn classify_completion(
        &mut self,
        generation: u64,
    ) -> TrashRefreshCompletionDecision {
        if self.active_generation != Some(generation) {
            return TrashRefreshCompletionDecision::Discard;
        }
        self.active_generation = None;
        self.cancellation = None;
        std::mem::replace(
            &mut self.active_completion_decision,
            TrashRefreshCompletionDecision::Apply,
        )
    }

    pub(crate) fn replace_snapshot(&mut self, snapshot: TrashScan) {
        self.snapshot = Some(snapshot);
    }

    pub(crate) fn snapshot(&self) -> Option<&TrashScan> {
        self.snapshot.as_ref()
    }

    pub(crate) fn record_error(&mut self, error: String) {
        self.last_error = Some(error);
    }

    pub(crate) fn clear_error(&mut self) {
        self.last_error = None;
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub(crate) fn warning_details_expanded(&self) -> bool {
        self.warning_details_expanded
    }

    pub(crate) fn toggle_warning_details(&mut self) {
        self.warning_details_expanded = !self.warning_details_expanded;
    }

    #[cfg(test)]
    fn is_pending(&self) -> bool {
        self.active_generation.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_waits_for_worker_exit_before_starting_replacement() {
        let mut state = TrashRefreshState::default();
        let stale = state.begin_if_idle().unwrap();
        state.invalidate_pending();

        assert!(stale.cancellation.is_cancelled());
        assert!(state.begin_if_idle().is_none());
        assert_eq!(
            state.classify_completion(stale.generation),
            TrashRefreshCompletionDecision::StartReplacement
        );
        let current = state.begin_if_idle().unwrap();
        assert!(state.is_pending());
        assert_eq!(
            state.classify_completion(stale.generation),
            TrashRefreshCompletionDecision::Discard
        );
        assert_eq!(
            state.classify_completion(current.generation),
            TrashRefreshCompletionDecision::Apply
        );
        assert!(!state.is_pending());
    }

    #[test]
    fn change_during_a_read_keeps_it_and_reads_once_more_after() {
        let mut state = TrashRefreshState::default();
        let running = state
            .begin_after_change()
            .expect("idle state starts a read");

        // 变化接连来：不打断这次，也只补一次。
        assert!(state.begin_after_change().is_none());
        assert!(state.begin_after_change().is_none());
        assert!(!running.cancellation.is_cancelled());

        assert_eq!(
            state.classify_completion(running.generation),
            TrashRefreshCompletionDecision::ApplyThenRefresh
        );
        assert!(state.begin_after_change().is_some());
    }

    #[test]
    fn discarding_without_consumers_rejects_the_active_worker_result() {
        let mut state = TrashRefreshState::default();
        state.replace_snapshot(TrashScan {
            entries: Vec::new(),
            skipped: Vec::new(),
        });
        let active = state.begin_if_idle().unwrap();

        state.discard_snapshot();

        assert!(active.cancellation.is_cancelled());
        assert!(state.snapshot().is_none());
        assert_eq!(
            state.classify_completion(active.generation),
            TrashRefreshCompletionDecision::Discard
        );
        assert!(state.snapshot().is_none());
        assert!(state.begin_if_idle().is_some());
    }

    #[test]
    fn new_consumer_promotes_a_discarded_worker_to_replacement() {
        let mut state = TrashRefreshState::default();
        let discarded = state.begin_if_idle().expect("discarded worker");
        state.discard_snapshot();

        assert!(state.begin_if_idle().is_none());
        assert_eq!(
            state.classify_completion(discarded.generation),
            TrashRefreshCompletionDecision::StartReplacement
        );
        assert!(state.begin_if_idle().is_some());
    }

    #[test]
    fn concurrent_consumers_share_one_pending_refresh_request() {
        let mut state = TrashRefreshState::default();
        let first = state.begin_if_idle();
        let duplicate = state.begin_if_idle();

        assert!(first.is_some());
        assert!(duplicate.is_none());
    }
}
