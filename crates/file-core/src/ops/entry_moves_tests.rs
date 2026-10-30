use std::fs;

use tempfile::tempdir;

use super::*;
use crate::entry_naming::InvalidEntryName;
use crate::ops::fake_entry_filesystem::{fake_path, is_temporary_name, FakeDirectory};

#[test]
fn case_only_rename_on_a_case_insensitive_filesystem_goes_through_a_temporary_name() {
    let mut directory = FakeDirectory::new(&[("a.txt", 1)]).case_insensitive();

    let renamed = rename_entry_with(&mut directory, &fake_path("a.txt"), OsStr::new("A.txt"));

    assert_eq!(renamed.unwrap(), fake_path("A.txt"));
    assert_eq!(directory.names(), ["A.txt"]);
}

#[test]
fn a_different_object_under_the_folded_name_is_reported_as_taken() {
    let mut directory = FakeDirectory::new(&[("a.txt", 1), ("B.txt", 2)]).case_insensitive();

    let renamed = rename_entry_with(&mut directory, &fake_path("a.txt"), OsStr::new("b.txt"));

    assert!(
        matches!(renamed, Err(FileError::EntryNameTaken { path }) if path == fake_path("b.txt"))
    );
    assert_eq!(directory.names(), ["B.txt", "a.txt"]);
}

#[test]
fn another_hard_link_name_is_reported_as_taken_and_the_entry_keeps_its_name() {
    let mut directory = FakeDirectory::new(&[("a.txt", 1), ("b.txt", 1)]);

    let renamed = rename_entry_with(&mut directory, &fake_path("a.txt"), OsStr::new("b.txt"));

    assert!(matches!(renamed, Err(FileError::EntryNameTaken { .. })));
    assert_eq!(directory.names(), ["a.txt", "b.txt"]);
}

#[test]
fn unsupported_no_replace_is_reported_instead_of_falling_back() {
    let mut directory = FakeDirectory::new(&[("a.txt", 1)]);
    directory.unsupported = true;

    let renamed = rename_entry_with(&mut directory, &fake_path("a.txt"), OsStr::new("b.txt"));

    assert!(matches!(
        renamed,
        Err(FileError::SafeRenameUnsupported { .. })
    ));
    assert_eq!(directory.names(), ["a.txt"]);
}

#[test]
fn a_failed_restore_reports_where_the_entry_was_left() {
    let mut directory = FakeDirectory::new(&[("a.txt", 1), ("b.txt", 1)]);
    directory.fail_when = Box::new(|from, to| is_temporary_name(from) && to == fake_path("a.txt"));

    let renamed = rename_entry_with(&mut directory, &fake_path("a.txt"), OsStr::new("b.txt"));

    let (cause, unrestored) = match renamed {
        Err(FileError::EntryMovesIncomplete {
            cause, unrestored, ..
        }) => (cause, unrestored),
        other => panic!("expected an incomplete restore, got {other:?}"),
    };
    assert!(matches!(*cause, FileError::EntryNameTaken { .. }));
    assert_eq!(unrestored.len(), 1);
    assert_eq!(unrestored[0].original, fake_path("a.txt"));
    assert!(is_temporary_name(&unrestored[0].current));
    assert!(directory.position(&unrestored[0].current).is_some());
}

#[test]
fn rename_moves_an_entry_to_an_absent_name() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    fs::write(&source, b"content").unwrap();

    let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, OsStr::new("b.txt"));

    assert_eq!(renamed.unwrap(), directory.path().join("b.txt"));
    assert_eq!(
        fs::read(directory.path().join("b.txt")).unwrap(),
        b"content"
    );
    assert!(!source.exists());
}

#[test]
fn rename_refuses_to_replace_an_existing_file() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    let target = directory.path().join("b.txt");
    fs::write(&source, b"source").unwrap();
    fs::write(&target, b"target").unwrap();

    let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, OsStr::new("b.txt"));

    assert!(matches!(renamed, Err(FileError::EntryNameTaken { path }) if path == target));
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&target).unwrap(), b"target");
}

#[test]
fn rename_refuses_to_replace_an_empty_directory() {
    // 普通 rename(2) 会用源目录替换空目录，正是要防止的覆盖。
    let directory = tempdir().unwrap();
    let source = directory.path().join("a");
    let target = directory.path().join("b");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("child.txt"), b"child").unwrap();
    fs::create_dir(&target).unwrap();

    let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, OsStr::new("b"));

    assert!(matches!(renamed, Err(FileError::EntryNameTaken { .. })));
    assert_eq!(fs::read(source.join("child.txt")).unwrap(), b"child");
    assert!(target.is_dir());
}

#[test]
fn rename_refuses_to_replace_a_dangling_symbolic_link() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    let target = directory.path().join("link");
    fs::write(&source, b"source").unwrap();
    std::os::unix::fs::symlink("missing", &target).unwrap();

    let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, OsStr::new("link"));

    assert!(matches!(renamed, Err(FileError::EntryNameTaken { .. })));
    assert_eq!(fs::read_link(&target).unwrap(), Path::new("missing"));
    assert_eq!(fs::read(&source).unwrap(), b"source");
}

#[test]
fn rename_refuses_another_hard_link_of_the_same_file() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    let other_name = directory.path().join("b.txt");
    fs::write(&source, b"shared").unwrap();
    fs::hard_link(&source, &other_name).unwrap();

    let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, OsStr::new("b.txt"));

    assert!(matches!(renamed, Err(FileError::EntryNameTaken { .. })));
    let mut names = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt"]);
}

#[test]
fn rename_to_the_current_name_changes_nothing() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    fs::write(&source, b"content").unwrap();

    let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, OsStr::new("a.txt"));

    assert_eq!(renamed.unwrap(), source);
    assert_eq!(fs::read(&source).unwrap(), b"content");
}

#[test]
fn rename_rejects_names_that_would_leave_the_directory_or_are_illegal() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    fs::write(&source, b"content").unwrap();

    let cases = [
        ("", InvalidEntryName::Empty),
        (".", InvalidEntryName::CurrentDirectory),
        ("..", InvalidEntryName::ParentDirectory),
        ("../escape.txt", InvalidEntryName::ContainsSlash),
        ("nested/b.txt", InvalidEntryName::ContainsSlash),
        ("nul\0.txt", InvalidEntryName::ContainsNul),
    ];
    for (name, expected) in cases {
        let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, OsStr::new(name));
        assert!(
            matches!(renamed, Err(FileError::InvalidEntryName { reason, .. }) if reason == expected),
            "{name:?}"
        );
    }
    assert_eq!(fs::read(&source).unwrap(), b"content");
    assert!(!directory
        .path()
        .parent()
        .unwrap()
        .join("escape.txt")
        .exists());
}

#[test]
fn rename_keeps_non_utf8_name_bytes() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let directory = tempdir().unwrap();
    let source = directory.path().join(OsStr::from_bytes(b"caf\xe9.txt"));
    fs::write(&source, b"content").unwrap();
    let new_name = OsString::from_vec(b"na\xefve.txt".to_vec());

    let renamed = rename_entry_with(&mut SystemEntryFilesystem, &source, &new_name).unwrap();

    assert_eq!(renamed.file_name().unwrap().as_bytes(), b"na\xefve.txt");
    assert_eq!(fs::read(&renamed).unwrap(), b"content");
}
