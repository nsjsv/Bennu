use desktop_linux::{
    NetworkConnection, NetworkConnectionId, NetworkMountCredentials, NetworkProtocol,
};
use iced::Task;

use super::FileBrowser;
use crate::commands::{
    network_connection_credentials_clear_command, network_connection_credentials_lookup_command,
    network_connection_credentials_store_command, network_connection_mount_command,
    network_connection_unmount_command, network_mount_states_command,
};
use crate::model::{ContextMenuState, Message, NavigationMode};
use crate::network_connections::{
    NetworkConnectionCredentialFallback, NetworkConnectionEditorMode, NetworkConnectionEditorState,
    NetworkConnectionMessage, NetworkConnectionMountCompletion, SavedNetworkConnection,
    SidebarNetworkConnectionAction, SidebarNetworkConnectionContextMenuState,
};

impl FileBrowser {
    pub(crate) fn network_connection_is_selected(&self, id: &NetworkConnectionId) -> bool {
        self.network_connections
            .selected_connection_id(&self.current_dir)
            .is_some_and(|selected_id| selected_id == id)
    }

    pub(super) fn refresh_network_mount_states(&mut self) -> Task<Message> {
        let connections = self.network_connections.connections();
        if connections.is_empty() {
            Task::none()
        } else {
            network_mount_states_command(connections)
        }
    }

    pub(super) fn startup_auto_connect_network_connections(&mut self) -> Task<Message> {
        let connections = self
            .user_config
            .network_connections
            .iter()
            .filter(|saved| saved.auto_connect)
            .map(|saved| saved.connection.clone())
            .collect::<Vec<_>>();
        if connections.is_empty() {
            return Task::none();
        }

        let mut commands = Vec::new();
        for connection in connections {
            commands.push(self.start_network_connection_mount_after_credential_lookup(
                connection,
                NetworkConnectionMountCompletion::RefreshOnly,
                NetworkConnectionCredentialFallback::MountWithoutCredentials,
            ));
        }
        Task::batch(commands)
    }

    pub(super) fn handle_network_connection_message(
        &mut self,
        message: NetworkConnectionMessage,
    ) -> Task<Message> {
        match message {
            NetworkConnectionMessage::AddRequested => self.open_network_connection_editor(),
            NetworkConnectionMessage::StatusRefreshRequested => self.refresh_network_mount_states(),
            NetworkConnectionMessage::MountStatesLoaded(Ok(states)) => {
                self.network_connections.accept_loaded(states);
                Task::none()
            }
            NetworkConnectionMessage::MountStatesLoaded(Err(error)) => {
                self.network_connections.accept_unavailable(error);
                Task::none()
            }
            NetworkConnectionMessage::Hovered(id) => {
                self.hovered_network_connection = Some(id);
                Task::none()
            }
            NetworkConnectionMessage::HoverCleared(id) => {
                if self.hovered_network_connection.as_ref() == Some(&id) {
                    self.hovered_network_connection = None;
                }
                Task::none()
            }
            NetworkConnectionMessage::Pressed(id) => self.press_network_connection(id),
            NetworkConnectionMessage::MiddlePressed(pane_id, id) => {
                self.activate_pane(pane_id);
                let rename_command = self.commit_rename_if_active();
                let Some(path) = self.network_connections.mounted_path_for(&id) else {
                    return rename_command;
                };
                Task::batch([rename_command, self.open_directory_from_middle_click(path)])
            }
            NetworkConnectionMessage::RightClicked(id) => self.right_click_network_connection(id),
            NetworkConnectionMessage::ActionSelected(id, action) => {
                self.perform_network_connection_action(id, action)
            }
            NetworkConnectionMessage::StoredCredentialsLoaded(
                connection,
                completion,
                fallback,
                result,
            ) => self.accept_network_connection_credentials_loaded(
                connection, completion, fallback, result,
            ),
            NetworkConnectionMessage::StoredCredentialsSaved(id, result) => {
                self.accept_network_connection_credentials_saved(id, result)
            }
            NetworkConnectionMessage::StoredCredentialsCleared(id, result) => {
                self.accept_network_connection_credentials_cleared(id, result)
            }
            NetworkConnectionMessage::MountFinished(connection, completion, result) => {
                self.accept_network_connection_mounted(connection, completion, result)
            }
            NetworkConnectionMessage::UnmountFinished(connection, result) => {
                self.accept_network_connection_unmounted(connection, result)
            }
            NetworkConnectionMessage::EditorProtocolSelected(protocol) => {
                self.select_network_connection_editor_protocol(protocol)
            }
            NetworkConnectionMessage::EditorLabelChanged(label) => {
                if let Some(editor) = &mut self.network_connection_editor {
                    editor.label = label;
                    editor.error = None;
                }
                Task::none()
            }
            NetworkConnectionMessage::EditorUriChanged(uri) => {
                if let Some(editor) = &mut self.network_connection_editor {
                    editor.uri = uri;
                    editor.error = None;
                }
                Task::none()
            }
            NetworkConnectionMessage::EditorUsernameChanged(username) => {
                if let Some(editor) = &mut self.network_connection_editor {
                    editor.username = username;
                    editor.error = None;
                }
                Task::none()
            }
            NetworkConnectionMessage::EditorPasswordChanged(password) => {
                if let Some(editor) = &mut self.network_connection_editor {
                    editor.password = password;
                    editor.error = None;
                }
                Task::none()
            }
            NetworkConnectionMessage::EditorAutoConnectToggled(auto_connect) => {
                if let Some(editor) = &mut self.network_connection_editor {
                    editor.auto_connect = auto_connect;
                    editor.error = None;
                }
                Task::none()
            }
            NetworkConnectionMessage::EditorSaved => self.save_network_connection_editor(),
            NetworkConnectionMessage::EditorCanceled => {
                self.network_connection_editor = None;
                Task::none()
            }
        }
    }

