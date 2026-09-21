use std::path::PathBuf;

use iced::Task;
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::commands::{
    apply_file_properties_permissions_to_enclosed_items_command, file_properties_command,
    set_file_properties_permissions_command, FilePropertiesPermissionTargets,
};
use crate::model::{
    FilePropertiesAggregateSnapshot, FilePropertiesCategory, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesLoadState, FilePropertiesMessage,
    FilePropertiesPermissionAccess, FilePropertiesPermissionClass, FilePropertiesPermissionUpdate,
    FilePropertiesPermissionWriteOutcome, FilePropertiesPermissions, FilePropertiesPresentation,
    FilePropertiesRequest, FilePropertiesState, FilePropertiesTargetSet, Message,
};

impl FileBrowser {
    pub(super) fn accept_file_properties_message(
        &mut self,
        message: FilePropertiesMessage,
    ) -> Task<Message> {
        match message {
            FilePropertiesMessage::Loaded(request, outcome) => {
                self.accept_file_properties(request, outcome)
            }
            FilePropertiesMessage::AggregateUpdated(request, snapshot) => {
                self.accept_file_properties_aggregate_progress(request, snapshot)
            }
            FilePropertiesMessage::DirectoryContentsUpdated(request, contents) => {
                self.accept_file_properties_directory_contents_progress(request, contents)
            }
            FilePropertiesMessage::DirectoryContentsLoaded(request, outcome) => {
                self.accept_file_properties_directory_contents(request, outcome)
            }
            FilePropertiesMessage::PermissionToggled(class, access) => {
                self.toggle_file_properties_permission(class, access)
            }
            FilePropertiesMessage::ApplyPermissionsToEnclosedItems => {
                self.apply_file_properties_permissions_to_enclosed_items()
            }
            FilePropertiesMessage::CategorySelected(category) => {
                self.select_file_properties_category(category)
            }
            FilePropertiesMessage::PermissionsUpdated(request, outcome) => {
                self.accept_file_properties_permissions(request, outcome)
            }
            FilePropertiesMessage::EnclosedPermissionsUpdated(request, outcome) => {
                self.accept_file_properties_enclosed_permissions(request, outcome)
            }
            FilePropertiesMessage::Requested(path) => self.open_file_properties(path),
        }
    }

