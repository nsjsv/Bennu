use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind};

use crate::app::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, BrowserViewMode,
    ColumnBrowserViewport, DirectoryExpansionLoadContext, ExpandedDirectory,
    ExpandedDirectoryLoadRequest, ExpandedDirectoryStatus, IconGridExpansionAnchor,
    IconGridExpansionContext, IconGridExpansionSessionId, IconGridExpansionState, ListColumnKind,
    SplitAxis, StartupEnvironment,
};
use crate::startup_rendering::{StartupRenderingEnvironment, StartupRenderingEnvironmentStatus};
use crate::thumbnail_cache::ColumnViewport;

fn test_entry(path: PathBuf, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        path,
        kind,
        EntryMetadata {
            len: 0,
            modified: None,
            ..EntryMetadata::default()
        },
        false,
        false,
        false,
    )
}

fn loaded_directory(entries: Vec<DirectoryEntry>) -> ExpandedDirectory {
    ExpandedDirectory {
        entries,
        directory_discovery: None,
        status: ExpandedDirectoryStatus::Loaded,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

fn icon_grid_request(browser: &FileBrowser, path: &PathBuf) -> ExpandedDirectoryLoadRequest {
    let state = browser
        .icon_grid_expansion
        .as_ref()
        .expect("icon grid expansion");
    let directory = state.directory(path).expect("expanded icon directory");
    ExpandedDirectoryLoadRequest {
        context: DirectoryExpansionLoadContext::IconGrid {
            pane_id: state.context().pane_id,
            current_dir: state.context().current_dir.clone(),
            session_id: state.context().session_id,
        },
        path: path.clone(),
        generation: directory.contents.load_generation,
    }
}

fn list_request(browser: &FileBrowser, path: &PathBuf) -> ExpandedDirectoryLoadRequest {
    let request = browser
        .pending_list_expansion_follow_request()
        .expect("pending list expansion follow request");
    assert_eq!(&request.path, path);
    request
}

fn finish_icon_animation(browser: &mut FileBrowser) {
    for _ in 0..6 {
        drop(browser.advance_icon_grid_expansion_animation());
    }
}

fn pane_from_tab_for_test(pane_id: BrowserPaneId, tab: BrowserTab) -> BrowserPane {
    BrowserPane {
        id: pane_id,
        current_dir: tab.directory.clone(),
        is_trash_view: tab.is_trash_view,
        entries: tab.entries.clone(),
        directory_discovery: tab.directory_discovery.clone(),
        directory_loading_placeholder: None,
        trash_entries: tab.trash_entries.clone(),
        selected: tab.selected.clone(),
        selected_paths: tab.selected_paths.clone(),
        selection_anchor: tab.selection_anchor.clone(),
        deepest_open_column_directory: tab.deepest_open_column_directory.clone(),
        expanded_directories: tab.expanded_directories.clone(),
        view_mode: tab.view_mode,
        column_browser_viewport: ColumnBrowserViewport::default(),
        column_viewports: HashMap::<PathBuf, ColumnViewport>::new(),
        tabs: vec![tab.clone()],
        active_tab_id: tab.id,
        directory_load_generation: 0,
        directory_load_cancel: None,
        back_stack: tab.back_stack.clone(),
        forward_stack: tab.forward_stack.clone(),
        directory_collection_phase: crate::model::DirectoryCollectionPhase::Ready,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: file_core::SortField::Name,
            direction: file_core::SortDirection::Ascending,
        },
    }
}

#[test]
fn new_browser_uses_configured_default_view_mode() {
    let mut config = config::default_user_config();
    config.browser_view_mode = BrowserViewMode::List;

    let (browser, _) = FileBrowser::new(config);

    assert_eq!(browser.view_mode, BrowserViewMode::List);
    assert!(browser
        .tabs
        .iter()
        .all(|tab| tab.view_mode == BrowserViewMode::List));
    assert!(browser
        .pane_by_id(BrowserPaneId::PRIMARY)
        .is_some_and(|pane| pane.view_mode == BrowserViewMode::List));
}

#[test]
fn loaded_user_config_updates_startup_view_mode() {
    let (mut browser, _) = FileBrowser::new(config::ui_thread_startup_config());
    let mut user_config = config::default_user_config();
    user_config.browser_view_mode = BrowserViewMode::List;

    drop(browser.accept_startup_environment(StartupEnvironment {
        home: PathBuf::from("/home/user"),
        system_language: config::UiLanguage::English,
        user_config,
        state_database_path: PathBuf::from("/tmp/state.sqlite"),
        rendering_environment_status: StartupRenderingEnvironmentStatus::ready(
            StartupRenderingEnvironment::fast_default(
                crate::startup_rendering::StartupRenderingBackend::Gl,
            ),
        ),
    }));

    assert_eq!(browser.view_mode, BrowserViewMode::List);
    assert!(browser
        .tabs
        .iter()
        .find(|tab| tab.id == browser.active_tab_id)
        .is_some_and(|tab| tab.view_mode == BrowserViewMode::List));
    assert!(browser
        .pane_by_id(BrowserPaneId::PRIMARY)
        .is_some_and(|pane| pane.view_mode == BrowserViewMode::List));
}

#[test]
fn selecting_view_mode_updates_persisted_default_view_mode() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.enter_list_header_column(BrowserPaneId::PRIMARY, ListColumnKind::Name));

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert_eq!(browser.view_mode, BrowserViewMode::List);
    assert_eq!(browser.user_config.browser_view_mode, BrowserViewMode::List);
    assert_eq!(
        browser.hovered_list_header_column(BrowserPaneId::PRIMARY),
        None
    );
}

