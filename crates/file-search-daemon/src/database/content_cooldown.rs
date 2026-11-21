//! 正文冷却：一个文件读入正文后要冷却一段时间才能再读。冷却期内的变化只记元数据，
//! 正文阶段置回 pending，到期由服务补建。重建一次的代价随文字量增长（读、切词、FTS
//! segment 与之后的合并），所以冷却时长也随文字量增长；一直在追加的日志因此最多每半小时
//! 重建一次，而不是每追加一行就整份重写。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::MAX_EXTRACT_BYTES;
use rusqlite::{params, Connection};

use crate::error::SearchResult;

use super::write_batch::WriteSavepoint;
use super::{path_from_storage_bytes, path_to_storage, FileSignature, SearchDatabase};

/// 读入正文的固定开销（元数据几页、一个 FTS segment、预览），再小的文件也冷却这么久。
const CONTENT_COOLDOWN_BASE_MS: i64 = 15_000;
/// 每 MB 正文多冷却 15 分钟：抽取上限 2 MB 时最长约半小时。
const CONTENT_COOLDOWN_MS_PER_MEGABYTE: i64 = 15 * 60 * 1_000;
const MEGABYTE: i64 = 1_000_000;

/// 正文不会超过抽取上限，冷却也就不会超过这个值。冷却截止时间比“现在 + 它”还晚，只可能是
/// 时钟往回拨过：按已到期处理，否则正文要晚上几个小时才更新。
pub(crate) const MAX_CONTENT_COOLDOWN_MS: i64 = content_cooldown_ms(MAX_EXTRACT_BYTES as usize);

const fn content_cooldown_ms(text_bytes: usize) -> i64 {
    let text_bytes = if text_bytes < MAX_EXTRACT_BYTES as usize {
        text_bytes as i64
    } else {
        MAX_EXTRACT_BYTES as i64
    };
    CONTENT_COOLDOWN_BASE_MS + text_bytes * CONTENT_COOLDOWN_MS_PER_MEGABYTE / MEGABYTE
}

/// 冷却截止时间按墙钟存：要跨重启保留，单调时钟做不到。
pub(crate) fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

pub(crate) fn content_cooldown_active(cooldown_until_ms: i64, now_ms: i64) -> bool {
    now_ms < cooldown_until_ms
        && cooldown_until_ms <= now_ms.saturating_add(MAX_CONTENT_COOLDOWN_MS)
}

pub(super) fn content_cooldown_until_ms(text_bytes: usize, now_ms: i64) -> i64 {
    now_ms.saturating_add(content_cooldown_ms(text_bytes))
}

/// 写进全文索引与预览的文字的指纹。三段各带长度前缀，段与段之间的边界不会因内容而含糊；
/// 64 位足够：只拿同一个文件的新旧两版相比。
pub(super) fn fulltext_digest(name_text: &str, content_text: &str, preview: Option<&str>) -> i64 {
    let mut hasher = blake3::Hasher::new();
    for part in [name_text, content_text, preview.unwrap_or_default()] {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut leading_bytes = [0_u8; 8];
    leading_bytes.copy_from_slice(&digest.as_bytes()[..8]);
    i64::from_le_bytes(leading_bytes)
}

/// 正文还在冷却的文件这次观察到的元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoolingFile {
    pub path: PathBuf,
    pub signature: FileSignature,
    pub modified_ms: Option<i64>,
    pub accessed_ms: Option<i64>,
    pub created_ms: Option<i64>,
    pub mime_type: Option<String>,
}

/// 只写能看到的元数据和签名，全文、预览、内容状态停在已入索引的那一版，冷却截止时间
/// 不动；值都没变时不改写（启动核对再看到同一个冷却中的文件不产生写入）。
pub(super) fn record_cooling_file(connection: &Connection, file: &CoolingFile) -> SearchResult<()> {
    let path = path_to_storage(&file.path);
    connection
        .prepare_cached(
            "UPDATE files
             SET size = ?2, modified_ms = ?3, accessed_ms = ?4, created_ms = ?5, mime_type = ?6,
                 device = ?7, inode = ?8, mtime_ns = ?9, ctime_ns = ?10
             WHERE path = ?1
               AND (size IS NOT ?2 OR modified_ms IS NOT ?3 OR accessed_ms IS NOT ?4
                    OR created_ms IS NOT ?5 OR mime_type IS NOT ?6 OR device IS NOT ?7
                    OR inode IS NOT ?8 OR mtime_ns IS NOT ?9 OR ctime_ns IS NOT ?10)",
        )?
        .execute(params![
            path,
            file.signature.size as i64,
            file.modified_ms,
            file.accessed_ms,
            file.created_ms,
            file.mime_type,
            file.signature.device.map(|device| device as i64),
            file.signature.inode.map(|inode| inode as i64),
            file.signature.mtime_ns,
            file.signature.ctime_ns,
        ])?;
    connection
        .prepare_cached(
            "UPDATE file_stage_state SET content_stage_state = 'pending'
             WHERE path = ?1 AND content_stage_state <> 'pending'",
        )?
        .execute(params![path])?;
    Ok(())
}