    fn press_network_connection(&mut self, id: NetworkConnectionId) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        self.context_menu = None;
        if let Some(path) = self.network_connections.mounted_path_for(&id) {
            return Task::batch([
                rename_command,
                self.navigate_to(path, NavigationMode::RecordHistory),
            ]);
        }
        let Some(connection) = self.network_connections.connection(&id).cloned() else {
            return rename_command;
        };
        let credentials = self.network_connections.remembered_credentials(&id);
        if credentials.is_none() && network_connection_needs_credentials_editor(&connection) {
            return Task::batch([
                rename_command,
                self.start_network_connection_mount_after_credential_lookup(
                    connection,
                    NetworkConnectionMountCompletion::NavigateToMount,
                    NetworkConnectionCredentialFallback::OpenEditor,
                ),
            ]);
        }
        Task::batch([
            rename_command,
            self.start_network_connection_mount(
                connection,
                credentials,
                NetworkConnectionMountCompletion::NavigateToMount,
            ),
        ])
    }

    fn right_click_network_connection(&mut self, id: NetworkConnectionId) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        let Some(connection) = self.network_connections.entry(&id).cloned() else {
            return rename_command;
        };
        if self
            .user_config
            .context_menus
            .network_connection_items(connection.available_actions())
            .is_empty()
        {
            return rename_command;
        }

        self.clear_preview();
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.selection_marquee = None;
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        let _ = self.cancel_address_editing();
        self.context_menu = Some(ContextMenuState::NetworkConnection(
            SidebarNetworkConnectionContextMenuState {
                connection,
                position: self.cursor_position,
            },
        ));
        rename_command
    }

    fn perform_network_connection_action(
        &mut self,
        id: NetworkConnectionId,
        action: SidebarNetworkConnectionAction,
    ) -> Task<Message> {
        self.context_menu = None;
        match action {
            SidebarNetworkConnectionAction::Connect => {
                let Some(connection) = self.network_connections.connection(&id).cloned() else {
                    return Task::none();
                };
                let credentials = self.network_connections.remembered_credentials(&id);
                if credentials.is_none() && network_connection_needs_credentials_editor(&connection)
                {
                    return self.start_network_connection_mount_after_credential_lookup(
                        connection,
                        NetworkConnectionMountCompletion::NavigateToMount,
                        NetworkConnectionCredentialFallback::OpenEditor,
                    );
                }
                self.start_network_connection_mount(
                    connection,
                    credentials,
                    NetworkConnectionMountCompletion::NavigateToMount,
                )
            }
            SidebarNetworkConnectionAction::Disconnect => {
                let Some(connection) = self.network_connections.connection(&id).cloned() else {
                    return Task::none();
                };
                self.network_connections.set_disconnecting(&id);
                network_connection_unmount_command(connection)
            }
            SidebarNetworkConnectionAction::Edit => self.edit_network_connection(id),
            SidebarNetworkConnectionAction::Remove => {
                let clear_command = self
                    .network_connections
                    .connection(&id)
                    .cloned()
                    .map(network_connection_credentials_clear_command)
                    .unwrap_or_else(Task::none);
                self.network_connections.remove(&id);
                self.user_config.network_connections = self.network_connections.saved_connections();
                Task::batch([self.persist_user_preferences_command(), clear_command])
            }
        }
    }

    fn accept_network_connection_credentials_loaded(
        &mut self,
        expected_connection: NetworkConnection,
        completion: NetworkConnectionMountCompletion,
        fallback: NetworkConnectionCredentialFallback,
        result: Result<Option<NetworkMountCredentials>, String>,
    ) -> Task<Message> {
        let id = expected_connection.id.clone();
        if !self
            .network_connections
            .remote_identity_matches(&expected_connection)
        {
            return Task::none();
        }
        let Some(connection) = self.network_connections.connection(&id).cloned() else {
            self.network_connections.pending_actions.remove(&id);
            return Task::none();
        };
        match result {
            Ok(Some(credentials)) => {
                self.start_network_connection_mount(connection, Some(credentials), completion)
            }
            Ok(None) => self.handle_missing_network_connection_credentials(
                connection, completion, fallback, None,
            ),
            Err(error) => self.handle_missing_network_connection_credentials(
                connection,
                completion,
                fallback,
                Some(format!("Could not load saved network password: {error}")),
            ),
        }
    }

    fn handle_missing_network_connection_credentials(
        &mut self,
        connection: NetworkConnection,
        completion: NetworkConnectionMountCompletion,
        fallback: NetworkConnectionCredentialFallback,
        error: Option<String>,
    ) -> Task<Message> {
        if let Some(error) = error {
            self.show_global_error(error);
        }
        match fallback {
            NetworkConnectionCredentialFallback::OpenEditor => {
                self.network_connections
                    .pending_actions
                    .remove(&connection.id);
                self.open_network_connection_credentials_editor(&connection);
                Task::none()
            }
            NetworkConnectionCredentialFallback::MountWithoutCredentials => {
                self.start_network_connection_mount(connection, None, completion)
            }
        }
    }

    fn accept_network_connection_credentials_saved(
        &mut self,
        _id: NetworkConnectionId,
        result: Result<(), String>,
    ) -> Task<Message> {
        if let Err(error) = result {
            self.show_global_error(format!(
                "Connected, but could not save network password: {error}"
            ));
        }
        Task::none()
    }

    fn accept_network_connection_credentials_cleared(
        &mut self,
        _id: NetworkConnectionId,
        result: Result<(), String>,
    ) -> Task<Message> {
        if let Err(error) = result {
            self.show_global_error(format!("Could not remove saved network password: {error}"));
        }
        Task::none()
    }

    fn accept_network_connection_mounted(
        &mut self,
        expected_connection: NetworkConnection,
        completion: NetworkConnectionMountCompletion,
        result: Result<desktop_linux::MountedNetworkConnection, String>,
    ) -> Task<Message> {
        let id = expected_connection.id.clone();
        if !self
            .network_connections
            .remote_identity_matches(&expected_connection)
        {
            return Task::none();
        }
        match result {
            Ok(mounted) => {
                let connection = mounted.connection.clone();
                let Some(path) = self.network_connections.accept_mounted(mounted) else {
                    return Task::none();
                };
                let store_command = self
                    .network_connections
                    .remembered_credentials(&id)
                    .map(|credentials| {
                        network_connection_credentials_store_command(connection, credentials)
                    })
                    .unwrap_or_else(Task::none);
                match completion {
                    NetworkConnectionMountCompletion::NavigateToMount => Task::batch([
                        self.navigate_to(path, NavigationMode::RecordHistory),
                        self.refresh_network_mount_states(),
                        store_command,
                    ]),
                    NetworkConnectionMountCompletion::RefreshOnly => {
                        Task::batch([self.refresh_network_mount_states(), store_command])
                    }
                }
            }
            Err(error) => {
                let clear_command = if self
                    .network_connections
                    .remembered_credentials(&id)
                    .is_some()
                {
                    self.network_connections
                        .connection(&id)
                        .cloned()
                        .map(network_connection_credentials_clear_command)
                        .unwrap_or_else(Task::none)
                } else {
                    Task::none()
                };
                if !self
                    .network_connections
                    .accept_mount_error(&expected_connection, error.clone())
                {
                    return Task::none();
                }
                self.show_global_error(format!("Could not connect network location: {error}"));
                clear_command
            }
        }
    }

    fn accept_network_connection_unmounted(
        &mut self,
        expected_connection: NetworkConnection,
        result: Result<(), String>,
    ) -> Task<Message> {
        if !self
            .network_connections
            .remote_identity_matches(&expected_connection)
        {
            return Task::none();
        }
        match result {
            Ok(()) => {
                if !self
                    .network_connections
                    .accept_unmounted(&expected_connection)
                {
                    return Task::none();
                }
                self.refresh_network_mount_states()
            }
            Err(error) => {
                let id = expected_connection.id;
                self.network_connections.pending_actions.remove(&id);
                self.show_global_error(format!("Could not disconnect network location: {error}"));
                Task::none()
            }
        }
    }

    fn open_network_connection_editor(&mut self) -> Task<Message> {
        let command = self.prepare_network_connection_editor();
        self.network_connection_editor = Some(NetworkConnectionEditorState::add());
        command
    }

    fn edit_network_connection(&mut self, id: NetworkConnectionId) -> Task<Message> {
        let command = self.prepare_network_connection_editor();
        let Some(entry) = self.network_connections.entry(&id) else {
            return command;
        };
        self.network_connection_editor = Some(NetworkConnectionEditorState::edit(entry));
        command
    }

    fn open_network_connection_credentials_editor(&mut self, connection: &NetworkConnection) {
        self.prepare_network_connection_editor_surface();
        self.network_connection_editor = Some(NetworkConnectionEditorState::connect(connection));
    }

    fn prepare_network_connection_editor(&mut self) -> Task<Message> {
        let rename_command = self.commit_rename_if_active();
        self.prepare_network_connection_editor_surface();
        rename_command
    }

    fn prepare_network_connection_editor_surface(&mut self) {
        self.context_menu = None;
        self.open_with = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.operation_queue.close_panel();
        let _ = self.cancel_address_editing();
        self.shortcut_capture = None;
        self.clear_pointer_driven_interaction_state();
    }

    fn select_network_connection_editor_protocol(
        &mut self,
        protocol: NetworkProtocol,
    ) -> Task<Message> {
        if let Some(editor) = &mut self.network_connection_editor {
            editor.protocol = protocol;
            editor.error = None;
        }
        Task::none()
    }

    fn start_network_connection_mount(
        &mut self,
        connection: NetworkConnection,
        credentials: Option<NetworkMountCredentials>,
        completion: NetworkConnectionMountCompletion,
    ) -> Task<Message> {
        if let Some(credentials) = credentials.as_ref() {
            self.network_connections
                .remember_credentials(&connection.id, credentials);
        }
        self.network_connections.set_connecting(&connection.id);
        network_connection_mount_command(connection, credentials, completion)
    }

    fn start_network_connection_mount_after_credential_lookup(
        &mut self,
        connection: NetworkConnection,
        completion: NetworkConnectionMountCompletion,
        fallback: NetworkConnectionCredentialFallback,
    ) -> Task<Message> {
        self.network_connections.set_connecting(&connection.id);
        network_connection_credentials_lookup_command(connection, completion, fallback)
    }

    fn save_network_connection_editor(&mut self) -> Task<Message> {
        let Some(editor) = self.network_connection_editor.clone() else {
            return Task::none();
        };
        let uri = uri_for_network_connection_save(editor.protocol, &editor.uri);
        let id = editor.id.clone().unwrap_or_else(|| {
            self.network_connections
                .unique_id_for(editor.label.trim(), uri.as_str())
        });
        let username = editor.trimmed_username();
        let connection =
            match build_network_connection_from_editor(&editor, id.clone(), &uri, username) {
                Ok(connection) => connection,
                Err(error) => {
                    if let Some(editor) = &mut self.network_connection_editor {
                        editor.error = Some(error.to_string());
                    }
                    return Task::none();
                }
            };
        if editor.password_is_filled() && connection.username().is_none() {
            if let Some(editor) = &mut self.network_connection_editor {
                editor.error = Some("Username is required when a password is provided".to_owned());
            }
            return Task::none();
        }
        let credentials = network_mount_credentials_from_editor(&editor, &connection);
        let should_connect =
            editor.mode == NetworkConnectionEditorMode::Connect || credentials.is_some();
        let auto_connect = if editor.mode == NetworkConnectionEditorMode::Connect {
            self.network_connections.auto_connect_for(&connection.id)
        } else {
            editor.auto_connect
        };
        let old_connection = editor
            .id
            .as_ref()
            .and_then(|id| self.network_connections.connection(id))
            .cloned();
        let clear_old_credentials_command = old_connection
            .filter(|old_connection| {
                old_connection.protocol != connection.protocol
                    || old_connection.uri != connection.uri
            })
            .map(network_connection_credentials_clear_command)
            .unwrap_or_else(Task::none);

        self.network_connections
            .upsert_saved(SavedNetworkConnection::new(
                connection.clone(),
                auto_connect,
            ));
        self.user_config.network_connections = self.network_connections.saved_connections();
        self.network_connection_editor = None;
        if should_connect {
            Task::batch([
                self.persist_user_preferences_command(),
                clear_old_credentials_command,
                self.start_network_connection_mount(
                    connection,
                    credentials,
                    NetworkConnectionMountCompletion::NavigateToMount,
                ),
            ])
        } else {
            Task::batch([
                self.persist_user_preferences_command(),
                clear_old_credentials_command,
                self.refresh_network_mount_states(),
            ])
        }
    }
}

