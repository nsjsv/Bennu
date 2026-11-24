//! 右侧停靠区的布局：从上到下几格，每格一个或几个面板叠成标签。纯数据，不碰界面。
//!
//! [`RightDockLayout`] 是存盘的那部分；[`RightDock`] 再加上「关掉的面板回哪儿」（只在这次
//! 运行里记），所有改布局的操作都在它上面，操作完下面这些一直成立：每个面板最多出现一次；
//! 没有空格子；每格的当前标签在范围里；各格份额至少 1、合计正好是 [`PORTION_SCALE`]；没有
//! 面板时栏不显示。

use std::collections::HashMap;

use crate::config::{normalize_right_dock_width, DEFAULT_RIGHT_DOCK_WIDTH};

/// 各格份额的合计，永远正好是它。高度按「份额 / 它」分，所以合计不能飘：飘大了（反复开关
/// 面板只增不减）各格就会被分到比栏本身更多的高度。
pub(crate) const PORTION_SCALE: u16 = 1000;

/// 停靠区里能放的面板。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DockPanel {
    Preview,
    Info,
    /// 正在跑的任务和做过的操作（能按顺序撤回）。
    TaskLog,
    /// 对选中项能用的转换、压缩、解压等和自定义动作，点一下就跑。
    QuickActions,
    /// 只列文件夹的树，跟着当前目录展开。
    FolderTree,
}

impl DockPanel {
    pub(crate) const ALL: [Self; 5] = [
        Self::Preview,
        Self::Info,
        Self::TaskLog,
        Self::QuickActions,
        Self::FolderTree,
    ];

    pub(crate) fn storage_name(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Info => "info",
            Self::TaskLog => "task_log",
            Self::QuickActions => "quick_actions",
            Self::FolderTree => "folder_tree",
        }
    }

    pub(crate) fn from_storage_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|panel| panel.storage_name() == name)
    }
}

/// 停靠区的一格：一个或几个面板叠成标签，`active` 是正显示的那个。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DockGroup {
    panels: Vec<DockPanel>,
    active: usize,
    portion: u16,
}

impl DockGroup {
    fn single(panel: DockPanel, portion: u16) -> Self {
        Self {
            panels: vec![panel],
            active: 0,
            portion: portion.max(1),
        }
    }

    pub(crate) fn panels(&self) -> &[DockPanel] {
        &self.panels
    }

    pub(crate) fn active_panel(&self) -> DockPanel {
        self.panels[self.active]
    }

    pub(crate) fn portion(&self) -> u16 {
        self.portion
    }
}

/// 存盘的布局。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RightDockLayout {
    visible: bool,
    width: f32,
    locked: bool,
    groups: Vec<DockGroup>,
}

impl Default for RightDockLayout {
    /// 预览在上、信息在下，跟旧版右侧面板的样子一样；默认不显示、锁定。
    fn default() -> Self {
        Self::from_previous_panel(
            false,
            DEFAULT_RIGHT_DOCK_WIDTH,
            crate::config::DEFAULT_RIGHT_PREVIEW_PREVIEW_RATIO,
        )
    }
}

impl RightDockLayout {
    /// 旧版右侧面板（开没开、宽度、预览区占比）换成同样子的布局。
    pub(crate) fn from_previous_panel(open: bool, width: f32, preview_ratio: f32) -> Self {
        let preview = (preview_ratio.clamp(0.0, 1.0) * f32::from(PORTION_SCALE)).round() as u16;
        let preview = preview.clamp(1, PORTION_SCALE - 1);
        Self {
            visible: open,
            width: normalize_right_dock_width(width),
            locked: true,
            groups: vec![
                DockGroup::single(DockPanel::Preview, preview),
                DockGroup::single(DockPanel::Info, PORTION_SCALE - preview),
            ],
        }
    }

    /// 从存盘的各项拼回布局，顺手规整：认不出的面板名、重复的面板、空格子去掉，当前标签夹回
    /// 范围，份额按存的比例缩到合计 [`PORTION_SCALE`]（旧版本存过合计飘得很大的份额）。返回
    /// 布局和「有没有去掉东西」（调用方据此提示部分设置已恢复默认）。
    pub(crate) fn from_parts<'a>(
        visible: bool,
        width: f32,
        locked: bool,
        groups: impl IntoIterator<Item = (Vec<&'a str>, usize, u16)>,
    ) -> (Self, bool) {
        let mut dropped = false;
        let mut seen = Vec::new();
        let mut kept = Vec::new();
        for (names, active, portion) in groups {
            let mut panels = Vec::new();
            for name in names {
                match DockPanel::from_storage_name(name) {
                    Some(panel) if !seen.contains(&panel) => {
                        seen.push(panel);
                        panels.push(panel);
                    }
                    _ => dropped = true,
                }
            }
            if panels.is_empty() {
                dropped = true;
                continue;
            }
            let active = if active < panels.len() {
                active
            } else {
                dropped = true;
                0
            };
            kept.push(DockGroup {
                panels,
                active,
                portion: portion.max(1),
            });
        }
        let mut layout = Self {
            visible: visible && !kept.is_empty(),
            width: normalize_right_dock_width(width),
            locked,
            groups: kept,
        };
        layout.normalize_portions();
        (layout, dropped)
    }

