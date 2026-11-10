//! 落到侧栏 LocalSend 入口：进待发列表并打开窗口，不排任何文件操作。

use std::path::PathBuf;

use super::*;
use crate::config;
use crate::model::{FileDropOrigin, FileDropTarget, InternalFileDragSnapshot};

fn staged(browser: &FileBrowser) -> Vec<PathBuf> {
    browser
        .local_send_staged
        .iter()
        .map(|item| item.path.clone())
        .collect()
}

#[test]
fn dragging_from_bennu_onto_local_send_stages_the_sources() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let source = PathBuf::from("/workspace/report.txt");

    drop(browser.dispatch_file_drop(
        FileDropOrigin::Internal(InternalFileDragSnapshot {
            source_session_id: None,
            sources: vec![source.clone()],
            bookmark_source: None,
        }),
        Some(FileDropTarget::LocalSend),
        vec![source.clone()],
    ));

    assert_eq!(staged(&browser), vec![source]);
    assert!(browser.local_send_window.is_some());
    assert!(browser.operation_queue.tasks().is_empty());
    assert!(browser.file_drop_prompt.is_none());
}

#[test]
fn dropping_files_from_other_apps_onto_local_send_stages_them() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let outside = PathBuf::from("/outside/photo.jpg");

    drop(browser.dispatch_file_drop(
        FileDropOrigin::External,
        Some(FileDropTarget::LocalSend),
        vec![outside.clone()],
    ));

    assert_eq!(staged(&browser), vec![outside]);
    assert!(browser.file_drop_prompt.is_none());
}
