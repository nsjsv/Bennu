use std::path::PathBuf;

use iced::Task;
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::commands::{
    apply_file_properties_permissions_to_enclosed_items_command, file_properties_command,
    set_file_properties_permissions_command, FilePropertiesPermissionTargets,
};
use crate::model::{
    FilePropertiesAggregateSnapshot, FilePropertiesDirectoryContents,
    FilePropertiesDirectoryContentsState, FilePropertiesKindDetails, FilePropertiesLoadState,
    FilePropertiesMessage, FilePropertiesPermissionAccess, FilePropertiesPermissionClass,
    FilePropertiesPermissionUpdate, FilePropertiesPermissionWriteOutcome,
    FilePropertiesPermissions, FilePropertiesPresentation, FilePropertiesRequest,
    FilePropertiesSnapshot, FilePropertiesState, FilePropertiesTargetSet, Message,
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
            FilePropertiesMessage::DefaultApplicationLoaded(request, outcome) => {
                if let Some(FilePropertiesKindDetails::File(file)) =
                    self.current_file_properties_details(&request)
                {
                    file.default_application = outcome.into();
                }
                Task::none()
            }
            FilePropertiesMessage::MediaDetailsLoaded(request, outcome) => {
                if let Some(FilePropertiesKindDetails::File(file)) =
                    self.current_file_properties_details(&request)
                {
                    file.media_details = Some(outcome.into());
                }
                Task::none()
            }
            FilePropertiesMessage::PermissionToggled(class, access) => {
                self.toggle_file_properties_permission(class, access)
            }
            FilePropertiesMessage::ApplyPermissionsToEnclosedItems => {
                self.apply_file_properties_permissions_to_enclosed_items()
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

        properties.load_state = match outcome {
            Ok(presentation) => FilePropertiesLoadState::Loaded(presentation),
            Err(error) => FilePropertiesLoadState::Failed(error),
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
        self.update_file_properties_directory_contents(
            &request,
            FilePropertiesDirectoryContentsState::Loading(Some(contents)),
        );
        Task::none()
    }

    pub(super) fn accept_file_properties_directory_contents(
        &mut self,
        request: FilePropertiesRequest,
        outcome: Result<FilePropertiesDirectoryContents, String>,
    ) -> Task<Message> {
        self.update_file_properties_directory_contents(
            &request,
            match outcome {
                Ok(contents) => FilePropertiesDirectoryContentsState::Loaded(contents),
                Err(error) => FilePropertiesDirectoryContentsState::Failed(error),
            },
        );
        Task::none()
    }

    /// 文件夹的大小就是量到的里面的总大小。
    fn update_file_properties_directory_contents(
        &mut self,
        request: &FilePropertiesRequest,
        state: FilePropertiesDirectoryContentsState,
    ) {
        let Some(snapshot) = self.current_single_file_properties(request) else {
            return;
        };
        let FilePropertiesKindDetails::Directory(current) = &mut snapshot.details else {
            return;
        };
        if let FilePropertiesDirectoryContentsState::Loading(Some(contents))
        | FilePropertiesDirectoryContentsState::Loaded(contents) = &state
        {
            snapshot.size_bytes = contents.total_size_bytes;
            snapshot.disk_size_bytes = contents.total_disk_size_bytes;
        }
        *current = state;
    }

    /// 这次请求的单目标快照；旧请求、多选、还没读完的是 None，后到的结果就丢掉。
    fn current_single_file_properties(
        &mut self,
        request: &FilePropertiesRequest,
    ) -> Option<&mut FilePropertiesSnapshot> {
        let properties = self.properties.as_mut()?;
        if !file_properties_request_matches(properties, request) {
            return None;
        }
        match &mut properties.load_state {
            FilePropertiesLoadState::Loaded(FilePropertiesPresentation::Single(snapshot)) => {
                Some(snapshot)
            }
            _ => None,
        }
    }

    fn current_file_properties_details(
        &mut self,
        request: &FilePropertiesRequest,
    ) -> Option<&mut FilePropertiesKindDetails> {
        self.current_single_file_properties(request)
            .map(|snapshot| &mut snapshot.details)
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
        if snapshot.details.kind() != file_core::FileKind::Directory {
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
                let (request, cancellation) = self.next_file_properties_request(targets);
                let mut reloading =
                    FilePropertiesState::loading(request.clone(), cancellation.clone());
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
#[path = "properties_tests.rs"]
mod tests;
