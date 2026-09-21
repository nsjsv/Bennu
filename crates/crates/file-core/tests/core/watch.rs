use super::*;

#[tokio::test]
async fn directory_watcher_coalesces_creates_into_added_entries() {
    let dir = tempdir().unwrap();
    let mut watcher = watch_directory(dir.path(), std::time::Duration::from_millis(40)).unwrap();

    fs::write(dir.path().join("one"), b"1").unwrap();
    fs::write(dir.path().join("two"), b"2").unwrap();

    let change = tokio::time::timeout(std::time::Duration::from_secs(2), watcher.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(change.directory, dir.path());
    let mut added_paths = change
        .changes
        .iter()
        .filter_map(|change| match change {
            ResolvedEntryChange::Added(entry) => Some(entry.path().to_path_buf()),
            _ => None,
        })
        .collect::<Vec<_>>();
    added_paths.sort();
    assert_eq!(
        added_paths,
        vec![dir.path().join("one"), dir.path().join("two")]
    );
}

#[tokio::test]
async fn directory_watcher_reports_rename_with_old_and_new_paths() {
    let dir = tempdir().unwrap();
    let mut watcher = watch_directory(dir.path(), std::time::Duration::from_millis(40)).unwrap();

    let source = dir.path().join("before.txt");
    let target = dir.path().join("after.txt");
    fs::write(&source, b"content").unwrap();
    // 等首个创建窗口独立结算，避免与 rename 事件合并干扰断言。
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), watcher.recv()).await;
    fs::rename(&source, &target).unwrap();

    let change = tokio::time::timeout(std::time::Duration::from_secs(2), watcher.recv())
        .await
        .unwrap()
        .unwrap();

    let renamed = change
        .changes
        .iter()
        .find_map(|change| match change {
            ResolvedEntryChange::Renamed { from, to } => Some((from.clone(), to.clone())),
            _ => None,
        })
        .expect("rename should surface as a single Renamed change");
    assert_eq!(renamed.0, source);
    assert_eq!(renamed.1.path(), target);
    assert_eq!(fs::read(renamed.1.path()).unwrap(), b"content");
}

#[tokio::test]
async fn directory_watcher_folds_create_then_delete_to_nothing() {
    let dir = tempdir().unwrap();
    let mut watcher = watch_directory(dir.path(), std::time::Duration::from_millis(40)).unwrap();

    let path = dir.path().join("ephemeral.txt");
    fs::write(&path, b"temporary").unwrap();
    fs::remove_file(&path).unwrap();

    let change = tokio::time::timeout(std::time::Duration::from_secs(2), watcher.recv())
        .await
        .unwrap()
        .unwrap();

    assert!(
        change
            .changes
            .iter()
            .all(|change| !matches!(change, ResolvedEntryChange::Added(_))),
        "create+delete inside one window must not surface an addition, got {:?}",
        change.changes
    );
    // 删除事件可以保留：应用方对不存在的条目幂等跳过。
}

#[tokio::test]
async fn directory_watcher_degrades_large_batches_to_rescan() {
    let dir = tempdir().unwrap();
    let mut watcher = watch_directory(dir.path(), std::time::Duration::from_millis(60)).unwrap();

    for index in 0..80 {
        fs::write(dir.path().join(format!("bulk-{index}")), b"x").unwrap();
    }

    let change = tokio::time::timeout(std::time::Duration::from_secs(4), watcher.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(change.changes.len(), 1);
    assert!(matches!(
        change.changes[0],
        ResolvedEntryChange::RescanRequired
    ));
}
