use std::fs;
use std::thread;
use std::time::Duration;

use rusqlite::Connection;
use tempfile::tempdir;

use crate::SearchIndexConfig;

use super::tests::{search_path_file_beside, wait_for_global_hit_count};
use super::SearchDaemonCore;

/// 别的连接每提交一次，这个值就变一次。
fn data_version(connection: &Connection) -> i64 {
    connection
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn files_created_and_deleted_before_settling_never_reach_the_index() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    let temporary = home.join("temp");
    fs::create_dir_all(&temporary).unwrap();
    fs::write(home.join("startup-note.txt"), "startup").unwrap();
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
    wait_for_global_hit_count(&database_path, "startup-note", 1);
    thread::sleep(Duration::from_millis(500));
    let observer = Connection::open(&database_path).unwrap();
    let before = data_version(&observer);

    // 写完、关掉，留一秒再删：一个聊天程序解密临时文件就是这样。只合并一小段时间的事件时，
    // 文件在这一秒里已经安静下来，会被收录再删掉。
    for index in 0..3 {
        let path = temporary.join(format!("Xhc61SZJZ{index}"));
        fs::write(&path, vec![b'x'; 64 * 1024]).unwrap();
        thread::sleep(Duration::from_secs(1));
        fs::remove_file(&path).unwrap();
    }
    thread::sleep(Duration::from_secs(3));
    assert_eq!(
        data_version(&observer),
        before,
        "short-lived files must not cause a single index commit"
    );

    fs::write(home.join("settled-note.txt"), "settled").unwrap();
    wait_for_global_hit_count(&database_path, "settled-note", 1);
    core.shutdown().unwrap();
}
