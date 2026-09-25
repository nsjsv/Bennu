use super::*;

fn entry(path: &str, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        kind,
        file_core::EntryMetadata::default(),
        false,
        false,
        false,
    )
}

#[test]
fn entry_middle_press_message_routes_real_directory_to_open_directory() {
    let message = entry_middle_press_message(
        BrowserPaneId::PRIMARY,
        &entry("/workspace/project", FileKind::Directory),
    );

    assert!(matches!(
        message,
        Message::OpenDirectoryFromMiddleClick(BrowserPaneId::PRIMARY, path)
            if path == PathBuf::from("/workspace/project")
    ));
}

#[test]
fn entry_middle_press_message_routes_archive_file_to_smart_extract() {
    let message = entry_middle_press_message(
        BrowserPaneId::PRIMARY,
        &entry("/workspace/bundle.zip", FileKind::File),
    );

    assert!(matches!(
        message,
        Message::ArchiveMiddlePressed(BrowserPaneId::PRIMARY, path)
            if path == PathBuf::from("/workspace/bundle.zip")
    ));
}

#[test]
fn entry_middle_press_message_prefers_directory_kind_over_zip_extension() {
    // 目录语义优先于压缩包扩展名：.zip 后缀的真实目录中键仍开新标签，
    // 不能被误路由进解压管线。
    let message = entry_middle_press_message(
        BrowserPaneId::PRIMARY,
        &entry("/workspace/bundle.zip", FileKind::Directory),
    );

    assert!(matches!(
        message,
        Message::OpenDirectoryFromMiddleClick(BrowserPaneId::PRIMARY, path)
            if path == PathBuf::from("/workspace/bundle.zip")
    ));
}
