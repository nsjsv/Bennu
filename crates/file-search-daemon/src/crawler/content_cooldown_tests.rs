use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use tempfile::{tempdir, TempDir};
use tokio_util::sync::CancellationToken;

use crate::database::{wall_clock_ms, SearchDatabase};
use crate::model::SearchQuery;
use crate::query_state::QueryState;
use crate::writer::IndexWriter;

use super::tests::{config_for, expire_content_cooldowns, writer_in};
use super::{RebuildStats, SearchIndexer};

struct CoolingFixture {
    _content: TempDir,
    db_dir: TempDir,
    writer: Arc<IndexWriter>,
    indexer: SearchIndexer,
    path: PathBuf,
}

/// 一个已经入了索引、正文还在冷却的文件，刚刚又被改过一次（改成 "needle two words"）。
async fn file_changed_inside_its_cooldown() -> CoolingFixture {
    let content = tempdir().unwrap();
    let db_dir = tempdir().unwrap();
    let path = content.path().join("note.txt");
    fs::write(&path, "needle one").unwrap();
    let writer = writer_in(&db_dir);
    let indexer = SearchIndexer::new(Arc::clone(&writer), config_for(content.path()));
    indexer.rebuild().await.unwrap();

    fs::write(&path, "needle two words").unwrap();
    let cooled = indexer.rebuild_paths(vec![path.clone()]).await.unwrap();
    assert_eq!(cooled.changed, 1);
    assert_eq!(cooled.reindexed, 0);
    assert_eq!(cooled.content_reads, 0);
    assert_eq!(cooled.database_mutations, 1);
    CoolingFixture {
        _content: content,
        db_dir,
        writer,
        indexer,
        path,
    }
}

async fn refresh_due_content(indexer: &SearchIndexer) -> RebuildStats {
    indexer
        .refresh_cooled_content_with_progress_cancelled(&CancellationToken::new(), |_| {})
        .await
        .unwrap()
}

fn hit_count(db_dir: &TempDir, term: &str) -> usize {
    SearchDatabase::open_read_only(&db_dir.path().join("search.sqlite"))
        .unwrap()
        .search(&SearchQuery::global(1, term), &QueryState::default())
        .unwrap()
        .hits
        .len()
}

fn database(db_dir: &TempDir) -> Connection {
    Connection::open(db_dir.path().join("search.sqlite")).unwrap()
}

fn content_stage(db_dir: &TempDir, path: &Path) -> String {
    database(db_dir)
        .query_row(
            "SELECT content_stage_state FROM file_stage_state WHERE path = ?1",
            [path.as_os_str().as_encoded_bytes()],
            |row| row.get(0),
        )
        .unwrap()
}

