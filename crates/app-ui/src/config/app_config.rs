//! config.toml 里归用户设置的三个键。文件的读写规则见 `config_file`，探针缓存段见
//! `renderer_probe_cache`。

use std::io;
use std::path::Path;

use super::config_file::{
    back_up_config_file, config_file_path, read_config_file, write_config_file, ConfigFileContent,
};
use super::settings_recovery::{ResetSettings, SettingsRecovery, SettingsSource};
use super::{default_user_config, RenderingGpuPreference, UserConfig};

const RENDERING_BACKEND_KEY: &str = "rendering_backend";
const SEARCH_CONTENT_INDEXING_ENABLED_KEY: &str = "search_content_indexing_enabled";
const SEARCH_MAX_EXTRACT_BYTES_KEY: &str = "search_max_extract_bytes";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppConfig {
    pub(crate) rendering_gpu_preference: RenderingGpuPreference,
    pub(crate) search_content_indexing_enabled: bool,
    pub(crate) search_max_extract_bytes: u64,
}

impl AppConfig {
    pub(crate) fn from_user_config(config: &UserConfig) -> Self {
        Self {
            rendering_gpu_preference: config.rendering_gpu_preference,
            search_content_indexing_enabled: config.search_content_indexing_enabled,
            search_max_extract_bytes: config.search_max_extract_bytes,
        }
    }

    pub(crate) fn apply_to_user_config(&self, config: &mut UserConfig) {
        config.rendering_gpu_preference = self.rendering_gpu_preference;
        config.search_content_indexing_enabled = self.search_content_indexing_enabled;
        config.search_max_extract_bytes = self.search_max_extract_bytes;
    }
}

pub(crate) fn default_app_config() -> AppConfig {
    AppConfig::from_user_config(&default_user_config())
}

/// 界面起来之前的读取方（渲染探针、GL 兜底重启）用：只读，读不了、读不懂的项用
/// 默认值。备份与提示归启动加载（`load_and_repair_app_config`）。
pub(crate) fn load_app_config() -> AppConfig {
    load_app_config_at(&config_file_path())
}

pub(super) fn load_app_config_at(path: &Path) -> AppConfig {
    match read_config_file(path) {
        Ok(ConfigFileContent::Parsed { document, .. }) => {
            app_config_from_document(&document).config
        }
        Ok(ConfigFileContent::Missing | ConfigFileContent::Unparseable { .. }) | Err(_) => {
            default_app_config()
        }
    }
}

/// 启动时读到的应用配置，以及读的过程中要告诉用户的事。
#[derive(Debug)]
pub(crate) struct LoadedAppConfig {
    pub(crate) config: AppConfig,
    pub(crate) recovery: Option<SettingsRecovery>,
}

/// 启动加载，规则与设置相同：读不懂的项恢复默认，原文件先备份再写回修复后的键；
/// 读不了时用默认值、本次不写回。
pub(crate) fn load_and_repair_app_config() -> LoadedAppConfig {
    load_and_repair_app_config_at(&config_file_path())
}

pub(super) fn load_and_repair_app_config_at(path: &Path) -> LoadedAppConfig {
    match read_config_file(path) {
        Err(error) => LoadedAppConfig {
            config: default_app_config(),
            recovery: Some(SettingsRecovery::Unavailable {
                source: SettingsSource::AppConfig,
                error: error.to_string(),
            }),
        },
        Ok(ConfigFileContent::Missing) => LoadedAppConfig {
            config: default_app_config(),
            recovery: None,
        },
        Ok(ConfigFileContent::Unparseable { text }) => repair_app_config(
            path,
            &text,
            toml::Table::new(),
            default_app_config(),
            ResetSettings::All,
        ),
        Ok(ConfigFileContent::Parsed { text, document }) => {
            let interpreted = app_config_from_document(&document);
            if interpreted.unreadable.is_empty() {
                return LoadedAppConfig {
                    config: interpreted.config,
                    recovery: None,
                };
            }
            let reset = ResetSettings::Keys(
                interpreted
                    .unreadable
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            );
            repair_app_config(path, &text, document, interpreted.config, reset)
        }
    }
}

