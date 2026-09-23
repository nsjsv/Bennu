//! 确认/覆盖/取消的收尾路径：OpenFile 按选中集回发路径，SaveFile 按
//! 名字定目标（已存在则进入两步覆盖确认），取消统一回 Cancelled。
//! 从 mod.rs 拆出以守住其 800 行上限；逻辑未变。

use std::path::PathBuf;

use super::{PickerSession, SessionEffect};
use crate::dbus_file_chooser::PickerResolution;
use crate::picker_request::PickerKind;

impl PickerSession {
    pub(crate) fn confirm(&mut self) -> SessionEffect {
        if self.overwrite_target.is_some() {
            return self.confirm_overwrite();
        }
        match &self.kind {
            PickerKind::OpenFile { .. } => {
                let mut paths: Vec<PathBuf> = self
                    .selection
                    .iter()
                    .filter_map(|&index| self.rows.get(index))
                    .map(|row| row.entry.path.clone())
                    .collect();
                paths.sort_unstable();
                paths.dedup();
                if paths.is_empty() {
                    return SessionEffect::None;
                }
                self.reply_confirmed(paths)
            }
            PickerKind::SaveFile { .. } => {
                let name = self.name_input.trim();
                if name.is_empty() {
                    return SessionEffect::None;
                }
                let target = self.directory.join(name);
                if target.exists() {
                    self.overwrite_target = Some(target);
                    return SessionEffect::None;
                }
                self.reply_confirmed(vec![target])
            }
        }
    }

    fn confirm_overwrite(&mut self) -> SessionEffect {
        let Some(target) = self.overwrite_target.take() else {
            return SessionEffect::None;
        };
        self.reply_confirmed(vec![target])
    }

    fn reply_confirmed(&mut self, paths: Vec<PathBuf>) -> SessionEffect {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(PickerResolution::Confirmed(paths.clone()));
        }
        SessionEffect::Confirmed(paths)
    }

    pub(crate) fn finish_cancelled(&mut self) -> SessionEffect {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(PickerResolution::Cancelled);
        }
        SessionEffect::Dismissed
    }
}
