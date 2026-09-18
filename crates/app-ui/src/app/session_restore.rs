use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use file_core::{SortDirection, SortField};
use iced::Task;

use super::FileBrowser;
use crate::commands::load_directory_command;
use crate::model::{
    empty_directory_entry_snapshot, BrowserPane, BrowserPaneId, BrowserPaneLayout,
    BrowserPaneSession, BrowserSessionSnapshot, BrowserTabSession, ClassifiedStartupSession,
    ColumnBrowserViewport, DirectoryCollectionPhase, DirectoryOrderPhase, Message,
    StartupSessionPlan, StartupSessionPlanRequest, StartupSessionSource,
};
use crate::thumbnail_cache::ColumnViewport;

impl FileBrowser {
    pub(super) fn startup_session_plan_request(&self, home: &Path) -> StartupSessionPlanRequest {
        let source = match self.user_config.startup_location_policy {
            crate::config::StartupLocationPolicy::Home => StartupSessionSource::Home,
            crate::config::StartupLocationPolicy::CustomDirectory => {
                StartupSessionSource::CustomDirectory(
                    self.user_config.startup_custom_directory.clone(),
                )
            }
            crate::config::StartupLocationPolicy::PreviousSession => {
                StartupSessionSource::PreviousSession
            }
        };
        StartupSessionPlanRequest {
            home: home.to_path_buf(),
            source,
        }
    }

    pub(super) fn configured_startup_session_plan_request(
        &self,
        home: &Path,
    ) -> Option<StartupSessionPlanRequest> {
        if !matches!(
            self.application_launch_request,
            crate::command_line::ApplicationLaunchRequest::ConfiguredStartup
        ) {
            return None;
        }
        Some(self.startup_session_plan_request(home))
    }

    pub(super) fn previous_session_plan_request(
        &self,
        home: &Path,
    ) -> Option<StartupSessionPlanRequest> {
        self.configured_startup_session_plan_request(home)
            .filter(|request| matches!(request.source, StartupSessionSource::PreviousSession))
    }

    pub(super) fn accept_startup_plan(
        &mut self,
        classified: ClassifiedStartupSession,
    ) -> Task<Message> {
        let Some(current_request) = self.configured_startup_session_plan_request(&self.home_dir)
        else {
            return Task::none();
        };
        if classified.request != current_request {
            return Task::none();
        }
        let home = self.home_dir.clone();
        self.apply_startup_session_plan(classified.plan, &home)
    }

    pub(super) fn apply_startup_session_plan(
        &mut self,
        plan: StartupSessionPlan,
        home: &Path,
    ) -> Task<Message> {
        match plan {
            StartupSessionPlan::Directory { directory, error } => {
                self.apply_single_startup_directory(directory, error)
            }
            StartupSessionPlan::Session(session) => self.apply_restored_session(session, home),
        }
    }

    pub(super) fn fallback_startup_directory_after_session_store_error(
        &mut self,
        home: &Path,
        error: String,
    ) -> Task<Message> {
        self.apply_single_startup_directory(
            home.to_path_buf(),
            Some(format!(
                "Failed to restore saved view state: {error}; opening the home directory."
            )),
        )
    }

    fn apply_single_startup_directory(
        &mut self,
        directory: PathBuf,
        error: Option<String>,
    ) -> Task<Message> {
        self.current_dir = directory.clone();
        self.is_trash_view = false;
        self.set_entries(empty_directory_entry_snapshot());
        self.directory_loading_placeholder = None;
        self.trash_entries.clear();
        self.deepest_open_column_directory = None;
        self.expanded_directories.clear();
        self.column_browser_viewport = ColumnBrowserViewport::default();
        self.column_viewports.clear();
        self.icon_grid_viewports.clear();
        self.back_stack.clear();
        self.forward_stack.clear();
        self.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        self.replace_global_error(error);
        self.tabs = vec![{
            let mut tab = crate::model::BrowserTab::directory(0, directory.clone());
            tab.view_mode = self.view_mode;
            tab
        }];
        self.active_tab_id = 0;
        self.pane_layout = BrowserPaneLayout::Single {
            active: BrowserPaneId::PRIMARY,
        };
        self.panes = vec![self.capture_active_pane_snapshot()];
        let request = self.next_directory_load_request(directory);
        let cancellation = self.directory_load_cancellation(&request);
        load_directory_command(request, self.options.clone(), cancellation)
    }

