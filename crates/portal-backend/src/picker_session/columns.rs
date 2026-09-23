//! 多栏视图状态（访达式栏链）：栏 0 = 根 `listing`，栏 n>0 内容复用
//! `expansions[chain[n]]` 的扫描缓存与 `ScanDirectory` 按路径回填；
//! 逐栏光标/选中与列表同一套语义（模式门控与多选算术共用
//! `selection::entry_click_gating` / `selection_after_click`）。确认
//! 语义的唯一读取口（`target_directory` / `selected_paths`）在
//! view_mode 子模块按焦点栏/最右选中栏分派，其余模块禁止各自分支。

use std::path::{Path, PathBuf};

use bennu_theme::column_geometry::{ColumnEntryGeometry, COLUMN_PADDING};
use file_core::entry::{DirectoryEntry, FileKind};

use super::expansion::ExpansionState;
use super::keyboard_nav::PAGE_FALLBACK_ROWS;
use super::scrollbar::ScrollbarViewport;
use super::thumbnails::VISIBLE_MARGIN_ROWS;
use super::{DirectoryListing, PickerSession, SessionEffect, SessionScrollRegion};
use crate::picker_request::PickerKind;

/// portal 固定基准档（主软件默认档 scale）。
pub(crate) const COLUMNS_SCALE: f32 = 1.0;
/// 固定可见栏数：超出横向滚动（prd）。
pub(crate) const VISIBLE_LANE_COUNT: usize = 3;
/// 栏最小宽度（prd）。
pub(crate) const MIN_LANE_WIDTH: f32 = 96.0;
/// 栏容器视口未测量（首帧探针未回）时的宽度估算基准。
const RAIL_FALLBACK_WIDTH: f32 = 720.0;

/// 单栏交互状态：键盘光标 / 选中集（OpenFile 多选时多行）/ shift 锚点
/// / 悬停行。选中集只允许存在于本栏内，跨栏确认取最右有选中项那栏。
#[derive(Debug, Default, Clone)]
struct LaneState {
    cursor: Option<usize>,
    selection: Vec<usize>,
    anchor: Option<usize>,
    hovered: Option<usize>,
}

impl LaneState {
    /// 行集重建（扫描回填/过滤切换）后的索引重验证：光标越界回退，
    /// 选中/锚点/悬停越界丢弃（与列表 refresh_rows 同规则）。
    fn revalidate(&mut self, count: usize) {
        self.cursor = self
            .cursor
            .filter(|_| count > 0)
            .map(|cursor| cursor.min(count - 1));
        self.selection.retain(|&index| index < count);
        self.anchor = self
            .anchor
            .filter(|&anchor| anchor < count)
            .or_else(|| self.selection.first().copied());
        self.hovered = self.hovered.filter(|&hovered| hovered < count);
    }
}

/// 多栏栏链状态。不变量：`chain[0]` == 会话 directory；`chain.len()`
/// == `lanes.len()`；`focused < chain.len()`。
#[derive(Debug)]
pub(crate) struct ColumnsState {
    chain: Vec<PathBuf>,
    focused: usize,
    lanes: Vec<LaneState>,
}

