use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use tempfile::tempdir;

use crate::SearchIndexConfig;

use super::tests::{search_path_file_beside, wait_for_global_hit_count};
use super::SearchDaemonCore;

fn wait_for_stored_size(database_path: &Path, path: &Path, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stored: Option<i64> = Connection::open(database_path)
            .unwrap()
            .query_row(
                "SELECT size FROM files WHERE path = ?1",
                [path.as_os_str().as_encoded_bytes()],
                |row| row.get(0),
            )
            .ok();
        if stored == Some(expected as i64) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the change was not recorded: size {stored:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_cooled_change_is_refreshed_by_the_worker_once_it_is_due() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    fs::create_dir(&home).unwrap();
    let note = home.join("cooling-note.txt");
    fs::write(&note, "alpha").unwrap();
    let database_path = directory.path().join("search.sqlite");
    let core = SearchDaemonCore::new(
        database_path.clone(),
        search_path_file_beside(&database_path),
        SearchIndexConfig {
            roots: vec![home.clone()],
            ..SearchIndexConfig::default()
        },
    )
    .unwrap();
    core.start_index_maintenance().unwrap();
    wait_for_global_hit_count(&database_path, "alpha", 1);

    fs::write(&note, "bravo bravo").unwrap();
    wait_for_stored_size(&database_path, &note, "bravo bravo".len());
    wait_for_global_hit_count(&database_path, "bravo", 0);
    wait_for_global_hit_count(&database_path, "alpha", 1);

    // 冷却算作已到期；下一件工作结束时 worker 从库里重排闹钟，补建随即进行。
    Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE file_stage_state SET content_cooldown_until_ms = 1
             WHERE content_cooldown_until_ms IS NOT NULL",
            [],
        )
        .unwrap();
    fs::write(home.join("other-note.txt"), "other").unwrap();
    wait_for_global_hit_count(&database_path, "bravo", 1);
    wait_for_global_hit_count(&database_path, "alpha", 0);
    core.shutdown().unwrap();
}
