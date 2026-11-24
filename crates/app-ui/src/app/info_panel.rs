//! 信息面板：看哪个目标（停靠区的悬停项、单选的、多选、没选中时的当前文件夹）、什么时候读、读到的
//! 结果对不对得上。单个条目用属性窗口同一个快速读取，另在后台读详细信息和 PDF 页数；多选和
//! 文件夹的数字用底栏已经算好的，不读盘。只在信息面板显示着时读（停靠区契约）。

use std::path::PathBuf;

use bennu_preview::info_overlay::InfoRow;
use iced::widget::text_editor;
use iced::Task;
use tokio_util::sync::CancellationToken;

use super::info_panel_rows::InfoPanelRows;
use super::FileBrowser;
use crate::info_panel::InfoField;
use crate::model::{DockMenuPanelSection, FilePropertiesSnapshot, Message};
use crate::right_dock::DockPanel;
use crate::selection_summary::PaneSelectionSummary;

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
    FieldToggled(InfoField),
    /// 某一行的值被点选、拖选、按键。`row_key` 是 `InfoValueRow::key`，行已经换掉时这条消息被丢弃。
    ValueAction {
        row_key: u64,
        action: text_editor::Action,
    },
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
    rows: InfoPanelRows,
}

impl InfoPanelState {
    pub(crate) fn target(&self) -> Option<&InfoTarget> {
        self.target.as_ref()
    }

    pub(crate) fn rows(&self) -> &InfoPanelRows {
        &self.rows
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
            InfoPanelMessage::FieldToggled(field) => {
                self.context_menu = None;
                self.user_config.info_panel.toggle(field);
                return self.persist_user_preferences_command();
            }
            InfoPanelMessage::ValueAction { row_key, action } => {
                self.info_panel.rows.perform(row_key, action);
            }
            InfoPanelMessage::MenuRequested => {
                self.context_menu = Some(self.right_dock_context_menu(Some(
                    DockMenuPanelSection::InfoFields(self.user_config.info_panel.clone()),
                )));
            }
        }
        Task::none()
    }

    /// `update` 出口：信息面板显示着时看目标变没变，变了就取消旧的、读新的，再把面板的文字
    /// 对到现在的状态上。文字的来源有目标、读到的东西、字段开关、选中数、文件夹数字、界面语言，
    /// 在出口统一比一次，比各个入口分别记着去重建不会漏。没显示时什么都不做，再显示时目标没变
    /// 就不重读。
    pub(super) fn sync_info_panel(&mut self) -> Task<Message> {
        if !self.right_dock.layout().shows(DockPanel::Info) {
            return Task::none();
        }
        let read = self.retarget_info_panel();
        let text = crate::view::info_panel_text(self);
        self.info_panel.rows.reconcile(text);
        read
    }

    fn retarget_info_panel(&mut self) -> Task<Message> {
        let target = self.info_panel_target();
        if target == self.info_panel.target {
            return Task::none();
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
                return Task::none();
            }
        };
        let (request, cancellation) = self.info_panel.replace_request();
        self.info_panel.facts = Some(EntryFacts {
            path: path.clone(),
            snapshot: None,
            details: None,
            pages: None,
        });
        crate::commands::info_panel_command(request, path, cancellation)
    }

    fn info_panel_target(&self) -> Option<InfoTarget> {
        let tab = self.tab();
        // 悬停项在场时它就是目标：多选的统计留到指针离开之后。
        if self.dock_hovered_item().is_none() && tab.selected_paths.len() > 1 {
            return Some(InfoTarget::Selection);
        }
        match self.dock_item() {
            Some(path) => Some(InfoTarget::Entry(path.to_path_buf())),
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
