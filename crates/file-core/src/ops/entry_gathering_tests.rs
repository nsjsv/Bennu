use std::fs;

use tempfile::tempdir;

use super::*;
use crate::ops::entry_moves::{EntryFilesystem, ObjectIdentity};
use crate::ops::recoverable_transfer::NoReplaceRenameError;

type RenameFailure = Box<dyn Fn(&Path, &Path) -> bool + Send>;
type RemoveDirFailure = Box<dyn Fn(&Path) -> bool + Send>;

/// 真实文件系统，只有谓词命中的改名或删目录返回注入的失败。
struct InjectedFailures {
    fail_rename: RenameFailure,
    fail_remove_dir: RemoveDirFailure,
}

impl InjectedFailures {
    fn none() -> Self {
        Self {
            fail_rename: Box::new(|_, _| false),
            fail_remove_dir: Box::new(|_| false),
        }
    }

    fn renaming_fails_when(fail_rename: impl Fn(&Path, &Path) -> bool + Send + 'static) -> Self {
        Self {
            fail_rename: Box::new(fail_rename),
            ..Self::none()
        }
    }
}

fn injected_error() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "injected failure")
}

impl EntryFilesystem for InjectedFailures {
    fn rename_noreplace(&mut self, from: &Path, to: &Path) -> Result<(), NoReplaceRenameError> {
        if (self.fail_rename)(from, to) {
            return Err(NoReplaceRenameError::Failed(injected_error()));
        }
        SystemEntryFilesystem.rename_noreplace(from, to)
    }

    fn object_identity(&mut self, path: &Path) -> io::Result<ObjectIdentity> {
        SystemEntryFilesystem.object_identity(path)
    }
}

impl EntryTreeFilesystem for InjectedFailures {
    fn create_dir(&mut self, path: &Path) -> io::Result<()> {
        SystemEntryFilesystem.create_dir(path)
    }

    fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        if (self.fail_remove_dir)(path) {
            return Err(injected_error());
        }
        SystemEntryFilesystem.remove_dir(path)
    }

    fn is_directory(&mut self, path: &Path) -> io::Result<bool> {
        SystemEntryFilesystem.is_directory(path)
    }
}

/// 第 `cancel_before` 步（从 0 数）开始前返回取消；记录每次上报的进度。
#[derive(Default)]
struct ScriptedPacer {
    steps_started: usize,
    cancel_before: Option<usize>,
    progress: Vec<(usize, usize)>,
}

impl ScriptedPacer {
    fn cancelling_before(step: usize) -> Self {
        Self {
            cancel_before: Some(step),
            ..Self::default()
        }
    }
}

impl EntryStepPacer for ScriptedPacer {
    fn before_step(&mut self) -> impl Future<Output = Result<(), FileError>> + Send {
        let cancel = self.cancel_before == Some(self.steps_started);
        self.steps_started += 1;
        async move {
            if cancel {
                Err(FileError::Cancelled)
            } else {
                Ok(())
            }
        }
    }

    fn step_finished(&mut self, completed: usize, total: usize) -> impl Future<Output = ()> + Send {
        self.progress.push((completed, total));
        async {}
    }
}

/// 目录下的全部相对路径（含子目录内容），排序后比较整棵树。
fn tree(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            paths.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() && !path.is_symlink() {
                pending.push(path);
            }
        }
    }
    paths.sort();
    paths
}

/// a.txt 与目录 b（内含 child.txt）。
fn two_sources(root: &Path) -> Vec<PathBuf> {
    fs::write(root.join("a.txt"), b"a").unwrap();
    fs::create_dir(root.join("b")).unwrap();
    fs::write(root.join("b/child.txt"), b"child").unwrap();
    vec![root.join("a.txt"), root.join("b")]
}

fn incomplete(error: &FileError) -> (&FileError, &[UnrestoredEntry], &[LeftoverDirectory]) {
    match error {
        FileError::EntryMovesIncomplete {
            cause,
            unrestored,
            leftover_directories,
        } => (cause, unrestored, leftover_directories),
        other => panic!("expected an incomplete rollback, got {other:?}"),
    }
}

#[tokio::test]
async fn gather_moves_every_source_into_a_new_folder() {
    let root = tempdir().unwrap();
    let sources = two_sources(root.path());
    let mut pacer = ScriptedPacer::default();

    let gathered = gather_entries_with(
        &mut InjectedFailures::none(),
        &root.path().join("folder"),
        &sources,
        &mut pacer,
    )
    .await
    .unwrap();

    assert_eq!(gathered.directory, root.path().join("folder"));
    assert_eq!(gathered.moved.len(), 2);
    assert_eq!(
        tree(root.path()),
        ["folder", "folder/a.txt", "folder/b", "folder/b/child.txt"]
    );
    assert_eq!(pacer.progress, [(1, 2), (2, 2)]);
}

