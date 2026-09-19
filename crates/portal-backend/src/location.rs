//! 选择窗口起始目录决策与"上次目录"记忆。记忆文件复用主应用的
//! `~/.config/bennu/` 配置目录，独立成 `portal.toml`，不与主程序配置耦合。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const PORTAL_MEMORY_FILE: &str = "portal.toml";
const APP_CONFIG_DIR: &str = "bennu";

#[derive(Debug, Default, Serialize, Deserialize)]
struct PortalMemory {
    last_directory: Option<PathBuf>,
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

/// 读取上次选择目录；文件缺失、损坏、路径非法均视为无记忆。
pub(crate) fn load_last_directory() -> Option<PathBuf> {
    let path = memory_file_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let memory: PortalMemory = toml::from_str(&text).ok()?;
    memory
        .last_directory
        .filter(|dir| dir.is_absolute() && dir.is_dir())
}

/// 记录上次选择目录；同目录不重复写盘。
pub(crate) fn store_last_directory(directory: &Path) {
    let Some(config_dir) = app_config_dir() else {
        return;
    };
    if let Some(current) = load_last_directory() {
        if current == directory {
            return;
        }
    }
    let memory = PortalMemory {
        last_directory: Some(directory.to_path_buf()),
    };
    let Ok(text) = toml::to_string_pretty(&memory) else {
        return;
    };
    if std::fs::create_dir_all(&config_dir).is_ok() {
        let path = config_dir.join(PORTAL_MEMORY_FILE);
        // 先写临时文件再改名，读方永远不会观察到半个文件。
        let staging = config_dir.join(format!("{PORTAL_MEMORY_FILE}.new"));
        if std::fs::write(&staging, text).is_ok()
            && std::fs::rename(&staging, path).is_err()
        {
            let _ = std::fs::remove_file(&staging);
        }
    }
}

fn app_config_dir() -> Option<PathBuf> {
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
            resolve_start_directory(
                Some(existing.path()),
                Some(remembered.path())
            ),
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
            resolve_start_directory(
                None,
                Some(Path::new("/nonexistent-picker-memory"))
            ),
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
}
