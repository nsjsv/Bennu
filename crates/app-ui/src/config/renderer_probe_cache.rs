//! config.toml 的 `renderer_probe_cache` 段，归启动探针所有。它是缓存：读不懂就当
//! 没有，下次探针重写；也因此不值得为它改动一份读不懂的文件——启动修复要看到
//! 原文才能备份。

use std::io;
use std::path::Path;

use super::config_file::{
    config_file_path, read_config_file, write_config_file, ConfigFileContent,
};

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

pub(crate) fn load_renderer_probe_cache() -> Option<RendererProbeCacheRecord> {
    load_renderer_probe_cache_at(&config_file_path())
}

pub(crate) fn save_renderer_probe_cache(record: &RendererProbeCacheRecord) -> io::Result<()> {
    save_renderer_probe_cache_at(&config_file_path(), record)
}

pub(crate) fn clear_renderer_probe_cache() -> io::Result<()> {
    clear_renderer_probe_cache_at(&config_file_path())
}

pub(super) fn load_renderer_probe_cache_at(path: &Path) -> Option<RendererProbeCacheRecord> {
    match read_config_file(path).ok()? {
        ConfigFileContent::Parsed { document, .. } => parse_renderer_probe_cache(&document),
        ConfigFileContent::Missing | ConfigFileContent::Unparseable { .. } => None,
    }
}

pub(super) fn save_renderer_probe_cache_at(
    path: &Path,
    record: &RendererProbeCacheRecord,
) -> io::Result<()> {
    let mut document = writable_document(path)?.unwrap_or_default();
    document.insert(
        RENDERER_PROBE_CACHE_KEY.to_owned(),
        toml::Value::Table(renderer_probe_cache_table(record)),
    );
    write_config_file(path, &document)
}

pub(super) fn clear_renderer_probe_cache_at(path: &Path) -> io::Result<()> {
    let Some(mut document) = writable_document(path)? else {
        return Ok(());
    };
    if document.remove(RENDERER_PROBE_CACHE_KEY).is_some() {
        write_config_file(path, &document)?;
    }
    Ok(())
}

/// 现有文档（文件不存在为 None）；文件读不懂时拒绝写入。
fn writable_document(path: &Path) -> io::Result<Option<toml::Table>> {
    match read_config_file(path)? {
        ConfigFileContent::Missing => Ok(None),
        ConfigFileContent::Parsed { document, .. } => Ok(Some(document)),
        ConfigFileContent::Unparseable { .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "config.toml cannot be parsed; leaving it untouched for startup to back up",
        )),
    }
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