/// 全文索引与预览存下的全部字节；比对它就能看出有没有改写过。
fn stored_fulltext(db_dir: &TempDir) -> Vec<(i64, Vec<u8>)> {
    let connection = database(db_dir);
    let mut statement = connection
        .prepare(
            "SELECT rowid, block FROM file_search_fts_data
             UNION ALL SELECT -file_rowid, CAST(preview AS BLOB) FROM file_search_snippets
             ORDER BY 1",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn a_change_inside_the_cooldown_records_metadata_and_keeps_the_indexed_text() {
    let fixture = file_changed_inside_its_cooldown().await;

    assert_eq!(hit_count(&fixture.db_dir, "words"), 0);
    assert_eq!(
        hit_count(&fixture.db_dir, "one"),
        1,
        "old text stays searchable"
    );
    let indexed_size: i64 = database(&fixture.db_dir)
        .query_row("SELECT size FROM files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(indexed_size, "needle two words".len() as i64);
    assert_eq!(content_stage(&fixture.db_dir, &fixture.path), "pending");
    let now_ms = wall_clock_ms();
    let due_ms = fixture
        .writer
        .next_content_refresh_due_ms(now_ms)
        .unwrap()
        .expect("the cooled change is owed a refresh");
    assert!(due_ms > now_ms);
}

#[tokio::test]
async fn the_cooled_content_is_rebuilt_once_its_cooldown_is_over() {
    let fixture = file_changed_inside_its_cooldown().await;

    let early = refresh_due_content(&fixture.indexer).await;
    assert_eq!(early, RebuildStats::default());
    assert_eq!(hit_count(&fixture.db_dir, "words"), 0);

    expire_content_cooldowns(&fixture.db_dir);
    let refreshed = refresh_due_content(&fixture.indexer).await;
    assert_eq!(refreshed.reindexed, 1);
    assert_eq!(refreshed.content_reads, 1);
    assert_eq!(hit_count(&fixture.db_dir, "words"), 1);
    assert_eq!(hit_count(&fixture.db_dir, "one"), 0);
    assert_eq!(content_stage(&fixture.db_dir, &fixture.path), "complete");
    assert_eq!(
        fixture
            .writer
            .next_content_refresh_due_ms(wall_clock_ms())
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_full_check_inside_the_cooldown_does_not_read_the_content() {
    let fixture = file_changed_inside_its_cooldown().await;

    let checked = fixture.indexer.rebuild().await.unwrap();
    assert_eq!(checked.content_reads, 0);
    assert_eq!(hit_count(&fixture.db_dir, "words"), 0);
    assert_eq!(content_stage(&fixture.db_dir, &fixture.path), "pending");
}

#[tokio::test]
async fn unchanged_indexed_text_leaves_the_full_text_index_untouched() {
    let content = tempdir().unwrap();
    let db_dir = tempdir().unwrap();
    let path = content.path().join("growing.log");
    // 测试配置只取前 1024 字节；之后的追加不改变入索引的文字。
    fs::write(&path, "needle line\n".repeat(100)).unwrap();
    let writer = writer_in(&db_dir);
    let indexer = SearchIndexer::new(Arc::clone(&writer), config_for(content.path()));
    indexer.rebuild().await.unwrap();
    let indexed = stored_fulltext(&db_dir);

    expire_content_cooldowns(&db_dir);
    fs::write(&path, "needle line\n".repeat(120)).unwrap();
    let appended = indexer.rebuild_paths(vec![path.clone()]).await.unwrap();

    assert_eq!(appended.content_reads, 1);
    assert_eq!(appended.reindexed, 1);
    assert_eq!(stored_fulltext(&db_dir), indexed);
    assert_eq!(hit_count(&db_dir, "needle"), 1);
}

#[tokio::test]
async fn a_due_refresh_of_a_deleted_file_removes_it_and_is_not_owed_again() {
    let fixture = file_changed_inside_its_cooldown().await;
    fs::remove_file(&fixture.path).unwrap();
    expire_content_cooldowns(&fixture.db_dir);

    refresh_due_content(&fixture.indexer).await;

    assert_eq!(hit_count(&fixture.db_dir, "needle"), 0);
    assert_eq!(
        fixture
            .writer
            .next_content_refresh_due_ms(wall_clock_ms())
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_due_refresh_of_an_unreadable_file_hides_it_without_coming_back() {
    let fixture = file_changed_inside_its_cooldown().await;
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o0)).unwrap();
    expire_content_cooldowns(&fixture.db_dir);

    refresh_due_content(&fixture.indexer).await;

    assert_eq!(hit_count(&fixture.db_dir, "needle"), 0);
    assert_eq!(
        fixture
            .writer
            .next_content_refresh_due_ms(wall_clock_ms())
            .unwrap(),
        None,
        "an unreadable file waits for the next change, not for a timer"
    );
    let again = refresh_due_content(&fixture.indexer).await;
    assert_eq!(again, RebuildStats::default());
}

#[tokio::test]
async fn a_cooldown_left_behind_by_a_clock_that_moved_back_is_due_now() {
    let fixture = file_changed_inside_its_cooldown().await;
    let ten_hours_ahead = wall_clock_ms() + 10 * 3_600_000;
    database(&fixture.db_dir)
        .execute(
            "UPDATE file_stage_state SET content_cooldown_until_ms = ?1",
            [ten_hours_ahead],
        )
        .unwrap();

    let now_ms = wall_clock_ms();
    let due_ms = fixture
        .writer
        .next_content_refresh_due_ms(now_ms)
        .unwrap()
        .unwrap();
    assert!(due_ms <= wall_clock_ms());
    let refreshed = refresh_due_content(&fixture.indexer).await;
    assert_eq!(refreshed.reindexed, 1);
    assert_eq!(hit_count(&fixture.db_dir, "words"), 1);
}
