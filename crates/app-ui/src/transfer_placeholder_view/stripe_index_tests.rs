// 条纹相位不变量的单元测试:与实现分文件以守住单文件 800 行约束。
// 覆盖:组头/占位/状态行插入不改变后续条目的条纹序号、展开子目录段
// 各自从 0 起算、组间不重置、None 模式下序号 == 原条目序(回归锚点)、
// 占位插入前后同一行条纹不变(修复传输期条纹跳变的老缺陷)。
// 占位注入直接预置行流构建器的占位缓存,与渲染走同一条合并路径,
// 不需要为测试拼装真实操作队列任务。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

use super::*;
use crate::config;
use crate::model::{BrowserPaneId, ExpandedDirectory, ExpandedDirectoryStatus, FileGroupingMode};
use crate::transfer_placeholders::TransferPlaceholder;

fn file_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::File,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn directory_entry(path: &str) -> DirectoryEntry {
    DirectoryEntry::new(
        PathBuf::from(path),
        FileKind::Directory,
        EntryMetadata::default(),
        false,
        false,
        false,
    )
}

fn file_placeholder(name: &str) -> TransferPlaceholder {
    TransferPlaceholder {
        name: name.to_owned(),
        is_directory: false,
        progress: None,
        total_bytes: None,
        enqueued_at: std::time::SystemTime::UNIX_EPOCH,
    }
}

fn browser_with_entries(
    entries: Vec<DirectoryEntry>,
    grouping: FileGroupingMode,
) -> crate::app::FileBrowser {
    let mut user_config = config::default_user_config();
    user_config.file_grouping = grouping;
    let (mut browser, _) = FileBrowser::new(user_config);
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = Arc::new(entries);
    browser
}

fn expanded(
    entries: Vec<DirectoryEntry>,
    status: ExpandedDirectoryStatus,
) -> ExpandedDirectory {
    ExpandedDirectory {
        entries,
        directory_discovery: None,
        status,
        is_expanded: true,
        is_collapsing: false,
        animation_progress: 1.0,
        load_generation: 0,
        load_context: None,
        load_cancel: None,
        directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
            field: SortField::Name,
            direction: SortDirection::Ascending,
        },
    }
}

/// 走真实行流构建器:占位经预置缓存从同一条合并路径进入行流。
fn build_rows_with_placeholders(
    browser: &crate::app::FileBrowser,
    placeholders: Vec<TransferPlaceholder>,
) -> Vec<ListTransferRow<'_>> {
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let row_height =
        crate::list_view::ListGeometry::for_level(browser.user_config().list_view_density)
            .row_height;
    let mut placeholder_cache = HashMap::new();
    placeholder_cache.insert(browser.current_dir.clone(), placeholders);
    let mut rows = Vec::new();
    push_directory_transfer_rows(
        browser,
        pane,
        &browser.entries,
        browser.current_dir.as_path(),
        transfer_sort_for_root(browser),
        0,
        1.0,
        row_height,
        &mut placeholder_cache,
        &mut rows,
    );
    rows
}

fn build_rows(browser: &crate::app::FileBrowser) -> Vec<ListTransferRow<'_>> {
    build_rows_with_placeholders(browser, Vec::new())
}

