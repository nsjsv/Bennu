use std::path::Path;

use file_core::EntryMetadata;

use super::*;

fn entry(path: &Path, kind: FileKind, len: u64) -> DirectoryEntry {
    let metadata = EntryMetadata {
        len,
        ..EntryMetadata::default()
    };
    DirectoryEntry::new(path.to_path_buf(), kind, metadata, false, false, false)
}

fn file(path: &Path, len: u64) -> DirectoryEntry {
    entry(path, FileKind::File, len)
}

fn folder(path: &Path) -> DirectoryEntry {
    entry(path, FileKind::Directory, 10)
}

fn begun(root: &Path, listing: Vec<DirectoryEntry>, expand_levels: u8) -> FolderPreview {
    let mut preview = FolderPreview::new(root.to_path_buf(), listing, ScanOptions::default());
    drop(preview.begin(1, expand_levels));
    preview
}

fn state<'a>(preview: &'a mut FolderPreview, path: &Path) -> &'a mut FolderState {
    preview.folder_state_mut(path).expect("folder node")
}

fn finished(files: u64, bytes: u64) -> UsageSnapshot {
    UsageSnapshot {
        usage: DirectoryUsage {
            files,
            bytes,
            ..DirectoryUsage::default()
        },
        progress: UsageProgress::Finished,
    }
}

/// 自动展开到设置的层数，再往下的文件夹自己测；同时最多读 4 个，其余排队。
#[test]
fn shallow_folders_expand_in_a_queue_and_deeper_ones_are_measured() {
    let root = Path::new("/root");
    let listing = (0..10)
        .map(|index| folder(&root.join(format!("folder-{index}"))))
        .collect();

    let mut preview = begun(root, listing, 1);

    assert_eq!(preview.reading.len(), LISTINGS_AT_ONCE);
    assert_eq!(preview.waiting.len(), 10 - LISTINGS_AT_ONCE);
    let first = root.join("folder-0");
    assert!(state(&mut preview, &first).expanded);
    assert!(state(&mut preview, &first).measurement.is_none());

    drop(preview.accept_listing(1, first.clone(), Ok(vec![folder(&first.join("deep"))])));

    // 读完一个，补上一个。
    assert_eq!(preview.reading.len(), LISTINGS_AT_ONCE);
    assert_eq!(preview.waiting.len(), 10 - LISTINGS_AT_ONCE - 1);
    let deep = state(&mut preview, &first.join("deep"));
    assert!(!deep.expanded);
    assert!(deep.measurement.is_some());
}

#[test]
fn without_auto_expansion_every_folder_measures_itself() {
    let root = Path::new("/root");
    let mut preview = begun(
        root,
        vec![folder(&root.join("a")), file(&root.join("b"), 3)],
        0,
    );

    assert!(preview.reading.is_empty());
    assert!(!state(&mut preview, &root.join("a")).expanded);
    assert!(state(&mut preview, &root.join("a")).measurement.is_some());
    assert!(preview.is_measuring());
}

/// 列出过内容的文件夹由孩子推出大小；没列出的用自己的测量；总计是根的推出值。
#[test]
fn listed_folders_add_up_their_children_and_the_summary_adds_up_the_root() {
    let root = Path::new("/root");
    let src = root.join("src");
    let mut preview = begun(root, vec![folder(&src), file(&root.join("README"), 7)], 1);
    drop(preview.accept_listing(
        1,
        src.clone(),
        Ok(vec![
            file(&src.join("main.rs"), 100),
            folder(&src.join("app")),
        ]),
    ));
    assert!(!preview.summary().settled);

    let app = state(&mut preview, &src.join("app"));
    app.measurement = None;
    app.measured = Some(finished(2, 50));
    preview.refresh_usage();

    let src_usage = state(&mut preview, &src).usage;
    assert!(src_usage.settled);
    // main.rs 100 + app 自己 10 + app 里面 50
    assert_eq!(src_usage.usage.bytes, 160);
    assert_eq!((src_usage.usage.files, src_usage.usage.folders), (3, 1));
    let summary = preview.summary();
    assert!(summary.settled);
    // README 7 + src 自己 10 + src 里面 160
    assert_eq!(summary.usage.bytes, 177);
    assert_eq!((summary.usage.files, summary.usage.folders), (4, 2));
}

/// 第一次列出内容时自己的测量停掉（孩子各自测，同一棵子树不走两遍）；已经算完的
/// 留着，孩子没算完前先显示它。
#[test]
fn listing_a_folder_stops_its_own_measurement_but_keeps_a_finished_one() {
    let root = Path::new("/root");
    let running = root.join("running");
    let done = root.join("done");
    let mut preview = begun(root, vec![folder(&running), folder(&done)], 0);
    {
        let done_state = state(&mut preview, &done);
        done_state.measurement = None;
        done_state.measured = Some(finished(9, 900));
    }

    drop(preview.toggle(&running));
    drop(preview.toggle(&done));
    drop(preview.accept_listing(1, running.clone(), Ok(vec![folder(&running.join("x"))])));
    drop(preview.accept_listing(1, done.clone(), Ok(vec![folder(&done.join("y"))])));

    let running_state = state(&mut preview, &running);
    assert!(running_state.measurement.is_none());
    assert!(!running_state.usage.settled);
    let done_state = state(&mut preview, &done);
    assert!(done_state.usage.settled);
    assert_eq!(done_state.usage.usage.bytes, 900);
}

