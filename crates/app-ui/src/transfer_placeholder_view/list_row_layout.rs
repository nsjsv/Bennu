//! 排好的列表行流。不借用条目：条目行只记（目录段, 下标），画哪行才现取条目，所以帧之间留得住
//! （什么时候重排见 `list_rows`）。范围、落点按每行底边的累计高度二分，不再逐行累加。

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;
use iced::Point;

use crate::app::panes::BrowserPaneView;
use crate::file_entry_presentation::SelectionRunPosition;
use crate::list_view::LIST_GROUP_HEADER_HEIGHT;
use crate::model::{DirectoryEntrySnapshot, FileGroupRailEntry};
use crate::transfer_placeholders::TransferPlaceholder;
use crate::virtual_range::VirtualRange;
use crate::visible_entries::{ListEntryHeights, VisibleEntry, VisibleEntryStatusRow};

/// 列表的一行：真实条目行、展开状态行、传输占位行与组头行交错。
///
/// 条纹相位不变量:整条行流共用一个计数器,按屏幕自上而下的顺序,
/// 条目/占位/展开状态行各消耗一个序号,只有组头行透传(它是分隔
/// chrome,不算数据行)。旧实现每个展开目录段各自从 0 起算,子段结束
/// 后根段接着旧值继续,展开处出现连续两行同色;QTreeView 的
/// alternatingRowColors 同样按可见行全局交替。
pub(crate) enum ListTransferRow<'a> {
    Entry {
        visible: VisibleEntry<'a>,
        /// 同名传输占位改为装饰挂在真实行上(图标叠加进度环),不再单独成行。
        transfer: Option<&'a TransferPlaceholder>,
        stripe_index: usize,
        /// 搜索结果里正文摘要那一行的高度；没有摘要为 0。
        snippet_height: f32,
    },
    DirectoryStatusRow {
        status: VisibleEntryStatusRow,
        depth: usize,
        height: f32,
        stripe_index: usize,
    },
    Placeholder {
        placeholder: &'a TransferPlaceholder,
        stripe_index: usize,
    },
    /// 根目录分组标题行:仅分组开启时插入文件段之前,标题是构造时本地化的成品文案。
    GroupHeader {
        title: &'a str,
        count: usize,
        height: f32,
    },
}

/// 一行记什么。10 万行的目录常驻约 1.2 MB，所以只放下标，占位和组头放旁表。
#[derive(Debug, Clone, Copy)]
enum ListRowSlot {
    Entry {
        segment: u32,
        index: u32,
    },
    Placeholder {
        placeholder: u32,
    },
    DirectoryStatus {
        status: VisibleEntryStatusRow,
        depth: u32,
        height: f32,
    },
    GroupHeader {
        header: u32,
    },
}

/// 条目行所在的目录：0 号是根层，其余是展开的子目录。
#[derive(Debug)]
struct ListRowSegment {
    directory: Option<PathBuf>,
    depth: usize,
    animation_progress: f32,
}

#[derive(Debug)]
struct ListGroupHeader {
    row: usize,
    title: String,
    count: usize,
}

#[derive(Debug)]
pub(crate) struct ListRowLayout {
    fingerprint: u64,
    row_height: f32,
    slots: Vec<ListRowSlot>,
    /// 每行底边离第一行顶的距离，按行序逐行相加（与原来逐行累加的结果一致）。
    bottoms: Vec<f32>,
    segments: Vec<ListRowSegment>,
    placeholder_rows: Vec<TransferPlaceholder>,
    /// （行号, 占位）按行号排好：挂在真实行上的同名传输占位。
    decorations: Vec<(usize, TransferPlaceholder)>,
    headers: Vec<ListGroupHeader>,
    rail: Vec<FileGroupRailEntry>,
}

impl ListRowLayout {
    pub(super) fn new(fingerprint: u64, row_height: f32) -> Self {
        Self {
            fingerprint,
            row_height,
            slots: Vec::new(),
            bottoms: Vec::new(),
            segments: Vec::new(),
            placeholder_rows: Vec::new(),
            decorations: Vec::new(),
            headers: Vec::new(),
            rail: Vec::new(),
        }
    }

    pub(super) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    pub(super) fn row_height(&self) -> f32 {
        self.row_height
    }

    pub(super) fn add_segment(
        &mut self,
        directory: Option<PathBuf>,
        depth: usize,
        animation_progress: f32,
    ) -> u32 {
        self.segments.push(ListRowSegment {
            directory,
            depth,
            animation_progress,
        });
        (self.segments.len() - 1) as u32
    }

    /// 段的（深度, 展开动画进度）。
    pub(super) fn segment_depth_progress(&self, segment: u32) -> (usize, f32) {
        let segment = &self.segments[segment as usize];
        (segment.depth, segment.animation_progress)
    }