    fn apply_restored_session(
        &mut self,
        session: BrowserSessionSnapshot,
        home: &Path,
    ) -> Task<Message> {
        let BrowserSessionSnapshot { panes, layout } = session;
        let restored_panes = panes
            .into_iter()
            .filter_map(restored_pane_from_session)
            .collect::<Vec<_>>();
        if restored_panes.is_empty() {
            return self.apply_single_startup_directory(
                home.to_path_buf(),
                Some(
                    "Saved view state could not be restored; opening the home directory."
                        .to_owned(),
                ),
            );
        }
        self.next_tab_id = next_tab_id_after_restore(&restored_panes);
        self.next_pane_id = next_pane_id_after_restore(&restored_panes);
        self.panes = restored_panes.clone();
        self.icon_grid_viewports.clear();
        self.pane_layout = sanitize_layout(layout, &restored_panes);
        let active_pane_id = self.pane_layout.active();
        let active_pane = restored_panes
            .iter()
            .find(|pane| pane.id == active_pane_id)
            .or_else(|| restored_panes.first())
            .cloned()
            .expect("restored panes is non-empty");
        self.pane_layout = self.pane_layout.with_active(active_pane.id);
        self.restore_pane_snapshot(active_pane);
        self.directory_collection_phase = DirectoryCollectionPhase::Discovering;
        self.clear_global_error();
        self.sync_active_pane_state();

        Task::batch([
            self.reload_visible_panes(),
            self.restore_visible_column_browser_viewports(),
        ])
    }
}

fn next_tab_id_after_restore(panes: &[BrowserPane]) -> usize {
    let occupied_ids = panes
        .iter()
        .flat_map(|pane| pane.tabs.iter().map(|tab| tab.id))
        .collect::<HashSet<_>>();
    let Some(highest_id) = occupied_ids.iter().copied().max() else {
        return 0;
    };
    highest_id
        .checked_add(1)
        .unwrap_or_else(|| first_unoccupied_tab_id(&occupied_ids))
}

fn first_unoccupied_tab_id(occupied_ids: &HashSet<usize>) -> usize {
    let mut candidate = 0usize;
    loop {
        if !occupied_ids.contains(&candidate) {
            return candidate;
        }
        candidate = candidate
            .checked_add(1)
            .expect("the restored session cannot occupy every tab identifier");
    }
}

fn next_pane_id_after_restore(panes: &[BrowserPane]) -> u64 {
    let occupied_ids = panes
        .iter()
        .map(|pane| pane.id.key())
        .collect::<HashSet<_>>();
    let Some(highest_id) = occupied_ids.iter().copied().max() else {
        return 0;
    };
    highest_id
        .checked_add(1)
        .unwrap_or_else(|| first_unoccupied_pane_id(&occupied_ids))
}

fn first_unoccupied_pane_id(occupied_ids: &HashSet<u64>) -> u64 {
    let mut candidate = 0u64;
    loop {
        if !occupied_ids.contains(&candidate) {
            return candidate;
        }
        candidate = candidate
            .checked_add(1)
            .expect("the restored session cannot occupy every pane identifier");
    }
}