/// 重读对账：还在的保留展开和子树，没了的删掉，新的加进来并起步，大小跟着更新。
#[test]
fn rereading_keeps_what_is_still_there_and_adds_or_drops_the_rest() {
    let root = Path::new("/root");
    let src = root.join("src");
    let mut preview = begun(root, vec![folder(&src), file(&root.join("old.txt"), 5)], 1);
    drop(preview.accept_listing(1, src.clone(), Ok(vec![file(&src.join("lib.rs"), 1)])));
    preview.select(src.join("lib.rs"));

    drop(preview.directory_changed(1, root.to_path_buf()));
    drop(preview.accept_listing(
        1,
        root.to_path_buf(),
        Ok(vec![
            folder(&src),
            folder(&root.join("new")),
            file(&root.join("new.txt"), 8),
        ]),
    ));

    let names = preview
        .entries()
        .iter()
        .map(|node| node.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["src", "new", "new.txt"]);
    let src_state = state(&mut preview, &src);
    assert!(src_state.expanded);
    assert!(matches!(&src_state.listing, FolderListing::Loaded(children) if children.len() == 1));
    // 新文件夹在第 0 层，按设置自动展开、排队读。
    assert!(state(&mut preview, &root.join("new")).expanded);
    assert_eq!(preview.selected(), Some(src.join("lib.rs").as_path()));
    assert_eq!(
        preview.watched_directories(),
        vec![root.to_path_buf(), src.clone()]
    );
}

#[test]
fn replies_from_an_older_session_are_ignored() {
    let root = Path::new("/root");
    let src = root.join("src");
    let mut preview = begun(root, vec![folder(&src)], 1);

    drop(preview.accept_listing(0, src.clone(), Ok(vec![file(&src.join("a"), 1)])));
    drop(preview.directory_changed(0, root.to_path_buf()));

    assert!(matches!(
        state(&mut preview, &src).listing,
        FolderListing::Waiting
    ));
    assert!(!preview.waiting.contains(&root.to_path_buf()));
}

#[test]
fn an_unreadable_folder_shows_a_status_row_and_marks_the_total_incomplete() {
    let root = Path::new("/root");
    let locked = root.join("locked");
    let mut preview = begun(root, vec![folder(&locked)], 1);

    drop(preview.accept_listing(1, locked.clone(), Err("Permission denied".to_owned())));

    assert!(matches!(
        preview.visible_rows()[1],
        FolderRow::Failed {
            depth: 1,
            error: "Permission denied"
        }
    ));
    let summary = preview.summary();
    assert!(summary.settled);
    assert_eq!(summary.usage.unreadable, 1);
}

#[test]
fn arrow_keys_walk_the_visible_rows_and_open_reports_the_entry() {
    let root = Path::new("/root");
    let src = root.join("src");
    let mut preview = begun(root, vec![folder(&src), file(&root.join("z.txt"), 1)], 1);
    drop(preview.accept_listing(1, src.clone(), Ok(vec![file(&src.join("lib.rs"), 1)])));

    // 没选中时先选第一行。
    assert_eq!(preview.press_key(FolderKey::Down).selected_row, Some(0));
    // 展开的文件夹按 → 进到第一个孩子。
    drop(preview.press_key(FolderKey::Expand));
    assert_eq!(preview.selected(), Some(src.join("lib.rs").as_path()));
    // 文件上按 ← 回到上一层那行；再按 ← 收起。
    drop(preview.press_key(FolderKey::Collapse));
    assert_eq!(preview.selected(), Some(src.as_path()));
    drop(preview.press_key(FolderKey::Collapse));
    assert!(!state(&mut preview, &src).expanded);
    // 收起后 ↓ 直接到下一项，到底不动。
    drop(preview.press_key(FolderKey::Down));
    let outcome = preview.press_key(FolderKey::Down);
    assert_eq!(outcome.selected_row, Some(1));
    let opened = preview.press_key(FolderKey::Open).open;
    assert_eq!(opened, Some((root.join("z.txt"), FileKind::File)));
    // 收起的文件夹按 → 展开。
    drop(preview.press_key(FolderKey::Up));
    drop(preview.press_key(FolderKey::Expand));
    assert!(state(&mut preview, &src).expanded);
}

/// 隐藏文件没显示时，测量也不算隐藏的，和看得见的对得上。
#[tokio::test]
async fn measurements_follow_the_hidden_files_setting() {
    let directory = tempfile::tempdir().unwrap();
    let sub = directory.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("seen"), b"12").unwrap();
    std::fs::write(sub.join(".unseen"), b"3456").unwrap();
    let listing = vec![folder(&sub)];

    for (include_hidden, expected_files) in [(false, 1), (true, 2)] {
        let options = ScanOptions {
            include_hidden,
            ..ScanOptions::default()
        };
        let mut preview =
            FolderPreview::new(directory.path().to_path_buf(), listing.clone(), options);
        drop(preview.begin(1, 0));
        let measurement = state(&mut preview, &sub)
            .measurement
            .clone()
            .expect("measurement");
        measurement.finished().await;
        preview.poll_measurements();

        let usage = state(&mut preview, &sub).usage;
        assert!(usage.settled);
        assert_eq!(
            usage.usage.files, expected_files,
            "include_hidden={include_hidden}"
        );
        assert!(!preview.is_measuring());
    }
}