    /// 条目行按所在段的展开动画进度收缩（与 list_entry_row 的容器高度一致）。
    pub(super) fn push_entry(
        &mut self,
        segment: u32,
        index: usize,
        snippet_height: f32,
        transfer: Option<TransferPlaceholder>,
    ) {
        if let Some(transfer) = transfer {
            self.decorations.push((self.slots.len(), transfer));
        }
        let progress = self.segments[segment as usize].animation_progress;
        let height = (self.row_height + snippet_height) * progress.clamp(0.0, 1.0);
        self.push(
            ListRowSlot::Entry {
                segment,
                index: index as u32,
            },
            height,
        );
    }

    pub(super) fn push_placeholder(&mut self, placeholder: TransferPlaceholder) {
        self.placeholder_rows.push(placeholder);
        let placeholder = (self.placeholder_rows.len() - 1) as u32;
        self.push(ListRowSlot::Placeholder { placeholder }, self.row_height);
    }

    pub(super) fn push_status(&mut self, status: VisibleEntryStatusRow, depth: usize, height: f32) {
        self.push(
            ListRowSlot::DirectoryStatus {
                status,
                depth: depth as u32,
                height,
            },
            height,
        );
    }

    /// 组头行：索引栏条目（短标签 → 组头顶）同时记下，与渲染同一份高度。
    pub(super) fn push_group_header(&mut self, title: String, index_label: String, count: usize) {
        let row = self.slots.len();
        self.rail.push(FileGroupRailEntry {
            index_label,
            top_offset: self.top(row),
        });
        self.headers.push(ListGroupHeader { row, title, count });
        let header = (self.headers.len() - 1) as u32;
        self.push(
            ListRowSlot::GroupHeader { header },
            LIST_GROUP_HEADER_HEIGHT,
        );
    }