#[test]
fn switching_tabs_restores_view_mode_and_expanded_directories() {
    let root = PathBuf::from("/workspace");
    let active_directory = root.join("active");
    let active_child = active_directory.join("child.txt");
    let other_directory = root.join("other");
    let other_child = other_directory.join("child.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![test_entry(active_directory.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::List;
    browser.expanded_directories.insert(
        active_directory.clone(),
        loaded_directory(vec![test_entry(active_child.clone(), FileKind::File)]),
    );
    browser.tabs = vec![
        BrowserTab::directory(0, root.clone()),
        BrowserTab::directory(1, other_directory.clone()),
    ];
    browser.active_tab_id = 0;
    browser.tabs[1].entries = vec![test_entry(other_directory.clone(), FileKind::Directory)].into();
    browser.tabs[1].view_mode = BrowserViewMode::Columns;
    browser.tabs[1].expanded_directories.insert(
        other_directory.clone(),
        loaded_directory(vec![test_entry(other_child.clone(), FileKind::File)]),
    );

    drop(browser.select_tab(1));

    assert_eq!(browser.view_mode, BrowserViewMode::Columns);
    assert!(browser.expanded_directories.contains_key(&other_directory));

    drop(browser.select_tab(0));

    assert_eq!(browser.view_mode, BrowserViewMode::List);
    assert!(browser.expanded_directories.contains_key(&active_directory));
    assert!(browser
        .tabs
        .iter()
        .find(|tab| tab.id == 1)
        .is_some_and(|tab| tab.view_mode == BrowserViewMode::Columns
            && tab.expanded_directories.contains_key(&other_directory)));
}

#[test]
fn switching_from_columns_to_list_keeps_only_open_column_chain() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let src = project.join("src");
    let main_rs = src.join("main.rs");
    let stale = root.join("stale");
    let stale_child = stale.join("old.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(stale.clone(), FileKind::Directory),
    ]
    .into();
    browser.view_mode = BrowserViewMode::Columns;
    browser.deepest_open_column_directory = Some(src.clone());
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(src.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        src.clone(),
        loaded_directory(vec![test_entry(main_rs, FileKind::File)]),
    );
    browser.expanded_directories.insert(
        stale.clone(),
        loaded_directory(vec![test_entry(stale_child, FileKind::File)]),
    );

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert_eq!(browser.view_mode, BrowserViewMode::List);
    assert_eq!(
        browser
            .expanded_directories
            .keys()
            .cloned()
            .collect::<HashSet<_>>(),
        HashSet::from([project.clone(), src.clone()])
    );
    assert!(browser
        .expanded_directories
        .values()
        .all(|expanded| expanded.is_expanded
            && !expanded.is_collapsing
            && (expanded.animation_progress - 1.0).abs() <= f32::EPSILON));
    assert!(!browser.expanded_directories.contains_key(&stale));
}