    /// 把份额按现有比例缩到合计正好 [`PORTION_SCALE`]、每格至少 1。合计会偏离的两个入口
    /// （新插一格、从存盘读入）都只经这里；拿走一格、分隔线拖动只在格子之间挪份额，合计本来
    /// 就不变。
    fn normalize_portions(&mut self) {
        let scale = u32::from(PORTION_SCALE);
        let weights: Vec<u32> = self
            .groups
            .iter()
            .map(|group| u32::from(group.portion.max(1)))
            .collect();
        let total: u32 = weights.iter().sum();
        let mut shares: Vec<u32> = weights
            .iter()
            .map(|weight| (weight * scale / total).max(1))
            .collect();
        let assigned: u32 = shares.iter().sum();
        if assigned < scale {
            // 向下取整少分的零头（少于一格一份），一份一份补给小数部分最大的格子。
            let mut by_remainder: Vec<usize> = (0..shares.len()).collect();
            by_remainder.sort_by_key(|&index| std::cmp::Reverse(weights[index] * scale % total));
            for &index in by_remainder.iter().take((scale - assigned) as usize) {
                shares[index] += 1;
            }
        }
        // 「至少 1」顶上去的份额从最大的格子扣回。
        for _ in scale..assigned {
            if let Some(largest) = shares.iter_mut().max() {
                *largest -= 1;
            }
        }
        for (group, share) in self.groups.iter_mut().zip(shares) {
            group.portion = share as u16;
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn width(&self) -> f32 {
        self.width
    }

    pub(crate) fn is_locked(&self) -> bool {
        self.locked
    }

    pub(crate) fn groups(&self) -> &[DockGroup] {
        &self.groups
    }

    /// 栏显示着、而且是它那格的当前标签：这时候它才该读东西、画东西。
    pub(crate) fn shows(&self, panel: DockPanel) -> bool {
        self.visible
            && self
                .groups
                .iter()
                .any(|group| group.active_panel() == panel)
    }

    pub(crate) fn contains(&self, panel: DockPanel) -> bool {
        self.group_of(panel).is_some()
    }

    fn group_of(&self, panel: DockPanel) -> Option<usize> {
        self.groups
            .iter()
            .position(|group| group.panels.contains(&panel))
    }
}

/// 关掉的面板回哪儿。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelHome {
    /// 跟这个面板叠在同一格。
    WithPanel(DockPanel),
    /// 自己一格，放在第几格。
    OwnGroup(usize),
}

/// 拖着的面板放到哪儿：叠进某格，或者在某格上面、下面另起一格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropTarget {
    Join(usize),
    Above(usize),
    Below(usize),
}

/// 本窗口在用的停靠区。
#[derive(Debug, Clone)]
pub(crate) struct RightDock {
    layout: RightDockLayout,
    homes: HashMap<DockPanel, PanelHome>,
}

impl RightDock {
    pub(crate) fn new(layout: RightDockLayout) -> Self {
        Self {
            layout,
            homes: HashMap::new(),
        }
    }

    pub(crate) fn layout(&self) -> &RightDockLayout {
        &self.layout
    }

    /// 隐藏时布局记着，再显示原样回来；一个面板都没有时给默认布局。
    pub(crate) fn toggle_visible(&mut self) {
        if self.layout.visible {
            self.layout.visible = false;
            return;
        }
        if self.layout.groups.is_empty() {
            self.layout.groups = RightDockLayout::default().groups;
        }
        self.layout.visible = true;
    }

    pub(crate) fn toggle_panel(&mut self, panel: DockPanel) {
        if self.layout.shows(panel) {
            self.close(panel);
        } else {
            self.open(panel);
        }
    }

    /// 打开并显示：已经在某格里就切到它；否则回到上次的地方。栏没显示就一起显示。
    pub(crate) fn open(&mut self, panel: DockPanel) {
        self.layout.visible = true;
        if self.layout.contains(panel) {
            self.select(panel);
            return;
        }
        let home = self.homes.remove(&panel);
        let sibling_group = match home {
            Some(PanelHome::WithPanel(sibling)) => self.layout.group_of(sibling),
            _ => None,
        };
        // 新格子取现有每格的平均份额：规整（按比例缩放）以后它正好占 1/(n+1)，其余按原比例让出。
        let portion = PORTION_SCALE / self.layout.groups.len().max(1) as u16;
        match (sibling_group, home) {
            (Some(group), _) => self.join(panel, group),
            (None, Some(PanelHome::OwnGroup(index))) => self.insert_group(panel, index, portion),
            _ => self.insert_group(panel, self.layout.groups.len(), portion),
        }
    }