fn build_network_connection_from_editor(
    editor: &NetworkConnectionEditorState,
    id: NetworkConnectionId,
    uri: &str,
    username: Option<String>,
) -> Result<NetworkConnection, desktop_linux::NetworkMountError> {
    if username.is_some() {
        NetworkConnection::new_with_username(
            id,
            editor.label.trim().to_owned(),
            editor.protocol,
            uri.to_owned(),
            username,
        )
    } else {
        NetworkConnection::new(
            id,
            editor.label.trim().to_owned(),
            editor.protocol,
            uri.to_owned(),
        )
    }
}

fn uri_for_network_connection_save(protocol: NetworkProtocol, uri: &str) -> String {
    let trimmed = uri.trim();
    if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!(
            "{}{trimmed}",
            default_network_connection_uri_prefix(protocol)
        )
    }
}

fn default_network_connection_uri_prefix(protocol: NetworkProtocol) -> &'static str {
    match protocol {
        NetworkProtocol::Smb => "smb://",
        NetworkProtocol::WebDav => "https://",
        NetworkProtocol::Sftp => "sftp://",
    }
}

fn network_mount_credentials_from_editor(
    editor: &NetworkConnectionEditorState,
    connection: &NetworkConnection,
) -> Option<NetworkMountCredentials> {
    if editor.password_is_filled() {
        Some(NetworkMountCredentials::new(
            connection.username(),
            editor.password.clone(),
        ))
    } else {
        None
    }
}

fn network_connection_needs_credentials_editor(connection: &NetworkConnection) -> bool {
    matches!(
        connection.protocol,
        NetworkProtocol::WebDav | NetworkProtocol::Sftp
    ) || connection.username().is_some()
}

#[cfg(test)]
mod tests;