#[test]
fn switching_from_list_to_columns_replaces_stale_column_history() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let src = project.join("src");
    let main_rs = src.join("main.rs");
    let stale = root.join("stale");
    let stale_child = stale.join("old.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(stale.clone(), FileKind::Directory),
    ]
    .into();
    browser.view_mode = BrowserViewMode::List;
    browser.selected = Some(main_rs.clone());
    browser.selected_paths = HashSet::from([main_rs.clone()]);
    browser.deepest_open_column_directory = Some(stale.clone());
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(src.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        src.clone(),
        loaded_directory(vec![test_entry(main_rs, FileKind::File)]),
    );
    browser.expanded_directories.insert(
        stale.clone(),
        loaded_directory(vec![test_entry(stale_child, FileKind::File)]),
    );

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Columns));

    assert_eq!(browser.view_mode, BrowserViewMode::Columns);
    assert_eq!(browser.deepest_open_column_directory, Some(src.clone()));
    assert_eq!(
        browser
            .expanded_directories
            .keys()
            .cloned()
            .collect::<HashSet<_>>(),
        HashSet::from([project.clone(), src.clone()])
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![root, project, src]
    );
}

#[test]
fn list_to_icons_follows_only_the_selected_loaded_branch() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let src = project.join("src");
    let target = src.join("main.rs");
    let stale = root.join("stale");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(stale.clone(), FileKind::Directory),
    ]
    .into();
    browser.view_mode = BrowserViewMode::List;
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(src.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        src.clone(),
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    );
    browser
        .expanded_directories
        .insert(stale.clone(), loaded_directory(Vec::new()));
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target.clone()]);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));

    assert_eq!(browser.selected, None);
    assert_eq!(browser.expanded_directories.len(), 3);
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(IconGridExpansionState::has_follow_plan));
    let project_request = icon_grid_request(&browser, &project);
    drop(browser.accept_complete_expanded_directory_fixture(
        project_request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: vec![test_entry(src.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));
    finish_icon_animation(&mut browser);
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| state.directory(&src).is_some()));
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| state.directory(&stale).is_none()));

    let src_request = icon_grid_request(&browser, &src);
    drop(browser.accept_complete_expanded_directory_fixture(
        src_request,
        Ok(DirectoryScan {
            path: src.clone(),
            entries: vec![test_entry(target.clone(), FileKind::File)],
            skipped: Vec::new(),
        }),
    ));
    finish_icon_animation(&mut browser);

    assert_eq!(browser.selected, Some(target.clone()));
    assert_eq!(browser.selected_paths, HashSet::from([target]));
    assert_eq!(
        browser
            .icon_grid_expansion
            .as_ref()
            .map(IconGridExpansionState::selection_directory),
        Some(src.as_path())
    );
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| !state.has_follow_plan()));
}

#[test]
fn list_to_icons_includes_the_selected_directory_when_it_is_expanded() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::List;
    browser
        .expanded_directories
        .insert(project.clone(), loaded_directory(Vec::new()));
    browser.selected = Some(project.clone());
    browser.selected_paths = HashSet::from([project.clone()]);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));

    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| state.root_path() == project));
    let request = icon_grid_request(&browser, &project);
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));
    finish_icon_animation(&mut browser);
    assert_eq!(browser.selected, Some(project));
}

#[test]
fn user_selection_cancels_list_to_icons_follow_before_old_load_completes() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let src = project.join("src");
    let target = src.join("main.rs");
    let other = root.join("other");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(other.clone(), FileKind::File),
    ]
    .into();
    browser.view_mode = BrowserViewMode::List;
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(src.clone(), FileKind::Directory)]),
    );
    browser.expanded_directories.insert(
        src.clone(),
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    );
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target]);
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));
    let request = icon_grid_request(&browser, &project);

    browser.select_path(other.clone());
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: vec![test_entry(src.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));
    finish_icon_animation(&mut browser);

    assert_eq!(browser.selected, Some(other));
    assert!(browser
        .icon_grid_expansion
        .as_ref()
        .is_some_and(|state| !state.has_follow_plan() && state.directory(&src).is_none()));
}

