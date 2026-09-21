#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use file_core::{
    delete_trash_entry, restore_trash_entry, scan_trash, trash_path_with_restore_entry,
    ScanOptions, TransferConflictStrategy, TrashCommitOutcome,
};
use tempfile::tempdir;

fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

#[tokio::test]
#[ignore = "requires FILE_MANAGER_TRASH_TEST_VOLUME on an isolated mounted filesystem"]
async fn volume_trash_round_trips_through_the_mounted_filesystem_location() {
    let volume = std::env::var_os("FILE_MANAGER_TRASH_TEST_VOLUME")
        .map(std::path::PathBuf::from)
        .expect("FILE_MANAGER_TRASH_TEST_VOLUME");
    let fixture = tempdir().unwrap();
    let data_home = fixture.path().join("data");
    let home = fixture.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::env::set_var("XDG_DATA_HOME", &data_home);
    std::env::set_var("HOME", &home);

    let source_name = OsString::from_vec(b"shared-nonutf8-\xfc.txt".to_vec());
    let home_source = fixture.path().join(&source_name);
    std::fs::write(&home_source, b"home-payload").unwrap();
    let home_entry = match trash_path_with_restore_entry(&home_source).await.unwrap() {
        TrashCommitOutcome::Tracked(entry) => entry,
        TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            panic!("Home Trash entry was not tracked: {}", warning.message)
        }
    };

    let source = volume.join(&source_name);
    std::fs::write(&source, b"volume-payload").unwrap();
    let entry = match trash_path_with_restore_entry(&source).await.unwrap() {
        TrashCommitOutcome::Tracked(entry) => entry,
        TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            let diagnostic_scan = scan_trash(ScanOptions::default()).await;
            panic!(
                "volume Trash entry was not tracked: {}; scan: {diagnostic_scan:#?}",
                warning.message
            )
        }
    };
    assert!(entry.trash_path.starts_with(&volume));
    assert!(entry.has_verified_identity());
    let scan = scan_trash(ScanOptions::default()).await.unwrap();
    assert!(scan
        .entries
        .iter()
        .any(|candidate| path_bytes(&candidate.original_path) == path_bytes(&source)));
    assert!(scan
        .entries
        .iter()
        .any(|candidate| path_bytes(&candidate.original_path) == path_bytes(&home_source)));

    let restored = restore_trash_entry(*entry, TransferConflictStrategy::Fail)
        .await
        .unwrap();
    assert_eq!(path_bytes(&restored), path_bytes(&source));
    assert_eq!(std::fs::read(&restored).unwrap(), b"volume-payload");
    delete_trash_entry(*home_entry).await.unwrap();
    assert!(!home_source.exists());
    assert_eq!(std::fs::read(&restored).unwrap(), b"volume-payload");

    let entry = match trash_path_with_restore_entry(&restored).await.unwrap() {
        TrashCommitOutcome::Tracked(entry) => entry,
        TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            panic!(
                "second volume Trash entry was not tracked: {}",
                warning.message
            )
        }
    };
    delete_trash_entry(*entry).await.unwrap();
    assert!(!restored.exists());
    assert!(scan_trash(ScanOptions::default())
        .await
        .unwrap()
        .entries
        .is_empty());
}

#[tokio::test]
async fn home_trash_round_trips_non_utf8_file_and_directory_names() {
    let fixture = tempdir().unwrap();
    let data_home = fixture.path().join("data");
    let home = fixture.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::env::set_var("XDG_DATA_HOME", &data_home);
    std::env::set_var("HOME", &home);
    let trash_root = data_home.join("Trash");
    std::fs::create_dir_all(trash_root.join("files")).unwrap();
    std::fs::create_dir_all(trash_root.join("info")).unwrap();
    let broken_info = trash_root.join("info").join("unrelated-broken.trashinfo");
    std::fs::write(&broken_info, b"not a trashinfo document").unwrap();

    let file_name = OsString::from_vec(b"nonutf8-file-\xff.txt".to_vec());
    let file_path = fixture.path().join(&file_name);
    std::fs::write(&file_path, b"file-payload").unwrap();
    let file_entry = match trash_path_with_restore_entry(&file_path).await.unwrap() {
        TrashCommitOutcome::Tracked(entry) => entry,
        TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            panic!("home Trash entry was not tracked: {}", warning.message)
        }
    };
    assert!(!file_path.exists());
    assert!(file_entry.has_verified_identity());
    let scan = scan_trash(ScanOptions::default()).await.unwrap();
    assert!(scan
        .entries
        .iter()
        .any(|entry| path_bytes(&entry.original_path) == path_bytes(&file_path)));
    assert!(scan
        .skipped
        .iter()
        .any(|warning| warning.path == broken_info));
    std::fs::remove_file(&broken_info).unwrap();

    let restored = restore_trash_entry(*file_entry, TransferConflictStrategy::Fail)
        .await
        .unwrap();
    assert_eq!(path_bytes(&restored), path_bytes(&file_path));
    assert_eq!(std::fs::read(&restored).unwrap(), b"file-payload");

    let hidden_path = fixture.path().join(".hidden-trash-entry");
    std::fs::write(&hidden_path, b"hidden").unwrap();
    let hidden_entry = match trash_path_with_restore_entry(&hidden_path).await.unwrap() {
        TrashCommitOutcome::Tracked(entry) => entry,
        TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            panic!("hidden Trash entry was not tracked: {}", warning.message)
        }
    };
    assert!(!scan_trash(ScanOptions::default())
        .await
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.original_path == hidden_path));
    assert!(scan_trash(ScanOptions {
        include_hidden: true,
        ..ScanOptions::default()
    })
    .await
    .unwrap()
    .entries
    .iter()
    .any(|entry| entry.original_path == hidden_path));
    delete_trash_entry(*hidden_entry).await.unwrap();

    let directory_name = OsString::from_vec(b"nonutf8-directory-\xfe".to_vec());
    let directory_path = fixture.path().join(&directory_name);
    std::fs::create_dir(&directory_path).unwrap();
    let child_name = OsString::from_vec(b"child-\xfd".to_vec());
    std::fs::write(directory_path.join(child_name), b"directory-payload").unwrap();
    let directory_entry = match trash_path_with_restore_entry(&directory_path)
        .await
        .unwrap()
    {
        TrashCommitOutcome::Tracked(entry) => entry,
        TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            panic!("directory Trash entry was not tracked: {}", warning.message)
        }
    };
    delete_trash_entry(*directory_entry).await.unwrap();
    assert!(!directory_path.exists());

    let final_path = fixture.path().join("empty-trash-check");
    std::fs::write(&final_path, b"final").unwrap();
    let final_entry = match trash_path_with_restore_entry(&final_path).await.unwrap() {
        TrashCommitOutcome::Tracked(entry) => entry,
        TrashCommitOutcome::CommittedWithoutRestoreEntry(warning) => {
            panic!("final Trash entry was not tracked: {}", warning.message)
        }
    };
    delete_trash_entry(*final_entry).await.unwrap();
    assert!(!scan_trash(ScanOptions::default())
        .await
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.original_path == final_path));
}
