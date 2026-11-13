//! 窗格发出的各种加载请求与它们的回信身份：目录扫描、元数据补齐、展开目录读取。

use std::path::PathBuf;

use file_core::DirectoryMetadataRequirement;

use super::{BrowserPaneId, IconGridExpansionSessionId, ListExpansionFollowSessionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryLoadRequest {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) path: PathBuf,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum DirectoryMetadataLoadContext {
    Root {
        pane_id: BrowserPaneId,
        path: PathBuf,
        collection_generation: u64,
    },
    Expanded {
        pane_id: BrowserPaneId,
        path: PathBuf,
        load_generation: u64,
    },
}

impl DirectoryMetadataLoadContext {
    pub(crate) fn pane_id(&self) -> BrowserPaneId {
        match self {
            Self::Root { pane_id, .. } | Self::Expanded { pane_id, .. } => *pane_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryMetadataLoadRequest {
    pub(crate) context: DirectoryMetadataLoadContext,
    pub(crate) request_generation: u64,
    pub(crate) requirement: DirectoryMetadataRequirement,
    pub(crate) targets: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryMetadataLoadFailure {
    Cancelled,
    ReadFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryExpansionLoadContext {
    BrowserTree {
        pane_id: BrowserPaneId,
    },
    ListFollow {
        pane_id: BrowserPaneId,
        tab_id: usize,
        current_dir: PathBuf,
        session_id: ListExpansionFollowSessionId,
    },
    IconGrid {
        pane_id: BrowserPaneId,
        current_dir: PathBuf,
        session_id: IconGridExpansionSessionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedDirectoryLoadRequest {
    pub(crate) context: DirectoryExpansionLoadContext,
    pub(crate) path: PathBuf,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryLoadFailure {
    DirectoryUnavailable { message: String },
    ReadFailed { message: String },
}
