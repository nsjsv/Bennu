//! 选择窗口起始目录与视图模式的跨窗口记忆。记忆文件复用主应用的
//! `~/.config/bennu/` 配置目录，独立成 `portal.toml`，不与主程序配置耦合。
//! last_directory 与 view_mode 由同一个 `store_portal_memory` 原子写回，
//! 避免两个写者各写各的字段互相覆盖。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const PORTAL_MEMORY_FILE: &str = "portal.toml";
const APP_CONFIG_DIR: &str = "bennu";

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PortalMemory {
    pub(crate) last_directory: Option<PathBuf>,
    /// 视图模式（PickerViewMode::storage_value）；serde default 保证
    /// 旧版文件（无 view_mode 字段）照常解析。
    #[serde(default)]
    pub(crate) view_mode: Option<String>,
}

impl PortalMemory {
    /// 测试辅助：从 TOML 文本解析（生产读取走 load_portal_memory）。
    #[cfg(test)]
    pub(crate) fn parse_for_test(text: &str) -> Self {
        toml::from_str(text).unwrap_or_default()
    }

    /// 起始目录候选：只在绝对路径且真实存在时有效。
    pub(crate) fn remembered_directory(&self) -> Option<&Path> {
        self.last_directory
            .as_deref()
            .filter(|dir| dir.is_absolute() && dir.is_dir())
    }

    /// 存储的视图模式原始值（语义映射在 PickerViewMode::from_storage_value）。
    pub(crate) fn stored_view_mode(&self) -> Option<&str> {
        self.view_mode.as_deref()
    }
}

/// 起始目录优先级：调用方 `current_folder` > 上次记忆 > 主目录。
/// 各级候选必须存在且是目录，否则顺延下一级。记忆值由调用方注入，
/// 避免逻辑分支依赖真实 HOME 的文件状态。
pub(crate) fn resolve_start_directory(
    current_folder: Option<&Path>,
    remembered_directory: Option<&Path>,
) -> PathBuf {
    if let Some(folder) = current_folder {
        if folder.is_dir() {
            return folder.to_path_buf();
        }
    }
    if let Some(remembered) = remembered_directory {
        if remembered.is_dir() {
            return remembered.to_path_buf();
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// 读取记忆；文件缺失、损坏视为无记忆。目录有效性由 `remembered_directory`
/// 在使用处判定。
pub(crate) fn load_portal_memory() -> PortalMemory {
    memory_file_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| toml::from_str::<PortalMemory>(&text).ok())
        .unwrap_or_default()
}

/// 整体写回记忆；内容不变不写盘。先写临时文件再改名，读方永远不会
/// 观察到半个文件。
pub(crate) fn store_portal_memory(memory: &PortalMemory) {
    let Some(config_dir) = app_config_dir() else {
        return;
    };
    if load_portal_memory() == *memory {
        return;
    }
    let Ok(text) = toml::to_string_pretty(memory) else {
        return;
    };
    if std::fs::create_dir_all(&config_dir).is_ok() {
        let path = config_dir.join(PORTAL_MEMORY_FILE);
        let staging = config_dir.join(format!("{PORTAL_MEMORY_FILE}.new"));
        if std::fs::write(&staging, text).is_ok() && std::fs::rename(&staging, path).is_err() {
            let _ = std::fs::remove_file(&staging);
        }
    }
}

pub(crate) fn app_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join(APP_CONFIG_DIR))
}

fn memory_file_path() -> Option<PathBuf> {
    app_config_dir().map(|dir| dir.join(PORTAL_MEMORY_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 记忆文件读写依赖真实 HOME，测试只覆盖纯逻辑分支；涉及文件系统的
    // 行为通过 resolve_start_directory 的降级链验证（临时 HOME 不可移植
    // 的部分留给手动验收）。

    #[test]
    fn resolve_prefers_existing_caller_folder() {
        let existing = tempfile::tempdir().unwrap();
        let remembered = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_start_directory(Some(existing.path()), Some(remembered.path())),
            existing.path()
        );
    }

    #[test]
    fn resolve_prefers_remembered_when_caller_folder_missing() {
        let remembered = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_start_directory(
                Some(Path::new("/nonexistent-picker-folder")),
                Some(remembered.path())
            ),
            remembered.path()
        );
    }

    #[test]
    fn resolve_skips_missing_remembered_directory() {
        // 记忆路径无效时跳过该级，落到主目录。
        assert_eq!(
            resolve_start_directory(None, Some(Path::new("/nonexistent-picker-memory"))),
            dirs::home_dir().unwrap()
        );
    }

    #[test]
    fn resolve_final_fallback_is_home() {
        assert_eq!(
            resolve_start_directory(None, None),
            dirs::home_dir().unwrap()
        );
    }

    #[test]
    fn legacy_file_without_view_mode_field_still_parses() {
        // 旧版 portal.toml 只有 last_directory：serde default 保证解析
        // 成功、视图模式缺省为无值（会话侧回落 List）。
        let memory: PortalMemory = toml::from_str("last_directory = \"/tmp\"\n").unwrap();
        assert_eq!(memory.last_directory.as_deref(), Some(Path::new("/tmp")));
        assert_eq!(memory.stored_view_mode(), None);
        assert_eq!(memory.remembered_directory(), Some(Path::new("/tmp")));
    }

    #[test]
    fn view_mode_field_round_trips() {
        let memory: PortalMemory =
            toml::from_str("last_directory = \"/tmp\"\nview_mode = \"icons\"\n").unwrap();
        assert_eq!(memory.stored_view_mode(), Some("icons"));

        let written = toml::to_string_pretty(&memory).unwrap();
        let reparsed: PortalMemory = toml::from_str(&written).unwrap();
        assert_eq!(memory, reparsed);
    }

    #[test]
    fn corrupted_document_falls_back_to_default() {
        let memory: PortalMemory = toml::from_str("view_mode = [broken").unwrap_or_default();
        assert_eq!(memory, PortalMemory::default());
    }
}
