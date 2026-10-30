use super::*;
use std::fs;
use tempfile::tempdir;

use super::super::entry_moves::ObjectIdentity;
use super::super::recoverable_transfer::NoReplaceRenameError;
use crate::entry_naming::InvalidEntryName;
use crate::ops::fake_entry_filesystem::{fake_path, is_temporary_name, FakeDirectory};

fn item(from: &Path, to: &Path) -> BatchRenameItem {
    BatchRenameItem {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    }
}

fn fake_item(from: &str, to: &str) -> BatchRenameItem {
    item(&fake_path(from), &fake_path(to))
}

/// a(对象 1) → x、b(对象 2) → y。
fn two_item_directory() -> (FakeDirectory, Vec<BatchRenameItem>) {
    let directory = FakeDirectory::new(&[("a", 1), ("b", 2)]);
    (directory, vec![fake_item("a", "x"), fake_item("b", "y")])
}

fn assert_failed_rename(
    cause: &FileError,
    from: impl Fn(&Path) -> bool,
    to: impl Fn(&Path) -> bool,
) {
    match cause {
        FileError::Rename {
            from: cause_from,
            to: cause_to,
            ..
        } => {
            assert!(from(cause_from), "unexpected source {cause_from:?}");
            assert!(to(cause_to), "unexpected target {cause_to:?}");
        }
        other => panic!("unexpected cause: {other:?}"),
    }
}

fn is_y(path: &Path) -> bool {
    path == fake_path("y")
}

fn incomplete(error: &FileError) -> (&FileError, &[UnrestoredEntry]) {
    match error {
        FileError::EntryMovesIncomplete { cause, unrestored } => (cause, unrestored),
        other => panic!("expected an incomplete rollback, got {other:?}"),
    }
}

#[test]
fn swapped_names_go_through_temporary_names() {
    let mut directory = FakeDirectory::new(&[("a", 1), ("b", 2)]);

    let completed = run_batch_rename(
        &mut directory,
        vec![fake_item("a", "b"), fake_item("b", "a")],
    )
    .unwrap();

    assert_eq!(completed.len(), 2);
    assert_eq!(directory.names(), ["a", "b"]);
    assert_eq!(directory.object_at("a"), Some(2));
    assert_eq!(directory.object_at("b"), Some(1));
}

#[test]
fn case_only_rename_works_on_a_case_insensitive_filesystem() {
    let mut directory = FakeDirectory::new(&[("a.txt", 1), ("b.txt", 2)]).case_insensitive();

    run_batch_rename(
        &mut directory,
        vec![fake_item("a.txt", "A.txt"), fake_item("b.txt", "B.txt")],
    )
    .unwrap();

    assert_eq!(directory.names(), ["A.txt", "B.txt"]);
}

#[test]
fn target_held_by_an_entry_outside_the_batch_is_rejected_before_any_rename() {
    let mut directory = FakeDirectory::new(&[("a", 1), ("b", 2), ("y", 9)]);
    let (_, items) = two_item_directory();

    let error = run_batch_rename(&mut directory, items).unwrap_err();

    assert!(matches!(error, FileError::EntryNameTaken { path } if path == fake_path("y")));
    assert_eq!(directory.names(), ["a", "b", "y"]);
}

#[test]
fn another_hard_link_name_is_reported_as_taken_and_rolled_back() {
    let mut directory = FakeDirectory::new(&[("a", 1), ("b", 1)]);

    let error = run_batch_rename(&mut directory, vec![fake_item("a", "b")]).unwrap_err();

    assert!(matches!(error, FileError::EntryNameTaken { path } if path == fake_path("b")));
    assert_eq!(directory.names(), ["a", "b"]);
}

#[test]
fn illegal_target_names_are_rejected_before_any_rename() {
    for (target, expected) in [
        ("..", InvalidEntryName::ParentDirectory),
        ("nul\0", InvalidEntryName::ContainsNul),
    ] {
        let (mut directory, _) = two_item_directory();

        let error = run_batch_rename(&mut directory, vec![fake_item("a", target)]).unwrap_err();

        assert!(
            matches!(error, FileError::InvalidEntryName { reason, .. } if reason == expected),
            "{target:?}: {error:?}"
        );
        assert_eq!(directory.names(), ["a", "b"]);
    }
}

#[test]
fn unsupported_no_replace_fails_without_renaming_anything() {
    let (mut directory, items) = two_item_directory();
    directory.unsupported = true;

    let error = run_batch_rename(&mut directory, items).unwrap_err();

    assert!(matches!(error, FileError::SafeRenameUnsupported { .. }));
    assert_eq!(directory.names(), ["a", "b"]);
}

#[test]
fn fully_rolled_back_failure_reports_only_the_original_cause() {
    let (mut directory, items) = two_item_directory();
    directory.fail_when = Box::new(|_, to| to == fake_path("y"));

    let error = run_batch_rename(&mut directory, items).unwrap_err();

    assert_failed_rename(&error, is_temporary_name, is_y);
    assert_eq!(directory.names(), ["a", "b"]);
    assert_eq!(directory.object_at("a"), Some(1));
    assert_eq!(directory.object_at("b"), Some(2));
}