#[tokio::test]
async fn gather_numbers_the_folder_when_the_name_is_taken() {
    let root = tempdir().unwrap();
    let sources = two_sources(root.path());
    fs::create_dir(root.path().join("folder")).unwrap();

    let gathered = gather_entries_with(
        &mut InjectedFailures::none(),
        &root.path().join("folder"),
        &sources,
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap();

    assert_eq!(gathered.directory, root.path().join("folder 2"));
    assert!(fs::read_dir(root.path().join("folder"))
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn gather_returns_to_the_original_state_when_a_move_fails() {
    let root = tempdir().unwrap();
    let sources = two_sources(root.path());
    let before = tree(root.path());
    let failing_source = root.path().join("b");
    let mut filesystem =
        InjectedFailures::renaming_fails_when(move |from, _| from == failing_source);

    let error = gather_entries_with(
        &mut filesystem,
        &root.path().join("folder"),
        &sources,
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::Rename { .. }), "{error:?}");
    assert_eq!(tree(root.path()), before);
}

#[tokio::test]
async fn gather_returns_to_the_original_state_when_cancelled_midway() {
    let root = tempdir().unwrap();
    let sources = two_sources(root.path());
    let before = tree(root.path());
    // 第 0 步建目录，第 1 步挪 a.txt，第 2 步挪 b 之前取消。
    let mut pacer = ScriptedPacer::cancelling_before(2);

    let error = gather_entries_with(
        &mut InjectedFailures::none(),
        &root.path().join("folder"),
        &sources,
        &mut pacer,
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
    assert_eq!(tree(root.path()), before);
}

#[tokio::test]
async fn gather_reports_an_item_it_could_not_move_back() {
    let root = tempdir().unwrap();
    let sources = two_sources(root.path());
    let (failing_source, original_a) = (root.path().join("b"), root.path().join("a.txt"));
    let mut filesystem = InjectedFailures::renaming_fails_when(move |from, to| {
        from == failing_source || to == original_a
    });

    let error = gather_entries_with(
        &mut filesystem,
        &root.path().join("folder"),
        &sources,
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    let (cause, unrestored, leftover_directories) = incomplete(&error);
    assert!(matches!(cause, FileError::Rename { .. }));
    assert_eq!(unrestored.len(), 1);
    assert_eq!(unrestored[0].original, root.path().join("a.txt"));
    assert_eq!(unrestored[0].current, root.path().join("folder/a.txt"));
    // 目录里还有没挪回的条目，只能留下；它的位置已经由 unrestored 说明。
    assert!(leftover_directories.is_empty());
    assert_eq!(
        tree(root.path()),
        ["b", "b/child.txt", "folder", "folder/a.txt"]
    );
}

#[tokio::test]
async fn gather_reports_a_new_folder_it_could_not_remove() {
    let root = tempdir().unwrap();
    let sources = two_sources(root.path());
    let failing_source = root.path().join("b");
    let mut filesystem = InjectedFailures {
        fail_rename: Box::new(move |from, _| from == failing_source),
        fail_remove_dir: Box::new(|_| true),
    };

    let error = gather_entries_with(
        &mut filesystem,
        &root.path().join("folder"),
        &sources,
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    let (_, unrestored, leftover_directories) = incomplete(&error);
    assert!(unrestored.is_empty());
    assert_eq!(leftover_directories.len(), 1);
    assert_eq!(leftover_directories[0].path, root.path().join("folder"));
    assert!(error.to_string().contains("could not remove new folders"));
}

#[tokio::test]
async fn tree_reuses_existing_intermediate_directories_and_records_only_new_ones() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("x")).unwrap();

    let created = create_directory_tree_with(
        &mut InjectedFailures::none(),
        root.path(),
        &["x/y".to_owned(), " p / q ".to_owned()],
        &[],
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap();

    assert_eq!(
        created.leaves,
        [root.path().join("x/y"), root.path().join("p/q")]
    );
    assert_eq!(
        created.created,
        [
            root.path().join("x/y"),
            root.path().join("p"),
            root.path().join("p/q")
        ]
    );
}

#[tokio::test]
async fn tree_moves_items_into_the_leaf_it_actually_created() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("a")).unwrap();
    fs::write(root.path().join("a/existing.txt"), b"keep").unwrap();
    fs::write(root.path().join("f.txt"), b"f").unwrap();
    let mut pacer = ScriptedPacer::default();

    let created = create_directory_tree_with(
        &mut InjectedFailures::none(),
        root.path(),
        &["a".to_owned()],
        &[root.path().join("f.txt")],
        &mut pacer,
    )
    .await
    .unwrap();

    assert_eq!(created.leaves, [root.path().join("a 2")]);
    assert_eq!(
        tree(root.path()),
        ["a", "a 2", "a 2/f.txt", "a/existing.txt"]
    );
    assert_eq!(pacer.progress, [(1, 2), (2, 2)]);
}

#[tokio::test]
async fn tree_removes_everything_it_created_when_a_move_fails() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("f.txt"), b"f").unwrap();
    let before = tree(root.path());
    let failing_source = root.path().join("f.txt");
    let mut filesystem =
        InjectedFailures::renaming_fails_when(move |from, _| from == failing_source);

    let error = create_directory_tree_with(
        &mut filesystem,
        root.path(),
        &["p/q".to_owned(), "r".to_owned()],
        &[root.path().join("f.txt")],
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::Rename { .. }), "{error:?}");
    assert_eq!(tree(root.path()), before);
}

