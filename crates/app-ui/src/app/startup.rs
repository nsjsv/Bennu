use std::path::{Path, PathBuf};

use iced::Task;

use super::FileBrowser;
use crate::commands::{
    operation_store_command, sidebar_devices_command, sidebar_locations_command,
    startup_session_plan_command,
};
use crate::config::UserConfig;
use crate::model::{
    BrowserPaneId, LoadedOperationStore, Message, SidebarLocation, StartupEnvironment,
    StartupSessionPlan,
};
use crate::sidebar::{home_sidebar_location, sidebar_favorite_configs};
use crate::startup_trace;

impl FileBrowser {
    pub(super) fn accept_startup_environment(
        &mut self,
        startup_environment: StartupEnvironment,
    ) -> Task<Message> {
        startup_trace::mark_once("startup_environment_loaded");
        let home = startup_environment.home;
        self.system_language = startup_environment.system_language;
        let state_database_path = startup_environment.state_database_path;
        let rendering_environment_status = startup_environment.rendering_environment_status;

        self.apply_loaded_user_config(startup_environment.user_config);
        self.home_dir = home.clone();
        self.pending_renderer_restart_environment = rendering_environment_status
            .restart_required
            .then_some(rendering_environment_status.environment);
        self.renderer_restart_notice_visible = self.pending_renderer_restart_environment.is_some();
        let configured_favorites = self.user_config.sidebar_favorites.clone();
        self.sidebar_locations = vec![home_sidebar_location(&home)];
        let configured_startup_request = self.configured_startup_session_plan_request(&home);
        let explicit_startup_session = self
            .application_launch_request
            .explicit_browser_session(self.view_mode);
        let startup_command = if let Some(session) = explicit_startup_session {
            self.apply_startup_session_plan(StartupSessionPlan::Session(session), &home)
        } else {
            let startup_request = configured_startup_request
                .as_ref()
                .expect("configured startup has a startup session request");
            match &startup_request.source {
                crate::model::StartupSessionSource::Home => self.apply_startup_session_plan(
                    StartupSessionPlan::Directory {
                        directory: home.clone(),
                        error: None,
                    },
                    &home,
                ),
                crate::model::StartupSessionSource::CustomDirectory(_) => {
                    startup_session_plan_command(startup_request.clone())
                }
                crate::model::StartupSessionSource::PreviousSession => Task::none(),
            }
        };
        let operation_store_startup_request = self.previous_session_plan_request(&home);

        Task::batch([
            startup_command,
            sidebar_locations_command(home, configured_favorites),
            sidebar_devices_command(),
            self.refresh_network_mount_states(),
            self.startup_auto_connect_network_connections(),
            operation_store_command(state_database_path, operation_store_startup_request),
        ])
    }

