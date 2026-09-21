use std::path::{Path, PathBuf};

use iced::Task;

use crate::model::{
    BrowserSessionSnapshot, ClassifiedStartupSession, Message, StartupDirectoryAvailability,
    StartupDirectoryValidationRequest, StartupSessionPlan, StartupSessionPlanRequest,
    StartupSessionSource,
};

pub(crate) fn startup_directory_validation_command(
    request: StartupDirectoryValidationRequest,
) -> Task<Message> {
    let issued_request = request.clone();
    Task::perform(
        validate_startup_directory(request.directory),
        move |availability| {
            Message::StartupCustomDirectoryValidated(issued_request.clone(), availability)
        },
    )
}

pub(crate) fn startup_session_plan_command(request: StartupSessionPlanRequest) -> Task<Message> {
    let fallback_request = request.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || classify_startup_session(request, None))
                .await
                .unwrap_or_else(|_| ClassifiedStartupSession {
                    plan: StartupSessionPlan::Directory {
                        directory: fallback_request.home.clone(),
                        error: Some(
                            "Failed to validate startup paths; opening the home directory."
                                .to_owned(),
                        ),
                    },
                    request: fallback_request,
                })
        },
        Message::StartupSessionClassified,
    )
}

pub(crate) fn classify_startup_session(
    request: StartupSessionPlanRequest,
    browser_session: Option<BrowserSessionSnapshot>,
) -> ClassifiedStartupSession {
    let plan = match &request.source {
        StartupSessionSource::Home => StartupSessionPlan::Directory {
            directory: request.home.clone(),
            error: None,
        },
        StartupSessionSource::CustomDirectory(custom_directory) => {
            if directory_is_usable(custom_directory) {
                StartupSessionPlan::Directory {
                    directory: custom_directory.clone(),
                    error: None,
                }
            } else {
                StartupSessionPlan::Directory {
                    directory: request.home.clone(),
                    error: Some(format!(
                        "Could not open startup directory {}; opening the home directory.",
                        custom_directory.to_string_lossy()
                    )),
                }
            }
        }
        StartupSessionSource::PreviousSession => match browser_session {
            Some(session) => StartupSessionPlan::Session(validated_browser_session(session)),
            None => StartupSessionPlan::Directory {
                directory: request.home.clone(),
                error: Some(
                    "No saved view state was found; opening the home directory.".to_owned(),
                ),
            },
        },
    };
    ClassifiedStartupSession { request, plan }
}

async fn validate_startup_directory(directory: PathBuf) -> StartupDirectoryAvailability {
    tokio::task::spawn_blocking(move || {
        if directory_is_usable(&directory) {
            StartupDirectoryAvailability::Usable
        } else {
            StartupDirectoryAvailability::Unavailable
        }
    })
    .await
    .unwrap_or(StartupDirectoryAvailability::Unavailable)
}

fn validated_browser_session(mut session: BrowserSessionSnapshot) -> BrowserSessionSnapshot {
    for pane in &mut session.panes {
        pane.tabs
            .retain(|tab| tab.is_trash_view || directory_is_usable(&tab.directory));
    }
    session.panes.retain(|pane| !pane.tabs.is_empty());
    session
}

fn directory_is_usable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fs;

    use super::*;
    use crate::model::{
        BrowserPaneId, BrowserPaneLayout, BrowserPaneSession, BrowserTabSession, BrowserViewMode,
        ColumnBrowserViewport,
    };

    fn startup_request(home: PathBuf, source: StartupSessionSource) -> StartupSessionPlanRequest {
        StartupSessionPlanRequest { home, source }
    }

    fn tab_session(id: usize, directory: PathBuf, is_trash_view: bool) -> BrowserTabSession {
        BrowserTabSession {
            id,
            directory,
            is_trash_view,
            selected: None,
            selected_paths: HashSet::new(),
            deepest_open_column_directory: None,
            expanded_directories: Vec::new(),
            view_mode: BrowserViewMode::List,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
        }
    }

    #[test]
    fn custom_startup_classification_accepts_directories_and_rejects_files() {
        let root = tempfile::tempdir().expect("create temp directory");
        let home = root.path().join("home");
        let custom_directory = root.path().join("custom");
        let custom_file = root.path().join("file");
        fs::create_dir_all(&home).expect("create home directory");
        fs::create_dir_all(&custom_directory).expect("create custom directory");
        fs::write(&custom_file, b"file").expect("create custom file");

        let classified = classify_startup_session(
            startup_request(
                home.clone(),
                StartupSessionSource::CustomDirectory(custom_directory.clone()),
            ),
            None,
        );
        let StartupSessionPlan::Directory { directory, error } = classified.plan else {
            panic!("custom startup should resolve to a directory plan");
        };
        assert_eq!(directory, custom_directory);
        assert_eq!(error, None);

        let classified = classify_startup_session(
            startup_request(
                home.clone(),
                StartupSessionSource::CustomDirectory(custom_file),
            ),
            None,
        );
        let StartupSessionPlan::Directory { directory, error } = classified.plan else {
            panic!("invalid custom startup should resolve to a directory plan");
        };
        assert_eq!(directory, home);
        assert!(error.is_some());
    }

    #[test]
    fn previous_session_classification_filters_invalid_tabs_but_keeps_trash() {
        let root = tempfile::tempdir().expect("create temp directory");
        let home = root.path().join("home");
        let valid_directory = root.path().join("valid");
        let missing_directory = root.path().join("missing");
        fs::create_dir_all(&home).expect("create home directory");
        fs::create_dir_all(&valid_directory).expect("create valid directory");
        let snapshot = BrowserSessionSnapshot {
            panes: vec![
                BrowserPaneSession {
                    id: BrowserPaneId::PRIMARY,
                    tabs: vec![
                        tab_session(1, valid_directory.clone(), false),
                        tab_session(2, missing_directory.clone(), false),
                    ],
                    active_tab_id: 2,
                    column_browser_viewport: ColumnBrowserViewport::default(),
                    column_viewports: HashMap::new(),
                },
                BrowserPaneSession {
                    id: BrowserPaneId(1),
                    tabs: vec![tab_session(3, missing_directory.clone(), true)],
                    active_tab_id: 3,
                    column_browser_viewport: ColumnBrowserViewport::default(),
                    column_viewports: HashMap::new(),
                },
                BrowserPaneSession {
                    id: BrowserPaneId(2),
                    tabs: vec![tab_session(4, missing_directory, false)],
                    active_tab_id: 4,
                    column_browser_viewport: ColumnBrowserViewport::default(),
                    column_viewports: HashMap::new(),
                },
            ],
            layout: BrowserPaneLayout::Single {
                active: BrowserPaneId::PRIMARY,
            },
        };

        let classified = classify_startup_session(
            startup_request(home, StartupSessionSource::PreviousSession),
            Some(snapshot),
        );
        let StartupSessionPlan::Session(session) = classified.plan else {
            panic!("previous session should resolve to a session plan");
        };

        assert_eq!(session.panes.len(), 2);
        assert_eq!(session.panes[0].tabs.len(), 1);
        assert_eq!(session.panes[0].tabs[0].directory, valid_directory);
        assert!(session.panes[1].tabs[0].is_trash_view);
    }
}