impl ColumnsState {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            chain: vec![root],
            focused: 0,
            lanes: vec![LaneState::default()],
        }
    }

    /// 导航/重进视图：栏链整体重置为新目录一栏。
    pub(crate) fn reset(&mut self, root: PathBuf) {
        *self = Self::new(root);
    }

    pub(crate) fn chain(&self) -> &[PathBuf] {
        &self.chain
    }

    pub(crate) fn focused(&self) -> usize {
        self.focused
    }

    /// 焦点栏目录（`target_directory` 的多栏取值）。
    pub(crate) fn focused_directory(&self) -> &Path {
        &self.chain[self.focused]
    }

    /// 截断 lane 右侧并追加 directory 为新栏；焦点随打开动作进入新栏
    /// （SaveFile 存进刚点开的目录 = 焦点栏目录）。
    fn open_lane(&mut self, lane: usize, directory: PathBuf) {
        self.chain.truncate(lane + 1);
        self.lanes.truncate(lane + 1);
        self.chain.push(directory);
        self.lanes.push(LaneState::default());
        self.focused = lane + 1;
    }

    /// 截断 lane 右侧（点文件/非目录条目）；焦点留在本栏。
    fn truncate_after(&mut self, lane: usize) {
        self.chain.truncate(lane + 1);
        self.lanes.truncate(lane + 1);
        self.focused = self.focused.min(lane);
    }

    /// 焦点退回父栏；已在第 0 栏时无动作（边界不越界）。
    fn move_focus_backward(&mut self) -> bool {
        if self.focused == 0 {
            return false;
        }
        self.focused -= 1;
        true
    }

    /// 焦点栏 = 最后一次点击/键盘所在栏（prd R9）；越界 lane 忽略。
    fn set_focus(&mut self, lane: usize) {
        if lane < self.chain.len() {
            self.focused = lane;
        }
    }

    pub(super) fn cursor(&self, lane: usize) -> Option<usize> {
        self.lanes.get(lane).and_then(|lane| lane.cursor)
    }

    pub(super) fn selection(&self, lane: usize) -> &[usize] {
        self.lanes
            .get(lane)
            .map(|lane| lane.selection.as_slice())
            .unwrap_or(&[])
    }

    pub(super) fn anchor(&self, lane: usize) -> Option<usize> {
        self.lanes.get(lane).and_then(|lane| lane.anchor)
    }

    fn hovered(&self, lane: usize) -> Option<usize> {
        self.lanes.get(lane).and_then(|lane| lane.hovered)
    }

    fn place_cursor(&mut self, lane: usize, index: usize) {
        if let Some(state) = self.lanes.get_mut(lane) {
            state.cursor = Some(index);
        }
    }

    pub(super) fn set_selection(
        &mut self,
        lane: usize,
        selection: Vec<usize>,
        anchor: Option<usize>,
    ) {
        if let Some(state) = self.lanes.get_mut(lane) {
            state.selection = selection;
            state.anchor = anchor;
        }
    }

    fn clear_selection(&mut self, lane: usize) {
        if let Some(state) = self.lanes.get_mut(lane) {
            state.selection.clear();
            state.anchor = None;
        }
    }

    fn set_hovered(&mut self, lane: usize, index: Option<usize>) {
        if let Some(state) = self.lanes.get_mut(lane) {
            state.hovered = index;
        }
    }

    /// 行集重建后的逐栏索引重验证。
    fn revalidate(&mut self, counts: &[usize]) {
        for (lane, state) in self.lanes.iter_mut().enumerate() {
            state.revalidate(counts.get(lane).copied().unwrap_or(0));
        }
        self.focused = self.focused.min(self.chain.len().saturating_sub(1));
    }
}

impl PickerSession {
    // ---------- 查询（view / keyboard / 确认读取口消费） ----------

    pub(crate) fn columns_chain(&self) -> &[PathBuf] {
        self.columns.chain()
    }

    /// 焦点栏序号（测试观察口；运行时焦点消费都在会话方法内）。
    #[cfg(test)]
    pub(crate) fn columns_focused(&self) -> usize {
        self.columns.focused()
    }

    /// 焦点栏选中集（含预览目标与跨模块查询）。
    pub(crate) fn columns_selection(&self, lane: usize) -> &[usize] {
        self.columns.selection(lane)
    }

    pub(crate) fn columns_anchor(&self, lane: usize) -> Option<usize> {
        self.columns.anchor(lane)
    }

    pub(crate) fn columns_cursor(&self, lane: usize) -> Option<usize> {
        self.columns.cursor(lane)
    }

    /// 栏内容源：栏 0 = 根 listing；栏 n>0 = 该栏目录的扫描缓存。
    fn column_source(&self, lane: usize) -> Option<&DirectoryListing> {
        let path = self.columns.chain().get(lane)?;
        if lane == 0 {
            Some(&self.listing)
        } else {
            self.expansions.get(path).map(|state| &state.listing)
        }
    }