/// 每行的条纹序号(组头行无条纹,记 usize::MAX 哨兵),供相位断言。
fn stripe_sequence(rows: &[ListTransferRow<'_>]) -> Vec<usize> {
    rows.iter()
        .map(|row| match row {
            ListTransferRow::Entry { stripe_index, .. }
            | ListTransferRow::Placeholder { stripe_index, .. }
            | ListTransferRow::DirectoryStatusRow { stripe_index, .. } => *stripe_index,
            ListTransferRow::GroupHeader { .. } => usize::MAX,
        })
        .collect()
}

#[test]
fn placeholder_row_consumes_its_own_stripe() {
    // 传输占位插进条目流:占位行消耗自己的序号,与相邻真实行交替。
    // 旧实现透传计数器,占位与下一真实行同相,列表出现连续两行同色
    // (用户报告的背景绘制问题);代价是占位插入/消失时后续行移动一次
    // 相位——与任何普通行增删一致,且占位被真实条目 1:1 替换时不移动。
    let entries = [file_entry("/workspace/a.txt"), file_entry("/workspace/z.txt")];
    let plain_browser = browser_with_entries(entries.to_vec(), FileGroupingMode::None);
    let without_placeholder = build_rows(&plain_browser);
    let placeholder_browser = browser_with_entries(entries.to_vec(), FileGroupingMode::None);
    let with_placeholder =
        build_rows_with_placeholders(&placeholder_browser, vec![file_placeholder("incoming.txt")]);

    assert_eq!(
        stripe_sequence(&without_placeholder),
        [0, 1],
        "None 模式回归锚点:无 chrome 行时条纹序号 == 条目序"
    );
    assert_eq!(
        stripe_sequence(&with_placeholder),
        [0, 1, 2],
        "占位行消耗序号:三行严格交替,不再与下一行同相"
    );
}

#[test]
fn group_headers_neither_consume_nor_reset_stripes() {
    // apple/avocado 同属组 A,zebra 属组 Z:组头行不占相位,组间计数
    // 延续(zebra 的序号是 2 而不是重新起算的 1)。
    let entries = [
        file_entry("/workspace/apple.txt"),
        file_entry("/workspace/avocado.md"),
        file_entry("/workspace/zebra.txt"),
    ];
    let browser = browser_with_entries(entries.to_vec(), FileGroupingMode::NameInitial);
    let rows = build_rows(&browser);

    assert_eq!(stripe_sequence(&rows), [usize::MAX, 0, 1, usize::MAX, 2]);
}

#[test]
fn expanded_directory_segments_restart_stripes_at_zero() {
    // 展开子目录是新的目录段:子条目各自从 0 起算;根段计数器在递归
    // 前后延续,f.txt 的序号不受展开内容多少影响。
    let root_children = [
        directory_entry("/workspace/project"),
        file_entry("/workspace/f.txt"),
    ];
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = Arc::new(root_children.to_vec());
    browser.expanded_directories.insert(
        PathBuf::from("/workspace/project"),
        expanded(
            vec![
                file_entry("/workspace/project/one.rs"),
                file_entry("/workspace/project/two.rs"),
            ],
            ExpandedDirectoryStatus::Loaded,
        ),
    );
    let rows = build_rows(&browser);

    // project(0) | one.rs(0) two.rs(1) 子段起算 | f.txt(1) 根段延续。
    assert_eq!(stripe_sequence(&rows), [0, 0, 1, 1]);
}

#[test]
fn expansion_status_row_does_not_consume_stripe() {
    // Error 状态行插在目录行与后续条目之间:状态行透传当前计数器,
    // 后续条目的条纹序号不变。
    let root_children = [
        directory_entry("/workspace/project"),
        file_entry("/workspace/f.txt"),
    ];
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.current_dir = PathBuf::from("/workspace");
    browser.entries = Arc::new(root_children.to_vec());
    browser.expanded_directories.insert(
        PathBuf::from("/workspace/project"),
        expanded(Vec::new(), ExpandedDirectoryStatus::Error),
    );
    let rows = build_rows(&browser);

    assert_eq!(stripe_sequence(&rows), [0, 1, 1]);
}

#[test]
fn placeholder_conversion_keeps_its_row_stripe_stable() {
    // 占位行完成为真实条目:占位消耗的序号与转为 Entry 后消耗的
    // 序号一致,同一行条纹在转换前后不跳变。
    let entries = [file_entry("/workspace/a.txt"), file_entry("/workspace/z.txt")];
    let placeholder_browser = browser_with_entries(entries.to_vec(), FileGroupingMode::None);
    let placeholder_rows =
        build_rows_with_placeholders(&placeholder_browser, vec![file_placeholder("m.txt")]);
    assert_eq!(stripe_sequence(&placeholder_rows), [0, 1, 2]);

    // m.txt 落地成真实条目:条纹按条目序 0/1/2,原占位所在的第 2 行
    // 在转换前后都是 1。
    let merged_browser = browser_with_entries(
        vec![
            file_entry("/workspace/a.txt"),
            file_entry("/workspace/m.txt"),
            file_entry("/workspace/z.txt"),
        ],
        FileGroupingMode::None,
    );
    let merged_rows = build_rows(&merged_browser);
    assert_eq!(stripe_sequence(&merged_rows), [0, 1, 2]);
}
