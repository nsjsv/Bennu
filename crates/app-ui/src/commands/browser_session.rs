use bennu_state::{BrowserSessionStore, StoreError, StoreResult};

use crate::model::{
    snapshot_from_stored, snapshot_to_stored, BrowserSessionSnapshot, SavedBrowserSession,
};

pub(super) async fn persist_browser_session(
    session_store: BrowserSessionStore,
    snapshot: BrowserSessionSnapshot,
) -> Result<(), String> {
    let Some(stored) = snapshot_to_stored(snapshot) else {
        return Ok(());
    };
    tokio::task::spawn_blocking(move || session_store.replace_browser_session(&stored))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// 读不懂的会话只记日志、原样留在库里，下次保存会话时覆盖（同 Firefox、Dolphin）。
pub(super) fn read_saved_browser_session(
    store: &BrowserSessionStore,
) -> StoreResult<SavedBrowserSession> {
    match store.read_browser_session() {
        Ok(None) => Ok(SavedBrowserSession::Missing),
        Ok(Some(stored)) => Ok(match snapshot_from_stored(stored) {
            Some(snapshot) => SavedBrowserSession::Restorable(snapshot),
            None => {
                tracing::warn!("saved browser session has no restorable pane; opening home");
                SavedBrowserSession::Unreadable
            }
        }),
        Err(StoreError::Json(error)) => {
            tracing::warn!(%error, "saved browser session is unreadable; opening home");
            Ok(SavedBrowserSession::Unreadable)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_session_is_reported_as_such_and_left_in_place() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state.sqlite");
        let store = BrowserSessionStore::new(&database).unwrap();
        rusqlite::Connection::open(&database)
            .unwrap()
            .execute(
                "INSERT INTO browser_session (session_key, payload_json, updated_at_ms)
                 VALUES ('main', '{ not json', 1)",
                [],
            )
            .unwrap();

        assert!(matches!(
            read_saved_browser_session(&store),
            Ok(SavedBrowserSession::Unreadable)
        ));
        let payload: String = rusqlite::Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT payload_json FROM browser_session WHERE session_key = 'main'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(payload, "{ not json");
    }

    #[test]
    fn no_saved_session_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let store = BrowserSessionStore::new(directory.path().join("state.sqlite")).unwrap();

        assert!(matches!(
            read_saved_browser_session(&store),
            Ok(SavedBrowserSession::Missing)
        ));
    }
}
