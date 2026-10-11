use super::*;
use std::fs;
use tempfile::tempdir;

fn real_rename(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

/// 真实改名,只有 `fails` 命中的那几步返回注入的失败。
fn rename_failing_when(
    fails: impl Fn(&Path, &Path) -> bool,
) -> impl FnMut(&Path, &Path) -> io::Result<()> {
    move |from: &Path, to: &Path| {
        if fails(from, to) {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected rename failure",
            ))
        } else {
            std::fs::rename(from, to)
        }
    }
}

fn item(from: &Path, to: &Path) -> BatchRenameItem {
    BatchRenameItem {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    }
}

/// 两个条目 a→x、b→y;空目录里预留的临时名依次为 tmp0(a)、tmp1(b),
/// 第二阶段回滚为已完成条目预留的下一个临时名是 tmp2。
struct TwoItemBatch {
    _dir: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
    x: PathBuf,
    y: PathBuf,
    tmp0: PathBuf,
    tmp1: PathBuf,
}

impl TwoItemBatch {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        let batch = Self {
            a: path("a.txt"),
            b: path("b.txt"),
            x: path("x.txt"),
            y: path("y.txt"),
            tmp0: path(".bennu-batch-rename-0.tmp"),
            tmp1: path(".bennu-batch-rename-1.tmp"),
            _dir: dir,
        };
        fs::write(&batch.a, b"A").unwrap();
        fs::write(&batch.b, b"B").unwrap();
        batch
    }

    fn items(&self) -> Vec<BatchRenameItem> {
        vec![item(&self.a, &self.x), item(&self.b, &self.y)]
    }
}

fn assert_rename_cause(cause: &FileError, from: &Path, to: &Path) {
    match cause {
        FileError::Rename {
            from: cause_from,
            to: cause_to,
            ..
        } => {
            assert_eq!(cause_from, from);
            assert_eq!(cause_to, to);
        }
        other => panic!("unexpected cause: {other:?}"),
    }
}

fn unrestored_locations(unrestored: &[UnrestoredBatchRename]) -> Vec<(PathBuf, PathBuf)> {
    unrestored
        .iter()
        .map(|item| (item.original.clone(), item.current.clone()))
        .collect()
}

#[test]
fn rollback_final_batch_renames_restores_completed_and_temporary_paths() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    let final_first = dir.path().join("alpha.txt");
    let temp_second = dir.path().join(".bennu-batch-rename-1.tmp");
    fs::write(&final_first, b"one").unwrap();
    fs::write(&temp_second, b"two").unwrap();

    let unrestored = rollback_final_batch_renames(
        &[(final_first.clone(), first.clone())],
        &[
            (dir.path().join(".bennu-batch-rename-0.tmp"), first.clone()),
            (temp_second.clone(), second.clone()),
        ],
        &mut real_rename,
    );

    assert!(unrestored.is_empty());
    assert_eq!(fs::read(&first).unwrap(), b"one");
    assert_eq!(fs::read(&second).unwrap(), b"two");
    assert!(!final_first.exists());
    assert!(!temp_second.exists());
}

#[test]
fn rollback_final_batch_renames_restores_cycles_without_overwriting() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    let third = dir.path().join("third.txt");
    let temp_first = dir.path().join(".bennu-batch-rename-0.tmp");
    let temp_second = dir.path().join(".bennu-batch-rename-1.tmp");
    let temp_third = dir.path().join(".bennu-batch-rename-2.tmp");
    fs::write(&second, b"one").unwrap();
    fs::write(&third, b"two").unwrap();
    fs::write(&temp_third, b"three").unwrap();

    let unrestored = rollback_final_batch_renames(
        &[
            (second.clone(), first.clone()),
            (third.clone(), second.clone()),
        ],
        &[
            (temp_first.clone(), first.clone()),
            (temp_second.clone(), second.clone()),
            (temp_third.clone(), third.clone()),
        ],
        &mut real_rename,
    );

    assert!(unrestored.is_empty());
    assert_eq!(fs::read(&first).unwrap(), b"one");
    assert_eq!(fs::read(&second).unwrap(), b"two");
    assert_eq!(fs::read(&third).unwrap(), b"three");
    assert!(!temp_third.exists());
}

#[test]
fn fully_rolled_back_failure_reports_only_the_original_cause() {
    let batch = TwoItemBatch::new();
    let y = batch.y.clone();
    let mut rename = rename_failing_when(move |_, to| to == y);

    let error = run_batch_rename(batch.items(), &mut rename).unwrap_err();

    assert_rename_cause(&error, &batch.tmp1, &batch.y);
    assert_eq!(fs::read(&batch.a).unwrap(), b"A");
    assert_eq!(fs::read(&batch.b).unwrap(), b"B");
    assert!(!batch.x.exists());
    assert!(!batch.tmp0.exists());
    assert!(!batch.tmp1.exists());
}

#[test]
fn first_phase_rollback_failure_reports_item_left_at_temporary_name() {
    let batch = TwoItemBatch::new();
    let (b, a) = (batch.b.clone(), batch.a.clone());
    let mut rename = rename_failing_when(move |from, to| from == b || to == a);

    let error = run_batch_rename(batch.items(), &mut rename).unwrap_err();

    let FileError::BatchRenameIncomplete { cause, unrestored } = &error else {
        panic!("unexpected error: {error:?}");
    };
    assert_rename_cause(cause, &batch.b, &batch.tmp1);
    assert_eq!(
        unrestored_locations(unrestored),
        vec![(batch.a.clone(), batch.tmp0.clone())]
    );
    assert_rename_cause(&unrestored[0].error, &batch.tmp0, &batch.a);
    assert_eq!(fs::read(&batch.tmp0).unwrap(), b"A");
    assert_eq!(fs::read(&batch.b).unwrap(), b"B");
}

#[test]
fn second_phase_park_failure_reports_item_left_at_target_name() {
    let batch = TwoItemBatch::new();
    let (y, x) = (batch.y.clone(), batch.x.clone());
    let mut rename = rename_failing_when(move |from, to| to == y || from == x);

    let error = run_batch_rename(batch.items(), &mut rename).unwrap_err();

    let FileError::BatchRenameIncomplete { cause, unrestored } = &error else {
        panic!("unexpected error: {error:?}");
    };
    assert_rename_cause(cause, &batch.tmp1, &batch.y);
    assert_eq!(
        unrestored_locations(unrestored),
        vec![(batch.a.clone(), batch.x.clone())]
    );
    assert_eq!(fs::read(&batch.x).unwrap(), b"A");
    assert_eq!(fs::read(&batch.b).unwrap(), b"B");
}

#[test]
fn second_phase_restore_failure_reports_item_left_at_hidden_temporary_name() {
    let batch = TwoItemBatch::new();
    let (y, b) = (batch.y.clone(), batch.b.clone());
    let mut rename = rename_failing_when(move |_, to| to == y || to == b);

    let error = run_batch_rename(batch.items(), &mut rename).unwrap_err();

    let FileError::BatchRenameIncomplete { cause, unrestored } = &error else {
        panic!("unexpected error: {error:?}");
    };
    assert_rename_cause(cause, &batch.tmp1, &batch.y);
    assert_eq!(
        unrestored_locations(unrestored),
        vec![(batch.b.clone(), batch.tmp1.clone())]
    );
    assert_eq!(fs::read(&batch.a).unwrap(), b"A");
    assert_eq!(fs::read(&batch.tmp1).unwrap(), b"B");

    let message = error.to_string();
    assert!(message.starts_with(&cause.to_string()));
    assert!(message.contains(&format!("{:?} -> {:?}", batch.b, batch.tmp1)));
}