    /// 关掉一个面板，记下它回哪儿；最后一个面板关掉时栏随之不显示。
    pub(crate) fn close(&mut self, panel: DockPanel) {
        let Some(index) = self.layout.group_of(panel) else {
            return;
        };
        let home = self.take(panel);
        self.homes
            .insert(panel, home.unwrap_or(PanelHome::OwnGroup(index)));
        if self.layout.groups.is_empty() {
            self.layout.visible = false;
        }
    }

    /// 切到这个面板（它那格的标签）。
    pub(crate) fn select(&mut self, panel: DockPanel) {
        if let Some(index) = self.layout.group_of(panel) {
            let group = &mut self.layout.groups[index];
            group.active = group
                .panels
                .iter()
                .position(|candidate| *candidate == panel)
                .unwrap_or(group.active);
        }
    }

    /// 拖着的面板放下。放回自己独占的那格（叠进去、或贴着自己上下）什么都不变。
    pub(crate) fn move_panel(&mut self, panel: DockPanel, target: DropTarget) {
        let Some(from) = self.layout.group_of(panel) else {
            return;
        };
        let alone = self.layout.groups[from].panels.len() == 1;
        let target_group = match target {
            DropTarget::Join(index) | DropTarget::Above(index) | DropTarget::Below(index) => index,
        };
        if target_group >= self.layout.groups.len() || (alone && target_group == from) {
            return;
        }
        // 拿走以后格子可能少了一格，记下那格里另一个面板，拿走后按它重新找那格（不能用当前
        // 标签：拖的可能正是它）。
        let Some(&anchor) = self.layout.groups[target_group]
            .panels
            .iter()
            .find(|candidate| **candidate != panel)
        else {
            return;
        };
        self.take(panel);
        let Some(index) = self.layout.group_of(anchor) else {
            return;
        };
        match target {
            DropTarget::Join(_) => self.join(panel, index),
            DropTarget::Above(_) => self.split_off(panel, index, index),
            DropTarget::Below(_) => self.split_off(panel, index, index + 1),
        }
    }

    pub(crate) fn toggle_locked(&mut self) {
        self.layout.locked = !self.layout.locked;
    }

    pub(crate) fn set_width(&mut self, width: f32) {
        self.layout.width = normalize_right_dock_width(width);
    }

    /// 第 `upper` 格和下面那格之间的分隔线：上面那格占两格合计的 `upper_fraction`。
    pub(crate) fn set_split(&mut self, upper: usize, upper_fraction: f32) {
        if upper + 1 >= self.layout.groups.len() {
            return;
        }
        let total = self.layout.groups[upper].portion + self.layout.groups[upper + 1].portion;
        let first = (f32::from(total) * upper_fraction.clamp(0.0, 1.0)).round() as u16;
        let first = first.clamp(1, total.saturating_sub(1).max(1));
        self.layout.groups[upper].portion = first;
        self.layout.groups[upper + 1].portion = (total - first).max(1);
    }

    /// 从布局里拿走一个面板（格子空了就去掉，份额给相邻的格子），返回它该回的地方。
    fn take(&mut self, panel: DockPanel) -> Option<PanelHome> {
        let index = self.layout.group_of(panel)?;
        let group = &mut self.layout.groups[index];
        let position = group
            .panels
            .iter()
            .position(|candidate| *candidate == panel)?;
        group.panels.remove(position);
        if let Some(&sibling) = group.panels.first() {
            // 拿走的在当前标签前面，或者拿走的正是排在最后的当前标签：当前标签往前挪一个。
            if position < group.active || group.active == group.panels.len() {
                group.active -= 1;
            }
            return Some(PanelHome::WithPanel(sibling));
        }
        let freed = self.layout.groups.remove(index).portion;
        let neighbour = index.saturating_sub(1);
        if let Some(group) = self.layout.groups.get_mut(neighbour) {
            group.portion += freed;
        }
        Some(PanelHome::OwnGroup(index))
    }

    fn join(&mut self, panel: DockPanel, index: usize) {
        let group = &mut self.layout.groups[index];
        group.panels.push(panel);
        group.active = group.panels.len() - 1;
    }

    /// 从第 `from` 格分出一半份额，在 `at` 处给 `panel` 另起一格。
    fn split_off(&mut self, panel: DockPanel, from: usize, at: usize) {
        let group = &mut self.layout.groups[from];
        let half = (group.portion / 2).max(1);
        group.portion = group.portion.saturating_sub(half).max(1);
        self.insert_group(panel, at, half);
    }

    /// 在第 `index` 格（夹到末尾）插一格，插完把合计规整回 [`PORTION_SCALE`]。
    fn insert_group(&mut self, panel: DockPanel, index: usize, portion: u16) {
        let index = index.min(self.layout.groups.len());
        self.layout
            .groups
            .insert(index, DockGroup::single(panel, portion));
        self.layout.normalize_portions();
    }
}

#[cfg(test)]
#[path = "right_dock_tests.rs"]
mod tests;