impl SearchDatabase {
    /// 取一页冷却已到期、欠着补建的路径，同时清掉它们的冷却截止时间：交出去以后不管是
    /// 补建、删除、不可访问还是归了别的 root，都不会再被选中，补建不会原地打转。
    pub(crate) fn take_due_content_refreshes(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> SearchResult<Vec<PathBuf>> {
        let transaction = WriteSavepoint::begin(&self.connection)?;
        let due_paths = transaction
            .prepare_cached(
                "SELECT path FROM file_stage_state
                 WHERE content_stage_state = 'pending' AND content_cooldown_until_ms IS NOT NULL
                   AND (content_cooldown_until_ms <= ?1 OR content_cooldown_until_ms > ?1 + ?2)
                 ORDER BY content_cooldown_until_ms
                 LIMIT ?3",
            )?
            .query_map(
                params![now_ms, MAX_CONTENT_COOLDOWN_MS, limit as i64],
                |row| row.get::<_, Vec<u8>>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        {
            let mut clear_cooldown = transaction.prepare_cached(
                "UPDATE file_stage_state SET content_cooldown_until_ms = NULL WHERE path = ?1",
            )?;
            for path in &due_paths {
                clear_cooldown.execute(params![path])?;
            }
        }
        transaction.release()?;
        Ok(due_paths.into_iter().map(path_from_storage_bytes).collect())
    }

    /// 最早一笔补建欠账的时刻；截止时间因时钟回拨而远在将来的算作现在。
    pub(crate) fn next_content_refresh_due_ms(&self, now_ms: i64) -> SearchResult<Option<i64>> {
        Ok(self.connection.query_row(
            "SELECT MIN(CASE WHEN content_cooldown_until_ms > ?1 + ?2 THEN ?1
                             ELSE content_cooldown_until_ms END)
             FROM file_stage_state
             WHERE content_stage_state = 'pending' AND content_cooldown_until_ms IS NOT NULL",
            params![now_ms, MAX_CONTENT_COOLDOWN_MS],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::database::{EntryStageProgress, IndexedEntryStageState, IndexedFile};
    use crate::extractor::ExtractionStatus;
    use crate::model::SearchFileKind;

    #[test]
    fn schema_fourteen_gains_the_cooldown_columns_with_nothing_known_yet() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("search.sqlite");
        let database = SearchDatabase::open(&database_path).unwrap();
        database
            .upsert_file(&IndexedFile {
                path: Path::new("/tmp/note.txt").to_path_buf(),
                parent_path: Path::new("/tmp").to_path_buf(),
                display_name: "note.txt".to_owned(),
                kind: SearchFileKind::File,
                size: 6,
                modified_ms: Some(1),
                accessed_ms: None,
                created_ms: None,
                mime_type: Some("text/plain".to_owned()),
                stage_state: IndexedEntryStageState {
                    metadata: EntryStageProgress::Complete,
                    content: EntryStageProgress::Complete,
                },
                content: Some("needle".to_owned()),
                extraction_status: ExtractionStatus::Indexed,
                device: Some(1),
                inode: Some(1),
                mtime_ns: Some(1),
                ctime_ns: Some(1),
            })
            .unwrap();
        database
            .connection
            .execute_batch(
                "DROP INDEX file_stage_state_content_refresh_due;
                 ALTER TABLE file_stage_state DROP COLUMN content_digest;
                 ALTER TABLE file_stage_state DROP COLUMN content_cooldown_until_ms;
                 PRAGMA user_version = 14;",
            )
            .unwrap();
        drop(database);

        let database = SearchDatabase::open(&database_path).unwrap();
        let stored: (Option<i64>, Option<i64>) = database
            .connection
            .query_row(
                "SELECT content_digest, content_cooldown_until_ms FROM file_stage_state",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored, (None, None));
        assert_eq!(
            database
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            crate::database::SCHEMA_VERSION
        );
        SearchDatabase::open_read_only(&database_path).unwrap();
    }

    #[test]
    fn cooldown_grows_with_the_indexed_text_and_stops_at_the_extraction_limit() {
        assert_eq!(content_cooldown_ms(0), 15_000);
        assert_eq!(content_cooldown_ms(100_000), 15_000 + 90_000);
        assert_eq!(content_cooldown_ms(2_000_000), 15_000 + 1_800_000);
        assert_eq!(
            content_cooldown_ms(50_000_000),
            MAX_CONTENT_COOLDOWN_MS,
            "text beyond the extraction limit is never indexed, so it cannot cost more"
        );
    }

    #[test]
    fn a_cooldown_far_beyond_the_longest_one_means_the_clock_went_back() {
        let now = 1_000_000_000;
        assert!(content_cooldown_active(now + 1, now));
        assert!(content_cooldown_active(now + MAX_CONTENT_COOLDOWN_MS, now));
        assert!(!content_cooldown_active(now, now));
        assert!(!content_cooldown_active(now - 1, now));
        assert!(!content_cooldown_active(
            now + MAX_CONTENT_COOLDOWN_MS + 1,
            now
        ));
    }

    #[test]
    fn the_digest_changes_with_any_written_text_and_keeps_part_boundaries() {
        let digest = fulltext_digest("name", "body text", Some("body text"));
        assert_eq!(
            digest,
            fulltext_digest("name", "body text", Some("body text"))
        );
        assert_ne!(
            digest,
            fulltext_digest("name", "body text!", Some("body text"))
        );
        assert_ne!(digest, fulltext_digest("name", "body text", None));
        assert_ne!(
            digest,
            fulltext_digest("names", "body text", Some("body text"))
        );
        assert_ne!(
            fulltext_digest("ab", "c", None),
            fulltext_digest("a", "bc", None),
            "moving text across the name/content boundary is a different index row"
        );
    }
}
