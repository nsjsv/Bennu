mod atoms;
mod lifecycle;
mod protocol;
mod runtime;
mod selection;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use tokio::sync::mpsc::UnboundedSender;

pub use runtime::X11DndError;

static NEXT_TARGET_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONTROLLER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11DndWindowHandle {
    pub window_xid: u32,
    pub screen: usize,
}

impl X11DndWindowHandle {
    pub fn new(window_xid: u32, screen: usize) -> Self {
        Self { window_xid, screen }
    }
}

#[derive(Debug)]
pub struct X11DndController {
    id: u64,
    scale_generation: AtomicU64,
}

impl X11DndController {
    pub fn new(initial_scale_generation: u64) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_CONTROLLER_ID.fetch_add(1, Ordering::Relaxed),
            scale_generation: AtomicU64::new(initial_scale_generation),
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn scale_generation(&self) -> u64 {
        self.scale_generation.load(Ordering::Acquire)
    }

    pub fn set_scale_generation(&self, generation: u64) {
        self.scale_generation.store(generation, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11FileDropTargetSessionId(u64);

impl X11FileDropTargetSessionId {
    pub fn unique() -> Self {
        Self(NEXT_TARGET_SESSION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for X11FileDropTargetSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X11DndDropPosition {
    pub root_x: i16,
    pub root_y: i16,
    pub client_x: i16,
    pub client_y: i16,
    pub timestamp: u32,
    pub scale_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11FileDropTargetEvent {
    Entered {
        target_session_id: X11FileDropTargetSessionId,
        position: X11DndDropPosition,
    },
    Moved {
        target_session_id: X11FileDropTargetSessionId,
        position: X11DndDropPosition,
    },
    Left {
        target_session_id: X11FileDropTargetSessionId,
    },
    Dropped {
        target_session_id: X11FileDropTargetSessionId,
        position: X11DndDropPosition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11DndFileDrop {
    pub target_session_id: X11FileDropTargetSessionId,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11DndEvent {
    RuntimeReady,
    FileDropTarget(X11FileDropTargetEvent),
    FilesDropped(X11DndFileDrop),
    FileDropFailed {
        target_session_id: X11FileDropTargetSessionId,
        details: String,
    },
    MainWindowDestroyed,
    RuntimeFailed(String),
}

pub fn spawn_x11_file_dnd(
    window_handle: X11DndWindowHandle,
    controller: Arc<X11DndController>,
    event_sender: UnboundedSender<X11DndEvent>,
    shutdown_receiver: std::sync::mpsc::Receiver<()>,
) -> Result<thread::JoinHandle<()>, X11DndError> {
    thread::Builder::new()
        .name("x11-file-dnd".to_owned())
        .spawn(move || {
            if let Err(error) = runtime::run_x11_file_dnd(
                window_handle,
                controller,
                event_sender.clone(),
                shutdown_receiver,
            ) {
                let _ = event_sender.send(X11DndEvent::RuntimeFailed(error.to_string()));
            }
        })
        .map_err(|source| X11DndError::ThreadSpawn { source })
}
