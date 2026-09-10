//! 改名迁移：File Manager → Bennu。
//!
//! 旧版本把数据放在 XDG 数据目录的 `file-manager/` 下（状态数据库、偏好、
//! 操作记录、搜索索引都在其中）。改名后首次启动把旧目录整体搬移到
//! `bennu/`，保证用户数据零丢失。搬移在同一文件系统内是原子 rename；
//! 迁移必须幂等——新目录已存在或旧目录不存在时为空操作。任何失败只
//! 记录告警并继续，迁移绝不阻塞应用启动。

use std::path::Path;

use tracing::warn;

pub const LEGACY_DATA_DIR_NAME: &str = "file-manager";
pub const DATA_DIR_NAME: &str = "bennu";

/// 把 XDG 数据目录下的 `file-manager/` 搬移为 `bennu/`（若需要）。
/// `data_base_dir` 传入 `dirs::data_dir()` 的结果，由调用方解析——
/// 本 crate 不依赖 dirs。
pub fn migrate_legacy_data_dir(data_base_dir: &Path) {
    migrate_legacy_dir(
        &data_base_dir.join(LEGACY_DATA_DIR_NAME),
        &data_base_dir.join(DATA_DIR_NAME),
    );
}

/// 把 XDG 配置目录下的 `file-manager/` 搬移为 `bennu/`（若需要）。
/// 搜索路径配置（search-paths.json）与 matugen 模板输出在配置目录下。
pub fn migrate_legacy_config_dir(config_base_dir: &Path) {
    migrate_legacy_dir(
        &config_base_dir.join(LEGACY_DATA_DIR_NAME),
        &config_base_dir.join(DATA_DIR_NAME),
    );
}

/// 搬移单个旧目录；供缓存目录等次级位置复用。
pub fn migrate_legacy_dir(legacy_dir: &Path, target_dir: &Path) {
    if !legacy_dir.exists() || target_dir.exists() {
        return;
    }
    let Some(parent) = target_dir.parent() else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        warn!(
            ?legacy_dir,
            ?target_dir,
            %error,
            "could not create parent directory for data dir migration"
        );
        return;
    }
    if let Err(error) = std::fs::rename(legacy_dir, target_dir) {
        warn!(
            ?legacy_dir,
            ?target_dir,
            %error,
            "legacy data dir migration failed; continuing with a fresh directory"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_legacy_dir_to_target_when_target_is_absent() {
        let base = tempfile::tempdir().expect("temp base");
        let legacy = base.path().join("legacy");
        let target = base.path().join("target");
        std::fs::create_dir_all(legacy.join("nested")).expect("create legacy");
        std::fs::write(legacy.join("nested").join("state.db"), b"data").expect("write");

        migrate_legacy_dir(&legacy, &target);

        assert!(!legacy.exists());
        assert_eq!(
            std::fs::read(target.join("nested").join("state.db")).expect("read"),
            b"data"
        );
    }

    #[test]
    fn leaves_both_dirs_untouched_when_target_already_exists() {
        let base = tempfile::tempdir().expect("temp base");
        let legacy = base.path().join("legacy");
        let target = base.path().join("target");
        std::fs::create_dir(&legacy).expect("create legacy");
        std::fs::write(legacy.join("old"), b"old").expect("write old");
        std::fs::create_dir(&target).expect("create target");
        std::fs::write(target.join("new"), b"new").expect("write new");

        migrate_legacy_dir(&legacy, &target);

        assert!(legacy.join("old").exists());
        assert_eq!(std::fs::read(target.join("new")).expect("read"), b"new");
    }

    #[test]
    fn is_a_no_op_when_neither_dir_exists() {
        let base = tempfile::tempdir().expect("temp base");

        migrate_legacy_dir(&base.path().join("legacy"), &base.path().join("target"));

        assert!(!base.path().join("target").exists());
    }
}
