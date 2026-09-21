use file_core::{TrashEntry, TrashScan};
use iced::Task;

use super::FileBrowser;
use crate::commands::{delayed_thumbnail_refresh_command, load_trash_command};
use crate::model::{
    trash_location_path, BrowserPane, BrowserTab, DirectoryCollectionPhase, Message,
    TrashRefreshCompletionDecision,
};

impl FileBrowser {
    pub(super) fn accept_trash_refresh_completion(
        &mut self,
        generation: u64,
        outcome: Result<TrashScan, String>,
    ) -> Task<Message> {
        match self.trash_refresh.classify_completion(generation) {
            TrashRefreshCompletionDecision::Discard => return Task::none(),
            TrashRefreshCompletionDecision::StartReplacement => {
                return self.refresh_trash_snapshot_for_trash_tabs();
            }
            TrashRefreshCompletionDecision::Apply => {}
        }
        let scan = match outcome {
            Ok(scan) => scan,
            Err(error) => {
                self.finish_trash_views_loading();
                self.trash_refresh.record_error(error);
                return if self.is_trash_view {
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

        let thumbnail_commands = self
            .pane_layout
            .visible_pane_ids()
            .into_iter()
            .filter(|pane_id| {
                self.pane_by_id(*pane_id)
                    .is_some_and(|pane| pane.is_trash_view)
            })
            .map(|pane_id| delayed_thumbnail_refresh_command(pane_id, trash_location_path()))
            .collect::<Vec<_>>();
        let remeasure_drop_layout = if self.is_trash_view {
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
        self.sync_active_pane_state();
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                if tab.is_trash_view {
                    apply_trash_snapshot_to_tab(tab, entries);
                }
            }
            if pane.is_trash_view {
                apply_trash_snapshot_to_pane(pane, entries);
            }
        }
        if let Some(active_pane) = self.pane_by_id(self.active_pane_id()).cloned() {
            self.apply_pane_browsing_snapshot(active_pane);
        }
    }

    fn finish_trash_views_loading(&mut self) {
        if self.is_trash_view {
            self.directory_collection_phase = DirectoryCollectionPhase::Ready;
            self.directory_loading_placeholder = None;
        }
        for pane in &mut self.panes {
            if pane.is_trash_view {
                pane.directory_collection_phase = DirectoryCollectionPhase::Ready;
                pane.directory_loading_placeholder = None;
                pane.sync_active_tab_state();
            }
        }
    }

    pub(super) fn request_trash_snapshot_refresh(&mut self) -> Task<Message> {
        let Some(request) = self.trash_refresh.begin_if_idle() else {
            return Task::none();
        };
        load_trash_command(
            request.generation,
            self.options.clone(),
            request.cancellation,
        )
    }

    pub(super) fn invalidate_trash_snapshot_refresh(&mut self) {
        self.trash_refresh.invalidate_pending();
    }

    pub(super) fn has_trash_tab(&self) -> bool {
        self.is_trash_view
            || self
                .panes
                .iter()
                .flat_map(|pane| pane.tabs.iter())
                .any(|tab| tab.is_trash_view)
    }

    pub(super) fn refresh_trash_snapshot_for_trash_tabs(&mut self) -> Task<Message> {
        if !self.has_trash_tab() {
            return Task::none();
        }
        self.request_trash_snapshot_refresh()
    }

    pub(super) fn refresh_trash_snapshot_for_visible_panes(&mut self) -> Task<Message> {
        if !self.has_trash_tab() {
            self.trash_refresh.discard_snapshot();
            return Task::none();
        }
        self.invalidate_trash_snapshot_refresh();
        self.request_trash_snapshot_refresh()
    }
}

fn apply_trash_snapshot_to_pane(pane: &mut BrowserPane, entries: &[TrashEntry]) {
    pane.current_dir = trash_location_path();
    pane.trash_entries = entries.to_vec();
    pane.entries = entries
        .iter()
        .map(|trash_entry| trash_entry.entry.clone())
        .collect::<Vec<_>>()
        .into();
    crate::model::retain_direct_entry_selection(
        &pane.entries,
        &mut pane.selected,
        &mut pane.selected_paths,
        &mut pane.selection_anchor,
    );
    pane.directory_loading_placeholder = None;
    pane.deepest_open_column_directory = None;
    for expanded in pane.expanded_directories.values_mut() {
        if let Some(cancellation) = expanded.load_cancel.take() {
            cancellation.cancel();
        }
    }
    pane.expanded_directories.clear();
    pane.directory_collection_phase = DirectoryCollectionPhase::Ready;
    pane.sync_active_tab_state();
}

fn apply_trash_snapshot_to_tab(tab: &mut BrowserTab, entries: &[TrashEntry]) {
    tab.directory = trash_location_path();
    tab.trash_entries = entries.to_vec();
    tab.entries = entries
        .iter()
        .map(|trash_entry| trash_entry.entry.clone())
        .collect::<Vec<_>>()
        .into();
    crate::model::retain_direct_entry_selection(
        &tab.entries,
        &mut tab.selected,
        &mut tab.selected_paths,
        &mut tab.selection_anchor,
    );
    tab.deepest_open_column_directory = None;
    for expanded in tab.expanded_directories.values_mut() {
        if let Some(cancellation) = expanded.load_cancel.take() {
            cancellation.cancel();
        }
    }
    tab.expanded_directories.clear();
    tab.directory_collection_phase = DirectoryCollectionPhase::Ready;
}
