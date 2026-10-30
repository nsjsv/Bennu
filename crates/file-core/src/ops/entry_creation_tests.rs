use std::fs;

use tempfile::tempdir;

use super::*;
use crate::entry_naming::InvalidEntryName;

#[tokio::test]
async fn a_free_preferred_name_is_used_as_is() {
    let root = tempdir().unwrap();

    let created = create_directory_at_available_name(root.path().join("新建文件夹"))
        .await
        .unwrap();

    assert_eq!(created, root.path().join("新建文件夹"));
    assert!(created.is_dir());
}

#[tokio::test]
async fn a_taken_directory_name_is_numbered_without_splitting_a_dotted_name() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("data.v2")).unwrap();

    let created = create_directory_at_available_name(root.path().join("data.v2"))
        .await
        .unwrap();

    assert_eq!(created, root.path().join("data.v2 2"));
}

#[tokio::test]
async fn a_taken_file_name_is_numbered_before_its_extension() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"existing").unwrap();
    fs::write(root.path().join("note 2.txt"), b"existing").unwrap();

    let created =
        create_file_with_contents_at_available_name(root.path().join("note.txt"), b"new".to_vec())
            .await
            .unwrap();

    assert_eq!(created, root.path().join("note 3.txt"));
    assert_eq!(fs::read(&created).unwrap(), b"new");
    assert_eq!(fs::read(root.path().join("note.txt")).unwrap(), b"existing");
}

#[tokio::test]
async fn a_dangling_link_counts_as_taken() {
    let root = tempdir().unwrap();
    std::os::unix::fs::symlink("missing", root.path().join("new.txt")).unwrap();

    let created = create_empty_file_at_available_name(root.path().join("new.txt"))
        .await
        .unwrap();

    assert_eq!(created, root.path().join("new 2.txt"));
    assert_eq!(
        fs::read_link(root.path().join("new.txt")).unwrap(),
        Path::new("missing")
    );
}

#[tokio::test]
async fn an_illegal_preferred_name_is_rejected() {
    let root = tempdir().unwrap();

    let error = create_empty_file_at_available_name(root.path().join(".."))
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            FileError::InvalidEntryName {
                reason: InvalidEntryName::ParentDirectory,
                ..
            }
        ),
        "{error:?}"
    );
}