#[test]
fn icons_to_list_reloads_its_own_single_chain_before_restoring_selection() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let src = project.join("src");
    let target = src.join("main.rs");
    let stale = root.join("stale");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(stale.clone(), FileKind::Directory),
    ]
    .into();
    browser.view_mode = BrowserViewMode::Icons;
    browser
        .expanded_directories
        .insert(stale, loaded_directory(Vec::new()));
    let mut expansion = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: root.clone(),
            session_id: IconGridExpansionSessionId::new(41),
        },
        IconGridExpansionAnchor {
            parent_directory: root.clone(),
            path: project.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(src.clone(), FileKind::Directory)]),
    );
    assert!(expansion.insert_directory(
        IconGridExpansionAnchor {
            parent_directory: project.clone(),
            path: src.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    ));
    browser.icon_grid_expansion = Some(expansion);
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target.clone()]);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert_eq!(
        browser.expanded_directories.keys().collect::<Vec<_>>(),
        vec![&project]
    );
    assert_eq!(browser.selected, None);
    assert!(browser.list_expansion_follow.is_some());
    let project_request = list_request(&browser, &project);
    let mut stale_request = project_request.clone();
    stale_request.generation += 1;
    drop(browser.accept_complete_expanded_directory_fixture(
        stale_request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: Vec::new(),
            skipped: Vec::new(),
        }),
    ));
    assert!(browser.list_expansion_follow.is_some());
    assert!(browser
        .expanded_directories
        .get(&project)
        .is_some_and(|expanded| matches!(expanded.status, ExpandedDirectoryStatus::Loading)));
    drop(browser.accept_complete_expanded_directory_fixture(
        project_request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: vec![test_entry(src.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));
    assert!(browser.expanded_directories.contains_key(&src));
    assert_eq!(browser.selected, None);

    let src_request = list_request(&browser, &src);
    drop(browser.accept_complete_expanded_directory_fixture(
        src_request,
        Ok(DirectoryScan {
            path: src,
            entries: vec![test_entry(target.clone(), FileKind::File)],
            skipped: Vec::new(),
        }),
    ));

    assert_eq!(browser.selected, Some(target.clone()));
    assert_eq!(browser.selected_paths, HashSet::from([target]));
    assert!(browser.list_expansion_follow.is_none());
}

#[test]
fn user_right_click_selection_cancels_icons_to_list_follow_before_old_load_completes() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let src = project.join("src");
    let target = src.join("main.rs");
    let other = root.join("other.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(other.clone(), FileKind::File),
    ]
    .into();
    browser.view_mode = BrowserViewMode::Icons;
    let mut expansion = IconGridExpansionState::new(
        IconGridExpansionContext {
            pane_id: BrowserPaneId::PRIMARY,
            current_dir: root.clone(),
            session_id: IconGridExpansionSessionId::new(42),
        },
        IconGridExpansionAnchor {
            parent_directory: root,
            path: project.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(src.clone(), FileKind::Directory)]),
    );
    assert!(expansion.insert_directory(
        IconGridExpansionAnchor {
            parent_directory: project.clone(),
            path: src.clone(),
            index: 0,
        },
        loaded_directory(vec![test_entry(target.clone(), FileKind::File)]),
    ));
    browser.icon_grid_expansion = Some(expansion);
    browser.selected = Some(target.clone());
    browser.selected_paths = HashSet::from([target]);
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));
    let request = list_request(&browser, &project);

    drop(browser.handle_entry_right_clicked(other.clone()));
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: project.clone(),
            entries: vec![test_entry(src.clone(), FileKind::Directory)],
            skipped: Vec::new(),
        }),
    ));

    assert_eq!(browser.selected, None);
    assert_eq!(browser.selected_paths, HashSet::from([other]));
    assert!(browser.list_expansion_follow.is_none());
    assert!(!browser.expanded_directories.contains_key(&project));
    assert!(!browser.expanded_directories.contains_key(&src));
}

