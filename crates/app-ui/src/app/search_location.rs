//! 搜索作为标签的一个位置：进入、离开、回信按会话号找回标签、结果变化后的善后。

use std::path::{Path, PathBuf};

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::list_view::{ListGeometry, LIST_HEADER_HEIGHT};
use crate::model::search::SearchRecall;
use crate::model::{
    empty_directory_entry_snapshot, BrowserLocation, BrowserPaneId, BrowserTab, BrowserViewMode,
    DirectoryCollectionPhase, ListColumnKind, Message, NavigationMode, SearchWorkspaceSessionId,
    SearchWorkspaceState,
};
use crate::transfer_placeholder_view::{
    build_list_transfer_rows, list_transfer_rows_content_height,
};

impl FileBrowser {
    /// 这次搜索所在的标签（任一窗格）。
    pub(super) fn search_tab(&self, session_id: SearchWorkspaceSessionId) -> Option<&BrowserTab> {
        self.panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .find(|tab| is_search_session(tab, session_id))
    }

    pub(super) fn search_tab_mut(
        &mut self,
        session_id: SearchWorkspaceSessionId,
    ) -> Option<&mut BrowserTab> {
        self.panes
            .iter_mut()
            .flat_map(|pane| pane.tabs.iter_mut())
            .find(|tab| is_search_session(tab, session_id))
    }

    /// 正显示这次搜索的窗格；搜索在后台标签里时没有。
    fn pane_showing_search(&self, session_id: SearchWorkspaceSessionId) -> Option<BrowserPaneId> {
        self.panes
            .iter()
            .find(|pane| is_search_session(pane.tab(), session_id))
            .map(|pane| pane.id)
    }

    /// 结果或进度变了：只有正显示这次搜索的窗格要重新收视口、取缩略图和文件夹摘要。
    pub(super) fn search_results_changed(
        &mut self,
        session_id: SearchWorkspaceSessionId,
    ) -> Task<Message> {
        let Some(pane_id) = self.pane_showing_search(session_id) else {
            return Task::none();
        };
        if pane_id == self.active_pane_id() {
            self.entries_replaced();
        }
        Task::batch([
            self.clamp_pane_viewports(&[pane_id]),
            self.schedule_thumbnail_refresh_for_pane(pane_id),
            self.schedule_visible_list_directory_summaries_for_pane(pane_id),
            self.remeasure_active_file_drop_layout(),
        ])
    }

