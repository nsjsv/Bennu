//! 待发列表与 LocalSend 窗口生命周期。

use std::path::PathBuf;

use super::*;
use crate::app::transfer::DirectSendState;

fn browser() -> FileBrowser {
    FileBrowser::new(crate::config::default_user_config()).0
}

fn staged_paths(browser: &FileBrowser) -> Vec<PathBuf> {
    browser
        .local_send_staged
        .iter()
        .map(|item| item.path.clone())
        .collect()
}

#[test]
fn staging_skips_archive_members_and_duplicates_then_opens_the_window() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("a.txt");
    std::fs::write(&file, b"hello").unwrap();
    let archive = directory.path().join("docs.zip");
    std::fs::write(&archive, b"payload").unwrap();
    let member = archive.join("photos/1.txt");
    let mut browser = browser();

    drop(browser.stage_local_send_paths(vec![file.clone(), member, file.clone()]));
    drop(browser.stage_local_send_paths(vec![file.clone(), archive.clone()]));

    // 压缩包文件本身是真文件，可以发；包里的成员不行。
    assert_eq!(staged_paths(&browser), vec![file, archive]);
    assert!(browser
        .local_send_staged
        .iter()
        .all(|item| item.size == StagedSize::Measuring));
    assert!(browser.global_error_notification.is_some());
    assert!(browser.local_send_window.is_some());
}

#[test]
fn sizes_fill_in_by_path_and_results_for_removed_items_are_dropped() {
    let mut browser = browser();
    drop(browser.stage_local_send_paths(vec![PathBuf::from("/a"), PathBuf::from("/b")]));
    let measured = StagedSize::Measured(RecursiveTotalSize {
        bytes: 7,
        complete: true,
    });

    browser.accept_staged_size(Path::new("/a"), measured.clone());
    browser.remove_staged_item(Path::new("/b"));
    browser.accept_staged_size(Path::new("/b"), measured.clone());

    assert_eq!(staged_paths(&browser), vec![PathBuf::from("/a")]);
    assert_eq!(browser.local_send_staged[0].size, measured);
}

#[test]
fn removing_clearing_and_closing_stop_the_size_measurements() {
    let mut browser = browser();
    drop(browser.stage_local_send_paths(vec![PathBuf::from("/a"), PathBuf::from("/b")]));
    let tokens: Vec<_> = browser
        .local_send_staged
        .iter()
        .map(|item| item.measuring.clone())
        .collect();

    browser.remove_staged_item(Path::new("/a"));
    assert!(tokens[0].is_cancelled());
    assert!(!tokens[1].is_cancelled());

    browser.clear_staged_items();
    assert!(tokens[1].is_cancelled());
    assert!(browser.local_send_staged.is_empty());

    drop(browser.stage_local_send_paths(vec![PathBuf::from("/c")]));
    let token = browser.local_send_staged[0].measuring.clone();
    drop(browser.close_local_send_window());
    assert!(token.is_cancelled());
    assert!(browser.local_send_staged.is_empty());
    assert_eq!(browser.local_send_window, None);
}

#[test]
fn closing_the_window_keeps_sends_that_are_still_running() {
    let mut browser = browser();
    drop(browser.stage_local_send_paths(vec![PathBuf::from("/a")]));
    browser
        .direct_sends
        .insert("phone-fp".to_owned(), DirectSendState::started());

    drop(browser.close_local_send_window());

    assert!(browser.direct_sends["phone-fp"].is_running());
}

#[test]
fn staged_size_reads_files_and_sums_folders() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("a.txt");
    std::fs::write(&file, b"hello").unwrap();
    let folder = directory.path().join("folder");
    std::fs::create_dir_all(folder.join("inner")).unwrap();
    std::fs::write(folder.join("b.txt"), b"abc").unwrap();
    std::fs::write(folder.join("inner/c.txt"), b"de").unwrap();

    let measure = |path: &Path| staged_size(path, CancellationToken::new());

    assert_eq!(
        measure(&file),
        StagedSize::Measured(RecursiveTotalSize {
            bytes: 5,
            complete: true
        })
    );
    // 文件夹与列表视图同一规则（子文件夹条目自身的大小也算进去，随文件系统而异）。
    assert_eq!(
        measure(&folder),
        StagedSize::Measured(
            read_directory_recursive_total_size(&folder, CancellationToken::new()).unwrap()
        )
    );
    assert!(matches!(
        measure(&directory.path().join("missing")),
        StagedSize::Unreadable(_)
    ));
}