    pub(super) fn accept_sidebar_locations(
        &mut self,
        sidebar_locations: Vec<SidebarLocation>,
    ) -> Task<Message> {
        let imported_favorites = if self.user_config.sidebar_favorites.is_none() {
            Some(sidebar_favorite_configs(&sidebar_locations))
        } else {
            None
        };
        self.sidebar_locations = sidebar_locations;
        if let Some(favorites) = imported_favorites {
            self.user_config.sidebar_favorites = Some(favorites);
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn accept_operation_store(
        &mut self,
        operation_store: Result<LoadedOperationStore, String>,
    ) -> Task<Message> {
        match operation_store {
            Ok(loaded_store) => {
                let persisted_column_width_overrides = loaded_store.column_width_overrides;
                let persisted_reference_content_width =
                    loaded_store.column_width_reference_content_width;
                let classified_startup_session = loaded_store.classified_startup_session;
                if let Some(error) = self
                    .operation_queue
                    .set_store_and_restore(loaded_store.task_queue_store)
                {
                    self.show_global_error(error);
                }
                if !persisted_column_width_overrides.is_empty() {
                    self.apply_column_width_overrides(
                        persisted_column_width_overrides,
                        persisted_reference_content_width,
                    );
                }
                let session_command = classified_startup_session
                    .map(|classified| self.accept_startup_plan(classified))
                    .unwrap_or_else(Task::none);
                return Task::batch([
                    session_command,
                    self.maybe_flush_pending_browser_session_save(),
                    self.continue_file_operation_persistence(),
                ]);
            }
            Err(error) => {
                self.show_global_error(format!(
                    "Failed to initialize file operation queue storage: {error}"
                ));
                if self.previous_session_plan_request(&self.home_dir).is_some() {
                    let home_dir = self.home_dir.clone();
                    return self
                        .fallback_startup_directory_after_session_store_error(&home_dir, error);
                }
            }
        }
        Task::none()
    }

    pub(super) fn accept_thumbnail_refresh_request(
        &mut self,
        pane_id: BrowserPaneId,
        directory: PathBuf,
    ) -> Task<Message> {
        if !self.thumbnail_refresh_matches_pane(pane_id, &directory) {
            return Task::none();
        }
        self.schedule_thumbnail_refresh_for_pane(pane_id)
    }

    fn thumbnail_refresh_matches_pane(&self, pane_id: BrowserPaneId, directory: &Path) -> bool {
        if pane_id == self.active_pane_id() {
            return !self.directory_collection_phase.is_discovering()
                && self.current_dir.as_path() == directory;
        }

        self.pane_by_id(pane_id).is_some_and(|pane| {
            !pane.directory_collection_phase.is_discovering()
                && pane.current_dir.as_path() == directory
        })
    }

    fn apply_loaded_user_config(&mut self, mut user_config: UserConfig) {
        user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
        self.thumbnail_cache
            .set_cache_dir(user_config.thumbnail_cache_dir.clone());
        self.sidebar_width = self.sidebar_width_for_window(user_config.sidebar_width);
        self.right_preview_panel_open = user_config.right_preview_panel_open;
        self.right_preview_panel_width = crate::config::normalize_right_preview_panel_width(
            user_config.right_preview_panel_width,
        );
        self.right_preview_preview_ratio = crate::config::normalize_right_preview_preview_ratio(
            user_config.right_preview_preview_ratio,
        );
        self.terminal_emulator = user_config.terminal_emulator;
        self.rendering_gpu_preference = user_config.rendering_gpu_preference;
        self.options.include_hidden = user_config.show_hidden_files;
        self.options.sort_field = user_config.list_view_preferences.sort().field;
        self.options.sort_direction = user_config.list_view_preferences.sort().direction;
        self.view_mode = user_config.browser_view_mode;
        self.preview_size_limit_mib_inputs =
            crate::config::preview_size_limit_mib_inputs(&user_config.preview_size_limits);
        self.preview_size_limit_mib_errors = [const { None }; 7];
        self.preview_directory_expand_levels_input =
            user_config.preview_directory_expand_levels.to_string();
        self.preview_directory_expand_levels_error = None;
        self.invalidate_startup_directory_validation();
        self.startup_custom_directory_input = user_config
            .startup_custom_directory
            .to_string_lossy()
            .into_owned();
        self.startup_custom_directory_error = None;
        self.network_connections =
            crate::network_connections::NetworkConnectionState::from_saved_connections(
                user_config.network_connections.clone(),
            );
        self.application_theme
            .replace_custom_color_scheme(user_config.custom_color_scheme.clone());
        self.user_config = user_config;
        self.refresh_current_language();
    }
}

#[cfg(test)]
mod cli_tests;

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::PathBuf;

    use crate::app::FileBrowser;
    use crate::commands::classify_startup_session;
    use crate::config::{self, StartupLocationPolicy};
    use crate::matugen_theme::{ColorSchemePreset, ThemeMode};
    use crate::model::{
        BrowserPaneId, BrowserPaneLayout, BrowserPaneSession, BrowserSessionSnapshot,
        BrowserTabSession, BrowserViewMode, ClassifiedStartupSession, ColumnBrowserViewport,
        ExpandedDirectoryStatus, LoadedOperationStore, SidebarLocation, SidebarLocationKind,
        SplitAxis, StartupEnvironment, StartupSessionPlanRequest, StartupSessionSource,
    };
    use crate::network_connections::SavedNetworkConnection;
    use crate::startup_rendering::{
        StartupRenderingEnvironment, StartupRenderingEnvironmentStatus,
    };
    use desktop_linux::{
        NetworkConnection, NetworkConnectionId, NetworkMountState, NetworkProtocol,
    };
    use file_operation_store::{
        StoredFileOperationVerification, StoredOperation, StoredPath, StoredTransfer,
        StoredTransferConflictStrategy, TaskQueueStore, TRANSFER_JOURNAL_VERSION,
    };
    use tempfile::TempDir;

    fn startup_environment(
        home: PathBuf,
        user_config: config::UserConfig,
        state_database_path: PathBuf,
    ) -> StartupEnvironment {
        StartupEnvironment {
            home,
            system_language: config::UiLanguage::English,
            user_config,
            state_database_path,
            rendering_environment_status: StartupRenderingEnvironmentStatus::ready(
                StartupRenderingEnvironment::fast_default(
                    crate::startup_rendering::StartupRenderingBackend::Gl,
                ),
            ),
        }
    }

    fn create_directory(root: &TempDir, name: &str) -> PathBuf {
        let directory = root.path().join(name);
        fs::create_dir_all(&directory).expect("create test directory");
        directory
    }

    fn loaded_store(
        root: &TempDir,
        classified_startup_session: Option<ClassifiedStartupSession>,
    ) -> LoadedOperationStore {
        LoadedOperationStore {
            task_queue_store: TaskQueueStore::new(root.path().join("state.sqlite"))
                .expect("create operation store"),
            column_width_overrides: HashMap::new(),
            column_width_reference_content_width: None,
            classified_startup_session,
        }
    }

    #[test]
    fn restoring_file_operations_keeps_queue_panel_closed() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let loaded_store = loaded_store(&temp_dir, None);
        let claimed_task = loaded_store
            .task_queue_store
            .insert_claimed_recoverable_transfer_task(&StoredOperation::Copy {
                transfers: vec![StoredTransfer {
                    source: StoredPath::from_path(&temp_dir.path().join("source")),
                    target: StoredPath::from_path(&temp_dir.path().join("target")),
                    conflict_strategy: StoredTransferConflictStrategy::Fail,
                }],
                verification: StoredFileOperationVerification::BasicMetadata,
                recovery_version: Some(TRANSFER_JOURNAL_VERSION),
            })
            .expect("persist recoverable task");
        drop(claimed_task);

        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        assert!(!browser.operation_queue.is_panel_open());

        drop(browser.accept_operation_store(Ok(loaded_store)));

        assert_eq!(browser.operation_queue.task_count(), 1);
        assert!(!browser.operation_queue.is_panel_open());
        assert_eq!(browser.operation_queue.unread_count(), 1);
    }

