//! 信息面板：看哪个目标（悬停的、单选的、多选、没选中时的当前文件夹）、什么时候读、读到的
//! 结果对不对得上。单个条目用属性窗口同一个快速读取，另在后台读详细信息和 PDF 页数；多选和
//! 文件夹的数字用底栏已经算好的，不读盘。只在信息面板显示着时读（停靠区契约）。

use std::path::PathBuf;
use std::time::Duration;

use bennu_preview::info_overlay::InfoRow;
use iced::Task;
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::info_panel::InfoField;
use crate::model::{
    ContextMenuState, DockMenuPanelSection, FilePropertiesSnapshot, Message, RightDockMenuState,
};
use crate::right_dock::DockPanel;
use crate::selection_summary::PaneSelectionSummary;

/// 指针在一个条目上停这么久才换成显示它：划过一串文件时不挨个读、挨个跑 ffprobe。
const HOVER_SETTLE_DELAY: Duration = Duration::from_millis(300);

#[derive(Debug, Clone)]
pub(crate) enum InfoPanelMessage {
    /// 快速读取到了；是图片、音视频、PDF 的，详细信息、页数后面再来。
    Loaded {
        request: u64,
        outcome: Result<Box<FilePropertiesSnapshot>, String>,
    },
    DetailsLoaded {
        request: u64,
        outcome: Result<Vec<InfoRow>, String>,
    },
    PagesLoaded {
        request: u64,
        pages: usize,
    },
    HoverSettled {
        generation: u64,
    },
    FieldToggled(InfoField),
    FollowHoverToggled,
    MenuRequested,
}

/// 面板在显示的东西。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InfoTarget {
    /// 一个条目：单选的，或悬停的那个。
    Entry(PathBuf),
    /// 选中了好几个。
    Selection,
    /// 什么都没选：当前文件夹。
    Folder(PathBuf),
}

/// 一个条目读到的东西；字段读到才有，没到的不显示。
#[derive(Debug, Clone)]
pub(crate) struct EntryFacts {
    pub(crate) path: PathBuf,
    pub(crate) snapshot: Option<Result<FilePropertiesSnapshot, String>>,
    pub(crate) details: Option<Result<Vec<InfoRow>, String>>,
    pub(crate) pages: Option<usize>,
}

/// 没选中时当前文件夹的数字（列表里的，跟隐藏文件开关走；文件大小不递归）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FolderCounts {
    pub(crate) items: usize,
    pub(crate) visible_files_bytes: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct InfoPanelState {
    target: Option<InfoTarget>,
    request: u64,
    cancellation: Option<CancellationToken>,
    facts: Option<EntryFacts>,
    /// 指针停在上面、还没等够的条目，和它的定时号。
    hover_pending: Option<(PathBuf, u64)>,
    hover_generation: u64,
    /// 等够了的悬停条目：面板正显示它。
    hover_settled: Option<PathBuf>,
}

impl InfoPanelState {
    pub(crate) fn target(&self) -> Option<&InfoTarget> {
        self.target.as_ref()
    }

    /// 读到的东西，而且就是面板正显示的那个条目的（防陈旧）。
    pub(crate) fn facts(&self) -> Option<&EntryFacts> {
        let facts = self.facts.as_ref()?;
        match &self.target {
            Some(InfoTarget::Entry(path) | InfoTarget::Folder(path)) if *path == facts.path => {
                Some(facts)
            }
            _ => None,
        }
    }

    fn replace_request(&mut self) -> (u64, CancellationToken) {
        if let Some(previous) = self.cancellation.take() {
            previous.cancel();
        }
        self.request += 1;
        let cancellation = CancellationToken::new();
        self.cancellation = Some(cancellation.clone());
        (self.request, cancellation)
    }

    fn facts_for(&mut self, request: u64) -> Option<&mut EntryFacts> {
        (request == self.request).then_some(())?;
        self.facts.as_mut()
    }
}

