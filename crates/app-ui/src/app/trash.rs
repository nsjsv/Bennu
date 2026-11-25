use std::path::PathBuf;

use file_core::{TrashEntry, TrashScan};
use iced::Task;

use super::FileBrowser;
use crate::commands::load_trash_command;
use crate::model::{
    BrowserLocation, BrowserTab, ContextMenuState, DirectoryCollectionPhase, Message,
    TrashContextMenuState, TrashRefreshCompletionDecision, TrashRefreshRequest,
};

impl FileBrowser {
    pub(super) fn handle_trash_blank_area_right_clicked(&mut self) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        self.preview_engine.clear_preview();
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.open_trash_context_menu(None, self.cursor_or_origin());
        rename_command
    }

    /// 配置后没有可见项时不进入菜单状态，与界面的面板选择同一谓词。
    pub(super) fn open_trash_context_menu(
        &mut self,
        target: Option<PathBuf>,
        position: iced::Point,
    ) {
        if self
            .user_config
            .context_menus
            .trash_items(target.is_some())
            .is_empty()
        {
            return;
        }
        self.context_menu = Some(ContextMenuState::Trash(TrashContextMenuState {
            target,
            position,
        }));
    }

    /// 侧栏回收站行不收拖放：拖着文件经过时清掉别处留下的落点。
    pub(super) fn handle_sidebar_trash_hovered(&mut self) -> Task<Message> {
        self.sidebar_trash_hovered = true;
        self.cursor_paste_directory = None;
        if self.file_drag.is_some() {
            self.set_file_drop_target(None);
        }
        Task::none()
    }

    pub(super) fn accept_trash_refresh_completion(
        &mut self,
        generation: u64,
        outcome: Result<TrashScan, String>,
    ) -> Task<Message> {
        match self.trash_refresh.classify_completion(generation) {
            TrashRefreshCompletionDecision::Discard => Task::none(),
            TrashRefreshCompletionDecision::StartReplacement => {
                self.refresh_trash_snapshot_after_change()
            }
            TrashRefreshCompletionDecision::Apply => self.apply_trash_refresh_outcome(outcome),
            TrashRefreshCompletionDecision::ApplyThenRefresh => {
                let applied = self.apply_trash_refresh_outcome(outcome);
                Task::batch([applied, self.refresh_trash_snapshot_after_change()])
            }
        }
    }

    fn apply_trash_refresh_outcome(&mut self, outcome: Result<TrashScan, String>) -> Task<Message> {
        let scan = match outcome {
            Ok(scan) => scan,
            Err(error) => {
                self.finish_trash_views_loading();
                self.trash_refresh.record_error(error);
                return if self.tab().location == BrowserLocation::Trash {
                    self.remeasure_active_file_drop_layout()
                } else {
                    Task::none()
                };
            }
        };
        let entries = scan.entries.clone();
        self.trash_refresh.replace_snapshot(scan);
        self.trash_refresh.clear_error();
        self.apply_trash_snapshot_to_all_views(&entries);

        let trash_pane_ids = self
            .pane_layout
            .visible_pane_ids()
            .into_iter()
            .filter(|pane_id| {
                self.pane_by_id(*pane_id)
                    .is_some_and(|pane| pane.tab().location == BrowserLocation::Trash)
            })
            .collect::<Vec<_>>();
        let clamp_command = self.clamp_pane_viewports(&trash_pane_ids);
        let thumbnail_commands = trash_pane_ids
            .into_iter()
            .map(|pane_id| self.schedule_thumbnail_refresh_for_pane(pane_id))
            .chain(std::iter::once(clamp_command))
            .collect::<Vec<_>>();
        let remeasure_drop_layout = if self.tab().location == BrowserLocation::Trash {
            self.remeasure_active_file_drop_layout()
        } else {
            Task::none()
        };
        Task::batch(
            thumbnail_commands
                .into_iter()
                .chain(std::iter::once(self.request_browser_session_save()))
                .chain(std::iter::once(remeasure_drop_layout)),
        )
    }

    fn apply_trash_snapshot_to_all_views(&mut self, entries: &[TrashEntry]) {
        for pane in &mut self.panes {
            for tab in pane
                .tabs
                .iter_mut()
                .filter(|tab| tab.location == BrowserLocation::Trash)
            {
                apply_trash_snapshot_to_tab(tab, entries);
            }
            if pane.tab().location == BrowserLocation::Trash {
                pane.directory_loading_placeholder = None;
            }
        }
        self.entries_replaced();
    }

    fn finish_trash_views_loading(&mut self) {
        for pane in &mut self.panes {
            if pane.tab().location == BrowserLocation::Trash {
                pane.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
                pane.directory_loading_placeholder = None;
            }
        }
    }

    /// 要显示回收站：有快照正在读就等它。
    pub(super) fn request_trash_snapshot_refresh(&mut self) -> Task<Message> {
        let request = self.trash_refresh.begin_if_idle();
        self.load_trash(request)
    }

    /// 回收站可能变了（磁盘监视报来、文件操作做完、用户要求刷新）：有标签显示回收站就保证再读
    /// 一次在这之后开始的；没有就丢掉快照，免得下次打开先显示过时的。
    pub(super) fn refresh_trash_snapshot_after_change(&mut self) -> Task<Message> {
        if !self.has_trash_tab() {
            self.trash_refresh.discard_snapshot();
            return Task::none();
        }
        let request = self.trash_refresh.begin_after_change();
        self.load_trash(request)
    }

    fn load_trash(&self, request: Option<TrashRefreshRequest>) -> Task<Message> {
        request.map_or_else(Task::none, |request| {
            load_trash_command(
                request.generation,
                self.options.clone(),
                request.cancellation,
            )
        })
    }

    pub(super) fn invalidate_trash_snapshot_refresh(&mut self) {
        self.trash_refresh.invalidate_pending();
    }

    pub(super) fn has_trash_tab(&self) -> bool {
        self.panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .any(|tab| tab.location == BrowserLocation::Trash)
    }
}

fn apply_trash_snapshot_to_tab(tab: &mut BrowserTab, entries: &[TrashEntry]) {
    tab.trash_entries = entries.to_vec();
    tab.entries = entries
        .iter()
        .map(|trash_entry| trash_entry.entry.clone())
        .collect::<Vec<_>>()
        .into();
    tab.retain_direct_entry_selection();
    tab.close_expanded_directories();
    tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
}
