use file_core::{DirectoryDiscovery, DirectoryDiscoveryBatch};
use iced::Task;

use super::FileBrowser;
use crate::model::{
    DirectoryExpansionLoadContext, DirectoryLoadFailure, ExpandedDirectory,
    ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus, Message,
};

impl FileBrowser {
    pub(super) fn accept_expanded_directory_discovery_batch(
        &mut self,
        request: ExpandedDirectoryLoadRequest,
        batch: DirectoryDiscoveryBatch,
    ) -> Task<Message> {
        if matches!(
            &request.context,
            DirectoryExpansionLoadContext::IconGrid { .. }
        ) {
            return self.accept_icon_grid_directory_discovery_batch(request, batch);
        }
        let mut entries = batch
            .entries
            .iter()
            .map(file_core::DiscoveredDirectoryEntry::display_entry)
            .collect::<Vec<_>>();
        file_core::sort_entries(&mut entries, &self.options);
        let Some(load_owner) = self.browser_tree_load_owner(&request) else {
            return Task::none();
        };
        let pane_id = load_owner.pane_id();
        if pane_id != self.active_pane_id() {
            let Some(pane) = self.pane_by_id_mut(pane_id) else {
                return Task::none();
            };
            let Some(expanded) = pane.expanded_directories.get_mut(&request.path) else {
                return Task::none();
            };
            if !expanded_load_request_matches_directory(&request, expanded) {
                return Task::none();
            }
            expanded.entries.extend(entries);
            return Task::none();
        }

        let Some(expanded) = self.expanded_directories.get_mut(&request.path) else {
            return Task::none();
        };
        if !expanded_load_request_matches_directory(&request, expanded) {
            return Task::none();
        }
        expanded.entries.extend(entries);
        Task::none()
    }

    pub(super) fn accept_expanded_directory_discovery(
        &mut self,
        request: ExpandedDirectoryLoadRequest,
        prebuilt: Result<crate::model::PrebuiltDirectoryDiscovery, DirectoryLoadFailure>,
    ) -> Task<Message> {
        if matches!(
            &request.context,
            DirectoryExpansionLoadContext::IconGrid { .. }
        ) {
            let entries =
                prebuilt.map(|prebuilt| std::sync::Arc::unwrap_or_clone(prebuilt.display_entries));
            return self.accept_icon_grid_directory_entries(request, entries);
        }
        let entries = prebuilt.map(|prebuilt| {
            (
                std::sync::Arc::unwrap_or_clone(prebuilt.display_entries),
                Some(prebuilt.discovery),
            )
        });
        self.accept_expanded_directory_entries(request, entries)
    }

    #[cfg(test)]
    pub(super) fn accept_complete_expanded_directory_fixture(
        &mut self,
        request: ExpandedDirectoryLoadRequest,
        scan: Result<file_core::DirectoryScan, DirectoryLoadFailure>,
    ) -> Task<Message> {
        if matches!(
            &request.context,
            DirectoryExpansionLoadContext::IconGrid { .. }
        ) {
            return self.accept_icon_grid_directory_entries(request, scan.map(|scan| scan.entries));
        }
        self.accept_expanded_directory_entries(request, scan.map(|scan| (scan.entries, None)))
    }

