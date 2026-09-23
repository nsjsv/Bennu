//! 界面文案表（英→中翻译）与 UiLanguage。自 app-ui 的 localization.rs
//! 与 config.rs 纯搬移下沉：预览视图的 readable_text 自动翻译与
//! app-ui 视图必须同源消费这张表，视图组迁入 bennu-preview 前先
//! 解除对 app-ui 的依赖；文案逐字节保真，禁止在两处复制。

use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, Ordering};

mod batch_rename;
mod convert;
mod document_preview;
mod exact_translation;
mod file_operation_notifications;
mod index_status;
mod properties;
mod search_paths;
mod search_service_recovery;
mod search_service_status;
mod search_workspace;
mod transfer;
mod trash;

/// 界面语言。自 app-ui config.rs 下沉：翻译表按它分派，语言设置
/// （UiLanguageSetting 与 TOML 存取）仍属 app-ui 存储域，经 re-export
/// 同源引用本枚举，避免两侧各持一份定义产生分叉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLanguage {
    English,
    Chinese,
}

impl UiLanguage {
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::English => 0,
            Self::Chinese => 1,
        }
    }

    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Chinese,
            _ => Self::English,
        }
    }
}

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(UiLanguage::English.as_u8());

/// 字节量的短格式展示（传输进度行与文件清单共用）。自 app-ui
/// view/transfer_window.rs 随传输词条下沉：transfer 进度行在文案表
/// 内消费它，留在视图层会让本 crate 反向依赖宿主 UI。
pub fn format_byte_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * KIB;
    const GIB: f64 = 1024.0 * MIB;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

pub fn set_current_language(language: UiLanguage) {
    CURRENT_LANGUAGE.store(language.as_u8(), Ordering::Relaxed);
}

pub fn current_language() -> UiLanguage {
    UiLanguage::from_u8(CURRENT_LANGUAGE.load(Ordering::Relaxed))
}

pub fn current_language_is_chinese() -> bool {
    current_language() == UiLanguage::Chinese
}

pub fn translate_current(text: &str) -> String {
    translate(current_language(), text).into_owned()
}

pub fn translate<'a>(language: UiLanguage, text: &'a str) -> Cow<'a, str> {
    if language == UiLanguage::English {
        return Cow::Borrowed(text);
    }

    if let Some(translated) = index_status::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = properties::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = search_paths::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = search_service_recovery::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = search_service_status::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = file_operation_notifications::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = document_preview::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = batch_rename::translate(text) {
        return Cow::Owned(translated);
    }
    if let Some(translated) = convert::translate(text) {
        return Cow::Owned(translated);
    }
    if let Some(translated) = search_workspace::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = transfer::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = exact_translation::translate(text) {
        return Cow::Borrowed(translated);
    }

    if let Some(translated) = dynamic_translation(text) {
        return Cow::Owned(translated);
    }

    Cow::Borrowed(text)
}

pub fn trash_refresh_failed(error: &str) -> String {
    trash::refresh_failed(current_language(), error)
}

pub fn trash_additional_warning_count(count: usize) -> String {
    trash::additional_warning_count(current_language(), count)
}

pub fn trash_warning_summary(count: usize) -> String {
    trash::warning_summary(current_language(), count)
}

pub fn trash_tracking_warning(language: UiLanguage, warning: &str) -> String {
    trash::tracking_warning(language, warning)
}

pub fn current_trash_tracking_warning(warning: &str) -> String {
    trash::tracking_warning(current_language(), warning)
}

/// 接收完成通知正文（成功数 + 失败数），构造时按当前语言产出成品。
pub fn transfer_receive_finished_body(received: usize, failed: usize) -> String {
    transfer::receive_finished_body(current_language(), received, failed)
}

/// 二维码会话进度行（已传字节 + 速度），构造时产出成品。
pub fn transfer_progress_line(bytes: u64, speed_bytes_per_second: u64, finished: bool) -> String {
    transfer::qr_progress_line(current_language(), bytes, speed_bytes_per_second, finished)
}

