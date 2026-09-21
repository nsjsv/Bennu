use file_search::{
    default_socket_path, search_directory_fallback, search_via_socket_with_cancellation,
    DirectoryFallbackLimits, SearchError, SearchExcludeRules, SearchProviderFailure, SearchQuery,
};
use iced::futures::SinkExt;
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::model::{DirectoryFallbackOutcome, IndexedSearchOutcome, IndexedSearchRequest, Message};

const DIRECTORY_FALLBACK_STREAM_CAPACITY: usize = 8;
const DIRECTORY_FALLBACK_WORKER_CAPACITY: usize = 4;
pub(crate) const DIRECTORY_FALLBACK_ENTRY_BUDGET: usize = 50_000;

pub(crate) fn search_command(
    request: IndexedSearchRequest,
    query: SearchQuery,
    cancellation: CancellationToken,
) -> Task<Message> {
    Task::perform(search_once(query, cancellation), move |outcome| {
        Message::SearchResultsLoaded(request, outcome)
    })
}

// 根目录可用性校验移入后台执行：NFS/失联挂载点上的同步 stat 可能阻塞数秒，
// 绝不能在 update 线程执行（原实现为同步 std::fs::metadata 的历史缺陷）。
pub(crate) fn search_with_scope_root_check_command(
    request: IndexedSearchRequest,
    query: SearchQuery,
    cancellation: CancellationToken,
) -> Task<Message> {
    let scope_root = match &query.scope {
        file_search::SearchScope::Directory(root) => Some(root.clone()),
        _ => None,
    };
    Task::perform(
        async move {
            let root_is_available = match &scope_root {
                Some(root) => tokio::fs::metadata(root)
                    .await
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(false),
                None => true,
            };
            Message::SearchScopeRootValidated(request, query, cancellation, root_is_available)
        },
        |message| message,
    )
}

pub(crate) fn directory_fallback_search_command(
    generation: u64,
    query: SearchQuery,
    rules: SearchExcludeRules,
    cancellation: CancellationToken,
) -> Task<Message> {
    Task::stream(iced::stream::channel(
        DIRECTORY_FALLBACK_STREAM_CAPACITY,
        async move |mut output| {
            let (batch_sender, mut batch_receiver) =
                tokio::sync::mpsc::channel(DIRECTORY_FALLBACK_WORKER_CAPACITY);
            let worker_cancellation = cancellation.clone();
            let fallback_worker = tokio::task::spawn_blocking(move || {
                search_directory_fallback(
                    &query,
                    &rules,
                    DirectoryFallbackLimits {
                        max_inspected_entries: DIRECTORY_FALLBACK_ENTRY_BUDGET,
                    },
                    &worker_cancellation,
                    |hits| {
                        if batch_sender.blocking_send(hits).is_err() {
                            worker_cancellation.cancel();
                        }
                    },
                )
            });

            while let Some(hits) = batch_receiver.recv().await {
                if output
                    .send(Message::SearchDirectoryBatchLoaded(generation, hits))
                    .await
                    .is_err()
                {
                    cancellation.cancel();
                    batch_receiver.close();
                    break;
                }
            }
            drop(batch_receiver);

            let completion = match fallback_worker.await {
                Ok(Ok(completion)) => DirectoryFallbackOutcome::Completed(completion),
                Ok(Err(SearchError::Cancelled)) => DirectoryFallbackOutcome::Cancelled,
                Ok(Err(error)) => DirectoryFallbackOutcome::Failed(error.to_string()),
                Err(error) => DirectoryFallbackOutcome::Failed(format!(
                    "directory fallback worker failed: {error}"
                )),
            };
            let _ = output
                .send(Message::SearchDirectoryFinished(generation, completion))
                .await;
        },
    ))
}

async fn search_once(query: SearchQuery, cancellation: CancellationToken) -> IndexedSearchOutcome {
    let socket_path = match default_socket_path() {
        Ok(socket_path) => socket_path,
        Err(error) => return indexed_outcome_from_error(error),
    };
    match search_via_socket_with_cancellation(&socket_path, query, cancellation).await {
        Ok(batch) => IndexedSearchOutcome::Batch(batch),
        Err(error) => indexed_outcome_from_error(error),
    }
}

fn indexed_outcome_from_error(error: SearchError) -> IndexedSearchOutcome {
    match error {
        SearchError::Cancelled => IndexedSearchOutcome::Cancelled,
        error @ (SearchError::ProtocolIo(_)
        | SearchError::Json(_)
        | SearchError::ProtocolFrameTooLarge(_)) => {
            IndexedSearchOutcome::TransportUnavailable(error.to_string())
        }
        SearchError::SearchFailed { failure, .. } => match failure {
            SearchProviderFailure::Unavailable { message } => {
                IndexedSearchOutcome::ProviderUnavailable(message)
            }
            SearchProviderFailure::InvalidQuery { message } => {
                IndexedSearchOutcome::InvalidQuery(message)
            }
            SearchProviderFailure::Fatal { message } => IndexedSearchOutcome::Fatal(message),
        },
        SearchError::InvalidQuery(message) => IndexedSearchOutcome::InvalidQuery(message),
        error => IndexedSearchOutcome::Fatal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use file_search::{SearchError, SearchProviderFailure};

    use super::indexed_outcome_from_error;
    use crate::model::IndexedSearchOutcome;

    #[test]
    fn structured_unavailable_is_preserved_for_provider_routing() {
        let outcome = indexed_outcome_from_error(SearchError::SearchFailed {
            query_id: 7,
            failure: SearchProviderFailure::Unavailable {
                message: "index is starting".to_owned(),
            },
        });
        assert_eq!(
            outcome,
            IndexedSearchOutcome::ProviderUnavailable("index is starting".to_owned())
        );
    }

    #[test]
    fn malformed_protocol_is_transport_unavailable_before_provider_selection() {
        let outcome = indexed_outcome_from_error(SearchError::ProtocolFrameTooLarge(8));

        assert!(matches!(
            outcome,
            IndexedSearchOutcome::TransportUnavailable(_)
        ));
    }

    #[test]
    fn socket_cancellation_stays_distinct_from_provider_failure() {
        assert_eq!(
            indexed_outcome_from_error(SearchError::Cancelled),
            IndexedSearchOutcome::Cancelled
        );
    }
}