    fn push(&mut self, slot: ListRowSlot, height: f32) {
        let bottom = self.content_height() + height;
        self.slots.push(slot);
        self.bottoms.push(bottom);
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// 总高:视口越界自愈的最大偏移口径,与渲染累计高度一致。
    pub(crate) fn content_height(&self) -> f32 {
        self.bottoms.last().copied().unwrap_or(0.0)
    }

    fn top(&self, row: usize) -> f32 {
        row.checked_sub(1)
            .map_or(0.0, |previous| self.bottoms[previous])
    }

    /// 分组索引栏条目;没分组时为空。
    pub(crate) fn rail_entries(&self) -> &[FileGroupRailEntry] {
        &self.rail
    }

    pub(crate) fn has_placeholder_rows(&self) -> bool {
        !self.placeholder_rows.is_empty()
    }

    /// 第 `row` 行，条目现从 `pane` 取。
    pub(crate) fn row<'r>(
        &'r self,
        pane: BrowserPaneView<'r>,
        row: usize,
    ) -> Option<ListTransferRow<'r>> {
        let stripe_index = row - self.headers.partition_point(|header| header.row < row);
        Some(match *self.slots.get(row)? {
            ListRowSlot::Entry { .. } => {
                let (segment, entry) = self.entry_at(pane, row)?;
                ListTransferRow::Entry {
                    visible: VisibleEntry {
                        entry,
                        depth: segment.depth,
                        animation_progress: segment.animation_progress,
                    },
                    transfer: self
                        .decorations
                        .binary_search_by_key(&row, |(decorated, _)| *decorated)
                        .ok()
                        .map(|found| &self.decorations[found].1),
                    stripe_index,
                    snippet_height: ListEntryHeights::at(pane.location, self.row_height)
                        .snippet_height(entry),
                }
            }
            ListRowSlot::Placeholder { placeholder } => ListTransferRow::Placeholder {
                placeholder: &self.placeholder_rows[placeholder as usize],
                stripe_index,
            },
            ListRowSlot::DirectoryStatus {
                status,
                depth,
                height,
            } => ListTransferRow::DirectoryStatusRow {
                status,
                depth: depth as usize,
                height,
                stripe_index,
            },
            ListRowSlot::GroupHeader { header } => {
                let header = &self.headers[header as usize];
                ListTransferRow::GroupHeader {
                    title: &header.title,
                    count: header.count,
                    height: LIST_GROUP_HEADER_HEIGHT,
                }
            }
        })
    }

    fn segment_entries<'p>(
        &self,
        pane: BrowserPaneView<'p>,
        segment: &ListRowSegment,
    ) -> Option<&'p DirectoryEntrySnapshot> {
        match &segment.directory {
            None => Some(pane.entries),
            Some(directory) => pane
                .expanded_directories
                .get(directory)
                .map(|expanded| &expanded.entries),
        }
    }

    fn entry_at<'p>(
        &self,
        pane: BrowserPaneView<'p>,
        row: usize,
    ) -> Option<(&ListRowSegment, &'p DirectoryEntry)> {
        let ListRowSlot::Entry { segment, index } = *self.slots.get(row)? else {
            return None;
        };
        let segment = &self.segments[segment as usize];
        let entry = self.segment_entries(pane, segment)?.get(index as usize)?;
        Some((segment, entry))
    }

    /// 范围里的条目行（行号, 条目）；占位、状态行、组头不是条目。按可见行调度的（缩略图、文件信息、
    /// 文件夹摘要、换目录时的加载占位）都从这里取，调度的行和画出来的是同一批。
    pub(crate) fn entries_in<'p>(
        &'p self,
        pane: BrowserPaneView<'p>,
        rows: Range<usize>,
    ) -> impl Iterator<Item = (usize, VisibleEntry<'p>)> + 'p {
        rows.filter_map(move |row| {
            let (segment, entry) = self.entry_at(pane, row)?;
            Some((
                row,
                VisibleEntry {
                    entry,
                    depth: segment.depth,
                    animation_progress: segment.animation_progress,
                },
            ))
        })
    }

    /// 虚拟范围:视口上下各多排 `overscan_rows` 行。
    pub(crate) fn range_for_viewport(
        &self,
        header_height: f32,
        viewport_offset: f32,
        viewport_height: f32,
        overscan_rows: usize,
    ) -> VirtualRange {
        if self.row_height <= f32::EPSILON || viewport_height <= f32::EPSILON {
            return VirtualRange::empty();
        }
        let viewport_top = viewport_offset.max(0.0);
        let overscan_height = overscan_rows as f32 * self.row_height;
        let top = (viewport_top - header_height - overscan_height).max(0.0);
        let bottom = (viewport_top + viewport_height - header_height).max(0.0) + overscan_height;
        // 与窗口相交的行（底边在 top 之下、顶边在 bottom 之上）是连续的一段；
        // 高度为 0 的行（折叠到底）落在窗口里也算。
        let start = self
            .bottoms
            .partition_point(|&row_bottom| row_bottom <= top);
        let end = if self.slots.is_empty() || bottom <= 0.0 {
            0
        } else {
            1 + self.bottoms[..self.slots.len() - 1].partition_point(|&row_top| row_top < bottom)
        };
        let total = self.content_height();
        if start >= end {
            return VirtualRange {
                start: self.len(),
                end: self.len(),
                before_height: total,
                after_height: 0.0,
            };
        }
        VirtualRange {
            start,
            end,
            before_height: self.top(start),
            after_height: (total - self.bottoms[end - 1]).max(0.0),
        }
    }

    /// 还没有视口时先排前 `initial_rows` 行。
    pub(crate) fn initial_range(&self, initial_rows: usize) -> VirtualRange {
        if self.row_height <= f32::EPSILON || initial_rows == 0 {
            return VirtualRange::empty();
        }
        let end = initial_rows.min(self.len());
        VirtualRange {
            start: 0,
            end,
            before_height: 0.0,
            after_height: (self.content_height() - self.top(end)).max(0.0),
        }
    }

    /// 条目行的纵向位置（顶, 高），含表头留量:reveal/键盘跟随与渲染同一份行流,占位行与
    /// 展开状态行都计入。同一路径在多段里出现（搜索结果里展开的目录）时取最上面那行。
    pub(crate) fn vertical_bounds(
        &self,
        pane: BrowserPaneView<'_>,
        path: &Path,
        header_height: f32,
    ) -> Option<(f32, f32)> {
        let targets = self
            .segments
            .iter()
            .enumerate()
            .filter_map(|(segment, entries)| {
                let index = self.segment_entries(pane, entries)?.position(path)?;
                Some((segment as u32, index as u32))
            })
            .collect::<Vec<_>>();
        let row = self.slots.iter().position(|slot| {
            matches!(*slot, ListRowSlot::Entry { segment, index } if targets.contains(&(segment, index)))
        })?;
        let top = self.top(row);
        Some((header_height + top, self.bottoms[row] - top))
    }

    /// 光标落点 → 条目路径:与虚拟范围/reveal 用同一份高度。组头/状态行/占位行不是条目,
    /// 落在表尾空白同样为 None。
    pub(crate) fn entry_path_at<'p>(
        &self,
        pane: BrowserPaneView<'p>,
        header_height: f32,
        point: Point,
    ) -> Option<&'p Path> {
        if self.row_height <= f32::EPSILON {
            return None;
        }
        let y = point.y - header_height;
        let row = self.bottoms.partition_point(|&row_bottom| row_bottom <= y);
        if y < self.top(row) {
            return None;
        }
        self.entry_at(pane, row).map(|(_, entry)| entry.path())
    }

    /// 选中连排位置:邻居行只在相邻真实条目同被选中时延续连排;占位与状态行天然断开。
    pub(crate) fn selection_run_position(
        &self,
        pane: BrowserPaneView<'_>,
        row: usize,
        selected_paths: &HashSet<PathBuf>,
    ) -> Option<SelectionRunPosition> {
        let selected = |row: usize| {
            self.entry_at(pane, row)
                .is_some_and(|(_, entry)| selected_paths.contains(entry.path()))
        };
        if !selected(row) {
            return None;
        }
        Some(SelectionRunPosition::from_neighbors(
            row.checked_sub(1).is_some_and(selected),
            selected(row + 1),
        ))
    }
}