/// 直推动度行（目标设备 + 文件序号 + 字节），构造时产出成品。
pub fn direct_send_progress_line(
    alias: String,
    file_name: String,
    file_index: usize,
    total_files: usize,
    bytes_sent: u64,
    file_size: u64,
) -> String {
    transfer::direct_send_progress_line(
        current_language(),
        &alias,
        &file_name,
        file_index,
        total_files,
        bytes_sent,
        file_size,
    )
}

/// 接收确认 Modal 正文（设备名 + 文件数），构造时产出成品。
pub fn incoming_transfer_summary(alias: String, count: usize) -> String {
    transfer::incoming_transfer_summary(current_language(), &alias, count)
}

pub fn detect_system_language() -> UiLanguage {
    detect_system_language_with(|key| std::env::var(key).ok())
}

fn detect_system_language_with<F>(lookup: F) -> UiLanguage
where
    F: Fn(&str) -> Option<String>,
{
    for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        let Some(value) = lookup(key) else {
            continue;
        };
        if locale_value_is_chinese(&value) {
            return UiLanguage::Chinese;
        }
    }
    UiLanguage::English
}

fn locale_value_is_chinese(value: &str) -> bool {
    value
        .split(':')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            segment
                .split(['.', '@'])
                .next()
                .unwrap_or(segment)
                .to_ascii_lowercase()
        })
        .any(|segment| segment == "zh" || segment.starts_with("zh_"))
}