    /// 栏条目（过滤/隐藏文件规则与列表同源，`is_allowed`）。
    pub(crate) fn column_entries_iter(&self, lane: usize) -> impl Iterator<Item = &DirectoryEntry> {
        let entries: &[DirectoryEntry] = match self.column_source(lane) {
            Some(DirectoryListing::Ready(entries)) => entries,
            _ => &[],
        };
        entries.iter().filter(|entry| self.is_allowed(entry))
    }

    pub(crate) fn column_entry_count(&self, lane: usize) -> usize {
        self.column_entries_iter(lane).count()
    }

    pub(crate) fn column_entry(&self, lane: usize, index: usize) -> Option<&DirectoryEntry> {
        self.column_entries_iter(lane).nth(index)
    }

    /// 行高亮 = 本栏选中集 ∪ 本栏键盘光标（唯一高亮规则，与列表
    /// `row_highlighted` 同一契约）。
    pub(crate) fn columns_row_highlighted(&self, lane: usize, index: usize) -> bool {
        self.columns.selection(lane).contains(&index) || self.columns.cursor(lane) == Some(index)
    }

    pub(crate) fn columns_hovered(&self, lane: usize) -> Option<usize> {
        self.columns.hovered(lane)
    }

    /// 栏宽：横向栏容器视口宽 ÷ 可见栏数，最小 96（首帧按估算基准）。
    pub(crate) fn columns_lane_width(&self) -> f32 {
        let rail_width = self
            .scrollbar_viewport_for(&SessionScrollRegion::ColumnsRail)
            .map(|viewport| viewport.viewport_width)
            .filter(|width| *width > f32::EPSILON)
            .unwrap_or(RAIL_FALLBACK_WIDTH);
        (rail_width / VISIBLE_LANE_COUNT as f32).max(MIN_LANE_WIDTH)
    }

    /// 最右有选中项那一栏的选中路径（`selected_paths` 的多栏取值）。
    pub(crate) fn columns_rightmost_selection_paths(&self) -> Vec<PathBuf> {
        for lane in (0..self.columns.chain().len()).rev() {
            let selection = self.columns.selection(lane);
            if !selection.is_empty() {
                return selection
                    .iter()
                    .filter_map(|&index| self.column_entry(lane, index))
                    .map(|entry| entry.path.clone())
                    .collect();
            }
        }
        Vec::new()
    }

    /// 焦点栏选中路径（离开多栏时的承接集）。
    pub(crate) fn columns_focused_selection_paths(&self) -> Vec<PathBuf> {
        let lane = self.columns.focused();
        self.columns
            .selection(lane)
            .iter()
            .filter_map(|&index| self.column_entry(lane, index))
            .map(|entry| entry.path.clone())
            .collect()
    }

    // ---------- 指针交互 ----------

    /// 栏内条目点击：光标先落（先于可选性门控），模式门控与 SaveFile
    /// 名字同步与列表同源；目录点开右侧子栏（截断原右侧），文件截断
    /// 右侧。回收站虚拟视图不扩栏（与主软件一致保持单栏）。
    pub(crate) fn columns_entry_clicked(
        &mut self,
        lane: usize,
        index: usize,
        ctrl: bool,
        shift: bool,
    ) -> SessionEffect {
        let Some(entry) = self.column_entry(lane, index).cloned() else {
            return SessionEffect::None;
        };
        self.columns.set_focus(lane);
        self.columns.place_cursor(lane, index);
        self.columns_apply_click_semantics(lane, index, ctrl, shift, &entry);
        if entry.kind == FileKind::Directory && !self.is_trash_view() {
            self.columns_open_child(lane, entry.path.clone())
        } else {
            self.columns.truncate_after(lane);
            SessionEffect::None
        }
    }

    /// 单击选中语义落本栏选中（模式门控 + SaveFile 名字同步 + 多选
    /// 算术，全部与列表同源）。
    fn columns_apply_click_semantics(
        &mut self,
        lane: usize,
        index: usize,
        ctrl: bool,
        shift: bool,
        entry: &DirectoryEntry,
    ) {
        if !self.entry_click_gating(entry) {
            // 光标落不可选行：本栏选中集与锚点熄灭（资源管理器语义）。
            self.columns.clear_selection(lane);
            return;
        }
        let multiple = matches!(self.kind, PickerKind::OpenFile { multiple: true, .. });
        let current = self.columns.selection(lane).to_vec();
        let anchor = self.columns.anchor(lane);
        let (selection, anchor) =
            Self::selection_after_click(&current, anchor, index, ctrl, shift, multiple);
        self.columns.set_selection(lane, selection, anchor);
    }

