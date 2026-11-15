use std::path::Path;

use super::*;

fn len(path: &Path) -> u64 {
    std::fs::symlink_metadata(path).unwrap().len()
}

/// root/a.txt, root/.hidden.txt, root/sub/b.txt, root/sub/deeper/c.txt, root/.dot/d.txt
fn sample_tree(root: &Path) -> Vec<std::path::PathBuf> {
    let sub = root.join("sub");
    let deeper = sub.join("deeper");
    let dot = root.join(".dot");
    std::fs::create_dir_all(&deeper).unwrap();
    std::fs::create_dir(&dot).unwrap();
    let files = [
        root.join("a.txt"),
        root.join(".hidden.txt"),
        sub.join("b.txt"),
        deeper.join("c.txt"),
        dot.join("d.txt"),
    ];
    for (index, file) in files.iter().enumerate() {
        std::fs::write(file, vec![b'x'; 100 * (index + 1)]).unwrap();
    }
    vec![
        files[0].clone(),
        files[1].clone(),
        sub,
        files[2].clone(),
        deeper,
        files[3].clone(),
        dot,
        files[4].clone(),
    ]
}

#[tokio::test]
async fn counts_everything_inside_including_folder_sizes() {
    let directory = tempfile::tempdir().unwrap();
    let entries = sample_tree(directory.path());

    let snapshot = UsageMeasurement::start(directory.path().to_path_buf(), HiddenEntries::Counted)
        .finished()
        .await;

    assert_eq!(snapshot.progress, UsageProgress::Finished);
    assert_eq!(snapshot.usage.files, 5);
    assert_eq!(snapshot.usage.folders, 3);
    assert_eq!(
        snapshot.usage.bytes,
        entries.iter().map(|path| len(path)).sum::<u64>()
    );
    assert!(snapshot.usage.disk_bytes > 0);
    assert_eq!(snapshot.usage.unreadable, 0);
}

#[tokio::test]
async fn skipped_hidden_entries_are_neither_counted_nor_entered() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    sample_tree(root);

    let snapshot = UsageMeasurement::start(root.to_path_buf(), HiddenEntries::Skipped)
        .finished()
        .await;

    assert_eq!(snapshot.usage.files, 3);
    assert_eq!(snapshot.usage.folders, 2);
    let visible = [
        root.join("a.txt"),
        root.join("sub"),
        root.join("sub/b.txt"),
        root.join("sub/deeper"),
        root.join("sub/deeper/c.txt"),
    ];
    assert_eq!(
        snapshot.usage.bytes,
        visible.iter().map(|path| len(path)).sum::<u64>()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_links_count_as_themselves_and_are_not_followed() {
    let directory = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("big.bin"), vec![0; 4096]).unwrap();
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(target.path(), &link).unwrap();

    let snapshot = UsageMeasurement::start(directory.path().to_path_buf(), HiddenEntries::Counted)
        .finished()
        .await;

    assert_eq!((snapshot.usage.files, snapshot.usage.folders), (1, 0));
    assert_eq!(snapshot.usage.bytes, len(&link));
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_subfolders_are_skipped_and_counted_but_an_unreadable_root_fails() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("blocked");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("secret.txt"), b"secret").unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // root 跑测试时读得了一切，权限挡不住，这种情况下不检查。
    let blocked_for_us = std::fs::read_dir(&blocked).is_err();

    let partial = UsageMeasurement::start(directory.path().to_path_buf(), HiddenEntries::Counted)
        .finished()
        .await;
    let failed = UsageMeasurement::start(blocked.clone(), HiddenEntries::Counted)
        .finished()
        .await;
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();

    if blocked_for_us {
        assert_eq!(partial.progress, UsageProgress::Finished);
        assert_eq!(
            (
                partial.usage.folders,
                partial.usage.files,
                partial.usage.unreadable
            ),
            (1, 0, 1)
        );
        assert!(matches!(failed.progress, UsageProgress::Failed(_)));
    }
}

#[tokio::test]
async fn a_missing_folder_fails() {
    let directory = tempfile::tempdir().unwrap();

    let snapshot = UsageMeasurement::start(directory.path().join("gone"), HiddenEntries::Counted)
        .finished()
        .await;

    assert!(matches!(snapshot.progress, UsageProgress::Failed(_)));
}

#[tokio::test]
async fn measurements_running_at_once_keep_their_own_counts() {
    let directory = tempfile::tempdir().unwrap();
    let folders = (0..24)
        .map(|index| {
            let folder = directory.path().join(format!("folder-{index}"));
            for depth in 0..=index % 5 {
                let level = (0..=depth).fold(folder.clone(), |path, level| {
                    path.join(format!("level-{level}"))
                });
                std::fs::create_dir_all(&level).unwrap();
                for file in 0..index {
                    std::fs::write(level.join(format!("{file}.bin")), vec![0; file]).unwrap();
                }
            }
            folder
        })
        .collect::<Vec<_>>();

    let measurements = folders
        .iter()
        .map(|folder| UsageMeasurement::start(folder.clone(), HiddenEntries::Counted))
        .collect::<Vec<_>>();
    for (index, measurement) in measurements.iter().enumerate() {
        let levels = (index % 5 + 1) as u64;
        let snapshot = measurement.finished().await;
        assert_eq!(snapshot.progress, UsageProgress::Finished);
        assert_eq!(snapshot.usage.folders, levels, "folder-{index}");
        assert_eq!(
            snapshot.usage.files,
            levels * index as u64,
            "folder-{index}"
        );
    }
}

#[tokio::test]
async fn dropping_a_measurement_leaves_the_workers_free_for_the_next() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..200 {
        let folder = directory.path().join(format!("{index}"));
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("file"), b"x").unwrap();
    }
    drop(UsageMeasurement::start(
        directory.path().to_path_buf(),
        HiddenEntries::Counted,
    ));

    let small = tempfile::tempdir().unwrap();
    std::fs::write(small.path().join("only"), b"1234").unwrap();
    let snapshot = UsageMeasurement::start(small.path().to_path_buf(), HiddenEntries::Counted)
        .finished()
        .await;

    assert_eq!(snapshot.usage.files, 1);
    assert_eq!(snapshot.usage.bytes, 4);
}
