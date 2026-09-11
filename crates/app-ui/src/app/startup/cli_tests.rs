use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use file_operation_store::TaskQueueStore;
use tempfile::TempDir;

use super::FileBrowser;
use crate::command_line::{parse_arguments, ApplicationLaunchRequest, CommandLineAction};
use crate::commands::classify_startup_session;
use crate::config::{self, StartupLocationPolicy};
use crate::model::{
    BrowserPaneId, BrowserPaneLayout, BrowserPaneSession, BrowserSessionSnapshot,
    BrowserTabSession, BrowserViewMode, ClassifiedStartupSession, ColumnBrowserViewport,
    LoadedOperationStore, StartupEnvironment, StartupSessionPlanRequest, StartupSessionSource,
};
use crate::startup_rendering::{StartupRenderingEnvironment, StartupRenderingEnvironmentStatus};

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

fn explicit_launch_request(
    paths: &[PathBuf],
    current_directory: &Path,
) -> ApplicationLaunchRequest {
    let arguments = paths
        .iter()
        .map(|path| OsString::from(path.as_os_str()))
        .collect::<Vec<_>>();
    match parse_arguments(arguments, current_directory).expect("parse explicit paths") {
        CommandLineAction::Launch(request) => request,
        _ => panic!("expected launch request"),
    }
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

fn classify_previous_session(
    home: PathBuf,
    session: BrowserSessionSnapshot,
) -> ClassifiedStartupSession {
    classify_startup_session(
        StartupSessionPlanRequest {
            home,
            source: StartupSessionSource::PreviousSession,
        },
        Some(session),
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
fn explicit_workspace_replaces_previous_session_without_persisting_it() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let home = create_directory(&temp_dir, "home");
    let first = create_directory(&temp_dir, "first");
    let second = create_directory(&temp_dir, "second");
    let selected_file = first.join("selected.txt");
    fs::write(&selected_file, "selected").expect("write selected file");
    let old_session_directory = create_directory(&temp_dir, "old-session");
    let mut user_config = config::default_user_config();
    user_config.startup_location_policy = StartupLocationPolicy::PreviousSession;
    user_config.browser_view_mode = BrowserViewMode::Icons;
    let request =
        explicit_launch_request(&[selected_file.clone(), second.clone()], temp_dir.path());
    let (mut browser, _) =
        FileBrowser::new_with_launch_request(config::ui_thread_startup_config(), request);

    drop(browser.accept_startup_environment(startup_environment(
        home.clone(),
        user_config,
        temp_dir.path().join("state.sqlite"),
    )));

    assert_eq!(browser.current_dir, first);
    assert_eq!(browser.tabs.len(), 2);
    assert_eq!(browser.tabs[0].directory, first);
    assert_eq!(browser.tabs[0].selected.as_ref(), Some(&selected_file));
    assert!(browser.tabs[0].selected_paths.contains(&selected_file));
    assert_eq!(browser.tabs[0].view_mode, BrowserViewMode::Icons);
    assert_eq!(browser.tabs[1].directory, second);
    assert_eq!(browser.active_tab_id, 0);
    assert!(!browser.should_save_browser_session());

    let old_snapshot = BrowserSessionSnapshot {
        panes: vec![BrowserPaneSession {
            id: BrowserPaneId::PRIMARY,
            tabs: vec![tab_session(0, old_session_directory, BrowserViewMode::List)],
            active_tab_id: 0,
            column_browser_viewport: ColumnBrowserViewport::default(),
            column_viewports: HashMap::new(),
        }],
        layout: BrowserPaneLayout::Single {
            active: BrowserPaneId::PRIMARY,
        },
    };
    drop(browser.accept_operation_store(Ok(loaded_store(
        &temp_dir,
        Some(classify_previous_session(home, old_snapshot)),
    ))));

    assert_eq!(browser.current_dir, first);
    assert_eq!(browser.tabs.len(), 2);
    assert_eq!(browser.tabs[1].directory, second);
    assert!(!browser.should_save_browser_session());
    drop(browser.request_browser_session_save());
    assert!(!browser.pending_browser_session_save);
    assert!(browser.last_browser_session_save.is_none());
}

#[test]
fn restored_workspace_reserves_identifiers_for_new_tabs() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let home = create_directory(&temp_dir, "home");
    let first = create_directory(&temp_dir, "first");
    let second = create_directory(&temp_dir, "second");
    let third = create_directory(&temp_dir, "third");
    let request = explicit_launch_request(&[first, second], temp_dir.path());
    let (mut browser, _) =
        FileBrowser::new_with_launch_request(config::ui_thread_startup_config(), request);

    drop(browser.accept_startup_environment(startup_environment(
        home,
        config::default_user_config(),
        temp_dir.path().join("state.sqlite"),
    )));
    drop(browser.open_directory_in_new_tab(third));

    let tab_ids = browser.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
    assert_eq!(tab_ids, vec![0, 1, 2]);
}
