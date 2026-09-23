//! 条目点击/双击/全选的选中语义（从 mod.rs 拆出控制行数）：
//! SaveFile 取名、OpenFile 按模式过滤可选类型、shift/ctrl 多选。

use file_core::entry::FileKind;

use super::{PickerSession, SessionEffect};
use crate::picker_request::PickerKind;

impl PickerSession {
    pub(super) fn click_entry(&mut self, index: usize, ctrl: bool, shift: bool) {
        // 点击即落光标（鼠标落点=键盘导航位置，Enter/翻页/type-ahead 同源）。
        // 必须在可选性门控之前：文件模式下点目录不落选中集，但位置
        // 仍要可见，否则纯目录页里点击/方向键全部隐身。
        self.place_cursor(index);
        let multiple = matches!(self.kind, PickerKind::OpenFile { multiple: true, .. });
        match self.kind {
            PickerKind::SaveFile { .. } => {
                // SaveFile：文件点击=取其名；文件/目录都落选中集
                // （单击选中、双击进入的桌面惯例；空格预览目标取
                // selection_anchor，无选中则空格静默无操作）。
                if let Some(row) = self.rows.get(index) {
                    if row.entry.kind == FileKind::File {
                        self.name_input = row.entry.name.to_string_lossy().into_owned();
                        self.overwrite_targets.clear();
                    }
                }
            }
            PickerKind::OpenFile { directory, .. } => {
                if let Some(row) = self.rows.get(index) {
                    let selectable = if directory {
                        row.entry.kind == FileKind::Directory
                    } else {
                        row.entry.kind == FileKind::File
                    };
                    if !selectable {
                        // 单击/光标落在不可选行：位置照记，但上一次选中集
                        // 必须熄灭（资源管理器语义），否则文件选中+文件夹
                        // 光标出现双高亮。
                        self.selection.clear();
                        self.selection_anchor = None;
                        return;
                    }
                }
            }
            // SaveFiles 只选目录：点击仅高亮（落到底部通用选中逻辑），
            // 名字列表是调用方资产，不可被点击改写。
            PickerKind::SaveFiles { .. } => {}
        }
        if shift && multiple {
            if let Some(anchor) = self.selection_anchor {
                let (lo, hi) = (anchor.min(index), anchor.max(index));
                self.selection = (lo..=hi).collect();
            } else {
                self.selection = vec![index];
                self.selection_anchor = Some(index);
            }
        } else if ctrl && multiple {
            if let Some(position) = self.selection.iter().position(|&i| i == index) {
                self.selection.remove(position);
            } else {
                self.selection.push(index);
                self.selection.sort_unstable();
            }
            self.selection_anchor = Some(index);
        } else {
            self.selection = vec![index];
            self.selection_anchor = Some(index);
        }
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
    pub(super) fn select_all(&mut self) {
        let PickerKind::OpenFile {
            multiple: true,
            directory,
            ..
        } = &self.kind
        else {
            return;
        };
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
