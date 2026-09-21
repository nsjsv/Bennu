use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FILE_DROP_TARGET_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandFileDropTargetSessionId(u64);

impl WaylandFileDropTargetSessionId {
    pub fn unique() -> Self {
        Self(NEXT_FILE_DROP_TARGET_SESSION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for WaylandFileDropTargetSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