fn dynamic_translation(text: &str) -> Option<String> {
    // 拖拽动作胶囊:动作词+落点目录名,英文前缀拼名、中文同序还原。
    // "Move to Trash" 是精确词条,在 exact_translation 里先命中,不会走到这里。
    if let Some(name) = text.strip_prefix("Create link to ") {
        return Some(format!("创建链接到{name}"));
    }
    if let Some(name) = text.strip_prefix("Copy to ") {
        return Some(format!("复制到{name}"));
    }
    if let Some(name) = text.strip_prefix("Move to ") {
        return Some(format!("移动到{name}"));
    }
    if let Some((shown, total)) =
        parse_two_counts(text, "Only showing ", " lines. Full line count: ", ".")
    {
        return Some(format!("当前仅显示 {shown} 行，完整行数为 {total}。"));
    }
    if let Some(count) = parse_prefixed_count(text, "", " tasks") {
        return Some(format!("{count} 个任务"));
    }
    if let Some(count) = parse_prefixed_count(text, "", " items") {
        return Some(format!("{count} 个项目"));
    }
    if let Some(count) = parse_prefixed_count(text, "", " explicit path(s)") {
        return Some(format!("{count} 个显式路径"));
    }
    if let Some(item_count) = text
        .strip_suffix(" will be added to:")
        .and_then(translated_item_count)
    {
        return Some(format!("将向以下位置添加 {item_count}："));
    }
    if let Some(item_count) = text
        .strip_prefix("Delete ")
        .and_then(|body| body.strip_suffix(" from Trash permanently? This cannot be undone."))
        .and_then(translated_item_count)
    {
        return Some(format!(
            "要从回收站永久删除 {item_count} 吗？此操作无法撤销。"
        ));
    }
    if let Some(item_count) = text
        .strip_prefix("Delete ")
        .and_then(|body| body.strip_suffix(" permanently? This cannot be undone."))
        .and_then(translated_item_count)
    {
        return Some(format!("要永久删除 {item_count} 吗？此操作无法撤销。"));
    }
    if let Some(size) = text.strip_prefix("Maximum content extraction: ") {
        return Some(format!("最大内容提取大小：{size}"));
    }
    if let Some(error) = text.strip_prefix("Service degraded: ") {
        return Some(format!("服务已降级：{error}"));
    }
    if let Some(error) = text.strip_prefix("Invalid regular expression: ") {
        return Some(format!("无效的正则表达式：{error}"));
    }
    if let Some(error) = text.strip_prefix("Service failed: ") {
        return Some(format!("服务失败：{error}"));
    }
    if let Some(size) = text.strip_prefix("Duration unknown · ") {
        return Some(format!("时长未知 · {size}"));
    }
    if let Some(position) = text.strip_prefix("Playing · ") {
        return Some(format!("播放中 · {position}"));
    }
    if let Some(position) = text.strip_prefix("Paused · ") {
        return Some(format!("已暂停 · {position}"));
    }
    if let Some(percent) = text
        .strip_prefix("Volume ")
        .and_then(|body| body.strip_suffix('%'))
    {
        return Some(format!("音量 {percent}%"));
    }
    if let Some(error) = text.strip_prefix("Unavailable: ") {
        return Some(format!("不可用：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not load: ") {
        return Some(format!("无法加载：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not load more text preview: ") {
        return Some(format!("无法继续加载文本预览：{error}"));
    }
    if let Some(error) = text.strip_prefix("Default open failed: ") {
        return Some(format!("默认打开失败：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not update permissions: ") {
        return Some(format!("无法更新权限：{error}"));
    }
    if let Some(error) = text.strip_prefix("Audio unavailable: ") {
        return Some(format!("音频不可用：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not load saved network password: ") {
        return Some(format!("无法加载已保存的网络密码：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not remove saved network password: ") {
        return Some(format!("无法移除已保存的网络密码：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not connect network location: ") {
        return Some(format!("无法连接网络位置：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not disconnect network location: ") {
        return Some(format!("无法断开网络位置：{error}"));
    }
    if let Some(error) = text.strip_prefix("File operation queue storage failed: ") {
        return Some(format!("文件操作队列存储失败：{error}"));
    }
    if let Some(error) = text.strip_prefix("Failed to initialize file operation queue storage: ") {
        return Some(format!("初始化文件操作队列存储失败：{error}"));
    }
    for (prefix, translated_prefix) in [
        ("Failed to save user preferences: ", "保存用户偏好失败："),
        (
            "Failed to save application configuration: ",
            "保存应用配置失败：",
        ),
        ("Failed to save column width: ", "保存列宽失败："),
        ("Failed to save browser session: ", "保存浏览会话失败："),
    ] {
        if let Some(error) = text.strip_prefix(prefix) {
            return Some(format!("{translated_prefix}{error}"));
        }
    }
    if let Some(path) = text
        .strip_prefix("Could not open startup directory ")
        .and_then(|body| body.strip_suffix("; opening the home directory."))
    {
        return Some(format!("无法打开启动目录 {path}；将打开主目录。"));
    }
    if let Some(error) = text
        .strip_prefix("Failed to restore saved view state: ")
        .and_then(|body| body.strip_suffix("; opening the home directory."))
    {
        return Some(format!("恢复已保存的视图状态失败：{error}；将打开主目录。"));
    }
    if let Some(content) = text
        .strip_prefix("File is too large to preview (")
        .and_then(|body| body.strip_suffix('.'))
    {
        let (size, maximum) = content.split_once("). Maximum preview size is ")?;
        return Some(format!(
            "文件过大，无法预览（{size}）。最大预览大小为 {maximum}。"
        ));
    }
    if let Some(path) = text.strip_prefix("Original: ") {
        return Some(format!("原始位置：{path}"));
    }
    if let Some(path) = text.strip_prefix("Directory: ") {
        return Some(format!("目录：{path}"));
    }

    None
}

fn parse_prefixed_count(text: &str, prefix: &str, suffix: &str) -> Option<usize> {
    text.strip_prefix(prefix)?
        .strip_suffix(suffix)?
        .parse()
        .ok()
}

fn parse_two_counts(
    text: &str,
    prefix: &str,
    middle: &str,
    suffix: &str,
) -> Option<(usize, usize)> {
    let content = text.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let (left, right) = content.split_once(middle)?;
    Some((left.parse().ok()?, right.parse().ok()?))
}

fn translated_item_count(label: &str) -> Option<String> {
    if label == "1 item" {
        return Some("1 个项目".to_owned());
    }
    let count = label.strip_suffix(" items")?.parse::<usize>().ok()?;
    Some(format!("{count} 个项目"))
}

#[cfg(test)]
mod tests;