    fn accept_expanded_directory_entries(
        &mut self,
        request: ExpandedDirectoryLoadRequest,
        entries: Result<
            (Vec<file_core::DirectoryEntry>, Option<DirectoryDiscovery>),
            DirectoryLoadFailure,
        >,
    ) -> Task<Message> {
        let Some(load_owner) = self.browser_tree_load_owner(&request) else {
            return Task::none();
        };
        let pane_id = load_owner.pane_id();
        let options = self.options.clone();
        if pane_id != self.active_pane_id() {
            let (pending_failure, loaded_child_count) = {
                let Some(pane) = self.pane_by_id_mut(pane_id) else {
                    return Task::none();
                };
                let Some(expanded) = pane.expanded_directories.get_mut(&request.path) else {
                    return Task::none();
                };
                if !expanded_load_request_matches_directory(&request, expanded) {
                    return Task::none();
                }

                expanded.load_context = None;
                let mut pending_failure = None;
                let mut loaded_child_count = None;
                match entries {
                    Ok((entries, discovery)) => {
                        expanded.directory_order_phase =
                            super::navigation::directory_order_phase_after_collection(
                                &options, &entries,
                            );
                        expanded.entries = entries;
                        expanded.directory_discovery = discovery;
                        expanded.status = ExpandedDirectoryStatus::Loaded;
                        loaded_child_count = Some(expanded.entries.len());
                    }
                    Err(failure) => {
                        expanded.entries.clear();
                        expanded.directory_discovery = None;
                        expanded.load_cancel = None;
                        expanded.status = ExpandedDirectoryStatus::Error;
                        pending_failure = Some(failure);
                    }
                }
                pane.sync_active_tab_state();
                (pending_failure, loaded_child_count)
            };
            self.resort_size_sorted_list_panes();
            if let Some(loaded_child_count) = loaded_child_count {
                self.remember_loaded_list_directory_children(&request.path, loaded_child_count);
            }
            if let Some(failure) = pending_failure {
                self.accept_expanded_directory_load_failure(pane_id, &request.path, failure);
            }
            return Task::batch([
                self.schedule_visible_list_directory_summaries_for_pane(pane_id),
                self.schedule_visible_directory_metadata(pane_id, None),
            ]);
        }

        let loaded_path = request.path.clone();
        let loaded_successfully = entries.is_ok();
        let mut loaded_child_count = None;
        let pending_failure = {
            let Some(expanded) = self.expanded_directories.get_mut(&request.path) else {
                return Task::none();
            };
            if !expanded_load_request_matches_directory(&request, expanded) {
                return Task::none();
            }
            expanded.load_context = None;

            match entries {
                Ok((entries, discovery)) => {
                    expanded.directory_order_phase =
                        super::navigation::directory_order_phase_after_collection(
                            &options, &entries,
                        );
                    expanded.entries = entries;
                    expanded.directory_discovery = discovery;
                    expanded.status = ExpandedDirectoryStatus::Loaded;
                    loaded_child_count = Some(expanded.entries.len());
                    None
                }
                Err(failure) => {
                    expanded.entries.clear();
                    expanded.directory_discovery = None;
                    expanded.load_cancel = None;
                    expanded.status = ExpandedDirectoryStatus::Error;
                    Some(failure)
                }
            }
        };

        if let Some(loaded_child_count) = loaded_child_count {
            self.remember_loaded_list_directory_children(&request.path, loaded_child_count);
        }
        if let Some(failure) = pending_failure {
            self.accept_expanded_directory_load_failure(pane_id, &request.path, failure);
        }
        let pending_keyboard_focus = self.complete_pending_keyboard_column_focus(&loaded_path);
        let expansion_follow = if load_owner.advances_list_follow() {
            self.advance_list_expansion_follow(loaded_successfully)
        } else {
            Task::none()
        };
        let view_switch_reveal = self.complete_pending_view_switch_reveal(&loaded_path);
        let command = self.focus_created_entry_for_rename();
        self.sync_active_tab_state();
        self.resort_size_sorted_list_panes();
        Task::batch([
            pending_keyboard_focus,
            expansion_follow,
            view_switch_reveal,
            command,
            self.schedule_visible_list_directory_summaries_for_pane(pane_id),
            self.schedule_visible_directory_metadata(pane_id, None),
            self.schedule_thumbnail_refresh(),
            self.remeasure_active_file_drop_layout(),
        ])
    }
    fn browser_tree_load_owner(
        &self,
        request: &ExpandedDirectoryLoadRequest,
    ) -> Option<BrowserTreeLoadOwner> {
        match &request.context {
            DirectoryExpansionLoadContext::BrowserTree { pane_id } => {
                Some(BrowserTreeLoadOwner::Persistent(*pane_id))
            }
            DirectoryExpansionLoadContext::ListFollow { pane_id, .. }
                if self.list_expansion_follow_request_is_current(request) =>
            {
                Some(BrowserTreeLoadOwner::ListFollow(*pane_id))
            }
            DirectoryExpansionLoadContext::ListFollow { .. }
            | DirectoryExpansionLoadContext::IconGrid { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserTreeLoadOwner {
    Persistent(crate::model::BrowserPaneId),
    ListFollow(crate::model::BrowserPaneId),
}

impl BrowserTreeLoadOwner {
    fn pane_id(self) -> crate::model::BrowserPaneId {
        match self {
            Self::Persistent(pane_id) | Self::ListFollow(pane_id) => pane_id,
        }
    }

    fn advances_list_follow(self) -> bool {
        matches!(self, Self::ListFollow(_))
    }
}

fn expanded_load_request_matches_directory(
    request: &ExpandedDirectoryLoadRequest,
    expanded: &ExpandedDirectory,
) -> bool {
    request.generation == expanded.load_generation
        && expanded.load_context.as_ref() == Some(&request.context)
}