    /// 活动标签进入一次搜索。与进入文件夹同一套清理：离开的位置记进历史（`mode` 要求时），
    /// 展开、选择、视口、在途的目录读取都作废。
    pub(super) fn enter_search_location(
        &mut self,
        search: SearchWorkspaceState,
        mode: NavigationMode,
    ) -> Task<Message> {
        self.cancel_expansion_follow_plans();
        let cancel_address_editing = self.cancel_address_editing();
        let tab = self.tab_mut();
        if mode == NavigationMode::RecordHistory {
            if let Some(entry) = tab.history_entry() {
                tab.back_stack.push(entry);
                tab.forward_stack.clear();
            }
        }
        self.clear_icon_grid_expansion();
        if let Some(drag) = &mut self.file_drag {
            drag.column_directories_snapshot.clear();
        }
        let tab = self.tab_mut();
        tab.location = BrowserLocation::Search(Box::new(search));
        tab.trash_entries.clear();
        tab.directory_discovery = None;
        tab.close_expanded_directories();
        tab.clear_selection();
        tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
        self.set_entries(empty_directory_entry_snapshot());
        let pane = self.pane_mut();
        pane.reset_directory_view();
        pane.directory_loading_placeholder = None;
        pane.cancel_directory_load();
        self.clear_selection_context();
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);
        self.follow_terminal_panel_directory();
        self.prune_list_directory_summaries_to_live_roots();
        Task::batch([
            cancel_address_editing,
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ])
    }

    /// 后退或前进回到一次搜索：按记下的意图建一次新搜索并重搜。
    pub(super) fn return_to_search(&mut self, recall: SearchRecall) -> Task<Message> {
        let search = SearchWorkspaceState::from_recall(
            recall,
            self.home_dir.clone(),
            self.next_search_session_id(),
        );
        let session_id = search.session_id();
        let entered = self.enter_search_location(search, NavigationMode::KeepHistory);
        Task::batch([entered, self.restart_search(session_id)])
    }

    /// 搜索结果滚到离末尾不到一屏时取下一页。
    pub(super) fn load_more_search_results_near_end(
        &mut self,
        pane_id: BrowserPaneId,
    ) -> Task<Message> {
        let Some(pane) = self.pane_view(pane_id) else {
            return Task::none();
        };
        if pane.location.search().is_none() {
            return Task::none();
        }
        let (offset_y, viewport_height, content_height) = match pane.view_mode {
            BrowserViewMode::Icons => {
                let viewport = pane.icon_grid_viewport;
                let layout = self.icon_grid_layout_for_pane(pane);
                (viewport.offset_y, viewport.height, layout.total_height())
            }
            BrowserViewMode::List | BrowserViewMode::Columns => {
                let Some(viewport) = pane.list_viewport else {
                    return Task::none();
                };
                let row_height =
                    ListGeometry::for_level(self.user_config.list_view_density).row_height;
                let rows = build_list_transfer_rows(self, pane, row_height);
                (
                    viewport.offset_y,
                    viewport.height,
                    LIST_HEADER_HEIGHT + list_transfer_rows_content_height(&rows, row_height),
                )
            }
        };
        if content_height - offset_y - viewport_height > viewport_height {
            return Task::none();
        }
        self.load_more_search_results(pane_id)
    }

    /// 窗格正显示的搜索。
    pub(super) fn pane_search_session(
        &self,
        pane_id: BrowserPaneId,
    ) -> Option<SearchWorkspaceSessionId> {
        self.pane_view(pane_id)?
            .location
            .search()
            .map(SearchWorkspaceState::session_id)
    }

    /// 搜索结果里点表头：换这次搜索的顺序，搜索服务对全部结果重排，从第一页重新取。
    pub(super) fn sort_search_by_column(
        &mut self,
        session_id: SearchWorkspaceSessionId,
        column: ListColumnKind,
    ) -> Task<Message> {
        let directories_first = self.options.directories_first;
        let Some(search) = self
            .search_tab_mut(session_id)
            .and_then(|tab| tab.location.search_mut())
        else {
            return Task::none();
        };
        if !search.click_sort_column(column, directories_first) {
            return Task::none();
        }
        self.restart_search(session_id)
    }

    /// 搜索结果是搜索那一刻的样子，在别处删掉的文件还留在结果里。打开前确认它还在；不在就
    /// 提示并从结果里拿掉，返回善后任务。还在（或不在搜索里）时返回 None。
    pub(super) fn drop_missing_search_result(&mut self, path: &Path) -> Option<Task<Message>> {
        self.active_search()?;
        let missing = matches!(
            std::fs::symlink_metadata(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        );
        if !missing {
            return None;
        }
        let name = path
            .file_name()
            .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
        self.show_global_error(tr!("“%1” no longer exists", name));
        Some(self.remove_deleted_search_results(&[path.to_path_buf()]))
    }

    /// 删掉或移到回收站的文件从所有搜索结果里拿掉（含删掉的文件夹里的结果）。
    pub(super) fn remove_deleted_search_results(
        &mut self,
        removed_paths: &[PathBuf],
    ) -> Task<Message> {
        let mut changed = Vec::new();
        for tab in self.panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
            if let Some(session_id) = tab.location.search().map(SearchWorkspaceState::session_id) {
                if tab.remove_search_results(removed_paths) {
                    changed.push(session_id);
                }
            }
        }
        Task::batch(
            changed
                .into_iter()
                .map(|session_id| self.search_results_changed(session_id))
                .collect::<Vec<_>>(),
        )
    }

    /// 拆分窗格复制出的搜索标签换一个会话号，免得两个标签争同一份回信。
    pub(super) fn rebind_copied_search(
        &mut self,
        tab: &mut BrowserTab,
    ) -> Option<SearchWorkspaceSessionId> {
        let session_id = self.next_search_session_id();
        tab.location.search_mut()?.rebind_session(session_id);
        Some(session_id)
    }

    /// 复制出的搜索标签按同样的意图重搜一次（它不持有原件在途的请求）。
    pub(super) fn restart_copied_search(
        &mut self,
        session_id: Option<SearchWorkspaceSessionId>,
    ) -> Task<Message> {
        session_id.map_or_else(Task::none, |session_id| self.restart_search(session_id))
    }
}

fn is_search_session(tab: &BrowserTab, session_id: SearchWorkspaceSessionId) -> bool {
    tab.location
        .search()
        .is_some_and(|search| search.session_id() == session_id)
}