#[test]
fn icons_to_list_without_interactive_chain_clears_hidden_list_expansion() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::Icons;
    browser
        .expanded_directories
        .insert(project, loaded_directory(Vec::new()));

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert!(browser.expanded_directories.is_empty());
    assert!(browser.list_expansion_follow.is_none());
}

#[test]
fn entering_icons_keeps_hierarchy_and_removes_hidden_selection() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let hidden_child = project.join("hidden.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::List;
    browser.deepest_open_column_directory = Some(project.clone());
    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(hidden_child.clone(), FileKind::File)]),
    );
    browser.selected = Some(hidden_child.clone());
    browser.selected_paths = HashSet::from([hidden_child.clone()]);
    browser.selection_anchor = Some(hidden_child);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));

    assert_eq!(browser.view_mode, BrowserViewMode::Icons);
    assert_eq!(browser.deepest_open_column_directory, Some(project.clone()));
    assert!(browser.expanded_directories.contains_key(&project));
    assert_eq!(browser.selected, None);
    assert!(browser.selected_paths.is_empty());
    assert_eq!(browser.selection_anchor, None);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert!(browser.expanded_directories.is_empty());
    assert_eq!(browser.deepest_open_column_directory, Some(project));
}

#[test]
fn icons_to_columns_rebuilds_chain_from_direct_selection() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let stale = root.join("stale");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![
        test_entry(project.clone(), FileKind::Directory),
        test_entry(stale.clone(), FileKind::Directory),
    ]
    .into();
    browser.view_mode = BrowserViewMode::Icons;
    browser.selected = Some(project.clone());
    browser.selected_paths = HashSet::from([project.clone()]);
    browser.deepest_open_column_directory = Some(stale.clone());
    browser
        .expanded_directories
        .insert(project.clone(), loaded_directory(Vec::new()));
    browser
        .expanded_directories
        .insert(stale, loaded_directory(Vec::new()));

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Columns));

    assert_eq!(browser.deepest_open_column_directory, Some(project.clone()));
    assert_eq!(
        browser
            .expanded_directories
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![project.clone()]
    );
    assert_eq!(
        crate::three_column_view::column_directories(&browser),
        vec![root, project]
    );
}

#[test]
fn icons_to_columns_without_selection_preserves_hidden_chain() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::Icons;
    browser.deepest_open_column_directory = Some(project.clone());
    browser
        .expanded_directories
        .insert(project.clone(), loaded_directory(Vec::new()));

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Columns));

    assert_eq!(browser.deepest_open_column_directory, Some(project.clone()));
    assert!(browser.expanded_directories.contains_key(&project));
}

#[test]
fn icon_selection_without_focus_restores_direct_focus_before_columns() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.view_mode = BrowserViewMode::List;
    browser.selected = None;
    browser.selected_paths = HashSet::from([project.clone()]);
    browser
        .expanded_directories
        .insert(project.clone(), loaded_directory(Vec::new()));

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));

    assert_eq!(browser.selected, Some(project.clone()));
    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Columns));

    assert_eq!(browser.deepest_open_column_directory, Some(project));
}

#[test]
fn activating_split_panes_preserves_each_pane_view_mode() {
    let left_dir = PathBuf::from("/workspace/left");
    let right_dir = PathBuf::from("/workspace/right");
    let left_tab = BrowserTab::directory(0, left_dir.clone());
    let mut right_tab = BrowserTab::directory(1, right_dir.clone());
    right_tab.view_mode = BrowserViewMode::Columns;
    right_tab.selected_paths = HashSet::new();

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = left_dir.clone();
    browser.tabs = vec![left_tab.clone()];
    browser.active_tab_id = left_tab.id;
    browser.view_mode = BrowserViewMode::Icons;
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: BrowserPaneId(1),
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };
    browser.panes = vec![
        pane_from_tab_for_test(BrowserPaneId::PRIMARY, left_tab),
        pane_from_tab_for_test(BrowserPaneId(1), right_tab),
    ];

    browser.activate_pane(BrowserPaneId(1));

    assert_eq!(browser.current_dir, right_dir);
    assert_eq!(browser.view_mode, BrowserViewMode::Columns);

    browser.view_mode = BrowserViewMode::Columns;
    browser.activate_pane(BrowserPaneId::PRIMARY);

    assert_eq!(browser.current_dir, left_dir);
    assert_eq!(browser.view_mode, BrowserViewMode::Icons);
    assert!(browser
        .pane_by_id(BrowserPaneId(1))
        .is_some_and(|pane| pane.view_mode == BrowserViewMode::Columns));
}

