use desktop_linux::{
    clear_network_connection_credentials, load_network_mount_states,
    lookup_network_connection_credentials, mount_network_connection_with_credentials,
    store_network_connection_credentials, unmount_network_connection, NetworkConnection,
    NetworkMountCredentials,
};
use iced::Task;

use crate::model::Message;
use crate::network_connections::{
    NetworkConnectionCredentialFallback, NetworkConnectionMessage, NetworkConnectionMountCompletion,
};

pub(crate) fn network_mount_states_command(connections: Vec<NetworkConnection>) -> Task<Message> {
    Task::perform(
        async move {
            load_network_mount_states(connections)
                .await
                .map_err(|error| error.to_string())
        },
        |outcome| Message::NetworkConnection(NetworkConnectionMessage::MountStatesLoaded(outcome)),
    )
}

pub(crate) fn network_connection_mount_command(
    connection: NetworkConnection,
    credentials: Option<NetworkMountCredentials>,
    completion: NetworkConnectionMountCompletion,
) -> Task<Message> {
    let expected_connection = connection.clone();
    Task::perform(
        async move {
            mount_network_connection_with_credentials(connection, credentials)
                .await
                .map_err(|error| error.to_string())
        },
        move |outcome| {
            Message::NetworkConnection(NetworkConnectionMessage::MountFinished(
                expected_connection.clone(),
                completion,
                outcome,
            ))
        },
    )
}

pub(crate) fn network_connection_credentials_lookup_command(
    connection: NetworkConnection,
    completion: NetworkConnectionMountCompletion,
    fallback: NetworkConnectionCredentialFallback,
) -> Task<Message> {
    let expected_connection = connection.clone();
    Task::perform(
        async move {
            lookup_network_connection_credentials(connection)
                .await
                .map_err(|error| error.to_string())
        },
        move |outcome| {
            Message::NetworkConnection(NetworkConnectionMessage::StoredCredentialsLoaded(
                expected_connection.clone(),
                completion,
                fallback,
                outcome,
            ))
        },
    )
}

pub(crate) fn network_connection_credentials_store_command(
    connection: NetworkConnection,
    credentials: NetworkMountCredentials,
) -> Task<Message> {
    let id = connection.id.clone();
    Task::perform(
        async move {
            store_network_connection_credentials(connection, credentials)
                .await
                .map_err(|error| error.to_string())
        },
        move |outcome| {
            Message::NetworkConnection(NetworkConnectionMessage::StoredCredentialsSaved(
                id.clone(),
                outcome,
            ))
        },
    )
}

pub(crate) fn network_connection_credentials_clear_command(
    connection: NetworkConnection,
) -> Task<Message> {
    let id = connection.id.clone();
    Task::perform(
        async move {
            clear_network_connection_credentials(connection)
                .await
                .map_err(|error| error.to_string())
        },
        move |outcome| {
            Message::NetworkConnection(NetworkConnectionMessage::StoredCredentialsCleared(
                id.clone(),
                outcome,
            ))
        },
    )
}

pub(crate) fn network_connection_unmount_command(connection: NetworkConnection) -> Task<Message> {
    let expected_connection = connection.clone();
    Task::perform(
        async move {
            unmount_network_connection(connection)
                .await
                .map_err(|error| error.to_string())
        },
        move |outcome| {
            Message::NetworkConnection(NetworkConnectionMessage::UnmountFinished(
                expected_connection.clone(),
                outcome,
            ))
        },
    )
}