#[test]
fn first_phase_rollback_failure_reports_item_left_at_temporary_name() {
    let (mut directory, items) = two_item_directory();
    directory.fail_when = Box::new(|from, to| from == fake_path("b") || to == fake_path("a"));

    let error = run_batch_rename(&mut directory, items).unwrap_err();

    let (cause, unrestored) = incomplete(&error);
    assert_failed_rename(cause, |from| from == fake_path("b"), is_temporary_name);
    assert_eq!(unrestored.len(), 1);
    assert_eq!(unrestored[0].original, fake_path("a"));
    assert!(is_temporary_name(&unrestored[0].current));
    let left_at = unrestored[0].current.file_name().unwrap().to_str().unwrap();
    assert_eq!(directory.object_at(left_at), Some(1));
    assert_eq!(directory.object_at("b"), Some(2));
}

#[test]
fn second_phase_park_failure_reports_item_left_at_target_name() {
    let (mut directory, items) = two_item_directory();
    directory.fail_when = Box::new(|from, to| to == fake_path("y") || from == fake_path("x"));

    let error = run_batch_rename(&mut directory, items).unwrap_err();

    let (cause, unrestored) = incomplete(&error);
    assert_failed_rename(cause, is_temporary_name, is_y);
    assert_eq!(unrestored.len(), 1);
    assert_eq!(unrestored[0].original, fake_path("a"));
    assert_eq!(unrestored[0].current, fake_path("x"));
    assert_eq!(directory.object_at("x"), Some(1));
    assert_eq!(directory.object_at("b"), Some(2));
}

#[test]
fn second_phase_restore_failure_reports_item_left_at_hidden_temporary_name() {
    let (mut directory, items) = two_item_directory();
    directory.fail_when = Box::new(|_, to| to == fake_path("y") || to == fake_path("b"));

    let error = run_batch_rename(&mut directory, items).unwrap_err();

    let (cause, unrestored) = incomplete(&error);
    assert_failed_rename(cause, is_temporary_name, is_y);
    assert_eq!(unrestored.len(), 1);
    assert_eq!(unrestored[0].original, fake_path("b"));
    assert!(is_temporary_name(&unrestored[0].current));
    let left_at = unrestored[0].current.file_name().unwrap().to_str().unwrap();
    assert_eq!(directory.object_at(left_at), Some(2));
    assert_eq!(directory.object_at("a"), Some(1));

    let message = error.to_string();
    assert!(message.starts_with(&cause.to_string()));
    assert!(message.contains(&format!(
        "{:?} -> {:?}",
        unrestored[0].original, unrestored[0].current
    )));
}

/// 真实文件系统,在第一次改名成功后于 `intruder` 处放一个文件,模拟执行期间
/// 别的程序新建了同名条目。
struct IntruderAfterFirstRename {
    intruder: PathBuf,
    placed: bool,
}

impl EntryFilesystem for IntruderAfterFirstRename {
    fn rename_noreplace(&mut self, from: &Path, to: &Path) -> Result<(), NoReplaceRenameError> {
        SystemEntryFilesystem.rename_noreplace(from, to)?;
        if !self.placed {
            fs::write(&self.intruder, b"intruder").unwrap();
            self.placed = true;
        }
        Ok(())
    }

    fn object_identity(&mut self, path: &Path) -> io::Result<ObjectIdentity> {
        SystemEntryFilesystem.object_identity(path)
    }
}

fn directory_names(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn target_created_during_the_rename_is_not_replaced() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    let target = directory.path().join("x.txt");
    fs::write(&source, b"source").unwrap();
    let mut filesystem = IntruderAfterFirstRename {
        intruder: target.clone(),
        placed: false,
    };

    let error = run_batch_rename(&mut filesystem, vec![item(&source, &target)]).unwrap_err();

    assert!(matches!(error, FileError::EntryNameTaken { path } if path == target));
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&target).unwrap(), b"intruder");
    assert_eq!(directory_names(directory.path()), ["a.txt", "x.txt"]);
}

#[test]
fn rotating_names_keeps_every_file() {
    let directory = tempdir().unwrap();
    let path = |name: &str| directory.path().join(name);
    for name in ["a", "b", "c"] {
        fs::write(path(name), name).unwrap();
    }

    run_batch_rename(
        &mut SystemEntryFilesystem,
        vec![
            item(&path("a"), &path("b")),
            item(&path("b"), &path("c")),
            item(&path("c"), &path("a")),
        ],
    )
    .unwrap();

    assert_eq!(fs::read(path("b")).unwrap(), b"a");
    assert_eq!(fs::read(path("c")).unwrap(), b"b");
    assert_eq!(fs::read(path("a")).unwrap(), b"c");
    assert_eq!(directory_names(directory.path()), ["a", "b", "c"]);
}

#[test]
fn renaming_onto_an_existing_file_outside_the_batch_changes_nothing() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("a.txt");
    let target = directory.path().join("b.txt");
    fs::write(&source, b"source").unwrap();
    fs::write(&target, b"target").unwrap();

    let error =
        run_batch_rename(&mut SystemEntryFilesystem, vec![item(&source, &target)]).unwrap_err();

    assert!(matches!(error, FileError::EntryNameTaken { .. }));
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&target).unwrap(), b"target");
}
