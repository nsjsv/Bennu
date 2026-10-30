use std::path::Path;

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::commands::load_directory_command;
use crate::model::{
    BrowserPaneId, DirectoryCollectionPhase, DirectoryLoadFailure, DirectoryLoadRequest, Message,
    NavigationMode,
};

impl FileBrowser {
    pub(super) fn accept_directory_load_failure(
        &mut self,
        request: DirectoryLoadRequest,
        failure: DirectoryLoadFailure,
    ) -> Task<Message> {
        let request_is_current = self.pane_by_id(request.pane_id).is_some_and(|pane| {
            request.generation == pane.directory_load_generation
                && request.path == pane.tab().current_dir
        });
        if !request_is_current {
            return Task::none();
        }

        match failure {
            DirectoryLoadFailure::DirectoryUnavailable { message } => {
                self.recover_unavailable_directory(request, message)
            }
            DirectoryLoadFailure::ReadFailed { message } => {
                self.finish_failed_directory_load(request.pane_id);
                self.show_global_error_from(GlobalErrorSource::DirectoryLoad, message);
                Task::none()
            }
        }
    }

    /// 目录消失：退回最近的存活上级。活动窗格走完整导航（清交互上下文、
    /// 地址栏同步），其它窗格只换自己标签的目录并重新加载。
    fn recover_unavailable_directory(
        &mut self,
        request: DirectoryLoadRequest,
        unavailable_message: String,
    ) -> Task<Message> {
        let Some(parent_directory) = request.path.parent().map(ToOwned::to_owned) else {
            self.finish_failed_directory_load(request.pane_id);
            self.show_global_error_from(GlobalErrorSource::DirectoryLoad, unavailable_message);
            return Task::none();
        };

        if request.pane_id == self.active_pane_id() {
            self.tab_mut()
                .forget_history_into(&request.path, &parent_directory);
            return self.navigate_to(parent_directory, NavigationMode::KeepHistory);
        }

        self.pane_by_id_mut(request.pane_id)
            .expect("the failed request belongs to an existing pane")
            .recover_from_unavailable_directory(&request.path, parent_directory.clone());
        self.follow_terminal_panel_directory();
        let (parent_request, cancellation) =
            self.begin_directory_load(request.pane_id, parent_directory);
        Task::batch([
            load_directory_command(parent_request, self.options.clone(), cancellation),
            self.request_browser_session_save(),
        ])
    }

    fn finish_failed_directory_load(&mut self, pane_id: BrowserPaneId) {
        let Some(pane) = self.pane_by_id_mut(pane_id) else {
            return;
        };
        pane.tab_mut().directory_collection_phase = DirectoryCollectionPhase::Ready;
        pane.directory_loading_placeholder = None;
        pane.directory_load_cancel = None;
    }

    pub(super) fn accept_expanded_directory_load_failure(
        &mut self,
        pane_id: BrowserPaneId,
        unavailable_directory: &Path,
        failure: DirectoryLoadFailure,
    ) {
        match failure {
            DirectoryLoadFailure::DirectoryUnavailable { .. } => {
                self.discard_unavailable_expanded_directory(pane_id, unavailable_directory);
            }
            DirectoryLoadFailure::ReadFailed { message } => {
                self.show_global_error_from(GlobalErrorSource::DirectoryLoad, message)
            }
        }
    }

    fn discard_unavailable_expanded_directory(
        &mut self,
        pane_id: BrowserPaneId,
        unavailable_directory: &Path,
    ) {
        if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.discard_unavailable_directory_subtree(unavailable_directory);
        }
        self.entries_replaced();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    use super::*;
    use crate::app::global_error::{recorded_global_error_count, reset_recorded_global_errors};
    use crate::config;
    use crate::model::{
        BrowserPane, BrowserTab, DirectoryExpansionLoadContext, ExpandedDirectory,
        ExpandedDirectoryStatus,
    };
    use crate::thumbnail_cache::ColumnViewport;