    /// 栏内条目双击/Enter 激活：目录 = 打开子栏（多栏的“进入”语义，
    /// 不重置栏链）；文件与列表激活同语义。
    pub(crate) fn columns_entry_double_clicked(
        &mut self,
        lane: usize,
        index: usize,
    ) -> SessionEffect {
        let Some(entry) = self.column_entry(lane, index).cloned() else {
            return SessionEffect::None;
        };
        if entry.kind == FileKind::Directory {
            if self.is_trash_view() {
                return SessionEffect::None;
            }
            return self.columns_open_child(lane, entry.path.clone());
        }
        match self.kind {
            PickerKind::OpenFile {
                multiple: false, ..
            } => {
                self.columns_apply_click_semantics(lane, index, false, false, &entry);
                self.confirm()
            }
            PickerKind::OpenFile { multiple: true, .. } => {
                // 与列表同语义：未选中则补进选中集。
                if !self.columns.selection(lane).contains(&index) {
                    let mut selection = self.columns.selection(lane).to_vec();
                    selection.push(index);
                    selection.sort_unstable();
                    self.columns.set_selection(lane, selection, Some(index));
                }
                SessionEffect::None
            }
            PickerKind::SaveFile { .. } => {
                self.entry_click_gating(&entry);
                SessionEffect::None
            }
            // SaveFiles 双击文件不确认：返回集是调用方名字列表。
            PickerKind::SaveFiles { .. } => SessionEffect::None,
        }
    }

    pub(crate) fn columns_set_hovered(&mut self, lane: usize, index: Option<usize>) {
        self.columns.set_hovered(lane, index);
    }

    /// Ctrl+A：全选焦点栏可选中条目（选中集不跨栏）。
    pub(crate) fn columns_select_all(&mut self, directory_only: bool) {
        let lane = self.columns.focused();
        let selection: Vec<usize> = self
            .column_entries_iter(lane)
            .enumerate()
            .filter(|(_, entry)| {
                if directory_only {
                    entry.kind == FileKind::Directory
                } else {
                    entry.kind == FileKind::File
                }
            })
            .map(|(index, _)| index)
            .collect();
        let anchor = selection.first().copied();
        self.columns.set_selection(lane, selection, anchor);
    }

    // ---------- 栏链 ----------

    /// 打开子栏：截断右侧 + 追加 + 注册扫描槽位（已有 Ready 缓存直接
    /// 展示不重扫）。
    pub(crate) fn columns_open_child(&mut self, lane: usize, directory: PathBuf) -> SessionEffect {
        // 被替换/新增栏的旧视口缓存对新内容是错误几何，先失效。
        let old_lane_count = self.columns.chain().len();
        self.columns.open_lane(lane, directory.clone());
        self.invalidate_lane_viewports(lane + 1, old_lane_count.max(lane + 1));
        self.refresh_rows();
        match self.expansions.get(&directory) {
            // 扫描已在途或已有内容：只收口横向滚动。
            Some(_) => SessionEffect::ScrollColumnsRailToEnd,
            None => {
                self.expansions.insert(
                    directory.clone(),
                    ExpansionState {
                        listing: DirectoryListing::Pending,
                        is_collapsing: false,
                        // 多栏没有展开动画：直接满进度，不挂帧时钟。
                        animation_progress: 1.0,
                    },
                );
                SessionEffect::ColumnScanAndReveal(directory)
            }
        }
    }

    fn invalidate_lane_viewports(&mut self, from: usize, until: usize) {
        for lane in from..until {
            self.scrollbar
                .invalidate_viewport(SessionScrollRegion::ColumnsLane(lane));
        }
    }

    /// 行集重建后的逐栏索引重验证（扫描回填/过滤切换共享）。
    pub(crate) fn columns_revalidate_lane_indices(&mut self) {
        let counts: Vec<usize> = (0..self.columns.chain().len())
            .map(|lane| self.column_entry_count(lane))
            .collect();
        self.columns.revalidate(&counts);
    }

