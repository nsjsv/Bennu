//! 条目点击/双击/全选的选中语义（从 mod.rs 拆出控制行数）：
//! SaveFile 取名、OpenFile 按模式过滤可选类型、shift/ctrl 多选。

use file_core::entry::FileKind;

use super::{PickerSession, SessionEffect};
use crate::picker_request::PickerKind;

impl PickerSession {
    pub(super) fn click_entry(&mut self, index: usize, ctrl: bool, shift: bool) {
        let multiple = matches!(self.kind, PickerKind::OpenFile { multiple: true, .. });
        match self.kind {
            PickerKind::SaveFile { .. } => {
                // SaveFile：文件点击=取其名；文件/目录都落选中集
                // （单击选中、双击进入的桌面惯例；空格预览目标取
                // selection_anchor，无选中则空格静默无操作）。
                if let Some(row) = self.rows.get(index) {
                    if row.entry.kind == FileKind::File {
                        self.name_input = row.entry.name.to_string_lossy().into_owned();
                        self.overwrite_target = None;
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
                        return;
                    }
                }
            }
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
                self.overwrite_target = None;
                SessionEffect::None
            }
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
