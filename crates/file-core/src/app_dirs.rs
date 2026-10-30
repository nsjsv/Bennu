//! 主目录、用户数据/配置/缓存目录和本应用目录的唯一取法，照 GLib 的
//! `g_get_home_dir`、`g_get_user_data_dir` 等。主程序、文件选择器、搜索服务、预览缓存、
//! 缩略图和回收站都从这里取。目录总有值：写不进去时由各功能照常报错，
//! 不退到当前目录或空路径。

use std::ffi::OsString;
use std::path::PathBuf;

use crate::data_dir_migration::DATA_DIR_NAME;

/// 同 GLib：`$HOME`，否则系统账户里的主目录；都没有时用 `/` 并记警告。
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| {
        tracing::warn!(
            target: "file_core::app_dirs",
            event = "home_directory_unavailable",
            "neither HOME nor the account database names a home directory; using /"
        );
        PathBuf::from("/")
    })
}

pub fn user_data_dir() -> PathBuf {
    xdg_base_dir(std::env::var_os("XDG_DATA_HOME"), ".local/share")
}

pub fn user_config_dir() -> PathBuf {
    xdg_base_dir(std::env::var_os("XDG_CONFIG_HOME"), ".config")
}

pub fn user_cache_dir() -> PathBuf {
    xdg_base_dir(std::env::var_os("XDG_CACHE_HOME"), ".cache")
}

pub fn app_data_dir() -> PathBuf {
    user_data_dir().join(DATA_DIR_NAME)
}

pub fn app_config_dir() -> PathBuf {
    user_config_dir().join(DATA_DIR_NAME)
}

pub fn app_cache_dir() -> PathBuf {
    user_cache_dir().join(DATA_DIR_NAME)
}

/// XDG 规范：变量里的相对路径无效，忽略，改用主目录下的默认位置。
fn xdg_base_dir(variable: Option<OsString>, under_home: &str) -> PathBuf {
    variable
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home_dir().join(under_home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_variables_are_used_only_when_absolute() {
        assert_eq!(
            xdg_base_dir(Some("/xdg/data".into()), ".local/share"),
            PathBuf::from("/xdg/data")
        );
        assert_eq!(
            xdg_base_dir(Some("relative/data".into()), ".local/share"),
            home_dir().join(".local/share")
        );
        assert_eq!(
            xdg_base_dir(Some("".into()), ".cache"),
            home_dir().join(".cache")
        );
        assert_eq!(xdg_base_dir(None, ".config"), home_dir().join(".config"));
        assert!(app_data_dir().ends_with(DATA_DIR_NAME));
    }
}