    #[test]
    fn unavailable_active_directory_recovers_to_parent_without_global_error() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let parent_directory = PathBuf::from("/workspace/project");
        let unavailable_directory = parent_directory.join("removed");
        browser.tab_mut().current_dir = unavailable_directory.clone();
        browser.pane_mut().directory_load_generation = 7;
        browser.pane_mut().directory_load_cancel = Some(tokio_util::sync::CancellationToken::new());
        browser.tab_mut().directory_collection_phase =
            crate::model::DirectoryCollectionPhase::Discovering;
        browser.tab_mut().back_stack = vec![
            parent_directory.clone(),
            unavailable_directory.join("older-child"),
        ];
        browser.tab_mut().forward_stack = vec![unavailable_directory.join("newer-child")];
        reset_recorded_global_errors();

        drop(browser.accept_directory_load_failure(
            DirectoryLoadRequest {
                pane_id: BrowserPaneId::PRIMARY,
                path: unavailable_directory.clone(),
                generation: 7,
            },
            unavailable_failure(&unavailable_directory),
        ));

        assert_eq!(browser.tab().current_dir, parent_directory.clone());
        assert!(browser.tab().directory_collection_phase.is_discovering());
        assert_eq!(browser.pane().directory_load_generation, 8);
        assert_eq!(browser.current_error(), None);
        assert_eq!(recorded_global_error_count(), 0);
        assert!(browser.tab().back_stack.is_empty());
        assert!(browser.tab().forward_stack.is_empty());
    }

    #[test]
    fn consecutive_unavailable_directories_recover_to_the_first_available_ancestor() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let available_ancestor = PathBuf::from("/workspace");
        let unavailable_parent = available_ancestor.join("project");
        let unavailable_directory = unavailable_parent.join("removed");
        browser.tab_mut().current_dir = unavailable_directory.clone();
        browser.pane_mut().directory_load_generation = 7;
        browser.tab_mut().directory_collection_phase =
            crate::model::DirectoryCollectionPhase::Discovering;
        reset_recorded_global_errors();

        drop(browser.accept_directory_load_failure(
            DirectoryLoadRequest {
                pane_id: BrowserPaneId::PRIMARY,
                path: unavailable_directory.clone(),
                generation: 7,
            },
            unavailable_failure(&unavailable_directory),
        ));
        assert_eq!(browser.tab().current_dir, unavailable_parent.clone());
        assert_eq!(browser.pane().directory_load_generation, 8);

        drop(browser.accept_directory_load_failure(
            DirectoryLoadRequest {
                pane_id: BrowserPaneId::PRIMARY,
                path: unavailable_parent.clone(),
                generation: 8,
            },
            unavailable_failure(&unavailable_parent),
        ));

        assert_eq!(browser.tab().current_dir, available_ancestor);
        assert_eq!(browser.pane().directory_load_generation, 9);
        assert!(browser.tab().directory_collection_phase.is_discovering());
        assert_eq!(browser.current_error(), None);
        assert_eq!(recorded_global_error_count(), 0);
    }

    #[test]
    fn stale_unavailable_directory_failure_does_not_change_navigation_state() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let current_directory = PathBuf::from("/workspace/current");
        let stale_directory = PathBuf::from("/workspace/stale");
        browser.tab_mut().current_dir = current_directory.clone();
        browser.pane_mut().directory_load_generation = 9;
        browser.tab_mut().directory_collection_phase =
            crate::model::DirectoryCollectionPhase::Discovering;
        reset_recorded_global_errors();

        drop(browser.accept_directory_load_failure(
            DirectoryLoadRequest {
                pane_id: BrowserPaneId::PRIMARY,
                path: stale_directory.clone(),
                generation: 8,
            },
            unavailable_failure(&stale_directory),
        ));

        assert_eq!(browser.tab().current_dir, current_directory);
        assert_eq!(browser.pane().directory_load_generation, 9);
        assert!(browser.tab().directory_collection_phase.is_discovering());
        assert_eq!(browser.current_error(), None);
        assert_eq!(recorded_global_error_count(), 0);
    }

    #[test]
    fn unavailable_inactive_directory_recovers_its_own_pane() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let inactive_pane_id = BrowserPaneId(9);
        let parent_directory = PathBuf::from("/workspace/secondary");
        let unavailable_directory = parent_directory.join("removed");
        let mut inactive_pane = BrowserPane::new(
            inactive_pane_id,
            BrowserTab::directory(0, unavailable_directory.clone()),
        );
        inactive_pane.directory_load_generation = 12;
        browser.panes.push(inactive_pane);
        reset_recorded_global_errors();

        drop(browser.accept_directory_load_failure(
            DirectoryLoadRequest {
                pane_id: inactive_pane_id,
                path: unavailable_directory.clone(),
                generation: 12,
            },
            unavailable_failure(&unavailable_directory),
        ));

        let recovered_pane = browser.pane_by_id(inactive_pane_id).expect("inactive pane");
        assert_eq!(recovered_pane.tab().current_dir, parent_directory);
        assert!(recovered_pane
            .tab()
            .directory_collection_phase
            .is_discovering());
        assert_eq!(recovered_pane.directory_load_generation, 13);
        assert_eq!(browser.current_error(), None);
        assert_eq!(recorded_global_error_count(), 0);
    }

    #[test]
    fn unavailable_root_child_closes_its_column_chain_without_global_error() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root_directory = PathBuf::from("/workspace");
        let unavailable_directory = root_directory.join("removed");
        let unavailable_child = unavailable_directory.join("child");
        let remaining_entry = root_directory.join("remaining.txt");
        browser.tab_mut().current_dir = root_directory.clone();
        browser.tab_mut().deepest_open_column_directory = Some(unavailable_child.clone());
        browser.tab_mut().entries = vec![
            test_entry(unavailable_directory.clone(), FileKind::Directory),
            test_entry(remaining_entry.clone(), FileKind::File),
        ]
        .into();
        browser.tab_mut().expanded_directories.insert(
            unavailable_directory.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 4),
        );
        browser.tab_mut().expanded_directories.insert(
            unavailable_child.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 2),
        );
        reset_recorded_global_errors();

        drop(browser.accept_complete_expanded_directory_fixture(
            crate::model::ExpandedDirectoryLoadRequest {
                context: DirectoryExpansionLoadContext::BrowserTree {
                    pane_id: BrowserPaneId::PRIMARY,
                },
                path: unavailable_directory.clone(),
                generation: 4,
            },
            Err(unavailable_failure(&unavailable_directory)),
        ));

        assert_eq!(
            browser
                .tab()
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![remaining_entry]
        );
        assert!(browser
            .tab()
            .expanded_directories
            .keys()
            .all(|path| !path.starts_with(&unavailable_directory)));
        assert_eq!(browser.tab().deepest_open_column_directory, None);
        assert_eq!(
            crate::three_column_view::column_directories(&browser),
            vec![root_directory]
        );
        assert_eq!(browser.current_error(), None);
        assert_eq!(recorded_global_error_count(), 0);
    }

    #[test]
    fn unavailable_middle_column_preserves_ancestor_columns_and_cleans_descendants() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root_directory = PathBuf::from("/workspace");
        let ancestor_directory = root_directory.join("A");
        let unavailable_directory = ancestor_directory.join("B");
        let unavailable_child = unavailable_directory.join("C");
        let remaining_selection = root_directory.join("remaining.txt");
        browser.tab_mut().current_dir = root_directory.clone();
        browser.tab_mut().deepest_open_column_directory = Some(unavailable_child.clone());
        browser.tab_mut().selected = Some(unavailable_child.clone());
        browser
            .tab_mut()
            .selected_paths
            .extend([unavailable_child.clone(), remaining_selection.clone()]);
        browser.tab_mut().selection_anchor = Some(unavailable_directory.clone());
        browser.tab_mut().expanded_directories.insert(
            ancestor_directory.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 1),
        );
        browser.tab_mut().expanded_directories.insert(
            unavailable_directory.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 4),
        );
        browser.tab_mut().expanded_directories.insert(
            unavailable_child.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 2),
        );
        browser
            .pane_mut()
            .column_viewports
            .insert(ancestor_directory.clone(), ColumnViewport::default());
        browser
            .pane_mut()
            .column_viewports
            .insert(unavailable_directory.clone(), ColumnViewport::default());
        browser
            .pane_mut()
            .column_viewports
            .insert(unavailable_child.clone(), ColumnViewport::default());
        let descendant_cancellation = browser
            .tab()
            .expanded_directories
            .get(&unavailable_child)
            .and_then(|directory| directory.load_cancel.clone())
            .expect("descendant load cancellation");
        reset_recorded_global_errors();

        drop(browser.accept_complete_expanded_directory_fixture(
            crate::model::ExpandedDirectoryLoadRequest {
                context: DirectoryExpansionLoadContext::BrowserTree {
                    pane_id: BrowserPaneId::PRIMARY,
                },
                path: unavailable_directory.clone(),
                generation: 4,
            },
            Err(unavailable_failure(&unavailable_directory)),
        ));

        assert_eq!(
            crate::three_column_view::column_directories(&browser),
            vec![root_directory, ancestor_directory.clone()]
        );
        assert_eq!(
            browser.tab().deepest_open_column_directory,
            Some(ancestor_directory.clone())
        );
        assert_eq!(browser.tab().selected, None);
        assert_eq!(browser.tab().selection_anchor, None);
        assert_eq!(browser.tab().selected_paths.len(), 1);
        assert!(browser.tab().selected_paths.contains(&remaining_selection));
        assert_eq!(browser.tab().expanded_directories.len(), 1);
        assert!(browser
            .tab()
            .expanded_directories
            .contains_key(&ancestor_directory));
        assert_eq!(browser.pane().column_viewports.len(), 1);
        assert!(browser
            .pane()
            .column_viewports
            .contains_key(&ancestor_directory));
        assert!(descendant_cancellation.is_cancelled());
        assert_eq!(browser.current_error(), None);
        assert_eq!(recorded_global_error_count(), 0);
    }

    #[test]
    fn unavailable_deepest_column_preserves_the_preceding_chain() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let root_directory = PathBuf::from("/workspace");
        let first_directory = root_directory.join("A");
        let second_directory = first_directory.join("B");
        let unavailable_directory = second_directory.join("C");
        browser.tab_mut().current_dir = root_directory.clone();
        browser.tab_mut().deepest_open_column_directory = Some(unavailable_directory.clone());
        browser.tab_mut().expanded_directories.insert(
            first_directory.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 1),
        );
        browser.tab_mut().expanded_directories.insert(
            second_directory.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 2),
        );
        browser.tab_mut().expanded_directories.insert(
            unavailable_directory.clone(),
            loading_expanded_directory(BrowserPaneId::PRIMARY, 5),
        );

        drop(browser.accept_complete_expanded_directory_fixture(
            crate::model::ExpandedDirectoryLoadRequest {
                context: DirectoryExpansionLoadContext::BrowserTree {
                    pane_id: BrowserPaneId::PRIMARY,
                },
                path: unavailable_directory.clone(),
                generation: 5,
            },
            Err(unavailable_failure(&unavailable_directory)),
        ));

        assert_eq!(
            crate::three_column_view::column_directories(&browser),
            vec![root_directory, first_directory, second_directory.clone()]
        );
        assert_eq!(
            browser.tab().deepest_open_column_directory,
            Some(second_directory)
        );
        assert!(!browser
            .tab()
            .expanded_directories
            .contains_key(&unavailable_directory));
    }

    #[test]
    fn unavailable_middle_column_truncates_an_inactive_pane_chain() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let inactive_pane_id = BrowserPaneId(9);
        let root_directory = PathBuf::from("/workspace");
        let ancestor_directory = root_directory.join("A");
        let unavailable_directory = ancestor_directory.join("B");
        let unavailable_child = unavailable_directory.join("C");
        let mut inactive_tab = BrowserTab::directory(0, root_directory.clone());
        inactive_tab.deepest_open_column_directory = Some(unavailable_child.clone());
        inactive_tab.expanded_directories.insert(
            ancestor_directory.clone(),
            loading_expanded_directory(inactive_pane_id, 1),
        );
        inactive_tab.expanded_directories.insert(
            unavailable_directory.clone(),
            loading_expanded_directory(inactive_pane_id, 4),
        );
        inactive_tab.expanded_directories.insert(
            unavailable_child,
            loading_expanded_directory(inactive_pane_id, 2),
        );
        browser
            .panes
            .push(BrowserPane::new(inactive_pane_id, inactive_tab));
        reset_recorded_global_errors();

        drop(browser.accept_complete_expanded_directory_fixture(
            crate::model::ExpandedDirectoryLoadRequest {
                context: DirectoryExpansionLoadContext::BrowserTree {
                    pane_id: inactive_pane_id,
                },
                path: unavailable_directory.clone(),
                generation: 4,
            },
            Err(unavailable_failure(&unavailable_directory)),
        ));

        let pane = browser.pane_by_id(inactive_pane_id).expect("inactive pane");
        assert_eq!(
            pane.tab().deepest_open_column_directory,
            Some(ancestor_directory.clone())
        );
        assert_eq!(
            pane.tabs[0].deepest_open_column_directory,
            Some(ancestor_directory.clone())
        );
        assert_eq!(
            crate::three_column_view::column_directories_for_pane(
                browser
                    .pane_view(inactive_pane_id)
                    .expect("inactive pane view")
            ),
            vec![root_directory, ancestor_directory]
        );
        assert_eq!(browser.current_error(), None);
        assert_eq!(recorded_global_error_count(), 0);
    }

    #[test]
    fn directory_read_failure_still_uses_global_error_channel() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        let locked_directory = PathBuf::from("/workspace/locked");
        browser.tab_mut().current_dir = locked_directory.clone();
        browser.pane_mut().directory_load_generation = 3;
        browser.tab_mut().directory_collection_phase =
            crate::model::DirectoryCollectionPhase::Discovering;
        reset_recorded_global_errors();

        drop(browser.accept_directory_load_failure(
            DirectoryLoadRequest {
                pane_id: BrowserPaneId::PRIMARY,
                path: locked_directory,
                generation: 3,
            },
            DirectoryLoadFailure::ReadFailed {
                message: "permission denied".to_owned(),
            },
        ));

        assert!(!browser.tab().directory_collection_phase.is_discovering());
        assert_eq!(browser.current_error(), Some("permission denied"));
        assert_eq!(recorded_global_error_count(), 1);
    }

    fn unavailable_failure(path: &Path) -> DirectoryLoadFailure {
        DirectoryLoadFailure::DirectoryUnavailable {
            message: format!("directory is unavailable: {path:?}"),
        }
    }

    fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
        DirectoryEntry::new(path, kind, EntryMetadata::default(), false, false, false)
    }

    fn loading_expanded_directory(pane_id: BrowserPaneId, generation: u64) -> ExpandedDirectory {
        ExpandedDirectory {
            entries: Vec::new(),
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loading,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: generation,
            load_context: Some(DirectoryExpansionLoadContext::BrowserTree { pane_id }),
            load_cancel: Some(tokio_util::sync::CancellationToken::new()),
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        }
    }
}