    /// 导航后重置栏链：面包屑/侧边栏/后退前进/地址栏统一在
    /// `enter_directory` 收口到此。
    pub(crate) fn columns_reset_for_navigation(&mut self) {
        let old_lane_count = self.columns.chain().len();
        let root = self.directory.clone();
        self.columns.reset(root);
        self.invalidate_lane_viewports(0, old_lane_count);
    }

    // ---------- 键盘 ----------

    /// ↑↓/Home/End/翻页/type-ahead 的落行规则：单击选中语义落本栏
    /// 选中（与列表 place_list_cursor 同源），不触栏链——栏链动作归
    /// →/Enter/点击。
    fn columns_place_cursor(&mut self, lane: usize, index: usize) -> SessionEffect {
        let Some(entry) = self.column_entry(lane, index).cloned() else {
            return SessionEffect::None;
        };
        self.columns.set_focus(lane);
        self.columns.place_cursor(lane, index);
        self.columns_apply_click_semantics(lane, index, false, false, &entry);
        self.columns_reveal_cursor(lane)
    }

    pub(crate) fn columns_move_cursor(&mut self, delta: isize) -> SessionEffect {
        self.keyboard_nav.clear_type_ahead();
        let lane = self.columns.focused();
        let count = self.column_entry_count(lane);
        if count == 0 {
            return SessionEffect::None;
        }
        let Some(current) = self.columns.cursor(lane) else {
            // 首次键盘移动落在当前选中首行（或第 0 行）。
            let landing = self.columns.selection(lane).first().copied().unwrap_or(0);
            return self.columns_place_cursor(lane, landing);
        };
        let target = current.saturating_add_signed(delta).min(count - 1);
        self.columns_place_cursor(lane, target)
    }

    pub(crate) fn columns_jump_cursor(&mut self, to_end: bool) -> SessionEffect {
        self.keyboard_nav.clear_type_ahead();
        let lane = self.columns.focused();
        let count = self.column_entry_count(lane);
        if count == 0 {
            return SessionEffect::None;
        }
        self.columns_place_cursor(lane, if to_end { count - 1 } else { 0 })
    }

    pub(crate) fn columns_page_cursor(&mut self, pages: isize) -> SessionEffect {
        self.keyboard_nav.clear_type_ahead();
        let lane = self.columns.focused();
        let count = self.column_entry_count(lane);
        if count == 0 {
            return SessionEffect::None;
        }
        let Some(current) = self.columns.cursor(lane) else {
            let landing = self.columns.selection(lane).first().copied().unwrap_or(0);
            return self.columns_place_cursor(lane, landing);
        };
        let rows = self.columns_page_rows(lane).max(1);
        let target = current
            .saturating_add_signed(pages.saturating_mul(rows))
            .min(count - 1);
        self.columns_place_cursor(lane, target)
    }

    fn columns_page_rows(&self, lane: usize) -> isize {
        self.scrollbar_viewport_for(&SessionScrollRegion::ColumnsLane(lane))
            .map(|viewport| {
                (viewport.viewport_height
                    / ColumnEntryGeometry::for_scale(COLUMNS_SCALE).entry_scroll_height)
                    as isize
            })
            .unwrap_or(PAGE_FALLBACK_ROWS)
    }

    /// type-ahead 匹配：缓冲归 keyboard_nav（与列表共享），匹配范围
    /// 限定焦点栏。
    pub(crate) fn columns_type_ahead_match(&mut self, needle: String) -> SessionEffect {
        let lane = self.columns.focused();
        let matched = self.column_entries_iter(lane).position(|entry| {
            entry
                .name
                .to_string_lossy()
                .to_lowercase()
                .contains(&needle)
        });
        match matched {
            Some(index) => self.columns_place_cursor(lane, index),
            None => SessionEffect::None,
        }
    }