    pub(super) fn open_selected_file_properties(&mut self) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        let paths = self.active_file_selection();
        let Ok(targets) = FilePropertiesTargetSet::new(paths) else {
            return Task::none();
        };
        self.open_file_properties_targets(targets)
    }

    pub(super) fn open_file_properties(&mut self, path: PathBuf) -> Task<Message> {
        self.open_file_properties_targets(FilePropertiesTargetSet::single(path))
    }

    pub(super) fn open_file_properties_targets(
        &mut self,
        targets: FilePropertiesTargetSet,
    ) -> Task<Message> {
        if self
            .properties
            .as_ref()
            .is_some_and(|properties| properties.targets == targets)
        {
            return self.ensure_properties_window();
        }
        self.context_menu = None;
        self.open_with = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();
        let (request, cancellation) = self.next_file_properties_request(targets);
        self.properties = Some(FilePropertiesState::loading(
            request.clone(),
            cancellation.clone(),
        ));

        Task::batch([
            self.commit_rename_if_active(),
            self.ensure_properties_window(),
            file_properties_command(request, cancellation),
        ])
    }

    pub(super) fn clear_file_properties_state(&mut self) {
        if let Some(properties) = self.properties.as_mut() {
            properties.cancel_load();
        }
        self.properties = None;
    }

    pub(super) fn next_file_properties_request(
        &mut self,
        targets: FilePropertiesTargetSet,
    ) -> (FilePropertiesRequest, CancellationToken) {
        if let Some(properties) = self.properties.as_mut() {
            properties.cancel_load();
        }
        self.properties_load_generation = self.properties_load_generation.wrapping_add(1);
        let cancellation = CancellationToken::new();
        (
            FilePropertiesRequest {
                targets,
                generation: self.properties_load_generation,
            },
            cancellation,
        )
    }

    pub(super) fn accept_file_properties(
        &mut self,
        request: FilePropertiesRequest,
        outcome: Result<FilePropertiesPresentation, String>,
    ) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if !file_properties_request_matches(properties, &request) {
            return Task::none();
        }

        let remains_loading_directory = outcome.as_ref().is_ok_and(|presentation| {
            matches!(
                presentation,
                FilePropertiesPresentation::Single(snapshot)
                    if matches!(
                        snapshot.directory_contents,
                        FilePropertiesDirectoryContentsState::Loading(_)
                    )
            )
        });
        if !remains_loading_directory {
            properties.load_cancel = None;
        }
        properties.load_state = match outcome {
            Ok(presentation) => FilePropertiesLoadState::Loaded(presentation),
            Err(error) => {
                properties.load_cancel = None;
                FilePropertiesLoadState::Failed(error)
            }
        };
        Task::none()
    }

    pub(super) fn accept_file_properties_aggregate_progress(
        &mut self,
        request: FilePropertiesRequest,
        snapshot: FilePropertiesAggregateSnapshot,
    ) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if file_properties_request_matches(properties, &request) {
            properties.load_state = FilePropertiesLoadState::LoadingAggregate(snapshot);
        }
        Task::none()
    }

    pub(super) fn accept_file_properties_directory_contents_progress(
        &mut self,
        request: FilePropertiesRequest,
        contents: FilePropertiesDirectoryContents,
    ) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if !file_properties_request_matches(properties, &request) {
            return Task::none();
        }
        let FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) =
            &mut properties.load_state
        else {
            return Task::none();
        };
        snapshot.size_bytes = contents.total_size_bytes;
        snapshot.disk_size_bytes = contents.total_disk_size_bytes;
        snapshot.directory_contents = FilePropertiesDirectoryContentsState::Loading(Some(contents));
        Task::none()
    }

    pub(super) fn accept_file_properties_directory_contents(
        &mut self,
        request: FilePropertiesRequest,
        outcome: Result<FilePropertiesDirectoryContents, String>,
    ) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if !file_properties_request_matches(properties, &request) {
            return Task::none();
        }
        let FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) =
            &mut properties.load_state
        else {
            return Task::none();
        };
        properties.load_cancel = None;
        match outcome {
            Ok(contents) => {
                snapshot.size_bytes = contents.total_size_bytes;
                snapshot.disk_size_bytes = contents.total_disk_size_bytes;
                snapshot.directory_contents =
                    FilePropertiesDirectoryContentsState::Loaded(contents);
            }
            Err(error) => {
                snapshot.directory_contents = FilePropertiesDirectoryContentsState::Failed(error);
            }
        }
        Task::none()
    }

    pub(super) fn select_file_properties_category(
        &mut self,
        category: FilePropertiesCategory,
    ) -> Task<Message> {
        if let Some(properties) = self.properties.as_mut() {
            properties.selected_category = category;
        }
        Task::none()
    }

    pub(super) fn toggle_file_properties_permission(
        &mut self,
        class: FilePropertiesPermissionClass,
        access: FilePropertiesPermissionAccess,
    ) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if properties.permission_update.is_in_progress() {
            return Task::none();
        }
        let FilePropertiesLoadState::Loaded(presentation) = &properties.load_state else {
            return Task::none();
        };
        let Some(current_permissions) = presentation.permissions() else {
            return Task::none();
        };
        let (write_targets, pending_update) = match presentation {
            FilePropertiesPresentation::Single(_) => (
                FilePropertiesPermissionTargets::Single(
                    properties
                        .targets
                        .single_path()
                        .expect("single presentation has one target")
                        .to_path_buf(),
                ),
                FilePropertiesPermissionUpdate::SavingCurrentItem {
                    permissions: current_permissions.toggled(class, access),
                },
            ),
            FilePropertiesPresentation::Aggregate(snapshot) => {
                if snapshot.permission_baselines.len() != snapshot.target_count {
                    return Task::none();
                }
                (
                    FilePropertiesPermissionTargets::TargetSet(
                        snapshot.permission_baselines.clone(),
                    ),
                    FilePropertiesPermissionUpdate::SavingTargetSet {
                        permissions: current_permissions.toggled(class, access),
                    },
                )
            }
        };
        let next_permissions = current_permissions.toggled(class, access);
        properties.permission_update = pending_update;
        set_file_properties_permissions_command(
            current_file_properties_request(properties),
            write_targets,
            next_permissions,
        )
    }

    pub(super) fn apply_file_properties_permissions_to_enclosed_items(&mut self) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if properties.permission_update.is_in_progress() {
            return Task::none();
        }
        let FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) =
            &properties.load_state
        else {
            return Task::none();
        };
        if snapshot.kind != file_core::FileKind::Directory {
            return Task::none();
        }
        let Some(current_permissions) = snapshot.permissions else {
            return Task::none();
        };

        properties.permission_update = FilePropertiesPermissionUpdate::ApplyingToEnclosedItems {
            permissions: current_permissions,
        };
        apply_file_properties_permissions_to_enclosed_items_command(
            current_file_properties_request(properties),
            current_permissions,
        )
    }

    pub(super) fn accept_file_properties_permissions(
        &mut self,
        request: FilePropertiesRequest,
        outcome: Result<FilePropertiesPermissionWriteOutcome, String>,
    ) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if !file_properties_request_matches(properties, &request) {
            return Task::none();
        }

        match outcome {
            Ok(FilePropertiesPermissionWriteOutcome::Single(permissions)) => {
                if let FilePropertiesLoadState::Loaded(presentation) = &mut properties.load_state {
                    presentation.set_permissions(permissions);
                }
                properties.permission_update = FilePropertiesPermissionUpdate::Idle;
                Task::none()
            }
            Ok(FilePropertiesPermissionWriteOutcome::Batch(batch)) => {
                let update = FilePropertiesPermissionUpdate::TargetSetCompleted {
                    succeeded_count: batch.succeeded_paths.len(),
                    failures: batch.failures,
                };
                let targets = properties.targets.clone();
                let selected_category = properties.selected_category;
                let (request, cancellation) = self.next_file_properties_request(targets);
                let mut reloading =
                    FilePropertiesState::loading(request.clone(), cancellation.clone());
                reloading.selected_category = selected_category;
                reloading.permission_update = update;
                self.properties = Some(reloading);
                file_properties_command(request, cancellation)
            }
            Err(error) => {
                properties.permission_update = FilePropertiesPermissionUpdate::Failed(error);
                Task::none()
            }
        }
    }

    pub(super) fn accept_file_properties_enclosed_permissions(
        &mut self,
        request: FilePropertiesRequest,
        outcome: Result<FilePropertiesPermissions, String>,
    ) -> Task<Message> {
        let Some(properties) = self.properties.as_mut() else {
            return Task::none();
        };
        if !file_properties_request_matches(properties, &request) {
            return Task::none();
        }

        match outcome {
            Ok(permissions) => {
                if let FilePropertiesLoadState::Loaded(presentation) = &mut properties.load_state {
                    presentation.set_permissions(permissions);
                }
                properties.permission_update = FilePropertiesPermissionUpdate::Idle;
            }
            Err(error) => {
                properties.permission_update = FilePropertiesPermissionUpdate::Failed(error);
            }
        }
        Task::none()
    }
}

