use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use file_core::{DirectoryEntry, EntryMetadata, FileKind};

use super::FileBrowser;
use crate::config;
use crate::model::{
    BrowserPane, BrowserPaneId, BrowserPaneLayout, BrowserTab, FileDragPhase, PaneDragState,
    PaneDropTarget, SplitAxis, SplitRegion, TabDragMode, TabDragState, TabSplitTarget,
};

/// 浏览状态只存在于标签里，所以窗格/标签的任何生命周期操作之后，这些结构
/// 关系都必须成立：活动窗格存在、每个窗格的活动标签存在、标签 id 全局唯一。
fn assert_ownership_invariants(browser: &FileBrowser) {
    assert!(browser.pane_by_id(browser.active_pane_id()).is_some());
    for pane_id in browser.pane_layout.visible_pane_ids() {
        assert!(browser.pane_by_id(pane_id).is_some());
    }
    let mut tab_ids = HashSet::new();
    for pane in &browser.panes {
        assert!(pane.tabs.iter().any(|tab| tab.id == pane.active_tab_id));
        for tab in &pane.tabs {
            assert!(tab_ids.insert(tab.id), "tab id {} appears twice", tab.id);
        }
    }
}

fn tab_with_selection(id: usize, directory: &str, selected_name: &str) -> BrowserTab {
    let mut tab = BrowserTab::directory(id, PathBuf::from(directory));
    let selected = tab.current_dir.join(selected_name);
    tab.entries = Arc::new(vec![DirectoryEntry::new(
        selected.clone(),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )]);
    tab.selected_paths.insert(selected.clone());
    tab.selected = Some(selected);
    tab
}

fn browser_with_tabs(tabs: Vec<BrowserTab>) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.next_tab_id = tabs.iter().map(|tab| tab.id).max().unwrap_or(0) + 1;
    let pane = browser.pane_mut();
    pane.active_tab_id = tabs[0].id;
    pane.tabs = tabs;
    browser
}

fn selected_of(tab: &BrowserTab) -> Option<&PathBuf> {
    tab.selected.as_ref()
}

fn split_drag(tab_id: usize, region: SplitRegion) -> TabDragState {
    TabDragState {
        source_pane_id: BrowserPaneId::PRIMARY,
        tab_id,
        phase: FileDragPhase::Dragging,
        mode: TabDragMode::Split,
        split_target: Some(TabSplitTarget { region }),
    }
}

#[test]
fn switching_tabs_leaves_the_background_tab_state_untouched() {
    let mut browser = browser_with_tabs(vec![
        tab_with_selection(0, "/a", "x.txt"),
        tab_with_selection(1, "/b", "y.txt"),
    ]);
    let background_entries = Arc::clone(&browser.pane().tabs[0].entries);

    drop(browser.select_tab(1));

    assert_eq!(browser.tab().current_dir, PathBuf::from("/b"));
    assert_eq!(selected_of(browser.tab()), Some(&PathBuf::from("/b/y.txt")));
    let background = &browser.pane().tabs[0];
    assert_eq!(selected_of(background), Some(&PathBuf::from("/a/x.txt")));
    assert!(Arc::ptr_eq(&background.entries, &background_entries));
    assert_ownership_invariants(&browser);

    drop(browser.select_tab(0));

    assert_eq!(selected_of(browser.tab()), Some(&PathBuf::from("/a/x.txt")));
    assert_eq!(
        selected_of(&browser.pane().tabs[1]),
        Some(&PathBuf::from("/b/y.txt"))
    );
    assert_ownership_invariants(&browser);
}

#[test]
fn closing_the_active_tab_activates_a_neighbor_and_keeps_the_others_state() {
    let mut browser = browser_with_tabs(vec![
        tab_with_selection(0, "/a", "x.txt"),
        tab_with_selection(1, "/b", "y.txt"),
        tab_with_selection(2, "/c", "z.txt"),
    ]);
    drop(browser.select_tab(1));

    drop(browser.close_tab(1));

    assert_eq!(browser.active_tab_id(), 0);
    assert_eq!(selected_of(browser.tab()), Some(&PathBuf::from("/a/x.txt")));
    let last = browser.pane().tabs.iter().find(|tab| tab.id == 2).unwrap();
    assert_eq!(selected_of(last), Some(&PathBuf::from("/c/z.txt")));
    assert_ownership_invariants(&browser);
}

