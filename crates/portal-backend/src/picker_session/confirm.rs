//! 确认/覆盖/取消的收尾路径：OpenFile 按选中集回发路径，SaveFile 按
//! 名字定目标（已存在则进入两步覆盖确认），SaveFiles 批量定目标，取
//! 消统一回 Cancelled。从 mod.rs 拆出以守住其 800 行上限。

use std::path::{Path, PathBuf};

use super::{PickerSession, SessionEffect};
use crate::dbus_file_chooser::{PickerResolution, ResolutionPayload};
use crate::picker_request::{FilePattern, FilterRule, PickerKind};

/// 扩展名自动补全：只在激活规则的第一个 `*.ext` 型 glob 能确定唯一
/// 扩展名时追加（PRD R1）。MIME 模式不映射扩展名；用户已带扩展名
/// （`Path::extension` 语义）尊重用户输入。已知边角（接受）：
/// `.bashrc` 无扩展名会被补成 `.bashrc.png`；`photo.` 的空扩展名按
/// "已有"处理不改写。
fn complete_extension(name: &str, rule: Option<&FilterRule>) -> String {
    let Some(rule) = rule else {
        return name.to_string();
    };
    if Path::new(name).extension().is_some() {
        return name.to_string();
    }
    for pattern in &rule.patterns {
        let FilePattern::Glob(glob) = pattern else {
            continue;
        };
        // `*` 后必须紧跟 `.`，ext 内不得再有通配符，才能保证唯一扩
        // 展名；`*.tar.gz` 解析出 "tar.gz" 直接追加即正确。
        let Some(ext) = glob
            .strip_prefix('*')
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            continue;
        };
        if ext.contains(['*', '?', '[']) {
            continue;
        }
        return format!("{name}.{ext}");
    }
    name.to_string()
}

impl PickerSession {
    pub(crate) fn confirm(&mut self) -> SessionEffect {
        // 回收站视图禁止 SaveFile/SaveFiles 确认（can_confirm 已禁用
        // 按钮，这里拦住输入框回车）：trash:/// 不是可写目标。
        // OpenFile 照常确认，返回条目的真实载荷路径。
        if self.is_trash_view()
            && matches!(
                self.kind,
                PickerKind::SaveFile { .. } | PickerKind::SaveFiles { .. }
            )
        {
            return SessionEffect::None;
        }
        if !self.overwrite_targets.is_empty() {
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
                // 补全在 join+exists 之前：覆盖确认文案与实际落盘名字
                // 一致（"照片"+已有"照片.png"须触发覆盖确认）。
                let completed = complete_extension(name, self.active_filter.active_rule());
                let target = self.directory.join(completed);
                if target.exists() {
                    self.overwrite_targets = vec![target];
                    return SessionEffect::None;
                }
                self.reply_confirmed(vec![target])
            }
            PickerKind::SaveFiles { default_names } => {
                if default_names.is_empty() {
                    return SessionEffect::None;
                }
                let targets = self.batch_targets(default_names);
                // 只把冲突子集送进覆盖确认；确认后仍返回全部目标（写
                // 文件由调用方执行，覆盖确认只是 UI 层防误覆盖）。
                let conflicts: Vec<PathBuf> = targets
                    .into_iter()
                    .filter(|target| target.exists())
                    .collect();
                if !conflicts.is_empty() {
                    self.overwrite_targets = conflicts;
                    return SessionEffect::None;
                }
                self.reply_confirmed(self.batch_targets(default_names))
            }
        }
    }

    /// SaveFiles 的全部确认目标：目录 / 每个名字（名字列表是调用方
    /// 资产，永不改写）。
    fn batch_targets(&self, names: &[String]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| self.directory.join(name))
            .collect()
    }

    fn confirm_overwrite(&mut self) -> SessionEffect {
        // SaveFile：冲突集即全部目标（单目标，扩展名已在首次判定时
        // 补全，不重算）；SaveFiles：重算全部目标，冲突子集只是触发
        // 条件，确认后含未冲突项一并返回。
        let conflicts = std::mem::take(&mut self.overwrite_targets);
        if conflicts.is_empty() {
            return SessionEffect::None;
        }
        let targets = match &self.kind {
            PickerKind::SaveFiles { default_names } => self.batch_targets(default_names),
            _ => conflicts,
        };
        self.reply_confirmed(targets)
    }

    fn reply_confirmed(&mut self, paths: Vec<PathBuf>) -> SessionEffect {
        if let Some(reply) = self.reply.take() {
            let payload = ResolutionPayload {
                paths: paths.clone(),
                choices: self.selected_choices_for_reply(),
                current_filter: self.active_filter.active_rule().cloned(),
            };
            let _ = reply.send(PickerResolution::Confirmed(payload));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(patterns: Vec<FilePattern>) -> FilterRule {
        FilterRule {
            name: "测试".to_string(),
            patterns,
        }
    }

    #[test]
    fn no_rule_or_mime_or_wildcard_ext_left_untouched() {
        // 无过滤规则：不补。
        assert_eq!(complete_extension("照片", None), "照片");
        // MIME 模式不映射扩展名。
        let mime = rule(vec![FilePattern::Mime("image/png".to_string())]);
        assert_eq!(complete_extension("照片", Some(&mime)), "照片");
        // `*.*` / 带通配符的 ext 无法确定唯一扩展名，不补。
        let wildcards = rule(vec![
            FilePattern::Glob("*.*".to_string()),
            FilePattern::Glob("*.p*g".to_string()),
            FilePattern::Glob("*.p[n]g".to_string()),
        ]);
        assert_eq!(complete_extension("照片", Some(&wildcards)), "照片");
        // MIME 在前也跳过，继续找后面的 glob。
        let mixed = rule(vec![
            FilePattern::Mime("image/png".to_string()),
            FilePattern::Glob("*.png".to_string()),
        ]);
        assert_eq!(complete_extension("照片", Some(&mixed)), "照片.png");
    }

    #[test]
    fn existing_extension_is_respected() {
        let png = rule(vec![FilePattern::Glob("*.png".to_string())]);
        assert_eq!(complete_extension("照片.bmp", Some(&png)), "照片.bmp");
        // "photo." 的空扩展名按已有处理（接受此边角）。
        assert_eq!(complete_extension("照片.", Some(&png)), "照片.");
    }

    #[test]
    fn first_parseable_glob_wins_including_multi_dot() {
        // 多模式取第一个可解析扩展名的 glob。
        let multi = rule(vec![
            FilePattern::Glob("*.jpe?g".to_string()),
            FilePattern::Glob("*.png".to_string()),
        ]);
        assert_eq!(complete_extension("照片", Some(&multi)), "照片.png");
        // `*.tar.gz`：ext "tar.gz" 直接追加。
        let tar = rule(vec![FilePattern::Glob("*.tar.gz".to_string())]);
        assert_eq!(complete_extension("包", Some(&tar)), "包.tar.gz");
        assert_eq!(complete_extension("包.tar", Some(&tar)), "包.tar");
    }
}
