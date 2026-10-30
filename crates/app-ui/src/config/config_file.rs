//! config.toml 这一个文件的读写规则（参照 KDE KConfig）：两个写入方——用户设置
//! （`app_config`）与渲染探针缓存（`renderer_probe_cache`）——各写各的键，文件里
//! 其余的键原样保留；语法坏的文件不经备份不会被覆盖。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::CONFIG_FILE_NAME;

pub(super) enum ConfigFileContent {
    Missing,
    Parsed {
        /// 原文：有读不懂的项要修复时据此备份。
        text: String,
        document: toml::Table,
    },
    Unparseable {
        text: String,
    },
}

pub(super) fn config_file_path() -> PathBuf {
    file_core::app_dirs::app_config_dir().join(CONFIG_FILE_NAME)
}

pub(super) fn read_config_file(path: &Path) -> io::Result<ConfigFileContent> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ConfigFileContent::Missing)
        }
        Err(error) => return Err(error),
    };
    Ok(match text.parse::<toml::Table>() {
        Ok(document) => ConfigFileContent::Parsed { text, document },
        Err(_) => ConfigFileContent::Unparseable { text },
    })
}

pub(super) fn write_config_file(path: &Path, document: &toml::Table) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(document).map_err(io::Error::other)?;
    fs::write(
        path,
        format!("# Bennu application configuration\n{content}"),
    )
}

/// 原文另存为同目录的 `config.toml.<Unix 毫秒>.bak`（`create_new`，不覆盖已有备份），
/// 写完落盘。
pub(super) fn back_up_config_file(path: &Path, text: &str) -> io::Result<PathBuf> {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    let mut backup = path.as_os_str().to_owned();
    backup.push(format!(".{}.bak", since_epoch.as_millis()));
    let backup = PathBuf::from(backup);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(backup)
}