fn file_properties_request_matches(
    properties: &FilePropertiesState,
    request: &FilePropertiesRequest,
) -> bool {
    properties.targets == request.targets && properties.load_generation == request.generation
}

fn current_file_properties_request(properties: &FilePropertiesState) -> FilePropertiesRequest {
    FilePropertiesRequest {
        targets: properties.targets.clone(),
        generation: properties.load_generation,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use file_core::FileKind;

    use super::*;
    use crate::model::{
        FilePropertiesPermissionWriteOutcome, FilePropertiesPresentation, FilePropertiesSnapshot,
        PermissionBatchOutcome, PermissionBatchPathFailure,
    };

    #[test]
    fn application_multi_selection_opens_the_shared_target_set_loader() {
        let first = PathBuf::from("/workspace/first");
        let second = PathBuf::from("/workspace/second");
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.entries = vec![
            file_core::DirectoryEntry::new(
                first.clone(),
                FileKind::File,
                file_core::EntryMetadata::default(),
                false,
                false,
                false,
            ),
            file_core::DirectoryEntry::new(
                second.clone(),
                FileKind::File,
                file_core::EntryMetadata::default(),
                false,
                false,
                false,
            ),
        ]
        .into();
        browser.selected = Some(first.clone());
        browser.selected_paths = std::collections::HashSet::from([first.clone(), second.clone()]);

        let _command = browser.open_selected_file_properties();

        let properties = browser.properties.as_ref().expect("properties opened");
        assert_eq!(properties.targets.paths().len(), 2);
        assert!(properties.targets.paths().contains(&first));
        assert!(properties.targets.paths().contains(&second));
        assert!(matches!(
            properties.load_state,
            FilePropertiesLoadState::Loading
        ));
    }

    #[test]
    fn aggregate_progress_remains_non_terminal_until_recursive_scan_finishes() {
        let targets = FilePropertiesTargetSet::new(vec![
            PathBuf::from("/workspace/first"),
            PathBuf::from("/workspace/second"),
        ])
        .expect("target set");
        let request = FilePropertiesRequest {
            targets: targets.clone(),
            generation: 3,
        };
        let progress = aggregate_snapshot(2);
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.properties = Some(FilePropertiesState::loading(
            request.clone(),
            CancellationToken::new(),
        ));

        let _progress =
            browser.accept_file_properties_aggregate_progress(request.clone(), progress.clone());
        let _toggle = browser.toggle_file_properties_permission(
            FilePropertiesPermissionClass::Owner,
            FilePropertiesPermissionAccess::Write,
        );

        let properties = browser.properties.as_ref().expect("properties remain open");
        assert!(matches!(
            properties.load_state,
            FilePropertiesLoadState::LoadingAggregate(_)
        ));
        assert_eq!(
            properties.permission_update,
            FilePropertiesPermissionUpdate::Idle
        );

        let _completed = browser
            .accept_file_properties(request, Ok(FilePropertiesPresentation::Aggregate(progress)));
        assert!(matches!(
            browser.properties.as_ref().unwrap().load_state,
            FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Aggregate(_))
        ));
        assert!(browser.properties.as_ref().unwrap().load_cancel.is_none());
    }

    #[test]
    fn properties_directory_contents_rejects_stale_generation_for_same_targets() {
        let path = PathBuf::from("/workspace/project");
        let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
        let stale_request = properties_request(path, 1);

        let _command = browser.accept_file_properties_directory_contents(
            stale_request,
            Ok(directory_contents(9, 9, 9)),
        );

        let snapshot = loaded_single_snapshot(&browser);
        assert!(matches!(
            snapshot.directory_contents,
            FilePropertiesDirectoryContentsState::Loading(None)
        ));
        assert_eq!(snapshot.size_bytes, 0);
    }

    #[test]
    fn properties_directory_contents_accepts_current_generation() {
        let path = PathBuf::from("/workspace/project");
        let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
        let request = properties_request(path, 2);

        let _command = browser
            .accept_file_properties_directory_contents(request, Ok(directory_contents(3, 2, 512)));

        let snapshot = loaded_single_snapshot(&browser);
        assert_eq!(snapshot.size_bytes, 512);
        assert!(matches!(
            snapshot.directory_contents,
            FilePropertiesDirectoryContentsState::Loaded(FilePropertiesDirectoryContents {
                file_count: 3,
                directory_count: 2,
                ..
            })
        ));
        assert!(browser.properties.as_ref().unwrap().load_cancel.is_none());
    }

    #[test]
    fn current_item_permission_save_updates_snapshot_permissions() {
        let path = PathBuf::from("/workspace/project");
        let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
        browser.properties.as_mut().unwrap().permission_update =
            FilePropertiesPermissionUpdate::SavingCurrentItem {
                permissions: FilePropertiesPermissions::from_mode(0o755),
            };

        let _command = browser.accept_file_properties_permissions(
            properties_request(path, 2),
            Ok(FilePropertiesPermissionWriteOutcome::Single(
                FilePropertiesPermissions::from_mode(0o700),
            )),
        );

        assert_eq!(
            loaded_single_snapshot(&browser).permissions,
            Some(FilePropertiesPermissions::from_mode(0o700))
        );
        assert_eq!(
            browser.properties.as_ref().unwrap().permission_update,
            FilePropertiesPermissionUpdate::Idle
        );
    }

    #[test]
    fn target_set_permission_outcome_reloads_and_preserves_partial_fact() {
        let first = PathBuf::from("/workspace/first");
        let second = PathBuf::from("/workspace/second");
        let targets =
            FilePropertiesTargetSet::new(vec![first.clone(), second.clone()]).expect("target set");
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.properties = Some(FilePropertiesState::loading(
            FilePropertiesRequest {
                targets: targets.clone(),
                generation: 4,
            },
            CancellationToken::new(),
        ));
        browser.properties_load_generation = 4;
        browser.properties.as_mut().unwrap().selected_category =
            FilePropertiesCategory::Permissions;

        let _command = browser.accept_file_properties_permissions(
            FilePropertiesRequest {
                targets,
                generation: 4,
            },
            Ok(FilePropertiesPermissionWriteOutcome::Batch(
                PermissionBatchOutcome {
                    succeeded_paths: vec![first],
                    failures: vec![PermissionBatchPathFailure {
                        path: second,
                        error: "identity changed".to_owned(),
                    }],
                },
            )),
        );

        let properties = browser.properties.as_ref().expect("properties remain open");
        assert_eq!(properties.load_generation, 5);
        assert!(matches!(
            properties.load_state,
            FilePropertiesLoadState::Loading
        ));
        assert_eq!(
            properties.selected_category,
            FilePropertiesCategory::Permissions
        );
        assert!(matches!(
            properties.permission_update,
            FilePropertiesPermissionUpdate::TargetSetCompleted {
                succeeded_count: 1,
                ref failures,
            } if failures.len() == 1
        ));
    }

    #[test]
    fn enclosed_permission_result_rejects_stale_generation_for_same_targets() {
        let path = PathBuf::from("/workspace/project");
        let mut browser = browser_with_loaded_directory_properties(path.clone(), 2);
        browser.properties.as_mut().unwrap().permission_update =
            FilePropertiesPermissionUpdate::ApplyingToEnclosedItems {
                permissions: FilePropertiesPermissions::from_mode(0o755),
            };

        let _command = browser.accept_file_properties_enclosed_permissions(
            properties_request(path, 1),
            Ok(FilePropertiesPermissions::from_mode(0o700)),
        );

        assert_eq!(
            loaded_single_snapshot(&browser).permissions,
            Some(FilePropertiesPermissions::from_mode(0o755))
        );
        assert!(matches!(
            browser.properties.as_ref().unwrap().permission_update,
            FilePropertiesPermissionUpdate::ApplyingToEnclosedItems { .. }
        ));
    }

    #[test]
    fn permission_actions_are_ignored_while_enclosed_permissions_are_applying() {
        let path = PathBuf::from("/workspace/project");
        let mut browser = browser_with_loaded_directory_properties(path, 2);
        browser.properties.as_mut().unwrap().permission_update =
            FilePropertiesPermissionUpdate::ApplyingToEnclosedItems {
                permissions: FilePropertiesPermissions::from_mode(0o755),
            };

        let _toggle = browser.toggle_file_properties_permission(
            FilePropertiesPermissionClass::Owner,
            FilePropertiesPermissionAccess::Write,
        );
        let _enclosed = browser.apply_file_properties_permissions_to_enclosed_items();

        assert!(matches!(
            browser.properties.as_ref().unwrap().permission_update,
            FilePropertiesPermissionUpdate::ApplyingToEnclosedItems { .. }
        ));
    }

    #[test]
    fn opening_new_properties_cancels_previous_load() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let old_cancel = CancellationToken::new();
        browser.properties = Some(FilePropertiesState::loading(
            properties_request(PathBuf::from("/workspace/old"), 1),
            old_cancel.clone(),
        ));
        browser.properties_load_generation = 1;

        let (_request, _cancel) = browser.next_file_properties_request(
            FilePropertiesTargetSet::single(PathBuf::from("/workspace/new")),
        );

        assert!(old_cancel.is_cancelled());
        assert_eq!(browser.properties_load_generation, 2);
    }

    fn browser_with_loaded_directory_properties(path: PathBuf, generation: u64) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        browser.properties = Some(FilePropertiesState {
            targets: FilePropertiesTargetSet::single(path.clone()),
            load_state: FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(
                directory_snapshot(path),
            )),
            selected_category: FilePropertiesCategory::Information,
            permission_update: FilePropertiesPermissionUpdate::Idle,
            load_generation: generation,
            load_cancel: Some(CancellationToken::new()),
        });
        browser
    }

    fn properties_request(path: PathBuf, generation: u64) -> FilePropertiesRequest {
        FilePropertiesRequest {
            targets: FilePropertiesTargetSet::single(path),
            generation,
        }
    }

    fn loaded_single_snapshot(browser: &FileBrowser) -> &FilePropertiesSnapshot {
        let FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) =
            &browser.properties.as_ref().unwrap().load_state
        else {
            panic!("single properties should remain loaded");
        };
        snapshot
    }

    fn directory_snapshot(path: PathBuf) -> FilePropertiesSnapshot {
        FilePropertiesSnapshot {
            name: OsString::from("project"),
            kind: FileKind::Directory,
            type_label: "Folder".to_owned(),
            location: path.parent().unwrap().to_path_buf(),
            created: None,
            modified: None,
            accessed: None,
            size_bytes: 0,
            disk_size_bytes: 0,
            directory_contents: FilePropertiesDirectoryContentsState::Loading(None),
            permissions: Some(FilePropertiesPermissions::from_mode(0o755)),
        }
    }

    fn aggregate_snapshot(target_count: usize) -> FilePropertiesAggregateSnapshot {
        FilePropertiesAggregateSnapshot {
            target_count,
            file_count: target_count,
            directory_count: 0,
            symlink_count: 0,
            other_count: 0,
            total_size_bytes: 0,
            total_disk_size_bytes: 0,
            recursive_contents: directory_contents(0, 0, 0),
            common_parent: Some(PathBuf::from("/workspace")),
            common_kind: Some(FileKind::File),
            common_created: None,
            common_modified: None,
            common_accessed: None,
            permissions: Some(FilePropertiesPermissions::from_mode(0o644)),
            permission_baselines: Vec::new(),
        }
    }

    fn directory_contents(
        file_count: usize,
        directory_count: usize,
        total_size_bytes: u64,
    ) -> FilePropertiesDirectoryContents {
        FilePropertiesDirectoryContents {
            file_count,
            directory_count,
            total_size_bytes,
            total_disk_size_bytes: total_size_bytes,
        }
    }
}
