use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{
    app_config_dir_path, default_user_config, toml_string, RenderingGpuPreference, UserConfig,
    CONFIG_FILE_NAME,
};

const THUMBNAIL_CACHE_DIR_KEY: &str = "thumbnail_cache_dir";
const RENDERING_BACKEND_KEY: &str = "rendering_backend";
const SEARCH_CONTENT_INDEXING_ENABLED_KEY: &str = "search_content_indexing_enabled";
const SEARCH_MAX_EXTRACT_BYTES_KEY: &str = "search_max_extract_bytes";
const RENDERER_PROBE_CACHE_KEY: &str = "renderer_probe_cache";

/// 渲染探针结果的落盘记录。字段保持原始字符串,语义校验由 startup_probe_cache
/// 完成;解析时任何字段类型不对就整段丢弃,由下次启动重新探针写回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RendererProbeCacheRecord {
    pub(crate) version: i64,
    pub(crate) backend: String,
    pub(crate) rendering_gpu_preference: String,
    pub(crate) wgpu_power_preference: Option<String>,
    pub(crate) mesa_vulkan_device_select: Option<String>,
    pub(crate) vulkan_loader_driver_select: Option<String>,
    pub(crate) display_gpu_device_select: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppConfig {
    pub(crate) thumbnail_cache_dir: PathBuf,
    pub(crate) rendering_gpu_preference: RenderingGpuPreference,
    pub(crate) search_content_indexing_enabled: bool,
    pub(crate) search_max_extract_bytes: u64,
    pub(crate) renderer_probe_cache: Option<RendererProbeCacheRecord>,
}

impl AppConfig {
    pub(crate) fn from_user_config(config: &UserConfig) -> Self {
        Self {
            thumbnail_cache_dir: config.thumbnail_cache_dir.clone(),
            rendering_gpu_preference: config.rendering_gpu_preference,
            search_content_indexing_enabled: config.search_content_indexing_enabled,
            search_max_extract_bytes: config.search_max_extract_bytes,
            // 探针缓存归启动探针路径所有;用户偏好持久化不携带它,
            // 保存时由 save_app_config_preserving_probe_cache 保留磁盘上的现有段。
            renderer_probe_cache: None,
        }
    }

    pub(crate) fn apply_to_user_config(&self, config: &mut UserConfig) {
        config.thumbnail_cache_dir = self.thumbnail_cache_dir.clone();
        config.rendering_gpu_preference = self.rendering_gpu_preference;
        config.search_content_indexing_enabled = self.search_content_indexing_enabled;
        config.search_max_extract_bytes = self.search_max_extract_bytes;
    }
}

pub(crate) fn default_app_config() -> AppConfig {
    AppConfig::from_user_config(&default_user_config())
}

pub(crate) fn load_app_config() -> AppConfig {
    let default = default_app_config();
    let Some(config_dir) = app_config_dir_path() else {
        return default;
    };

    load_app_config_from_dir(&config_dir, default)
}

pub(crate) fn save_app_config(config: &AppConfig) -> io::Result<()> {
    let config_file = config_file_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "application configuration directory is unavailable",
        )
    })?;
    write_app_config(&config_file, config)
}

/// 用户偏好持久化专用:调用方构造的 AppConfig 不携带探针缓存
/// (from_user_config 置 None),保存时保留磁盘上已有的缓存段。
/// 探针路径写回新结果必须走 save_app_config,不能走这里。
pub(crate) fn save_app_config_preserving_probe_cache(config: &AppConfig) -> io::Result<()> {
    let config_file = config_file_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "application configuration directory is unavailable",
        )
    })?;
    save_app_config_preserving_probe_cache_at(&config_file, config)
}

pub(super) fn save_app_config_preserving_probe_cache_at(
    config_file: &Path,
    config: &AppConfig,
) -> io::Result<()> {
    let mut config = config.clone();
    if config.renderer_probe_cache.is_none() {
        config.renderer_probe_cache = fs::read_to_string(config_file)
            .ok()
            .and_then(|content| content.parse::<toml::Table>().ok())
            .and_then(|document| parse_renderer_probe_cache(&document));
    }
    write_app_config(config_file, &config)
}

pub(super) fn load_app_config_from_dir(config_dir: &Path, default: AppConfig) -> AppConfig {
    let config_file = config_dir.join(CONFIG_FILE_NAME);
    match fs::read_to_string(&config_file) {
        Ok(content) => parse_toml_app_config(&content, default),
        Err(error) if error.kind() == io::ErrorKind::NotFound => default,
        Err(_) => default,
    }
}

