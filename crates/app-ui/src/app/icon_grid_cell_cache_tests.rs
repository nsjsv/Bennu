//! 图标网格排好的格子按输入复用的不变量：输入没变（滚动、悬停、选中、展开动画）拿到的是同一份；
//! 任何一项输入变了都重排，而且与清掉缓存从头排的一模一样。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use file_core::{DirectoryEntry, DirectoryScan, EntryMetadata, FileKind, SortDirection};
use file_operations::test_persistence::{attach_store, enqueue_persisted};

use crate::app::FileBrowser;
use crate::config;
use crate::icon_grid_layout::{IconGridCell, IconGridFlowSegment, IconGridPanelLayout};
use crate::model::{
    BrowserLocation, BrowserPaneId, BrowserViewMode, ExpandedDirectoryLoadRequest,
    FileGroupingMode, IconGridExpansionAnchor,
};

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

/// /workspace：project（已展开，里面 main.rs）、a.txt、m.txt，图标视图。
fn browser() -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = vec![
        entry("/workspace/project", FileKind::Directory),
        entry("/workspace/a.txt", FileKind::File),
        entry("/workspace/m.txt", FileKind::File),
    ]
    .into();
    browser.tab_mut().choose_view_mode(BrowserViewMode::Icons);
    browser.tab_mut().directory_collection_phase = crate::model::DirectoryCollectionPhase::Ready;
    drop(browser.toggle_icon_grid_directory(
        BrowserPaneId::PRIMARY,
        IconGridExpansionAnchor {
            parent_directory: PathBuf::from("/workspace"),
            path: PathBuf::from("/workspace/project"),
            index: 0,
        },
    ));
    finish_scan(
        &mut browser,
        "/workspace/project",
        vec![entry("/workspace/project/main.rs", FileKind::File)],
    );
    browser
}

fn finish_scan(browser: &mut FileBrowser, path: &str, entries: Vec<DirectoryEntry>) {
    let path = Path::new(path);
    let state = browser.icon_grid_expansion.as_ref().unwrap();
    let request = ExpandedDirectoryLoadRequest {
        context: super::icon_grid_load_context(state.context()),
        path: path.to_path_buf(),
        generation: state.directory(path).unwrap().contents.load_generation,
    };
    drop(browser.accept_complete_expanded_directory_fixture(
        request,
        Ok(DirectoryScan {
            path: path.to_path_buf(),
            entries,
            skipped: Vec::new(),
        }),
    ));
}

/// 排一次几何，返回这次用的格子（窗格缓存里的那份）和画出来的样子。
fn lay_out(browser: &FileBrowser) -> (Rc<impl std::fmt::Debug>, Vec<String>) {
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let layout = browser.icon_grid_layout_for_pane(pane);
    let mut described = Vec::new();
    describe(layout.root(), &mut described);
    let plan = browser.pane().icon_cells.borrow().clone().unwrap();
    (plan, described)
}

fn describe(panel: &IconGridPanelLayout<'_>, described: &mut Vec<String>) {
    described.push(format!("panel {:?} {}", panel.status, panel.height));
    for segment in &panel.flow {
        match segment {
            IconGridFlowSegment::Rows(rows) => {
                for index in 0..rows.cell_count() {
                    described.push(match rows.cell(index).unwrap() {
                        IconGridCell::Entry(cell) => format!(
                            "entry {} #{} transfer={:?}",
                            cell.entry.path().display(),
                            cell.entry_index,
                            cell.transfer.map(|transfer| &transfer.name)
                        ),
                        IconGridCell::Placeholder(placeholder) => {
                            format!("placeholder {}", placeholder.name)
                        }
                    });
                }
            }
            IconGridFlowSegment::Band(band) => {
                described.push(format!("band {}", band.directory.display()));
                describe(&band.panel, described);
            }
            IconGridFlowSegment::GroupHeader(header) => {
                described.push(format!("header {} {}", header.title, header.count));
            }
        }
    }
}

/// 清掉窗格的缓存从头排。
fn fresh(browser: &FileBrowser) -> Vec<String> {
    *browser.pane().icon_cells.borrow_mut() = None;
    lay_out(browser).1
}

#[test]
fn unchanged_inputs_reuse_the_cells_already_laid_out() {
    let mut browser = browser();
    let (before, _) = lay_out(&browser);

    // 滚动、悬停、选中、可见格子补上文件信息（没按大小、日期分组）、展开动画都不改格子。
    drop(browser.handle_icon_grid_scrolled(BrowserPaneId::PRIMARY, 120.0, 500.0, 300.0));
    let a = PathBuf::from("/workspace/a.txt");
    browser.hovered_entry = Some(a.clone());
    browser.tab_mut().selected_paths.insert(a.clone());
    browser.tab_mut().selected = Some(a);
    browser.entry_metadata_revision += 1;
    drop(browser.advance_icon_grid_expansion_animation());

    let (after, _) = lay_out(&browser);
    assert!(Rc::ptr_eq(&before, &after));
}

#[test]
fn every_input_change_lays_out_again() {
    let cases: Vec<(&str, Box<dyn Fn(&mut FileBrowser)>)> = vec![
        (
            "根层条目",
            Box::new(|browser| {
                browser
                    .tab_mut()
                    .entries
                    .make_mut()
                    .push(entry("/workspace/z.txt", FileKind::File));
            }),
        ),
        (
            "展开目录的条目",
            Box::new(|browser| {
                browser
                    .icon_grid_expansion
                    .as_mut()
                    .unwrap()
                    .directory_mut(Path::new("/workspace/project"))
                    .unwrap()
                    .contents
                    .entries
                    .make_mut()
                    .push(entry("/workspace/project/lib.rs", FileKind::File));
            }),
        ),
        (
            "分组方式",
            Box::new(|browser| browser.user_config.file_grouping = FileGroupingMode::NameInitial),
        ),
        (
            "排序",
            Box::new(|browser| browser.options.sort_direction = SortDirection::Descending),
        ),
        (
            "传入占位",
            Box::new(|browser| {
                let mut transfer = file_operations::QueuedTransfer::new(
                    PathBuf::from("/source/incoming.txt"),
                    PathBuf::from("/workspace/project/incoming.txt"),
                );
                transfer.conflict_strategy = file_core::TransferConflictStrategy::Fail;
                enqueue_persisted(
                    &mut browser.operation_queue,
                    file_operations::QueuedFileOperation::Copy {
                        transfers: vec![transfer],
                        verification: file_core::FileOperationVerification::BasicMetadata,
                    },
                );
            }),
        ),
    ];
    for (input, change) in cases {
        let storage = tempfile::tempdir().unwrap();
        let mut browser = browser();
        attach_store(
            &mut browser.operation_queue,
            bennu_state::TaskQueueStore::new(storage.path().join("state.sqlite")).unwrap(),
        );
        let (before, _) = lay_out(&browser);

        change(&mut browser);
        let (after, described) = lay_out(&browser);

        assert!(!Rc::ptr_eq(&before, &after), "{input} 变了没重排");
        assert_eq!(described, fresh(&browser), "{input}");
    }
}

#[test]
fn entry_metadata_counts_only_when_grouping_reads_it() {
    // 按大小分组读条目里的大小：补上文件信息（快照不变）也要重排。
    let mut browser = browser();
    browser.user_config.file_grouping = FileGroupingMode::Size;
    let (before, _) = lay_out(&browser);

    browser.entry_metadata_revision += 1;

    assert!(!Rc::ptr_eq(&before, &lay_out(&browser).0));
}
