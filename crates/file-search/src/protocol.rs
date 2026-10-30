//! 搜索服务的线协议：帧格式与客户端调用。socket 绑定、会话和查询校验等服务端部分
//! 在 file-search-daemon；两端共用同一份帧读写，主程序的测试也用它模拟服务端。

use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

use crate::error::{SearchError, SearchResult};
use crate::model::{
    SearchPathConfigurationStatus, SearchQuery, SearchResultBatch, SearchServiceEvent,
    SearchServiceRequest, SearchServiceStatus,
};
use crate::{SearchPathPreferences, VersionedSearchPathPreferences};

pub const MAX_REQUEST_FRAME_BYTES: u32 = 64 * 1024;
const MAX_EVENT_FRAME_BYTES: u32 = 2_500_000;

pub fn default_socket_path() -> SearchResult<PathBuf> {
    crate::runtime_identity::SearchRuntimeIdentity::from_environment()
        .map(crate::runtime_identity::SearchRuntimeIdentity::socket_path)
}

pub async fn search_via_socket(
    socket_path: &Path,
    query: SearchQuery,
) -> SearchResult<SearchResultBatch> {
    search_via_socket_with_cancellation(socket_path, query, CancellationToken::new()).await
}

pub async fn search_via_socket_with_cancellation(
    socket_path: &Path,
    query: SearchQuery,
    cancellation: CancellationToken,
) -> SearchResult<SearchResultBatch> {
    let mut stream = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(SearchError::Cancelled),
        stream = UnixStream::connect(socket_path) => stream?,
    };
    let search_request = SearchServiceRequest::Search(query.clone());
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(SearchError::Cancelled),
        outcome = write_service_request(&mut stream, &search_request) => outcome?,
    }
    loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => break,
            event = read_service_event(&mut stream) => match event? {
                SearchServiceEvent::Results(batch) if batch.query_id == query.query_id => {
                    return Ok(batch);
                }
                SearchServiceEvent::SearchFailed { query_id, failure }
                    if query_id == query.query_id =>
                {
                    return Err(SearchError::SearchFailed { query_id, failure });
                }
                _ => {}
            }
        }
    }

    write_service_request(
        &mut stream,
        &SearchServiceRequest::Cancel {
            query_id: query.query_id,
        },
    )
    .await?;
    loop {
        match read_service_event(&mut stream).await? {
            SearchServiceEvent::Cancelled { query_id } if query_id == query.query_id => {
                return Err(SearchError::Cancelled);
            }
            SearchServiceEvent::Results(batch) if batch.query_id == query.query_id => {
                return Ok(batch);
            }
            SearchServiceEvent::SearchFailed { query_id, failure }
                if query_id == query.query_id =>
            {
                return Err(SearchError::SearchFailed { query_id, failure });
            }
            _ => {}
        }
    }
}

pub async fn status_via_socket(socket_path: &Path) -> SearchResult<SearchServiceStatus> {
    let mut stream = UnixStream::connect(socket_path).await?;
    write_service_request(&mut stream, &SearchServiceRequest::Status).await?;
    loop {
        if let SearchServiceEvent::Status(status) = read_service_event(&mut stream).await? {
            return Ok(status);
        }
    }
}

pub async fn path_configuration_via_socket(
    socket_path: &Path,
) -> SearchResult<(
    VersionedSearchPathPreferences,
    SearchPathConfigurationStatus,
)> {
    let mut stream = UnixStream::connect(socket_path).await?;
    write_service_request(&mut stream, &SearchServiceRequest::GetPathConfiguration).await?;
    loop {
        match read_service_event(&mut stream).await? {
            SearchServiceEvent::PathConfiguration {
                configuration,
                status,
            } => return Ok((configuration, status)),
            SearchServiceEvent::PathConfigurationFailed { failure, status: _ } => {
                return Err(SearchError::InvalidConfiguration(failure.to_string()));
            }
            _ => {}
        }
    }
}

pub async fn configure_path_preferences_via_socket(
    socket_path: &Path,
    expected_revision: u64,
    preferences: SearchPathPreferences,
) -> SearchResult<(
    VersionedSearchPathPreferences,
    SearchPathConfigurationStatus,
)> {
    let mut stream = UnixStream::connect(socket_path).await?;
    write_service_request(
        &mut stream,
        &SearchServiceRequest::ConfigurePathPreferences {
            expected_revision,
            preferences,
        },
    )
    .await?;
    loop {
        match read_service_event(&mut stream).await? {
            SearchServiceEvent::PathConfiguration {
                configuration,
                status,
            } => return Ok((configuration, status)),
            SearchServiceEvent::PathConfigurationFailed { failure, status: _ } => {
                return Err(SearchError::InvalidConfiguration(failure.to_string()));
            }
            _ => {}
        }
    }
}

/// 客户端用版本握手识别并退休旧应用遗留的 daemon。
pub async fn version_via_socket(socket_path: &Path) -> SearchResult<(u32, String)> {
    let mut stream = UnixStream::connect(socket_path).await?;
    write_service_request(&mut stream, &SearchServiceRequest::Version).await?;
    loop {
        if let SearchServiceEvent::Version { protocol, build } =
            read_service_event(&mut stream).await?
        {
            return Ok((protocol, build));
        }
    }
}

/// socket 不存在或拒绝连接表示没有 daemon 正在监听；其它连接错误必须保留。
pub async fn shutdown_via_socket(socket_path: &Path) -> SearchResult<()> {
    let stream = match UnixStream::connect(socket_path).await {
        Ok(stream) => stream,
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(());
        }
        Err(source) => return Err(source.into()),
    };
    shutdown_connected_service(stream).await
}

pub async fn shutdown_connected_service(mut stream: UnixStream) -> SearchResult<()> {
    write_service_request(&mut stream, &SearchServiceRequest::Shutdown).await?;
    // server 在自有资源清理完成后才关闭连接，EOF 因而是可等待的完成边界。
    let mut buffer = [0_u8; 1];
    let _ = stream.read(&mut buffer).await;
    Ok(())
}

pub async fn read_service_request(
    reader: &mut (impl AsyncRead + Unpin),
) -> SearchResult<SearchServiceRequest> {
    read_frame(reader, MAX_REQUEST_FRAME_BYTES).await
}

pub async fn write_service_request(
    writer: &mut (impl AsyncWrite + Unpin),
    request: &SearchServiceRequest,
) -> SearchResult<()> {
    write_frame(writer, request, MAX_REQUEST_FRAME_BYTES).await
}

pub async fn read_service_event(
    reader: &mut (impl AsyncRead + Unpin),
) -> SearchResult<SearchServiceEvent> {
    read_frame(reader, MAX_EVENT_FRAME_BYTES).await
}

pub async fn write_service_event(
    writer: &mut (impl AsyncWrite + Unpin),
    event: &SearchServiceEvent,
) -> SearchResult<()> {
    write_frame(writer, event, MAX_EVENT_FRAME_BYTES).await
}

async fn read_frame<T: DeserializeOwned>(
    reader: &mut (impl AsyncRead + Unpin),
    max_frame_bytes: u32,
) -> SearchResult<T> {
    let len = reader.read_u32().await?;
    if len > max_frame_bytes {
        return Err(SearchError::ProtocolFrameTooLarge(len));
    }
    let mut bytes = vec![0; len as usize];
    reader.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn write_frame<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &T,
    max_frame_bytes: u32,
) -> SearchResult<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > max_frame_bytes as usize {
        return Err(SearchError::ProtocolFrameTooLarge(bytes.len() as u32));
    }
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}
