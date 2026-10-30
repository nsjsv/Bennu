use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use iced::Task;

use super::global_error::GlobalErrorSource;
use super::FileBrowser;
use crate::commands::load_directory_command;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserPaneSession, BrowserSessionSnapshot,
    BrowserTab, BrowserTabSession, ClassifiedStartupSession, ColumnBrowserViewport, Message,
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
            Some(tr!(
                "Failed to restore saved view state: %1; opening the home directory.",
                error
            )),
        )
    }

    fn apply_single_startup_directory(
        &mut self,
        directory: PathBuf,
        error: Option<String>,
    ) -> Task<Message> {
        // 启动目录替换整个窗格集合；窗格对象本身保留，让加载代号继续单调递增，
        // 旧请求的迟到响应不会撞上新一轮的代号。
        let mut tab = BrowserTab::directory(0, directory.clone());
        tab.view_mode = self.tab().view_mode;
        self.panes.retain(|pane| pane.id == BrowserPaneId::PRIMARY);
        let pane = self
            .pane_by_id_mut(BrowserPaneId::PRIMARY)
            .expect("startup runs with the primary pane");
        pane.tabs = vec![tab];
        pane.active_tab_id = 0;
        pane.reset_directory_view();
        self.icon_grid_viewports.clear();
        self.pane_layout = BrowserPaneLayout::Single {
            active: BrowserPaneId::PRIMARY,
        };
        self.reset_active_pane_context();
        // 换到启动目录：上一处目录的加载错误过时了；启动目录本身的问题是一条不归来源的提示。
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);
        if let Some(error) = error {
            self.show_global_error(error);
        }
        let (request, cancellation) = self.begin_directory_load(BrowserPaneId::PRIMARY, directory);
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
                Some(tr!(
                    "Saved view state could not be restored; opening the home directory."
                )),
            );
        }
        self.next_tab_id = next_tab_id_after_restore(&restored_panes);
        self.next_pane_id = next_pane_id_after_restore(&restored_panes);
        self.pane_layout = sanitize_layout(layout, &restored_panes);
        self.panes = restored_panes;
        self.icon_grid_viewports.clear();
        self.reset_active_pane_context();
        self.retract_global_error(GlobalErrorSource::DirectoryLoad);

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
    let tabs = restored_tabs.iter().map(restored_tab).collect();
    let mut restored = BrowserPane::with_tabs(pane.id, tabs, active_tab.id);
    restored.column_browser_viewport =
        sanitize_column_browser_viewport(pane.column_browser_viewport);
    restored.column_viewports = sanitize_column_viewports(pane.column_viewports);
    Some(restored)
}

/// 会话恢复不入包内：包内/包根目录归一到归档所在真实目录；包内选中与
/// 多栏展开链丢弃（真实目录中这些条目并不存在）。真实路径的选中照常
/// 恢复（CLI 显式工作区同样途经此处）。
fn restored_tab(session: &BrowserTabSession) -> BrowserTab {
    let is_real_file = |path: &Path| {
        file_core::archive_path_identity(path) == file_core::ArchivePathIdentity::RealFile
    };
    let mut tab = session.to_browser_tab();
    if let Some(real_directory) = file_core::real_directory_outside_archive(&tab.current_dir) {
        tab.current_dir = real_directory;
    }
    tab.selected = tab.selected.take().filter(|path| is_real_file(path));
    tab.selected_paths.retain(|path| is_real_file(path));
    tab.selection_anchor = None;
    tab.expanded_directories
        .retain(|directory, _| is_real_file(directory));
    tab
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