#[test]
fn splitting_a_tab_off_moves_its_state_to_the_new_pane() {
    let mut browser = browser_with_tabs(vec![
        tab_with_selection(0, "/a", "x.txt"),
        tab_with_selection(1, "/b", "y.txt"),
    ]);
    browser.tab_drag = Some(split_drag(1, SplitRegion::Right));

    drop(browser.finish_tab_drag());

    assert_eq!(browser.panes.len(), 2);
    let source = browser.pane_by_id(BrowserPaneId::PRIMARY).unwrap();
    assert_eq!(source.tabs.len(), 1);
    assert_eq!(selected_of(source.tab()), Some(&PathBuf::from("/a/x.txt")));
    // 新窗格成为活动窗格，访问器读到的就是被拆出来的那个标签。
    assert_ne!(browser.active_pane_id(), BrowserPaneId::PRIMARY);
    assert_eq!(browser.active_tab_id(), 1);
    assert_eq!(selected_of(browser.tab()), Some(&PathBuf::from("/b/y.txt")));
    assert_ownership_invariants(&browser);
}

#[test]
fn splitting_the_only_tab_clones_it_without_sharing_mutable_state() {
    let mut browser = browser_with_tabs(vec![tab_with_selection(0, "/a", "x.txt")]);
    browser.tab_drag = Some(split_drag(0, SplitRegion::Right));

    drop(browser.finish_tab_drag());

    assert_eq!(browser.panes.len(), 2);
    let clone_pane_id = browser.active_pane_id();
    assert_ne!(clone_pane_id, BrowserPaneId::PRIMARY);
    assert_ne!(browser.active_tab_id(), 0);
    assert_eq!(selected_of(browser.tab()), Some(&PathBuf::from("/a/x.txt")));

    // 改动克隆出来的标签不能影响原窗格：两份状态各有各的所有者。
    browser.tab_mut().selected = None;
    browser.tab_mut().current_dir = PathBuf::from("/elsewhere");
    let original = browser.pane_by_id(BrowserPaneId::PRIMARY).unwrap().tab();
    assert_eq!(original.current_dir, PathBuf::from("/a"));
    assert_eq!(selected_of(original), Some(&PathBuf::from("/a/x.txt")));
    assert_ownership_invariants(&browser);
}

#[test]
fn merging_panes_moves_every_tab_with_its_state_into_the_target() {
    let mut browser = browser_with_tabs(vec![
        tab_with_selection(0, "/a", "x.txt"),
        tab_with_selection(2, "/c", "z.txt"),
    ]);
    browser.panes.push(BrowserPane::new(
        BrowserPaneId(1),
        tab_with_selection(1, "/b", "y.txt"),
    ));
    browser.pane_layout = BrowserPaneLayout::Split {
        axis: SplitAxis::Horizontal,
        first: BrowserPaneId::PRIMARY,
        second: BrowserPaneId(1),
        active: BrowserPaneId::PRIMARY,
        first_portion: 500,
    };
    drop(browser.select_tab(2));
    browser.pane_drag = Some(PaneDragState {
        source_pane_id: BrowserPaneId::PRIMARY,
        phase: FileDragPhase::Dragging,
        target: Some(PaneDropTarget::Merge(BrowserPaneId(1))),
    });

    drop(browser.finish_pane_drag());

    assert_eq!(browser.panes.len(), 1);
    assert_eq!(browser.active_pane_id(), BrowserPaneId(1));
    let merged = browser.pane();
    assert_eq!(
        merged.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        vec![1, 0, 2]
    );
    // 源窗格的活动标签随合并成为目标窗格的活动标签。
    assert_eq!(browser.active_tab_id(), 2);
    for (id, selected) in [(0, "/a/x.txt"), (1, "/b/y.txt"), (2, "/c/z.txt")] {
        let tab = merged.tabs.iter().find(|tab| tab.id == id).unwrap();
        assert_eq!(selected_of(tab), Some(&PathBuf::from(selected)));
    }
    assert_ownership_invariants(&browser);
}
