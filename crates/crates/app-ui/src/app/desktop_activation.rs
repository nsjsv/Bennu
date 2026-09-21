use std::collections::HashSet;
use std::path::PathBuf;

use desktop_linux::{DesktopActivationEvent, LocalWorkspaceRequest};
use iced::{window, Task};

use super::FileBrowser;
use crate::model::{BrowserTab, FilePropertiesTargetSet, Message};

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopWorkspaceMergePlan {
    tabs: Vec<DesktopTabMerge>,
    next_tab_id: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopTabMerge {
    pane_id: crate::model::BrowserPaneId,
    tab_id: usize,
    directory: PathBuf,
    selected_paths: Vec<PathBuf>,
    placement: DesktopTabPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopTabPlacement {
    Reuse,
    Append,
}

impl FileBrowser {
    pub(super) fn accept_desktop_activation(
        &mut self,
        event: DesktopActivationEvent,
    ) -> Task<Message> {
        match event {
            DesktopActivationEvent::FocusMainWindow(startup_id) => {
                tracing::debug!(
                    target: "app_ui::desktop_activation",
                    startup_id = startup_id.as_str(),
                    "desktop activation requested main window focus"
                );
                self.focus_main_window()
            }
            DesktopActivationEvent::MergeWorkspace(workspace, startup_id) => {
                tracing::debug!(
                    target: "app_ui::desktop_activation",
                    startup_id = startup_id.as_str(),
                    tab_count = workspace.tabs().len(),
                    "desktop activation requested workspace merge"
                );
                self.merge_desktop_workspace(workspace)
            }
            DesktopActivationEvent::OpenProperties(targets, startup_id) => {
                tracing::debug!(
                    target: "app_ui::desktop_activation",
                    startup_id = startup_id.as_str(),
                    target_count = targets.paths().len(),
                    "desktop activation requested properties"
                );
                let targets = FilePropertiesTargetSet::new(targets.paths().to_vec())
                    .expect("desktop property targets are non-empty");
                self.open_file_properties_targets(targets)
            }
        }
    }

    pub(super) fn accept_desktop_activation_runtime_failure(
        &mut self,
        error: String,
    ) -> Task<Message> {
        self.file_manager_activation = None;
        self.show_global_error(format!("Desktop activation stopped: {error}"));
        Task::none()
    }

    fn focus_main_window(&mut self) -> Task<Message> {
        self.focused_window = self.main_window;
        window::gain_focus(self.main_window)
    }

    fn prepare_desktop_workspace_merge(&mut self) -> Task<Message> {
        let commit_rename = self.commit_rename_if_active();
        let cancel_address_editing = self.cancel_address_editing();
        let close_search_workspace = self.close_search_workspace();

        self.destructive_action_confirmation = None;
        self.file_drop_prompt = None;
        self.transfer_conflict = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.batch_rename = None;
        self.network_connection_editor = None;
        self.open_with = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();
        self.clear_icon_grid_expansion_for_context_change();
        self.clear_transient_interaction_state();
        self.clear_pointer_driven_interaction_state();

        Task::batch([
            commit_rename,
            cancel_address_editing,
            close_search_workspace,
        ])
    }

    fn merge_desktop_workspace(&mut self, workspace: LocalWorkspaceRequest) -> Task<Message> {
        let prepare_workspace_merge = self.prepare_desktop_workspace_merge();
        self.sync_active_tab_state();
        let original_active_pane_id = self.active_pane_id();
        let occupied_tab_ids = self
            .panes
            .iter()
            .flat_map(|pane| pane.tabs.iter().map(|tab| tab.id))
            .collect::<HashSet<_>>();
        let plan = desktop_workspace_merge_plan(
            &workspace,
            &self.panes,
            original_active_pane_id,
            self.active_tab_id,
            &occupied_tab_ids,
            self.next_tab_id,
        );
        let first_target = plan
            .tabs
            .first()
            .expect("validated desktop workspace has a tab");
        let first_target_pane_id = first_target.pane_id;
        let first_target_tab_id = first_target.tab_id;
        let mut appended_tab_ids = Vec::new();

        for tab_merge in &plan.tabs {
            let pane = self
                .pane_by_id_mut(tab_merge.pane_id)
                .expect("merge plan references existing pane");
            match tab_merge.placement {
                DesktopTabPlacement::Reuse => {
                    let tab = pane
                        .tabs
                        .iter_mut()
                        .find(|tab| tab.id == tab_merge.tab_id)
                        .expect("merge plan references existing tab");
                    replace_tab_selection(tab, &tab_merge.selected_paths);
                }
                DesktopTabPlacement::Append => {
                    let mut tab =
                        BrowserTab::directory(tab_merge.tab_id, tab_merge.directory.clone());
                    tab.view_mode = pane.view_mode;
                    replace_tab_selection(&mut tab, &tab_merge.selected_paths);
                    pane.tabs.push(tab);
                    appended_tab_ids.push(tab_merge.tab_id);
                }
            }
        }
        self.next_tab_id = plan.next_tab_id;

        let updated_active_pane = self
            .pane_by_id(original_active_pane_id)
            .expect("active pane remains available")
            .clone();
        self.restore_pane_snapshot(updated_active_pane);
        if first_target_pane_id != original_active_pane_id {
            self.activate_pane(first_target_pane_id);
        }
        if first_target_pane_id == original_active_pane_id {
            for tab_id in appended_tab_ids {
                self.start_tab_intro_animation(tab_id);
            }
        }
        self.sync_tab_bar_visibility();

        let activate_first_target = if first_target_tab_id == self.active_tab_id {
            let first_tab = self
                .tabs
                .iter()
                .find(|tab| tab.id == first_target_tab_id)
                .expect("first target tab exists")
                .clone();
            self.selected = first_tab.selected;
            self.selected_paths = first_tab.selected_paths;
            self.selection_anchor = first_tab.selection_anchor;
            self.reload_current()
        } else {
            self.select_tab(first_target_tab_id)
        };

        Task::batch([
            prepare_workspace_merge,
            activate_first_target,
            self.request_browser_session_save(),
            self.focus_main_window(),
        ])
    }
}

fn desktop_workspace_merge_plan(
    workspace: &LocalWorkspaceRequest,
    panes: &[crate::model::BrowserPane],
    active_pane_id: crate::model::BrowserPaneId,
    active_tab_id: usize,
    occupied_tab_ids: &HashSet<usize>,
    next_tab_id: usize,
) -> DesktopWorkspaceMergePlan {
    let mut allocated_ids = occupied_tab_ids.clone();
    let mut next_tab_id = next_tab_id;
    let mut planned_tabs = Vec::with_capacity(workspace.tabs().len());

    for requested_tab in workspace.tabs() {
        let reusable_tab = panes
            .iter()
            .find(|pane| pane.id == active_pane_id)
            .and_then(|pane| {
                pane.tabs
                    .iter()
                    .find(|tab| {
                        tab.id == active_tab_id
                            && !tab.is_trash_view
                            && tab.directory == requested_tab.directory()
                    })
                    .map(|tab| (pane.id, tab))
            })
            .or_else(|| {
                panes.iter().find_map(|pane| {
                    pane.tabs
                        .iter()
                        .find(|tab| {
                            !tab.is_trash_view && tab.directory == requested_tab.directory()
                        })
                        .map(|tab| (pane.id, tab))
                })
            });
        let (pane_id, tab_id, placement) = if let Some((pane_id, tab)) = reusable_tab {
            (pane_id, tab.id, DesktopTabPlacement::Reuse)
        } else {
            while allocated_ids.contains(&next_tab_id) {
                next_tab_id = next_tab_id.wrapping_add(1);
            }
            let allocated = next_tab_id;
            allocated_ids.insert(allocated);
            next_tab_id = next_tab_id.wrapping_add(1);
            (active_pane_id, allocated, DesktopTabPlacement::Append)
        };
        planned_tabs.push(DesktopTabMerge {
            pane_id,
            tab_id,
            directory: requested_tab.directory().to_path_buf(),
            selected_paths: requested_tab.selected_paths().to_vec(),
            placement,
        });
    }

    DesktopWorkspaceMergePlan {
        tabs: planned_tabs,
        next_tab_id,
    }
}

fn replace_tab_selection(tab: &mut BrowserTab, selected_paths: &[PathBuf]) {
    tab.selected = selected_paths.first().cloned();
    tab.selected_paths = selected_paths.iter().cloned().collect();
    tab.selection_anchor = selected_paths.first().cloned();
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::model::{AddressEditingSession, AddressEditingSessionId};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn merge_plan_reuses_active_then_stable_tabs_and_allocates_unoccupied_ids() {
        let root = TempDir::new().expect("create root");
        let first = root.path().join("first");
        let second = root.path().join("second");
        let third = root.path().join("third");
        fs::create_dir_all(&first).expect("create first");
        fs::create_dir_all(&second).expect("create second");
        fs::create_dir_all(&third).expect("create third");
        let workspace = LocalWorkspaceRequest::from_cli_paths(vec![
            second.clone(),
            first.clone(),
            third.clone(),
        ])
        .expect("classify workspace");
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.current_dir = second.clone();
        browser.tabs = vec![
            BrowserTab::directory(5, first.clone()),
            BrowserTab::directory(7, second.clone()),
        ];
        browser.active_tab_id = 7;
        browser.sync_active_tab_state();
        let active_pane_id = browser.active_pane_id();

        let plan = desktop_workspace_merge_plan(
            &workspace,
            &browser.panes,
            active_pane_id,
            7,
            &HashSet::from([5, 7, 8]),
            7,
        );

        assert_eq!(
            plan.tabs
                .iter()
                .map(|tab| (tab.pane_id, tab.tab_id, tab.placement))
                .collect::<Vec<_>>(),
            vec![
                (active_pane_id, 7, DesktopTabPlacement::Reuse),
                (active_pane_id, 5, DesktopTabPlacement::Reuse),
                (active_pane_id, 9, DesktopTabPlacement::Append),
            ]
        );
        assert_eq!(plan.next_tab_id, 10);
    }

    #[test]
    fn merge_reuses_inactive_pane_and_appends_missing_tabs_to_original_active_pane() {
        let root = TempDir::new().expect("create root");
        let active_directory = root.path().join("active");
        let inactive_directory = root.path().join("inactive");
        let missing_directory = root.path().join("missing");
        fs::create_dir_all(&active_directory).expect("create active");
        fs::create_dir_all(&inactive_directory).expect("create inactive");
        fs::create_dir_all(&missing_directory).expect("create missing");
        let selected = inactive_directory.join("selected.txt");
        fs::write(&selected, b"selected").expect("write selected");
        let workspace = LocalWorkspaceRequest::from_cli_paths(vec![
            selected.clone(),
            missing_directory.clone(),
        ])
        .expect("classify workspace");

        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let primary = crate::model::BrowserPaneId::PRIMARY;
        let secondary = crate::model::BrowserPaneId(1);
        browser.current_dir = active_directory.clone();
        browser.tabs = vec![BrowserTab::directory(0, active_directory.clone())];
        browser.active_tab_id = 0;
        browser.sync_active_tab_state();
        let mut secondary_pane = browser.capture_active_pane_snapshot();
        secondary_pane.id = secondary;
        secondary_pane.current_dir = inactive_directory.clone();
        secondary_pane.tabs = vec![BrowserTab::directory(5, inactive_directory.clone())];
        secondary_pane.active_tab_id = 5;
        browser.panes.push(secondary_pane);
        browser.pane_layout = crate::model::BrowserPaneLayout::Split {
            axis: crate::model::SplitAxis::Horizontal,
            first: primary,
            second: secondary,
            active: primary,
            first_portion: 500,
        };
        browser.next_tab_id = 5;

        let _task = browser.merge_desktop_workspace(workspace);

        assert_eq!(browser.active_pane_id(), secondary);
        assert_eq!(browser.active_tab_id, 5);
        assert_eq!(browser.selected_paths, HashSet::from([selected]));
        let primary_pane = browser.pane_by_id(primary).expect("primary pane");
        assert_eq!(primary_pane.tabs.len(), 2);
        assert_eq!(primary_pane.tabs[0].directory, active_directory);
        assert_eq!(primary_pane.tabs[1].directory, missing_directory);
        assert_eq!(primary_pane.tabs[1].id, 6);
        assert_eq!(browser.next_tab_id, 7);
    }

    #[test]
    fn merge_reusing_current_tab_clears_the_previous_address_session() {
        let root = TempDir::new().expect("create root");
        let requested = root.path().join("requested");
        fs::create_dir_all(&requested).expect("create requested");
        let workspace = LocalWorkspaceRequest::from_cli_paths(vec![requested.clone()])
            .expect("classify workspace");
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.current_dir = requested.clone();
        browser.tabs = vec![BrowserTab::directory(0, requested.clone())];
        browser.active_tab_id = 0;
        browser.address_editing = Some(AddressEditingSession::new(
            browser.active_pane_id(),
            AddressEditingSessionId(1),
            &requested,
        ));

        let _task = browser.merge_desktop_workspace(workspace);

        assert!(browser.address_editing.is_none());
    }

    #[test]
    fn merge_replaces_only_requested_tab_selection_and_preserves_other_tabs() {
        let root = TempDir::new().expect("create root");
        let requested = root.path().join("requested");
        let untouched = root.path().join("untouched");
        fs::create_dir_all(&requested).expect("create requested");
        fs::create_dir_all(&untouched).expect("create untouched");
        let selected = requested.join("selected.txt");
        fs::write(&selected, b"selected").expect("write selected");
        let workspace = LocalWorkspaceRequest::from_cli_paths(vec![selected.clone()])
            .expect("classify workspace");
        let (mut browser, _task) = FileBrowser::new(crate::config::ui_thread_startup_config());
        browser.current_dir = requested.clone();
        browser.tabs = vec![
            BrowserTab::directory(0, requested.clone()),
            BrowserTab::directory(1, untouched.clone()),
        ];
        browser.active_tab_id = 0;
        browser.next_tab_id = 2;
        let untouched_before = browser.tabs[1].clone();

        let _task = browser.merge_desktop_workspace(workspace);

        assert_eq!(browser.tabs.len(), 2);
        assert_eq!(browser.tabs[0].selected, Some(selected.clone()));
        assert_eq!(browser.tabs[0].selected_paths, HashSet::from([selected]));
        assert_eq!(browser.tabs[1].directory, untouched_before.directory);
        assert_eq!(browser.tabs[1].back_stack, untouched_before.back_stack);
    }
}