    /// ←：焦点退回父栏，光标落在打开原焦点栏的那条目录上。
    pub(crate) fn columns_backward(&mut self) -> SessionEffect {
        self.keyboard_nav.clear_type_ahead();
        if !self.columns.move_focus_backward() {
            return SessionEffect::None;
        }
        let parent_lane = self.columns.focused();
        let opened = self.columns.chain().get(parent_lane + 1).cloned();
        let opened_index = opened.and_then(|opened| {
            self.column_entries_iter(parent_lane)
                .position(|entry| entry.path == opened)
        });
        if let Some(index) = opened_index {
            self.columns.place_cursor(parent_lane, index);
        }
        self.columns_reveal_cursor(parent_lane)
    }

    /// →：进入焦点栏光标所在目录；新栏首项落光标（仅位置不落选中，
    /// 确认集仍指向被进入的目录所在栏）。
    pub(crate) fn columns_forward(&mut self) -> SessionEffect {
        self.keyboard_nav.clear_type_ahead();
        let lane = self.columns.focused();
        let Some(cursor) = self.columns.cursor(lane) else {
            return SessionEffect::None;
        };
        let Some(entry) = self.column_entry(lane, cursor) else {
            return SessionEffect::None;
        };
        if entry.kind != FileKind::Directory || self.is_trash_view() {
            return SessionEffect::None;
        }
        let directory = entry.path.clone();
        let effect = self.columns_open_child(lane, directory);
        self.columns.place_cursor(lane + 1, 0);
        effect
    }

    /// 栏内可见条目索引区间（缩略图调度用）：视口按栏行几何换算，
    /// 上下各留 VISIBLE_MARGIN_ROWS 行余量（与列表调度同一余量）。
    pub(crate) fn columns_visible_window(
        &self,
        lane: usize,
        viewport: &ScrollbarViewport,
    ) -> Option<(usize, usize)> {
        let count = self.column_entry_count(lane);
        if count == 0 {
            return None;
        }
        let geometry = ColumnEntryGeometry::for_scale(COLUMNS_SCALE);
        let stride = geometry.entry_scroll_height;
        // portal 逐行直渲染：首行起点 = padding 顶（无虚拟 spacer）。
        let top = f32::from(COLUMN_PADDING[0]);
        let first_row = (((viewport.offset_y - top) / stride).floor() as isize)
            .saturating_sub(VISIBLE_MARGIN_ROWS)
            .max(0) as usize;
        if first_row >= count {
            return None;
        }
        let last_row = ((((viewport.offset_y + viewport.viewport_height - top) / stride).ceil()
            as usize)
            .saturating_sub(1)
            .saturating_add(VISIBLE_MARGIN_ROWS as usize))
        .min(count - 1);
        if last_row < first_row {
            return None;
        }
        Some((first_row, last_row))
    }

    /// 光标滚入本栏视野：偏移贴边值写入会话缓存后交 main 层执行
    /// scroll_to（无视口缓存时跳过，探针回信后下一次移动即可跟随）。
    fn columns_reveal_cursor(&mut self, lane: usize) -> SessionEffect {
        let Some(cursor) = self.columns.cursor(lane) else {
            return SessionEffect::None;
        };
        let Some(viewport) = self.scrollbar_viewport_for(&SessionScrollRegion::ColumnsLane(lane))
        else {
            return SessionEffect::None;
        };
        let geometry = ColumnEntryGeometry::for_scale(COLUMNS_SCALE);
        // portal 逐行直渲染（无虚拟 spacer）：首行起点 = padding 顶。
        let top = f32::from(COLUMN_PADDING[0]) + cursor as f32 * geometry.entry_scroll_height;
        let bottom = top + geometry.entry_height;
        let target = if top < viewport.offset_y {
            top
        } else if bottom > viewport.offset_y + viewport.viewport_height {
            bottom - viewport.viewport_height
        } else {
            return SessionEffect::None;
        };
        let clamped = target.clamp(
            0.0,
            (viewport.content_height - viewport.viewport_height).max(0.0),
        );
        self.record_keyboard_lane_scroll(lane, clamped);
        // 偏移突变让新行进入可见区间：与滚动路径同源补排缩略图。
        self.schedule_visible_thumbnails();
        SessionEffect::ScrollColumnLaneTo {
            lane,
            offset_y: clamped,
        }
    }
}
