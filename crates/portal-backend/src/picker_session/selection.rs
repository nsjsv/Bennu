//! 条目点击/双击/全选的选中语义（从 mod.rs 拆出控制行数）：
//! SaveFile 取名、OpenFile 按模式过滤可选类型、shift/ctrl 多选。

use file_core::entry::{DirectoryEntry, FileKind};

use super::{PickerSession, PickerViewMode, SessionEffect};
use crate::picker_request::PickerKind;

impl PickerSession {
    /// 点击/键盘落行的模式门控与 SaveFile 名字同步（列表与多栏共用）：
    /// 返回 false = 该行在当前请求模式下不可选（选中集由调用方按各
    /// 自的行模型清空）。
    pub(super) fn entry_click_gating(&mut self, entry: &DirectoryEntry) -> bool {
        match &self.kind {
            PickerKind::SaveFile { .. } => {
                // SaveFile：文件点击 = 取其名（桌面惯例；空格预览目标
                // 取 selection_anchor，无选中则静默无操作）。
                if entry.kind == FileKind::File {
                    self.name_input = entry.name.to_string_lossy().into_owned();
                    self.overwrite_targets.clear();
                }
                true
            }
            PickerKind::OpenFile { directory, .. } => {
                if *directory {
                    entry.kind == FileKind::Directory
                } else {
                    entry.kind == FileKind::File
                }
            }
            // SaveFiles 只选目录：点击仅高亮（落到底部通用选中逻辑），
            // 名字列表是调用方资产，不可被点击改写。
            PickerKind::SaveFiles { .. } => true,
        }
    }

    /// 单击选中集算术（列表与多栏共用）：shift 区间 / ctrl 翻转 /
    /// 普通单选；锚点语义同列表。
    pub(super) fn selection_after_click(
        current: &[usize],
        anchor: Option<usize>,
        index: usize,
        ctrl: bool,
        shift: bool,
        multiple: bool,
    ) -> (Vec<usize>, Option<usize>) {
        if shift && multiple {
            if let Some(anchor) = anchor {
                let (lo, hi) = (anchor.min(index), anchor.max(index));
                ((lo..=hi).collect(), Some(anchor))
            } else {
                (vec![index], Some(index))
            }
        } else if ctrl && multiple {
            let mut selection = current.to_vec();
            if let Some(position) = selection.iter().position(|&i| i == index) {
                selection.remove(position);
            } else {
                selection.push(index);
                selection.sort_unstable();
            }
            (selection, Some(index))
        } else {
            (vec![index], Some(index))
        }
    }

    pub(super) fn click_entry(&mut self, index: usize, ctrl: bool, shift: bool) {
        // 点击即落光标（鼠标落点=键盘导航位置，Enter/翻页/type-ahead 同源）。
        // 必须在可选性门控之前：文件模式下点目录不落选中集，但位置
        // 仍要可见，否则纯目录页里点击/方向键全部隐身。
        self.place_cursor(index);
        let multiple = matches!(self.kind, PickerKind::OpenFile { multiple: true, .. });
        // 克隆条目终结行借用：门控要改写会话状态（SaveFile 名字）。
        let clicked = self.rows.get(index).map(|row| row.entry.clone());
        if let Some(entry) = clicked {
            if !self.entry_click_gating(&entry) {
                // 单击/光标落在不可选行：位置照记，但上一次选中集
                // 必须熄灭（资源管理器语义），否则文件选中+文件夹
                // 光标出现双高亮。
                self.selection.clear();
                self.selection_anchor = None;
                return;
            }
        }
        let (selection, anchor) = Self::selection_after_click(
            &self.selection,
            self.selection_anchor,
            index,
            ctrl,
            shift,
            multiple,
        );
        self.selection = selection;
        self.selection_anchor = anchor;
    }

    /// 行高亮：显式选中集，或键盘光标所在行。光标在可选行上时两者
    /// 重合；在不可选行（文件模式下的目录）上时靠光标保持可见。
    pub(crate) fn row_highlighted(&self, index: usize) -> bool {
        self.selection.contains(&index) || self.list_cursor() == Some(index)
    }

    pub(super) fn activate_entry(&mut self, index: usize) -> SessionEffect {
        let Some(row) = self.rows.get(index) else {
            return SessionEffect::None;
        };
        if row.entry.kind == FileKind::Directory {
            return self.begin_navigation(row.entry.path.clone());
        }
        // 文件双击 = 选中并立即确认（桌面选择器惯例）。
        match self.kind {
            PickerKind::OpenFile {
                multiple: false, ..
            } => {
                self.selection = vec![index];
                self.confirm()
            }
            PickerKind::OpenFile { multiple: true, .. } => {
                if !self.selection.contains(&index) {
                    self.selection.push(index);
                    self.selection.sort_unstable();
                }
                SessionEffect::None
            }
            PickerKind::SaveFile { .. } => {
                self.name_input = row.entry.name.to_string_lossy().into_owned();
                self.overwrite_targets.clear();
                SessionEffect::None
            }
            // SaveFiles 双击文件不确认：返回集是调用方名字列表，与
            // 单个条目无关（目录双击在 match 之前已走导航）。
            PickerKind::SaveFiles { .. } => SessionEffect::None,
        }
    }

    /// Ctrl+A：仅 OpenFile 多选模式响应；按模式的可选类型圈定范围。
    /// 多栏下作用域 = 焦点栏（选中集不跨栏）。
    pub(super) fn select_all(&mut self) {
        let PickerKind::OpenFile {
            multiple: true,
            directory,
            ..
        } = &self.kind
        else {
            return;
        };
        if self.view_mode() == PickerViewMode::Columns {
            self.columns_select_all(*directory);
            return;
        }
        self.selection = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                if *directory {
                    row.entry.kind == FileKind::Directory
                } else {
                    row.entry.kind == FileKind::File
                }
            })
            .map(|(index, _)| index)
            .collect();
        self.selection_anchor = self.selection.first().copied();
    }
}
