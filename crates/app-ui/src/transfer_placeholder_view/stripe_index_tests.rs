// 条纹相位不变量的单元测试:与实现分文件以守住单文件 800 行约束。
// 覆盖:组头不消耗序号、占位/状态行各占一个序号、展开子目录与根目录
// 共用一个计数器、组间不重置、None 模式下序号 == 原条目序(回归锚点)、
// 占位插入前后同一行条纹不变(修复传输期条纹跳变的老缺陷)。
// 占位直接交给行流构建器的根目录,与渲染走同一条合并路径,
// 不需要为测试拼装真实操作队列任务。
use std::path::PathBuf;

use file_core::{DirectoryEntry, EntryMetadata, FileKind, SortDirection, SortField};

use super::list_rows::list_row_layout_with_root_placeholders;
use super::*;
use crate::config;
use crate::model::BrowserLocation;
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
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = entries.into();
    browser
}

fn expanded(entries: Vec<DirectoryEntry>, status: ExpandedDirectoryStatus) -> ExpandedDirectory {
    ExpandedDirectory {
        entries: entries.into(),
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

/// 走真实行流构建器:占位直接给根目录,从同一条合并路径进入行流。
/// 返回每行的条纹序号(组头行无条纹,记 usize::MAX 哨兵),供相位断言。
fn stripes_with_placeholders(
    browser: &crate::app::FileBrowser,
    placeholders: Vec<TransferPlaceholder>,
) -> Vec<usize> {
    let pane = browser.pane_view(BrowserPaneId::PRIMARY).unwrap();
    let row_height =
        crate::list_view::ListGeometry::for_level(browser.user_config().list_view_density)
            .row_height;
    let layout = list_row_layout_with_root_placeholders(browser, pane, row_height, placeholders);
    (0..layout.len())
        .map(|row| match layout.row(pane, row).unwrap() {
            ListTransferRow::Entry { stripe_index, .. }
            | ListTransferRow::Placeholder { stripe_index, .. }
            | ListTransferRow::DirectoryStatusRow { stripe_index, .. } => stripe_index,
            ListTransferRow::GroupHeader { .. } => usize::MAX,
        })
        .collect()
}

fn stripes(browser: &crate::app::FileBrowser) -> Vec<usize> {
    stripes_with_placeholders(browser, Vec::new())
}

fn group_header_count(browser: &crate::app::FileBrowser) -> usize {
    stripes(browser)
        .into_iter()
        .filter(|stripe| *stripe == usize::MAX)
        .count()
}

#[test]
fn placeholder_row_consumes_its_own_stripe() {
    // 传输占位插进条目流:占位行消耗自己的序号,与相邻真实行交替。
    // 旧实现透传计数器,占位与下一真实行同相,列表出现连续两行同色
    // (用户报告的背景绘制问题);代价是占位插入/消失时后续行移动一次
    // 相位——与任何普通行增删一致,且占位被真实条目 1:1 替换时不移动。
    let entries = [
        file_entry("/workspace/a.txt"),
        file_entry("/workspace/z.txt"),
    ];
    let browser = browser_with_entries(entries.to_vec(), FileGroupingMode::None);

    assert_eq!(
        stripes(&browser),
        [0, 1],
        "None 模式回归锚点:无 chrome 行时条纹序号 == 条目序"
    );
    assert_eq!(
        stripes_with_placeholders(&browser, vec![file_placeholder("incoming.txt")]),
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

    assert_eq!(stripes(&browser), [usize::MAX, 0, 1, usize::MAX, 2]);
}

#[test]
fn expanded_directories_continue_the_visible_row_stripes() {
    // 用户报告:子段从 0 重新起算时,展开处与其后的根条目出现连续两行
    // 同色。嵌套两层展开后,所有可见行仍须严格一深一浅交替。
    let root_children = [
        directory_entry("/workspace/project"),
        file_entry("/workspace/f.txt"),
    ];
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = root_children.to_vec().into();
    browser.tab_mut().expanded_directories.insert(
        PathBuf::from("/workspace/project"),
        expanded(
            vec![
                directory_entry("/workspace/project/src"),
                file_entry("/workspace/project/two.rs"),
            ],
            ExpandedDirectoryStatus::Loaded,
        ),
    );
    browser.tab_mut().expanded_directories.insert(
        PathBuf::from("/workspace/project/src"),
        expanded(
            vec![file_entry("/workspace/project/src/main.rs")],
            ExpandedDirectoryStatus::Loaded,
        ),
    );
    // project | src | main.rs | two.rs | f.txt
    assert_eq!(stripes(&browser), [0, 1, 2, 3, 4]);
}

#[test]
fn expansion_status_row_consumes_its_own_stripe() {
    // Error 状态行是一条可见行:与条目一样占一个序号,否则它会与
    // 下一条目同色。
    let root_children = [
        directory_entry("/workspace/project"),
        file_entry("/workspace/f.txt"),
    ];
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
    browser.tab_mut().entries = root_children.to_vec().into();
    browser.tab_mut().expanded_directories.insert(
        PathBuf::from("/workspace/project"),
        expanded(Vec::new(), ExpandedDirectoryStatus::Error),
    );
    assert_eq!(stripes(&browser), [0, 1, 2]);
}

#[test]
fn placeholder_conversion_keeps_its_row_stripe_stable() {
    // 占位行完成为真实条目:占位消耗的序号与转为 Entry 后消耗的
    // 序号一致,同一行条纹在转换前后不跳变。
    let entries = [
        file_entry("/workspace/a.txt"),
        file_entry("/workspace/z.txt"),
    ];
    let placeholder_browser = browser_with_entries(entries.to_vec(), FileGroupingMode::None);
    assert_eq!(
        stripes_with_placeholders(&placeholder_browser, vec![file_placeholder("m.txt")]),
        [0, 1, 2]
    );

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
    assert_eq!(stripes(&merged_browser), [0, 1, 2]);
}

#[test]
fn grouping_splits_the_root_level_but_never_expanded_children() {
    // 根层 a.txt / b.txt 首字母不同，分成两组；展开的 project 里
    // x.rs / y.rs 首字母也不同，但子目录平铺，不再多出组头。
    let mut browser = browser_with_entries(
        vec![
            directory_entry("/workspace/project"),
            file_entry("/workspace/a.txt"),
            file_entry("/workspace/b.txt"),
        ],
        FileGroupingMode::NameInitial,
    );
    browser.tab_mut().expanded_directories.insert(
        PathBuf::from("/workspace/project"),
        expanded(
            vec![
                file_entry("/workspace/project/x.rs"),
                file_entry("/workspace/project/y.rs"),
            ],
            ExpandedDirectoryStatus::Loaded,
        ),
    );

    assert_eq!(group_header_count(&browser), 2);
}

#[test]
fn grouping_also_splits_a_root_that_is_not_a_directory() {
    // 回收站的条目来自回收站列表，没有所在目录，但根层照样分组。
    let mut browser = browser_with_entries(
        vec![
            file_entry("/origin-a/apple.txt"),
            file_entry("/origin-b/banana.txt"),
        ],
        FileGroupingMode::NameInitial,
    );
    browser.tab_mut().location = BrowserLocation::Trash;

    assert_eq!(group_header_count(&browser), 2);
}