impl FileBrowser {
    pub(super) fn handle_info_panel_message(&mut self, message: InfoPanelMessage) -> Task<Message> {
        match message {
            InfoPanelMessage::Loaded { request, outcome } => {
                if let Some(facts) = self.info_panel.facts_for(request) {
                    facts.snapshot = Some(outcome.map(|snapshot| *snapshot));
                }
            }
            InfoPanelMessage::DetailsLoaded { request, outcome } => {
                if let Some(facts) = self.info_panel.facts_for(request) {
                    facts.details = Some(outcome);
                }
            }
            InfoPanelMessage::PagesLoaded { request, pages } => {
                if let Some(facts) = self.info_panel.facts_for(request) {
                    facts.pages = Some(pages);
                }
            }
            InfoPanelMessage::HoverSettled { generation } => {
                if let Some((path, pending)) = self.info_panel.hover_pending.take() {
                    if pending == generation {
                        self.info_panel.hover_settled = Some(path);
                    } else {
                        self.info_panel.hover_pending = Some((path, pending));
                    }
                }
            }
            InfoPanelMessage::FieldToggled(field) => {
                self.context_menu = None;
                self.user_config.info_panel.toggle(field);
                return self.persist_user_preferences_command();
            }
            InfoPanelMessage::FollowHoverToggled => {
                self.context_menu = None;
                self.user_config.info_panel.follow_hover =
                    !self.user_config.info_panel.follow_hover;
                return self.persist_user_preferences_command();
            }
            InfoPanelMessage::MenuRequested => {
                let layout = self.right_dock.layout();
                self.context_menu = Some(ContextMenuState::RightDock(RightDockMenuState {
                    position: self.cursor_or_origin(),
                    open_panels: DockPanel::ALL
                        .into_iter()
                        .filter(|panel| layout.contains(*panel))
                        .collect(),
                    locked: layout.is_locked(),
                    panel_section: Some(DockMenuPanelSection::InfoFields(
                        self.user_config.info_panel.clone(),
                    )),
                }));
            }
        }
        Task::none()
    }

    /// `update` 出口：信息面板显示着时看目标变没变，变了就取消旧的、读新的。没显示时什么都不做，
    /// 再显示时目标没变就不重读。
    pub(super) fn sync_info_panel(&mut self) -> Task<Message> {
        if !self.right_dock.layout().shows(DockPanel::Info) {
            return Task::none();
        }
        let hover = self.track_info_panel_hover();
        let target = self.info_panel_target();
        if target == self.info_panel.target {
            return hover;
        }
        self.info_panel.target = target.clone();
        let path = match target {
            Some(InfoTarget::Entry(path) | InfoTarget::Folder(path))
                // 包里面的虚拟路径读不了，只显示名字。
                if file_core::archive_path_identity(&path)
                    != file_core::ArchivePathIdentity::InsideArchive =>
            {
                path
            }
            _ => {
                if let Some(previous) = self.info_panel.cancellation.take() {
                    previous.cancel();
                }
                self.info_panel.facts = None;
                return hover;
            }
        };
        let (request, cancellation) = self.info_panel.replace_request();
        self.info_panel.facts = Some(EntryFacts {
            path: path.clone(),
            snapshot: None,
            details: None,
            pages: None,
        });
        Task::batch([
            hover,
            crate::commands::info_panel_command(request, path, cancellation),
        ])
    }

    /// 悬停开着、指针在某个条目上时等它停稳；离开条目立刻回到选中项。
    fn track_info_panel_hover(&mut self) -> Task<Message> {
        let hovered = self
            .user_config
            .info_panel
            .follow_hover
            .then(|| self.hovered_entry.clone())
            .flatten();
        let state = &mut self.info_panel;
        let Some(path) = hovered else {
            state.hover_pending = None;
            state.hover_settled = None;
            return Task::none();
        };
        let waiting = state
            .hover_pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == path);
        if waiting || state.hover_settled.as_ref() == Some(&path) {
            return Task::none();
        }
        state.hover_generation += 1;
        let generation = state.hover_generation;
        state.hover_pending = Some((path, generation));
        // 定时器在任务跑起来时才建：建它要在 tokio 运行时里。
        Task::perform(
            async { tokio::time::sleep(HOVER_SETTLE_DELAY).await },
            move |()| Message::InfoPanel(InfoPanelMessage::HoverSettled { generation }),
        )
    }

    fn info_panel_target(&self) -> Option<InfoTarget> {
        if let Some(path) = &self.info_panel.hover_settled {
            return Some(InfoTarget::Entry(path.clone()));
        }
        let tab = self.tab();
        if tab.selected_paths.len() > 1 {
            return Some(InfoTarget::Selection);
        }
        match &tab.selected {
            Some(path) => Some(InfoTarget::Entry(path.clone())),
            None => tab
                .current_directory()
                .map(|directory| InfoTarget::Folder(directory.to_path_buf())),
        }
    }

    /// 多选时的个数和大小：底栏同一份统计。
    pub(crate) fn info_panel_selection(&self) -> Option<PaneSelectionSummary> {
        self.pane_selection_summary(self.pane_view(self.active_pane_id())?)
    }

    /// 没选中时当前文件夹的项数和文件合计：列表里的，不读盘。
    pub(crate) fn info_panel_folder_counts(&self) -> FolderCounts {
        FolderCounts {
            items: self.tab().entries.len(),
            visible_files_bytes: self
                .displayed_directory_totals(self.active_pane_id())
                .and_then(|totals| totals.visible_files_bytes),
        }
    }
}

#[cfg(test)]
#[path = "info_panel_tests.rs"]
mod tests;
