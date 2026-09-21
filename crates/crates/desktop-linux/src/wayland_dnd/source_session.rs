use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const MAX_FILE_DRAG_ICON_EDGE: u32 = 512;
static NEXT_CONTROLLER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_FILE_DRAG_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaylandFileDragSourceEvent {
    Started(WaylandFileDragSessionId),
    Dropped(WaylandFileDragSessionId),
    Finished(WaylandFileDragSessionId),
    Cancelled(WaylandFileDragSessionId),
    Rejected {
        session_id: WaylandFileDragSessionId,
        details: String,
    },
}

impl WaylandFileDragSourceEvent {
    pub fn session_id(&self) -> WaylandFileDragSessionId {
        match self {
            Self::Started(session_id)
            | Self::Dropped(session_id)
            | Self::Finished(session_id)
            | Self::Cancelled(session_id)
            | Self::Rejected { session_id, .. } => *session_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandFileDragSessionId(u64);

impl fmt::Display for WaylandFileDragSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone)]
pub struct WaylandFileDragIcon {
    width: u32,
    height: u32,
    premultiplied_rgba: Vec<u8>,
}

impl WaylandFileDragIcon {
    pub fn new(
        width: u32,
        height: u32,
        premultiplied_rgba: Vec<u8>,
    ) -> Result<Self, WaylandFileDragIconError> {
        if width == 0
            || height == 0
            || width > MAX_FILE_DRAG_ICON_EDGE
            || height > MAX_FILE_DRAG_ICON_EDGE
        {
            return Err(WaylandFileDragIconError::InvalidDimensions { width, height });
        }
        let expected = width as usize * height as usize * 4;
        if premultiplied_rgba.len() != expected {
            return Err(WaylandFileDragIconError::InvalidPixelLength {
                width,
                height,
                expected,
                actual: premultiplied_rgba.len(),
            });
        }

        Ok(Self {
            width,
            height,
            premultiplied_rgba,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn premultiplied_rgba(&self) -> &[u8] {
        &self.premultiplied_rgba
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WaylandFileDragIconError {
    #[error("Wayland file drag icon dimensions {width}x{height} are outside 1..={MAX_FILE_DRAG_ICON_EDGE}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error(
        "Wayland file drag icon {width}x{height} requires {expected} premultiplied RGBA bytes, received {actual}"
    )]
    InvalidPixelLength {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
}

#[derive(Debug)]
pub enum WaylandDndCommand {
    StartFileDrag {
        session_id: WaylandFileDragSessionId,
        paths: Vec<PathBuf>,
        icon: WaylandFileDragIcon,
    },
}

#[derive(Debug)]
pub struct WaylandDndController {
    id: u64,
    command_sender: UnboundedSender<WaylandDndCommand>,
    command_receiver: Mutex<Option<UnboundedReceiver<WaylandDndCommand>>>,
}

impl WaylandDndController {
    pub fn new() -> Arc<Self> {
        let (command_sender, command_receiver) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Self {
            id: NEXT_CONTROLLER_ID.fetch_add(1, Ordering::Relaxed),
            command_sender,
            command_receiver: Mutex::new(Some(command_receiver)),
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn start_file_drag(
        &self,
        paths: Vec<PathBuf>,
        icon: WaylandFileDragIcon,
    ) -> Result<WaylandFileDragSessionId, WaylandDndCommandError> {
        if paths.is_empty() {
            return Err(WaylandDndCommandError::NoPaths);
        }
        let session_id =
            WaylandFileDragSessionId(NEXT_FILE_DRAG_SESSION_ID.fetch_add(1, Ordering::Relaxed));
        self.command_sender
            .send(WaylandDndCommand::StartFileDrag {
                session_id,
                paths,
                icon,
            })
            .map_err(|_| WaylandDndCommandError::WorkerStopped)?;
        Ok(session_id)
    }

    pub(super) fn take_command_receiver(&self) -> Option<UnboundedReceiver<WaylandDndCommand>> {
        self.command_receiver.lock().ok()?.take()
    }
}

#[derive(Debug, Error)]
pub enum WaylandDndCommandError {
    #[error("Wayland file drag requires at least one source path")]
    NoPaths,
    #[error("Wayland drag-and-drop worker is not running")]
    WorkerStopped,
}
