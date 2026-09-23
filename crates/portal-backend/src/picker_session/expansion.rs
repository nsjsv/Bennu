//! 目录展开/收起动画（访达列表语义）：动画进度推进、行级联同步与
//! 收起完成后的行重建。独立成文件控制 mod.rs 的行数。

use std::path::PathBuf;

use file_core::entry::DirectoryEntry;

use super::{DirectoryListing, PickerSession};

/// 列表行高与行间距：行几何常量的唯一真值（PickerRow 所在地），view/
/// thumbnails/keyboard_nav 共用，消除各自硬编码的漂移。
pub(crate) const LIST_ROW_HEIGHT: f32 = 28.0;
/// 列表行间距：view.rs 列容器的 spacing 同值。
pub(crate) const LIST_ROW_SPACING: f32 = 2.0;
/// 列表行几何步长：行高 + 列间距。滚动
/// 偏移按行数累积成 offset = index × 步长，必须用步长而不是裸行高换算
/// ——间距会随行数累积成漂移，深滚动位置下可见余量会被它吃掉。
pub(crate) const LIST_ROW_STRIDE: f32 = LIST_ROW_HEIGHT + LIST_ROW_SPACING;

/// 展开动画每帧步进：60Hz 下单程约 165ms，与主应用列表展开节奏一致。
const EXPANSION_ANIMATION_STEP: f32 = 0.18;

/// 扁平化后的可见行：根条目与已展开子级按深度排列。行模型与展开
/// 动画字段（height/expand_progress）同属行几何语义，放本模块。
pub(crate) struct PickerRow {
    pub(crate) entry: DirectoryEntry,
    pub(crate) depth: usize,
    /// 本行自身高度比例：祖先展开进度的级联（本行不裁自己），驱动
    /// 行高裁剪动画；根行恒 1.0。
    pub(crate) height_progress: f32,
    /// 本行目录自身的展开动画进度（0=未展开，1=完全展开），驱动
    /// 箭头旋转；非目录或未展开行恒 0。
    pub(crate) expand_progress: f32,
}

/// 一个已展开目录的子内容（访达语义：子级按父深度 +1 缩进）。
pub(crate) struct ExpansionState {
    pub(crate) listing: DirectoryListing,
    /// 收起动画播放中：子行保留在行列表里缩回，播完才真正移除。
    pub(crate) is_collapsing: bool,
    /// 0.0→1.0 展开；1.0→0.0 收起。与 listing 解耦：扫描完成前后
    /// 动画都独立推进。
    pub(crate) animation_progress: f32,
}

impl PickerSession {
    /// 推进所有展开/收起动画一帧。帧时钟的挂载/摘除由 `is_animating`
    /// 决定，与本返回值无关。
    pub(crate) fn advance_animations(&mut self) -> ExpansionFrame {
        let mut changed = false;
        for state in self.expansions.values_mut() {
            if state.is_collapsing {
                let next = (state.animation_progress - EXPANSION_ANIMATION_STEP).max(0.0);
                if next < state.animation_progress {
                    state.animation_progress = next;
                    changed = true;
                }
            } else if state.animation_progress < 1.0 {
                let next = (state.animation_progress + EXPANSION_ANIMATION_STEP).min(1.0);
                if next > state.animation_progress {
                    state.animation_progress = next;
                    changed = true;
                }
            }
        }
        // 收起播完的条目此刻才真正移除：行列表的结构性变化集中在这里。
        let finished: Vec<PathBuf> = self
            .expansions
            .iter()
            .filter(|(_, state)| state.is_collapsing && state.animation_progress <= f32::EPSILON)
            .map(|(path, _)| path.clone())
            .collect();
        if finished.is_empty() {
            if changed {
                self.sync_row_animation();
            }
            return ExpansionFrame {
                changed,
                collapse_finished: false,
            };
        }
        for path in &finished {
            self.expansions.remove(path);
        }
        self.refresh_rows();
        ExpansionFrame {
            changed: true,
            collapse_finished: true,
        }
    }

    /// 行动画字段同步：行序即 DFS 序，用深度栈维护祖先级联进度，
    /// 避免每帧重建行列表（refresh_rows 会克隆全部条目）。
    pub(crate) fn sync_row_animation(&mut self) {
        let mut ancestors: Vec<(usize, f32)> = Vec::new();
        for row in self.rows.iter_mut() {
            while ancestors
                .last()
                .is_some_and(|(depth, _)| *depth >= row.depth)
            {
                ancestors.pop();
            }
            row.height_progress = ancestors.last().map_or(1.0, |(_, progress)| *progress);
            if let Some(state) = self.expansions.get(&row.entry.path) {
                row.expand_progress = state.animation_progress;
                ancestors.push((row.depth, state.animation_progress));
            } else {
                row.expand_progress = 0.0;
            }
        }
    }
}

/// 一帧展开动画的推进结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExpansionFrame {
    /// 本帧有任何进度推进（动画仍需帧时钟）。
    pub(crate) changed: bool,
    /// 本帧有收起动画播完并触发行列表结构性收缩：内容高度骤变，
    /// 调用方据此核实滚动条溢出，别让视口缓存陈旧到 auto-hide。
    pub(crate) collapse_finished: bool,
}