#[tokio::test]
async fn tree_rejects_dot_segments_before_creating_anything() {
    let root = tempdir().unwrap();

    let error = create_directory_tree_with(
        &mut InjectedFailures::none(),
        root.path(),
        &["ok".to_owned(), "a/../b".to_owned()],
        &[],
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        FileError::InvalidEntryName {
            reason: InvalidEntryName::ParentDirectory,
            ..
        }
    ));
    assert!(tree(root.path()).is_empty());
}

#[tokio::test]
async fn tree_fails_when_an_intermediate_segment_is_a_file() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("x"), b"file").unwrap();

    let error = create_directory_tree_with(
        &mut InjectedFailures::none(),
        root.path(),
        &["new".to_owned(), "x/y".to_owned()],
        &[],
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, FileError::CreateDirectory { .. }),
        "{error:?}"
    );
    assert_eq!(tree(root.path()), ["x"]);
}

/// 收纳后的状态：folder 里有 a.txt 与 b/child.txt。
fn gathered_state(root: &Path) -> (PathBuf, Vec<PathBuf>) {
    let folder = root.join("folder");
    fs::create_dir(&folder).unwrap();
    two_sources(&folder);
    (folder, vec![root.join("a.txt"), root.join("b")])
}

#[tokio::test]
async fn restore_moves_entries_back_and_removes_the_empty_folder() {
    let root = tempdir().unwrap();
    let (folder, targets) = gathered_state(root.path());

    let restored = restore_gathered_entries_with(
        &mut InjectedFailures::none(),
        &folder,
        &targets,
        std::slice::from_ref(&folder),
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap();

    assert!(restored.kept_directories.is_empty());
    assert_eq!(tree(root.path()), ["a.txt", "b", "b/child.txt"]);
}

#[tokio::test]
async fn restore_keeps_a_folder_that_gained_new_content() {
    let root = tempdir().unwrap();
    let (folder, targets) = gathered_state(root.path());
    fs::write(folder.join("added.txt"), b"new").unwrap();

    let restored = restore_gathered_entries_with(
        &mut InjectedFailures::none(),
        &folder,
        &targets,
        std::slice::from_ref(&folder),
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap();

    assert_eq!(restored.kept_directories.len(), 1);
    assert_eq!(restored.kept_directories[0].path, folder);
    assert_eq!(
        tree(root.path()),
        ["a.txt", "b", "b/child.txt", "folder", "folder/added.txt"]
    );
}

#[tokio::test]
async fn restore_does_not_replace_an_entry_that_reappeared_at_the_original_place() {
    let root = tempdir().unwrap();
    let (folder, targets) = gathered_state(root.path());
    fs::write(root.path().join("b"), b"newcomer").unwrap();
    let before = tree(root.path());

    let error = restore_gathered_entries_with(
        &mut InjectedFailures::none(),
        &folder,
        &targets,
        std::slice::from_ref(&folder),
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(&error, FileError::EntryNameTaken { path } if *path == root.path().join("b")));
    // a.txt 已挪回，又随回滚回到 folder；先来的 b 原样保留。
    assert_eq!(tree(root.path()), before);
    assert_eq!(fs::read(root.path().join("b")).unwrap(), b"newcomer");
}

#[tokio::test]
async fn restore_removes_nested_created_directories_deepest_first() {
    let root = tempdir().unwrap();
    let outer = root.path().join("p");
    fs::create_dir(&outer).unwrap();
    let (folder, targets) = gathered_state(&outer);
    let targets = targets
        .iter()
        .map(|target| root.path().join(target.file_name().unwrap()))
        .collect::<Vec<_>>();

    let restored = restore_gathered_entries_with(
        &mut InjectedFailures::none(),
        &folder,
        &targets,
        &[outer.clone(), folder.clone()],
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap();

    assert!(restored.kept_directories.is_empty());
    assert_eq!(tree(root.path()), ["a.txt", "b", "b/child.txt"]);
}
