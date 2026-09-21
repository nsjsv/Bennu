//! 目录展开/收起动画（访达列表语义）：动画进度推进、行级联同步与
//! 收起完成后的行重建。独立成文件控制 mod.rs 的行数。

use std::path::PathBuf;

use super::{DirectoryListing, PickerSession};

/// 展开动画每帧步进：60Hz 下单程约 165ms，与主应用列表展开节奏一致。
const EXPANSION_ANIMATION_STEP: f32 = 0.18;

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
