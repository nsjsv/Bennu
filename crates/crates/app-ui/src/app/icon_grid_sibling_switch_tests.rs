use std::path::{Path, PathBuf};

use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind};

use super::*;
use crate::config;

fn entry(path: &str, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        kind,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn browser_with_entries(entries: Vec<DirectoryEntry>) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = entries.into();
    browser.view_mode = BrowserViewMode::Icons;
    browser.directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    browser
}

fn anchor(parent: &str, path: &str, index: usize) -> IconGridExpansionAnchor {
    IconGridExpansionAnchor {
        parent_directory: PathBuf::from(parent),
        path: PathBuf::from(path),
        index,
    }
}

fn current_request(browser: &FileBrowser, path: &Path) -> ExpandedDirectoryLoadRequest {
    let state = browser
        .icon_grid_expansion
        .as_ref()
        .expect("icon grid expansion");
    let expanded = state.directory(path).expect("expanded directory");
    ExpandedDirectoryLoadRequest {
        context: icon_grid_load_context(state.context()),
        path: path.to_path_buf(),
        generation: expanded.contents.load_generation,
    }
}

fn finish_scan(browser: &mut FileBrowser, path: &Path, entries: Vec<DirectoryEntry>) {
    let request = current_request(browser, path);
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: path.to_path_buf(),
            entries,
            skipped: Vec::new(),
        }),
    ));
}

fn finish_open_animation(browser: &mut FileBrowser) {
    for _ in 0..8 {
        drop(browser.advance_icon_grid_expansion_animation());
    }
}

#[test]
fn nested_sibling_switch_closes_old_before_loading_new() {
    let root = PathBuf::from("/workspace/root");
    let alpha = PathBuf::from("/workspace/root/alpha");
    let beta = PathBuf::from("/workspace/root/beta");
    let alpha_child = PathBuf::from("/workspace/root/alpha/child.txt");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![
            entry(alpha.to_str().unwrap(), FileKind::Directory),
            entry(beta.to_str().unwrap(), FileKind::Directory),
        ],
    );
    finish_open_animation(&mut browser);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace/root", "/workspace/root/alpha", 0),
    ));
    finish_scan(
        &mut browser,
        &alpha,
        vec![entry(alpha_child.to_str().unwrap(), FileKind::File)],
    );
    finish_open_animation(&mut browser);
    browser.select_path(alpha_child);
    let session_id = browser
        .icon_grid_expansion
        .as_ref()
        .unwrap()
        .context()
        .session_id;

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace/root", "/workspace/root/beta", 1),
    ));

    let closing = browser.icon_grid_expansion.as_ref().unwrap();
    assert!(closing
        .directory(&alpha)
        .is_some_and(|directory| directory.contents.is_collapsing));
    assert!(closing.directory(&beta).is_none());
    assert_eq!(closing.directory_count(), 2);
    assert_eq!(
        closing
            .pending_child(&root)
            .map(|pending| pending.path.as_path()),
        Some(beta.as_path())
    );
    assert_eq!(
        browser.icon_grid_disclosure(BrowserPaneId::PRIMARY, Path::new("/workspace"), &beta),
        Some((0.0, false))
    );
    assert_eq!(browser.selected, Some(alpha.clone()));

    finish_open_animation(&mut browser);

    let replacement = browser.icon_grid_expansion.as_ref().unwrap();
    assert!(replacement.directory(&alpha).is_none());
    assert!(matches!(
        replacement.directory(&beta).unwrap().contents.status,
        ExpandedDirectoryStatus::Loading
    ));
    assert!(replacement.pending_child(&root).is_none());
    assert_eq!(replacement.directory_count(), 2);
    assert_eq!(replacement.context().session_id, session_id);
}

#[test]
fn clicking_closing_child_cancels_pending_sibling_and_reopens_old_branch() {
    let root = PathBuf::from("/workspace/root");
    let alpha = PathBuf::from("/workspace/root/alpha");
    let beta = PathBuf::from("/workspace/root/beta");
    let mut browser =
        browser_with_entries(vec![entry(root.to_str().unwrap(), FileKind::Directory)]);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace", "/workspace/root", 0),
    ));
    finish_scan(
        &mut browser,
        &root,
        vec![
            entry(alpha.to_str().unwrap(), FileKind::Directory),
            entry(beta.to_str().unwrap(), FileKind::Directory),
        ],
    );
    finish_open_animation(&mut browser);
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace/root", "/workspace/root/alpha", 0),
    ));
    finish_scan(&mut browser, &alpha, Vec::new());
    finish_open_animation(&mut browser);

    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace/root", "/workspace/root/beta", 1),
    ));
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        anchor("/workspace/root", "/workspace/root/alpha", 0),
    ));

    let state = browser.icon_grid_expansion.as_ref().unwrap();
    assert!(state.pending_child(&root).is_none());
    assert!(state.directory(&beta).is_none());
    assert!(state.directory(&alpha).is_some_and(|directory| {
        directory.contents.is_expanded && !directory.contents.is_collapsing
    }));
}