/// 备份失败时原文件不动，本次也不再写回配置。备份成功而写回失败只记日志：原文件
/// 已有备份，之后照常保存会再写。
fn repair_app_config(
    path: &Path,
    original_text: &str,
    mut document: toml::Table,
    config: AppConfig,
    reset: ResetSettings,
) -> LoadedAppConfig {
    let recovery = match back_up_config_file(path, original_text) {
        Ok(backup) => {
            insert_app_config_keys(&mut document, &config);
            if let Err(error) = write_config_file(path, &document) {
                tracing::warn!(%error, backup = %backup.display(), "repaired config.toml was not written back");
            }
            SettingsRecovery::Restored {
                source: SettingsSource::AppConfig,
                reset,
                backup,
            }
        }
        Err(error) => SettingsRecovery::NotBackedUp {
            source: SettingsSource::AppConfig,
            error: error.to_string(),
        },
    };
    LoadedAppConfig {
        config,
        recovery: Some(recovery),
    }
}

/// 只写这三个键，文件里其余的键（探针缓存段、本版本不认识的键）原样保留；现有
/// 文件读不懂时先备份再写。
pub(crate) fn save_app_config(config: &AppConfig) -> io::Result<()> {
    save_app_config_at(&config_file_path(), config)
}

pub(super) fn save_app_config_at(path: &Path, config: &AppConfig) -> io::Result<()> {
    let mut document = match read_config_file(path)? {
        ConfigFileContent::Missing => toml::Table::new(),
        ConfigFileContent::Parsed { document, .. } => document,
        ConfigFileContent::Unparseable { text } => {
            back_up_config_file(path, &text)?;
            toml::Table::new()
        }
    };
    insert_app_config_keys(&mut document, config);
    write_config_file(path, &document)
}

/// `app_config_from_document` 的结果：配置，以及存在但读不懂、已用默认值的键。
struct InterpretedAppConfig {
    config: AppConfig,
    unreadable: Vec<&'static str>,
}

fn app_config_from_document(document: &toml::Table) -> InterpretedAppConfig {
    let mut config = default_app_config();
    let mut unreadable = Vec::new();
    if let Some(preference) = read_key(document, RENDERING_BACKEND_KEY, &mut unreadable, |value| {
        value
            .as_str()
            .and_then(RenderingGpuPreference::from_config_value)
    }) {
        config.rendering_gpu_preference = preference;
    }
    if let Some(enabled) = read_key(
        document,
        SEARCH_CONTENT_INDEXING_ENABLED_KEY,
        &mut unreadable,
        toml::Value::as_bool,
    ) {
        config.search_content_indexing_enabled = enabled;
    }
    if let Some(bytes) = read_key(
        document,
        SEARCH_MAX_EXTRACT_BYTES_KEY,
        &mut unreadable,
        |value| {
            value
                .as_integer()
                .and_then(|bytes| u64::try_from(bytes).ok())
        },
    ) {
        config.search_max_extract_bytes = bytes.max(1);
    }
    InterpretedAppConfig { config, unreadable }
}

/// 键不存在为 None（用默认值，旧版本没有这一项不算读不懂）；存在但 `read` 解释不了
/// 时记入 `unreadable`。
fn read_key<'a, T>(
    document: &'a toml::Table,
    key: &'static str,
    unreadable: &mut Vec<&'static str>,
    read: impl FnOnce(&'a toml::Value) -> Option<T>,
) -> Option<T> {
    let value = read(document.get(key)?);
    if value.is_none() {
        unreadable.push(key);
    }
    value
}

fn insert_app_config_keys(document: &mut toml::Table, config: &AppConfig) {
    document.insert(
        RENDERING_BACKEND_KEY.to_owned(),
        toml::Value::String(config.rendering_gpu_preference.config_value().to_owned()),
    );
    document.insert(
        SEARCH_CONTENT_INDEXING_ENABLED_KEY.to_owned(),
        toml::Value::Boolean(config.search_content_indexing_enabled),
    );
    document.insert(
        SEARCH_MAX_EXTRACT_BYTES_KEY.to_owned(),
        toml::Value::Integer(config.search_max_extract_bytes as i64),
    );
}