pub(super) fn parse_toml_app_config(content: &str, default: AppConfig) -> AppConfig {
    let Ok(document) = content.parse::<toml::Table>() else {
        return default;
    };

    let mut config = default;
    if let Some(value) = toml_string(&document, THUMBNAIL_CACHE_DIR_KEY) {
        config.thumbnail_cache_dir = PathBuf::from(value);
    }
    if let Some(value) = toml_string(&document, RENDERING_BACKEND_KEY) {
        if let Some(preference) = RenderingGpuPreference::from_config_value(value) {
            config.rendering_gpu_preference = preference;
        }
    }
    if let Some(value) = document
        .get(SEARCH_CONTENT_INDEXING_ENABLED_KEY)
        .and_then(toml::Value::as_bool)
    {
        config.search_content_indexing_enabled = value;
    }
    if let Some(value) = document
        .get(SEARCH_MAX_EXTRACT_BYTES_KEY)
        .and_then(toml::Value::as_integer)
        .and_then(|value| u64::try_from(value).ok())
    {
        config.search_max_extract_bytes = value.max(1);
    }
    config.renderer_probe_cache = parse_renderer_probe_cache(&document);
    config
}

fn parse_renderer_probe_cache(document: &toml::Table) -> Option<RendererProbeCacheRecord> {
    let section = document.get(RENDERER_PROBE_CACHE_KEY)?.as_table()?;
    let table_string = |key: &str| -> Option<String> {
        section
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    Some(RendererProbeCacheRecord {
        version: section.get("version")?.as_integer()?,
        backend: table_string("backend")?,
        rendering_gpu_preference: table_string("rendering_gpu_preference")?,
        wgpu_power_preference: table_string("wgpu_power_preference"),
        mesa_vulkan_device_select: table_string("mesa_vulkan_device_select"),
        vulkan_loader_driver_select: table_string("vulkan_loader_driver_select"),
        display_gpu_device_select: table_string("display_gpu_device_select"),
    })
}

pub(super) fn write_app_config(path: &Path, config: &AppConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = config.thumbnail_cache_dir.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let content = toml_app_config_content(config).map_err(io::Error::other)?;
    fs::write(path, content)
}

pub(super) fn toml_app_config_content(config: &AppConfig) -> Result<String, toml::ser::Error> {
    let mut document = toml::Table::new();
    document.insert(
        THUMBNAIL_CACHE_DIR_KEY.to_owned(),
        toml::Value::String(config.thumbnail_cache_dir.to_string_lossy().into_owned()),
    );
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
    if let Some(record) = &config.renderer_probe_cache {
        document.insert(
            RENDERER_PROBE_CACHE_KEY.to_owned(),
            toml::Value::Table(renderer_probe_cache_table(record)),
        );
    }

    let content = toml::to_string_pretty(&document)?;
    Ok(format!(
        "# Bennu application configuration\n{content}"
    ))
}

fn renderer_probe_cache_table(record: &RendererProbeCacheRecord) -> toml::Table {
    let mut section = toml::Table::new();
    section.insert("version".to_owned(), toml::Value::Integer(record.version));
    section.insert(
        "backend".to_owned(),
        toml::Value::String(record.backend.clone()),
    );
    section.insert(
        "rendering_gpu_preference".to_owned(),
        toml::Value::String(record.rendering_gpu_preference.clone()),
    );
    // 可选字段直接省略键;写成空串会被解析回 Some("") 破坏 round-trip。
    let optional_string = |section: &mut toml::Table, key: &str, value: &Option<String>| {
        if let Some(value) = value {
            section.insert(key.to_owned(), toml::Value::String(value.clone()));
        }
    };
    optional_string(
        &mut section,
        "wgpu_power_preference",
        &record.wgpu_power_preference,
    );
    optional_string(
        &mut section,
        "mesa_vulkan_device_select",
        &record.mesa_vulkan_device_select,
    );
    optional_string(
        &mut section,
        "vulkan_loader_driver_select",
        &record.vulkan_loader_driver_select,
    );
    optional_string(
        &mut section,
        "display_gpu_device_select",
        &record.display_gpu_device_select,
    );
    section
}

fn config_file_path() -> Option<PathBuf> {
    app_config_dir_path().map(|path| path.join(CONFIG_FILE_NAME))
}