fn restored_pane_from_session(pane: BrowserPaneSession) -> Option<BrowserPane> {
    let restored_tabs = pane.tabs;
    if restored_tabs.is_empty() {
        return None;
    }
    let active_tab = restored_tabs
        .iter()
        .find(|tab| tab.id == pane.active_tab_id)
        .or_else(|| restored_tabs.first())
        .cloned()
        .expect("restored tabs is non-empty");
    let tabs = restored_tabs
        .iter()
        .map(|tab| {
            let mut restored = BrowserTabSession::to_browser_tab(tab);
            if let Some(real_directory) =
                file_core::real_directory_outside_archive(&restored.directory)
            {
                restored.directory = real_directory;
            }
            restored
        })
        .collect::<Vec<_>>();
    let active_expanded_directories = active_tab
        .restored_expanded_directories()
        .into_iter()
        .filter(|(directory, _)| file_core::archive_path_identity(directory) == file_core::ArchivePathIdentity::RealFile)
        .collect();
    // 会话恢复不入包内：包内/包根目录归一到归档所在真实目录；包内
    // 选中与多栏展开链丢弃（真实目录中这些条目并不存在）。真实路径
    // 的选中照常恢复（CLI 显式工作区同样途经此处）。
    let normalized_tab_directory =
        file_core::real_directory_outside_archive(&active_tab.directory)
            .unwrap_or_else(|| active_tab.directory.clone());
    let mut browser_pane = BrowserPane {
        id: pane.id,
        current_dir: normalized_tab_directory,
        is_trash_view: active_tab.is_trash_view,
        entries: Vec::new().into(),
        directory_discovery: None,
        directory_loading_placeholder: None,
        trash_entries: Vec::new(),
        selected: restored_selection(active_tab.selected.clone()),
        selected_paths: active_tab
            .selected_paths
            .into_iter()
            .filter(|path| file_core::archive_path_identity(path) == file_core::ArchivePathIdentity::RealFile)
            .collect(),
        selection_anchor: None,
        deepest_open_column_directory: active_tab.deepest_open_column_directory,
        expanded_directories: active_expanded_directories,
        view_mode: active_tab.view_mode,
        column_browser_viewport: sanitize_column_browser_viewport(pane.column_browser_viewport),
        column_viewports: sanitize_column_viewports(pane.column_viewports),
        tabs,
        active_tab_id: active_tab.id,
        directory_load_generation: 0,
        directory_load_cancel: None,
        back_stack: active_tab.back_stack,
        forward_stack: active_tab.forward_stack,
        directory_collection_phase: DirectoryCollectionPhase::Discovering,
        directory_order_phase: DirectoryOrderPhase::Ready {
            field: SortField::Name,
            direction: SortDirection::Ascending,
        },
    };
    browser_pane.sync_active_tab_state();
    Some(browser_pane)
}

fn restored_selection(selected: Option<PathBuf>) -> Option<PathBuf> {
    let selected = selected?;
    (file_core::archive_path_identity(&selected) == file_core::ArchivePathIdentity::RealFile)
        .then_some(selected)
}

fn sanitize_column_browser_viewport(viewport: ColumnBrowserViewport) -> ColumnBrowserViewport {
    if viewport.offset_x.is_finite() && viewport.width.is_finite() {
        return ColumnBrowserViewport {
            offset_x: viewport.offset_x.max(0.0),
            width: viewport.width.max(1.0),
        };
    }
    ColumnBrowserViewport::default()
}

fn sanitize_column_viewports(
    viewports: HashMap<PathBuf, ColumnViewport>,
) -> HashMap<PathBuf, ColumnViewport> {
    viewports
        .into_iter()
        .filter(|(_, viewport)| viewport.offset_y.is_finite() && viewport.height.is_finite())
        .collect()
}

fn sanitize_layout(layout: BrowserPaneLayout, panes: &[BrowserPane]) -> BrowserPaneLayout {
    let exists = |id: BrowserPaneId| panes.iter().any(|pane| pane.id == id);
    match layout {
        BrowserPaneLayout::Single { active } if exists(active) => {
            BrowserPaneLayout::Single { active }
        }
        BrowserPaneLayout::Split {
            axis,
            first,
            second,
            active,
            first_portion,
        } if exists(first) && exists(second) && exists(active) => BrowserPaneLayout::Split {
            axis,
            first,
            second,
            active,
            first_portion,
        },
        _ => BrowserPaneLayout::Single {
            active: panes[0].id,
        },
    }
}
