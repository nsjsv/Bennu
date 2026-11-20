use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, EntryMetadata, FileKind};
use iced::event;
use iced::keyboard::{key, Key, Modifiers};

use super::FileBrowser;
use crate::config;
use crate::model::{BrowserLocation, BrowserPaneLayout, BrowserViewMode};
use crate::shortcuts::{FileContentKey, ShortcutAction};

fn browser_in(directory: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(directory.to_path_buf());
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser
}

fn press(browser: &mut FileBrowser, key: Key, modifiers: Modifiers) {
    drop(browser.handle_keyboard_key_pressed(key, modifiers, event::Status::Ignored));
}

fn f(number: u8) -> Key {
    Key::Named(match number {
        3 => key::Named::F3,
        _ => unreachable!("only F3 is pressed directly here"),
    })
}

fn tab_count(browser: &FileBrowser) -> usize {
    browser.panes.iter().map(|pane| pane.tabs.len()).sum()
}

#[test]
fn f3_splits_the_current_folder_and_merges_back_without_losing_tabs() {
    let mut browser = browser_in(Path::new("/workspace"));
    browser.tab_mut().choose_view_mode(BrowserViewMode::List);

    press(&mut browser, f(3), Modifiers::default());
    assert!(matches!(
        browser.pane_layout,
        BrowserPaneLayout::Split { .. }
    ));
    assert_eq!(
        browser.tab().location,
        BrowserLocation::Directory(PathBuf::from("/workspace"))
    );
    assert_eq!(browser.tab().chosen_view_mode(), BrowserViewMode::List);
    let right_side = browser.active_pane_id();

    // 右边再开一个标签页，再切回左边合并：右边的两个标签页都并过来，当前标签页不变。
    press(&mut browser, Key::Character("t".into()), Modifiers::CTRL);
    press(&mut browser, f(3), Modifiers::CTRL);
    assert_ne!(browser.active_pane_id(), right_side);
    let left_tab = browser.pane().active_tab_id;

    press(&mut browser, f(3), Modifiers::default());
    assert!(matches!(
        browser.pane_layout,
        BrowserPaneLayout::Single { .. }
    ));
    assert_eq!(browser.panes.len(), 1);
    assert_eq!(tab_count(&browser), 3);
    assert_eq!(browser.pane().active_tab_id, left_tab);
}

/// 跑完按键返回的任务，取出冲突检查带回来的传输（目标路径和是移动还是复制）。
async fn checked_transfers(
    task: iced::Task<crate::model::Message>,
) -> Option<(crate::model::TransferConflictMode, Vec<PathBuf>)> {
    use iced::futures::StreamExt;

    let mut stream = iced_runtime::task::into_stream(task)?;
    while let Some(action) = stream.next().await {
        if let iced_runtime::Action::Output(crate::model::Message::TransferConflictsChecked {
            mode,
            transfers,
            ..
        }) = action
        {
            return Some((
                mode,
                transfers
                    .into_iter()
                    .map(|transfer| transfer.target)
                    .collect(),
            ));
        }
    }
    None
}

#[tokio::test]
async fn shift_f5_and_shift_f6_copy_and_move_into_the_other_side() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let report = left.path().join("report.txt");
    std::fs::write(&report, b"data").unwrap();
    let mut browser = browser_in(left.path());
    browser.tab_mut().entries = vec![DirectoryEntry::new(
        report.clone(),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )]
    .into();
    browser.select_path(report.clone());
    let copy = FileContentKey::Action(ShortcutAction::CopyToOtherPane);
    let move_key = FileContentKey::Action(ShortcutAction::MoveToOtherPane);

    // 没分屏时不做事。
    assert_eq!(
        checked_transfers(browser.invoke_routed_file_content_key(copy.clone())).await,
        None
    );

    drop(browser.open_directory_in_split(
        right.path().to_path_buf(),
        crate::model::SplitAxis::Horizontal,
    ));
    press(&mut browser, f(3), Modifiers::CTRL);
    browser.select_path(report.clone());
    // 之前剪切的东西不受“移动到另一侧”影响。
    browser.pending_operation = Some(crate::model::PendingOperation::Move(vec![PathBuf::from(
        "/elsewhere/cut.txt",
    )]));

    let expected = vec![right.path().join("report.txt")];
    assert_eq!(
        checked_transfers(browser.invoke_routed_file_content_key(copy.clone())).await,
        Some((crate::model::TransferConflictMode::Copy, expected.clone()))
    );
    assert_eq!(
        checked_transfers(browser.invoke_routed_file_content_key(move_key.clone())).await,
        Some((crate::model::TransferConflictMode::Move, expected))
    );
    assert!(browser.pending_operation.is_some());
}