    fn classify_previous_session(
        home: PathBuf,
        session: Option<BrowserSessionSnapshot>,
    ) -> ClassifiedStartupSession {
        classify_startup_session(
            StartupSessionPlanRequest {
                home,
                source: StartupSessionSource::PreviousSession,
            },
            session,
        )
    }

    fn tab_session(id: usize, directory: PathBuf, view_mode: BrowserViewMode) -> BrowserTabSession {
        BrowserTabSession {
            id,
            directory,
            is_trash_view: false,
            selected: None,
            selected_paths: HashSet::new(),
            deepest_open_column_directory: None,
            expanded_directories: Vec::new(),
            view_mode,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
        }
    }

    #[test]
    fn loaded_startup_environment_keeps_renderer_notice_hidden_when_env_matches() {
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            PathBuf::from("/home/user"),
            config::default_user_config(),
            PathBuf::from("/tmp/state.sqlite"),
        )));

        assert!(!browser.renderer_restart_notice_visible);
        assert!(browser.pending_renderer_restart_environment.is_none());
    }

    #[test]
    fn loaded_custom_color_scheme_replaces_the_runtime_theme_snapshot() {
        let mut user_config = config::default_user_config();
        user_config.theme_mode = ThemeMode::Light;
        user_config.color_scheme = ColorSchemePreset::Custom;
        user_config.custom_color_scheme.light.background = iced::Color::from_rgb8(1, 2, 3);
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            PathBuf::from("/home/user"),
            user_config,
            PathBuf::from("/tmp/state.sqlite"),
        )));

        assert_eq!(
            browser.theme_preview_colors(ColorSchemePreset::Custom)[0],
            iced::Color::from_rgb8(1, 2, 3)
        );
    }

    #[test]
    fn imported_sidebar_locations_become_configured_preferences() {
        let mut user_config = config::default_user_config();
        user_config.sidebar_favorites = None;
        let (mut browser, _) = FileBrowser::new(user_config);
        let home = PathBuf::from("/home/user");
        let projects = PathBuf::from("/srv/projects");

        drop(browser.accept_sidebar_locations(vec![
            SidebarLocation {
                label: "Home".to_owned(),
                path: home,
                kind: SidebarLocationKind::Home,
            },
            SidebarLocation {
                label: "Projects".to_owned(),
                path: projects.clone(),
                kind: SidebarLocationKind::Bookmark,
            },
        ]));

        assert_eq!(
            browser.user_config.sidebar_favorites,
            Some(vec![config::SidebarFavoriteConfig {
                label: "Projects".to_owned(),
                path: projects,
            }])
        );
    }

    #[test]
    fn home_startup_opens_home_directory() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            config::default_user_config(),
            temp_dir.path().join("state.sqlite"),
        )));

        assert_eq!(browser.current_dir, home);
        assert!(!browser.is_trash_view);
        assert_eq!(browser.current_error(), None);
        assert!(browser.directory_collection_phase.is_discovering());
        assert_eq!(browser.tabs.len(), 1);
        assert_eq!(browser.pane_layout.active(), BrowserPaneId::PRIMARY);
    }

    #[test]
    fn custom_startup_opens_configured_directory() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let workspace = create_directory(&temp_dir, "workspace");
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::CustomDirectory;
        user_config.startup_custom_directory = workspace.clone();
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        assert_ne!(browser.current_dir, workspace);
        let classified =
            classify_startup_session(browser.startup_session_plan_request(&home), None);
        drop(browser.accept_startup_plan(classified));

        assert_eq!(browser.current_dir, workspace);
        assert_eq!(browser.current_error(), None);
        assert!(browser.directory_collection_phase.is_discovering());
    }

    #[test]
    fn invalid_custom_startup_directory_falls_back_home_with_error() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let missing = temp_dir.path().join("missing");
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::CustomDirectory;
        user_config.startup_custom_directory = missing;
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        assert_eq!(browser.current_error(), None);
        let classified =
            classify_startup_session(browser.startup_session_plan_request(&home), None);
        drop(browser.accept_startup_plan(classified));

        assert_eq!(browser.current_dir, home);
        assert!(browser
            .current_error()
            .is_some_and(|error| error.contains("Could not open startup directory")));
    }

    #[test]
    fn previous_session_legacy_disabled_save_view_state_is_normalized() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let previous = create_directory(&temp_dir, "previous");
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
        user_config.save_view_state = false;
        let snapshot = BrowserSessionSnapshot {
            panes: vec![BrowserPaneSession {
                id: BrowserPaneId::PRIMARY,
                tabs: vec![tab_session(0, previous.clone(), BrowserViewMode::List)],
                active_tab_id: 0,
                column_browser_viewport: ColumnBrowserViewport::default(),
                column_viewports: HashMap::new(),
            }],
            layout: BrowserPaneLayout::Single {
                active: BrowserPaneId::PRIMARY,
            },
        };
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        drop(browser.accept_operation_store(Ok(loaded_store(
            &temp_dir,
            Some(classify_previous_session(home, Some(snapshot))),
        ))));

        assert_eq!(browser.current_dir, previous);
        assert!(browser.user_config.save_view_state);
    }

    #[test]
    fn home_startup_policy_does_not_schedule_browser_session_write() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::Home;
        user_config.save_view_state = false;
        let (mut browser, _) = FileBrowser::new(user_config);

        drop(browser.accept_operation_store(Ok(loaded_store(&temp_dir, None))));
        drop(browser.request_browser_session_save());

        assert!(!browser.pending_browser_session_save);
        assert!(browser.last_browser_session_save.is_none());
    }

    #[test]
    fn previous_session_restores_browser_tabs_and_panes_only() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let left = create_directory(&temp_dir, "left");
        let left_second = create_directory(&temp_dir, "left-second");
        let right = create_directory(&temp_dir, "right");
        let expanded = left_second.join("expanded");
        fs::create_dir_all(&expanded).expect("create expanded child directory");
        let selected_path = left_second.join("selected.txt");
        let mut active_left_tab = tab_session(2, left_second.clone(), BrowserViewMode::Columns);
        active_left_tab.selected = Some(selected_path.clone());
        active_left_tab.selected_paths = HashSet::from([selected_path]);
        active_left_tab.deepest_open_column_directory = Some(expanded.clone());
        active_left_tab.expanded_directories = vec![expanded.clone()];
        active_left_tab.back_stack = vec![left.clone()];
        active_left_tab.forward_stack = vec![right.clone()];
        let mut right_tab = tab_session(3, right.clone(), BrowserViewMode::List);
        right_tab.back_stack = vec![home.clone()];
        let snapshot = BrowserSessionSnapshot {
            panes: vec![
                BrowserPaneSession {
                    id: BrowserPaneId::PRIMARY,
                    tabs: vec![
                        tab_session(1, left.clone(), BrowserViewMode::List),
                        active_left_tab,
                    ],
                    active_tab_id: 2,
                    column_browser_viewport: ColumnBrowserViewport {
                        offset_x: 365.0,
                        width: 920.0,
                    },
                    column_viewports: HashMap::from([(
                        expanded.clone(),
                        crate::thumbnail_cache::ColumnViewport {
                            offset_y: 12.0,
                            height: 240.0,
                        },
                    )]),
                },
                BrowserPaneSession {
                    id: BrowserPaneId(1),
                    tabs: vec![right_tab],
                    active_tab_id: 3,
                    column_browser_viewport: ColumnBrowserViewport::default(),
                    column_viewports: HashMap::new(),
                },
            ],
            layout: BrowserPaneLayout::Split {
                axis: SplitAxis::Horizontal,
                first: BrowserPaneId::PRIMARY,
                second: BrowserPaneId(1),
                active: BrowserPaneId(1),
                first_portion: 500,
            },
        };
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
        user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        drop(browser.accept_operation_store(Ok(loaded_store(
            &temp_dir,
            Some(classify_previous_session(home.clone(), Some(snapshot))),
        ))));

        assert_eq!(browser.current_dir, right);
        assert_eq!(browser.view_mode, BrowserViewMode::List);
        assert!(matches!(
            browser.pane_layout,
            BrowserPaneLayout::Split {
                axis: SplitAxis::Horizontal,
                active: BrowserPaneId(1),
                ..
            }
        ));
        assert_eq!(browser.panes.len(), 2);
        assert_eq!(browser.next_tab_id, 4);
        assert_eq!(browser.next_pane_id, 2);
        let left_pane = browser
            .pane_by_id(BrowserPaneId::PRIMARY)
            .expect("left pane restored");
        assert_eq!(left_pane.active_tab_id, 2);
        assert_eq!(left_pane.tabs.len(), 2);
        assert!(left_pane.expanded_directories.contains_key(&expanded));
        assert_eq!(
            left_pane.column_browser_viewport,
            ColumnBrowserViewport {
                offset_x: 365.0,
                width: 920.0,
            }
        );
        assert_eq!(
            left_pane
                .column_viewports
                .get(&expanded)
                .map(|viewport| (viewport.offset_y, viewport.height)),
            Some((12.0, 240.0))
        );
        assert!(browser.preview.is_none());
        assert!(browser.properties.is_none());
        assert!(browser.preview_window.is_none());
        assert!(browser.properties_window.is_none());
        assert!(browser.settings_window.is_none());
    }

    #[test]
    fn previous_session_restores_column_chain_from_deepest_directory() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let project = home.join("project");
        let source = project.join("src");
        fs::create_dir_all(&source).expect("create nested column directories");
        let selected_path = source.join("main.rs");
        let mut active_tab = tab_session(1, home.clone(), BrowserViewMode::Columns);
        active_tab.selected = Some(selected_path.clone());
        active_tab.selected_paths = HashSet::from([selected_path]);
        active_tab.deepest_open_column_directory = Some(source.clone());
        let snapshot = BrowserSessionSnapshot {
            panes: vec![BrowserPaneSession {
                id: BrowserPaneId::PRIMARY,
                tabs: vec![active_tab],
                active_tab_id: 1,
                column_browser_viewport: ColumnBrowserViewport::default(),
                column_viewports: HashMap::new(),
            }],
            layout: BrowserPaneLayout::Single {
                active: BrowserPaneId::PRIMARY,
            },
        };
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
        user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        drop(browser.accept_operation_store(Ok(loaded_store(
            &temp_dir,
            Some(classify_previous_session(home.clone(), Some(snapshot))),
        ))));

        assert_eq!(browser.current_dir, home.clone());
        assert_eq!(
            browser.deepest_open_column_directory.as_ref(),
            Some(&source)
        );
        assert!(browser
            .expanded_directories
            .get(&project)
            .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading)));
        assert!(browser
            .expanded_directories
            .get(&source)
            .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading)));
        assert_eq!(
            crate::three_column_view::column_directories(&browser),
            vec![home, project, source]
        );
    }

    #[test]
    fn previous_session_without_saved_state_falls_back_home_with_error() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
        user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        drop(browser.accept_operation_store(Ok(loaded_store(
            &temp_dir,
            Some(classify_previous_session(home.clone(), None)),
        ))));

        assert!(browser
            .current_error()
            .is_some_and(|error| error.contains("No saved view state was found")));
    }

    #[test]
    fn previous_session_with_invalid_directories_falls_back_home_with_error() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let missing = temp_dir.path().join("missing");
        let snapshot = BrowserSessionSnapshot {
            panes: vec![BrowserPaneSession {
                id: BrowserPaneId::PRIMARY,
                tabs: vec![tab_session(0, missing, BrowserViewMode::List)],
                active_tab_id: 0,
                column_browser_viewport: ColumnBrowserViewport::default(),
                column_viewports: HashMap::new(),
            }],
            layout: BrowserPaneLayout::Single {
                active: BrowserPaneId::PRIMARY,
            },
        };
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
        user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        drop(browser.accept_operation_store(Ok(loaded_store(
            &temp_dir,
            Some(classify_previous_session(home.clone(), Some(snapshot))),
        ))));

        assert!(browser
            .current_error()
            .is_some_and(|error| error.contains("Saved view state could not be restored")));
    }

    #[test]
    fn previous_session_falls_back_home_when_operation_store_fails() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let home = create_directory(&temp_dir, "home");
        let mut user_config = config::default_user_config();
        user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
        user_config.save_view_state = user_config.startup_location_policy.saves_view_state();
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());

        drop(browser.accept_startup_environment(startup_environment(
            home.clone(),
            user_config,
            temp_dir.path().join("state.sqlite"),
        )));
        drop(browser.accept_operation_store(Err("state database is unavailable".to_owned())));

        assert!(browser
            .current_error()
            .is_some_and(|error| error.contains("Failed to restore saved view state")));
    }

    #[test]
    fn loaded_user_config_applies_saved_theme_selection() {
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());
        let mut user_config = config::default_user_config();
        user_config.theme_mode = ThemeMode::Dark;
        user_config.color_scheme = ColorSchemePreset::Nord;

        drop(browser.accept_startup_environment(startup_environment(
            PathBuf::from("/home/user"),
            user_config,
            PathBuf::from("/tmp/state.sqlite"),
        )));

        assert!(
            browser
                .application_theme
                .active(
                    browser.user_config.theme_mode,
                    browser.user_config.color_scheme
                )
                .extended_palette()
                .is_dark
        );
        assert_eq!(browser.user_config.theme_mode, ThemeMode::Dark);
        assert_eq!(browser.user_config.color_scheme, ColorSchemePreset::Nord);
    }

    #[test]
    fn startup_marks_only_auto_connect_network_connections_connecting() {
        let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());
        let auto_connection = NetworkConnection::new(
            NetworkConnectionId::new("auto"),
            "Auto",
            NetworkProtocol::Smb,
            "smb://server/auto",
        )
        .unwrap();
        let manual_connection = NetworkConnection::new(
            NetworkConnectionId::new("manual"),
            "Manual",
            NetworkProtocol::Smb,
            "smb://server/manual",
        )
        .unwrap();
        let auto_id = auto_connection.id.clone();
        let manual_id = manual_connection.id.clone();
        let mut user_config = config::default_user_config();
        user_config.network_connections = vec![
            SavedNetworkConnection::new(auto_connection, true),
            SavedNetworkConnection::new(manual_connection, false),
        ];

        drop(browser.accept_startup_environment(startup_environment(
            PathBuf::from("/home/user"),
            user_config,
            PathBuf::from("/tmp/state.sqlite"),
        )));

        assert_eq!(browser.current_dir, PathBuf::from("/home/user"));
        assert!(browser.network_connections.is_pending(&auto_id));
        assert!(!browser.network_connections.is_pending(&manual_id));
        assert!(matches!(
            browser
                .network_connections
                .entry(&auto_id)
                .map(|entry| &entry.state),
            Some(NetworkMountState::Connecting)
        ));
        assert!(matches!(
            browser
                .network_connections
                .entry(&manual_id)
                .map(|entry| &entry.state),
            Some(NetworkMountState::Disconnected)
        ));
    }
}