#[test]
fn switching_from_columns_to_list_reveals_selected_entry_immediately() {
    let root = PathBuf::from("/workspace");
    let notes = root.join("notes.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![test_entry(notes.clone(), FileKind::File)].into();
    browser.selected = Some(notes.clone());
    browser.selected_paths = HashSet::from([notes.clone()]);
    browser.view_mode = BrowserViewMode::Columns;

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert_eq!(browser.pending_view_switch_reveal, None);
    assert!(browser.view_switch_reveal_scroll(&notes).is_some());
}

#[test]
fn switching_from_list_to_columns_reveals_selected_entry_immediately() {
    let root = PathBuf::from("/workspace");
    let notes = root.join("notes.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![test_entry(notes.clone(), FileKind::File)].into();
    browser.selected = Some(notes.clone());
    browser.selected_paths = HashSet::from([notes.clone()]);
    browser.view_mode = BrowserViewMode::List;

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Columns));

    assert_eq!(browser.pending_view_switch_reveal, None);
    assert!(browser.view_switch_reveal_scroll(&notes).is_some());
}

#[test]
fn switching_from_list_to_icons_reveals_selected_entry_immediately() {
    let root = PathBuf::from("/workspace");
    let notes = root.join("notes.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![test_entry(notes.clone(), FileKind::File)].into();
    browser.selected = Some(notes.clone());
    browser.selected_paths = HashSet::from([notes.clone()]);
    browser.view_mode = BrowserViewMode::List;

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Icons));

    assert_eq!(browser.pending_view_switch_reveal, None);
    assert!(browser.view_switch_reveal_scroll(&notes).is_some());
}

#[test]
fn switching_with_loading_directory_parks_pending_reveal() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let notes = project.join("notes.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root.clone();
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.selected = Some(notes.clone());
    browser.selected_paths = HashSet::from([notes.clone()]);
    browser.view_mode = BrowserViewMode::List;
    let mut loading = loaded_directory(vec![]);
    loading.status = ExpandedDirectoryStatus::Loading;
    browser
        .expanded_directories
        .insert(project.clone(), loading);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Columns));

    assert_eq!(
        browser.pending_view_switch_reveal,
        Some((BrowserPaneId::PRIMARY, notes.clone()))
    );

    browser.expanded_directories.insert(
        project.clone(),
        loaded_directory(vec![test_entry(notes.clone(), FileKind::File)]),
    );
    drop(browser.complete_pending_view_switch_reveal(&project));

    assert_eq!(browser.pending_view_switch_reveal, None);
}

#[test]
fn pending_view_switch_reveal_discarded_when_selection_changes() {
    let root = PathBuf::from("/workspace");
    let project = root.join("project");
    let notes = project.join("notes.txt");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.entries = vec![test_entry(project.clone(), FileKind::Directory)].into();
    browser.selected = Some(notes.clone());
    browser.selected_paths = HashSet::from([notes]);
    browser.view_mode = BrowserViewMode::List;
    let mut loading = loaded_directory(vec![]);
    loading.status = ExpandedDirectoryStatus::Loading;
    browser
        .expanded_directories
        .insert(project.clone(), loading);

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::Columns));
    assert!(browser.pending_view_switch_reveal.is_some());

    browser.selected = None;
    browser.selected_paths.clear();
    drop(browser.complete_pending_view_switch_reveal(&project));

    assert_eq!(browser.pending_view_switch_reveal, None);
}

#[test]
fn switching_without_selection_skips_pending_reveal() {
    let root = PathBuf::from("/workspace");
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = root;
    browser.view_mode = BrowserViewMode::Columns;

    drop(browser.select_browser_view_mode(BrowserPaneId::PRIMARY, BrowserViewMode::List));

    assert_eq!(browser.pending_view_switch_reveal, None);
}
