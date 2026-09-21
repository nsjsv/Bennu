use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::commands::{
    configure_search_paths_command, search_path_configuration_command,
    search_path_directory_chooser_command,
};
use crate::model::{
    Message, SearchPathConfigureRequest, SearchPathEntryKind, SearchServiceDiagnostic,
};

impl FileBrowser {
    pub(super) fn refresh_search_path_configuration(&self) -> Task<Message> {
        search_path_configuration_command()
    }

    pub(super) fn accept_search_path_configuration(
        &mut self,
        outcome: Result<
            (
                file_search::VersionedSearchPathPreferences,
                file_search::SearchPathConfigurationStatus,
            ),
            SearchServiceDiagnostic,
        >,
    ) -> Task<Message> {
        match outcome {
            Ok((snapshot, status)) => {
                let stale = self
                    .search_service
                    .path_settings
                    .is_stale_revision(snapshot.revision);
                let request = self.search_service.path_settings.accept_snapshot(snapshot);
                if !stale {
                    self.update_search_path_configuration_status(status);
                }
                configure_search_path_request(request)
            }
            Err(diagnostic) => {
                self.search_service
                    .path_settings
                    .accept_refresh_failure(diagnostic.technical_detail);
                Task::none()
            }
        }
    }

    pub(super) fn accept_search_path_configuration_applied(
        &mut self,
        outcome: Result<
            (
                file_search::VersionedSearchPathPreferences,
                file_search::SearchPathConfigurationStatus,
            ),
            SearchServiceDiagnostic,
        >,
    ) -> Task<Message> {
        match outcome {
            Ok((snapshot, status)) => {
                let stale = self
                    .search_service
                    .path_settings
                    .is_stale_revision(snapshot.revision);
                let request = self.search_service.path_settings.accept_applied(snapshot);
                if !stale {
                    self.update_search_path_configuration_status(status);
                }
                let next_configuration = configure_search_path_request(request);
                Task::batch([next_configuration, self.restart_search_workspace()])
            }
            Err(diagnostic) => {
                self.search_service
                    .path_settings
                    .accept_apply_failure(diagnostic.technical_detail);
                Task::none()
            }
        }
    }

    pub(super) fn update_search_path_input(
        &mut self,
        kind: SearchPathEntryKind,
        value: String,
    ) -> Task<Message> {
        match kind {
            SearchPathEntryKind::CustomRoot => {
                self.search_service.path_settings.custom_root_input = value
            }
            SearchPathEntryKind::Exclusion => {
                self.search_service.path_settings.exclusion_input = value
            }
        }
        Task::none()
    }

    pub(super) fn commit_search_path_input(&mut self, kind: SearchPathEntryKind) -> Task<Message> {
        let path = match kind {
            SearchPathEntryKind::CustomRoot => {
                PathBuf::from(self.search_service.path_settings.custom_root_input.clone())
            }
            SearchPathEntryKind::Exclusion => {
                PathBuf::from(self.search_service.path_settings.exclusion_input.clone())
            }
        };
        if path.as_os_str().is_empty() {
            return Task::none();
        }
        self.queue_search_path(kind, path)
    }

    pub(super) fn open_search_path_directory_chooser(
        &mut self,
        kind: SearchPathEntryKind,
    ) -> Task<Message> {
        if self.search_service.path_settings.picker_in_flight.is_some() {
            return Task::none();
        }
        self.search_service.path_settings.picker_in_flight = Some(kind);
        if self
            .search_service
            .path_settings
            .picker_failure
            .as_ref()
            .is_some_and(|(failed_kind, _)| *failed_kind == kind)
        {
            self.search_service.path_settings.picker_failure = None;
        }
        search_path_directory_chooser_command(kind)
    }

    pub(super) fn accept_search_path_directory(
        &mut self,
        kind: SearchPathEntryKind,
        outcome: Result<Option<PathBuf>, String>,
    ) -> Task<Message> {
        if self.search_service.path_settings.picker_in_flight != Some(kind) {
            return Task::none();
        }
        self.search_service.path_settings.picker_in_flight = None;
        self.search_service.path_settings.picker_failure = None;
        match outcome {
            Ok(Some(path)) => self.queue_search_path(kind, path),
            Ok(None) => Task::none(),
            Err(message) => {
                self.search_service.path_settings.picker_failure = Some((kind, message));
                Task::none()
            }
        }
    }

    pub(super) fn remove_search_path(
        &mut self,
        kind: SearchPathEntryKind,
        path: PathBuf,
    ) -> Task<Message> {
        let mut preferences = self.search_service.path_settings.draft.clone();
        match kind {
            SearchPathEntryKind::CustomRoot => preferences
                .custom_roots
                .retain(|candidate| candidate != &path),
            SearchPathEntryKind::Exclusion => preferences
                .exclusions
                .retain(|candidate| candidate != &path),
        }
        self.queue_search_path_preferences(preferences)
    }

    pub(super) fn retry_search_path_configuration(&mut self) -> Task<Message> {
        let status_failed = self
            .search_service
            .confirmed_status
            .as_ref()
            .and_then(|service| service.index_status.as_ref())
            .is_some_and(|index| {
                matches!(
                    index.path_configuration.phase,
                    file_search::SearchPathConfigurationPhase::Failed { .. }
                )
            });
        if self.search_service.path_settings.apply_in_flight
            || (self.search_service.path_settings.failure.is_none() && !status_failed)
            || !self
                .search_service
                .path_settings
                .request_retry_after_refresh()
        {
            return Task::none();
        }
        search_path_configuration_command()
    }

    fn queue_search_path(&mut self, kind: SearchPathEntryKind, path: PathBuf) -> Task<Message> {
        if !path.is_absolute() {
            self.search_service.path_settings.failure =
                Some("Search locations must be absolute paths".to_owned());
            return Task::none();
        }
        let mut preferences = self.search_service.path_settings.draft.clone();
        let paths = match kind {
            SearchPathEntryKind::CustomRoot => &mut preferences.custom_roots,
            SearchPathEntryKind::Exclusion => &mut preferences.exclusions,
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
        if self
            .search_service
            .path_settings
            .picker_failure
            .as_ref()
            .is_some_and(|(failed_kind, _)| *failed_kind == kind)
        {
            self.search_service.path_settings.picker_failure = None;
        }
        match kind {
            SearchPathEntryKind::CustomRoot => {
                self.search_service.path_settings.custom_root_input.clear()
            }
            SearchPathEntryKind::Exclusion => {
                self.search_service.path_settings.exclusion_input.clear()
            }
        }
        self.queue_search_path_preferences(preferences)
    }

    fn queue_search_path_preferences(
        &mut self,
        preferences: file_search::SearchPathPreferences,
    ) -> Task<Message> {
        configure_search_path_request(self.search_service.path_settings.queue(preferences))
    }

    pub(super) fn update_search_path_configuration_status(
        &mut self,
        status: file_search::SearchPathConfigurationStatus,
    ) {
        if let Some(index_status) = self
            .search_service
            .confirmed_status
            .as_mut()
            .and_then(|service| service.index_status.as_mut())
        {
            index_status.path_configuration = status;
        }
    }
}

fn configure_search_path_request(request: Option<SearchPathConfigureRequest>) -> Task<Message> {
    request
        .map(configure_search_paths_command)
        .unwrap_or_else(Task::none)
}
