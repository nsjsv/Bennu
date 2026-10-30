use std::path::PathBuf;

use super::*;
use crate::config;
use file_operations::test_persistence::queued_task_id;

#[test]
fn advanced_new_folder_renames_the_leaf_it_actually_created() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    let parent = PathBuf::from("/docs");
    let task_id = queued_task_id(browser.operation_queue.enqueue(
        QueuedFileOperation::CreateDirectories {
            parent: parent.clone(),
            names: vec!["foo".to_owned()],
            gather_sources: Vec::new(),
        },
    ));
    // "foo" 已被占用，执行端续编号建成了 "foo 2"。
    let created = PathBuf::from("/docs/foo 2");

    drop(browser.accept_file_operation_finished(
        task_id,
        FileOperationCompletion::Succeeded(FileOperationOutcome::CreateDirectories {
            parent,
            leaves: vec!["foo".to_owned()],
            leaf_paths: vec![created.clone()],
            created: vec![created.clone()],
            moved: Vec::new(),
        }),
    ));

    assert_eq!(browser.pending_created_entry_rename, Some(created));
}
