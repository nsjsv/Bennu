//! 空格预览的配置域：分类型大小上限、后缀规则与目录展开层级，以及
//! 「存储形态 → 域类型」的迁移组合语义（含 legacy 全局上限 seeding），
//! app-ui 与 portal 两宿主单源共用。serde 存储结构
//! `StoredPreviewPreferences` 由本 crate 定义（设置文档在宿主侧，预览段是其中
//! 一块 flatten 的键）；读哪个库、怎么读是宿主的事，本 crate 不依赖存储层。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::text_view::TextViewSettings;

/// 空格预览偏好段的存储形态：legacy 全局上限（max_preview_file_bytes，
/// 只读迁移，新代码写 None）、分类型大小上限、后缀规则与目录展开层级。
/// 字段声明顺序即序列化顺序，冻结为既有文档布局，不得重排。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredPreviewPreferences {
    #[serde(default)]
    pub max_preview_file_bytes: Option<u64>,
    /// 旧版本的文本大小上限；文本改为按需读取后不再使用，只为保持文档布局保留，新代码写 None。
    #[serde(default)]
    pub preview_text_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_image_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_video_size_bytes: Option<u64>,
    /// 旧版本的音频大小上限；音频边读边放后不再使用，只为保持文档布局保留，新代码写 None。
    #[serde(default)]
    pub preview_audio_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_archive_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_document_size_bytes: Option<u64>,
    #[serde(default)]
    pub preview_sqlite_size_bytes: Option<u64>,
    /// 空格预览的分类型后缀规则；None 表示旧版本数据，读取端回退内置
    /// 默认列表。单类型内 None 同样回退默认，空列表是合法用户选择。
    #[serde(default)]
    pub preview_extension_rules: Option<StoredPreviewExtensionRules>,
    #[serde(default)]
    pub preview_directory_expand_levels: Option<u8>,
    /// 额外当作文本的后缀（文本主要由类型库判定）。None 表示还没迁移过，读取端
    /// 从旧的 `preview_extension_rules.text` 迁移一次。
    #[serde(default)]
    pub preview_extra_text_extensions: Option<Vec<String>>,
    /// 文本预览字号缩放（百分比）与自动换行；None 用默认。
    #[serde(default)]
    pub text_preview_zoom_percent: Option<u16>,
    #[serde(default)]
    pub text_preview_wrap: Option<bool>,
    /// 额外当作音频的后缀（音频主要由类型库判定）。None 表示还没迁移过，读取端
    /// 从旧的 `preview_extension_rules.audio` 迁移一次。
    #[serde(default)]
    pub preview_extra_audio_extensions: Option<Vec<String>>,
    /// 音频、视频预览共用的音量（0–100）；None 用默认。
    #[serde(default)]
    pub media_preview_volume_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StoredPreviewExtensionRules {
    /// 旧版本的替换式文本后缀表，只读迁移，新代码写 None。
    #[serde(default)]
    pub text: Option<Vec<String>>,
    #[serde(default)]
    pub image: Option<Vec<String>>,
    #[serde(default)]
    pub video: Option<Vec<String>>,
    /// 旧版本的替换式音频后缀表，只读迁移，新代码写 None。
    #[serde(default)]
    pub audio: Option<Vec<String>>,
    #[serde(default)]
    pub sqlite: Option<Vec<String>>,
    #[serde(default)]
    pub archive: Option<Vec<String>>,
    #[serde(default)]
    pub document: Option<Vec<String>>,
}

pub const PREVIEW_FILE_SIZE_UNIT_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_PREVIEW_IMAGE_SIZE_BYTES: u64 = 100 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub const DEFAULT_PREVIEW_VIDEO_SIZE_BYTES: u64 = 1024 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub const DEFAULT_PREVIEW_ARCHIVE_SIZE_BYTES: u64 = 25 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub const DEFAULT_PREVIEW_SQLITE_SIZE_BYTES: u64 = 100 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub const DEFAULT_PREVIEW_DOCUMENT_SIZE_BYTES: u64 = 100 * PREVIEW_FILE_SIZE_UNIT_BYTES;
pub const MIN_PREVIEW_DIRECTORY_EXPAND_LEVELS: u8 = 0;
pub const MAX_PREVIEW_DIRECTORY_EXPAND_LEVELS: u8 = 3;
pub const DEFAULT_PREVIEW_DIRECTORY_EXPAND_LEVELS: u8 = 1;
/// 旧版本的内置文本后缀表。文本改由类型库判定后它只用于迁移：几乎所有用户存的
/// 都是这份默认值（保存时写全量），原样搬成额外后缀会让 `.ts` 视频也被硬当文本。
const LEGACY_DEFAULT_PREVIEW_TEXT_EXTENSIONS: [&str; 22] = [
    "txt", "md", "log", "conf", "ini", "yaml", "yml", "json", "xml", "toml", "sh", "py", "js",
    "ts", "c", "cpp", "h", "rs", "java", "css", "html", "csv",
];
/// 旧版本的内置音频后缀表，只用于迁移，理由同文本：保存时写的是全量，原样搬成
/// 额外后缀会让这 7 个后缀永远绕过类型库。
const LEGACY_DEFAULT_PREVIEW_AUDIO_EXTENSIONS: [&str; 7] =
    ["mp3", "wav", "flac", "ogg", "oga", "m4a", "aac"];
/// 空格预览其余类型的默认后缀表，镜像各预览类型的内置判定；
/// 后缀以小写、无前导点的规范形态存储。替换式语义：用户可增删，
/// 删除即该后缀不再按此类型预览。
pub const DEFAULT_PREVIEW_IMAGE_EXTENSIONS: [&str; 11] = [
    "avif", "bmp", "gif", "ico", "jpg", "jpeg", "png", "svg", "tif", "tiff", "webp",
];
pub const DEFAULT_PREVIEW_VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "m4v", "mkv", "mov", "webm", "avi"];
pub const DEFAULT_PREVIEW_SQLITE_EXTENSIONS: [&str; 4] = ["db", "sqlite", "sqlite3", "db3"];
pub const DEFAULT_PREVIEW_ARCHIVE_EXTENSIONS: [&str; 6] =
    ["zip", "tar", "tar.gz", "tgz", "7z", "rar"];
pub const DEFAULT_PREVIEW_DOCUMENT_EXTENSIONS: [&str; 10] = [
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewFileSizeKind {
    Text,
    Image,
    Video,
    Audio,
    Archive,
    Document,
    Sqlite,
}

impl PreviewFileSizeKind {
    pub const ALL: [PreviewFileSizeKind; 7] = [
        Self::Text,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Archive,
        Self::Document,
        Self::Sqlite,
    ];
}

/// 有大小上限的预览类型。文本按需从磁盘读、音频边读边放，内存都不随文件大小增长，不设上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSizeLimitKind {
    Image,
    Video,
    Archive,
    Document,
    Sqlite,
}

impl PreviewSizeLimitKind {
    pub const ALL: [PreviewSizeLimitKind; 5] = [
        Self::Image,
        Self::Video,
        Self::Archive,
        Self::Document,
        Self::Sqlite,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewFileSizeLimits {
    pub image_bytes: u64,
    pub video_bytes: u64,
    pub archive_bytes: u64,
    pub document_bytes: u64,
    pub sqlite_bytes: u64,
}

impl PreviewFileSizeLimits {
    pub fn with_default_limits() -> Self {
        Self {
            image_bytes: DEFAULT_PREVIEW_IMAGE_SIZE_BYTES,
            video_bytes: DEFAULT_PREVIEW_VIDEO_SIZE_BYTES,
            archive_bytes: DEFAULT_PREVIEW_ARCHIVE_SIZE_BYTES,
            document_bytes: DEFAULT_PREVIEW_DOCUMENT_SIZE_BYTES,
            sqlite_bytes: DEFAULT_PREVIEW_SQLITE_SIZE_BYTES,
        }
    }

    pub fn limit(self, kind: PreviewSizeLimitKind) -> u64 {
        match kind {
            PreviewSizeLimitKind::Image => self.image_bytes,
            PreviewSizeLimitKind::Video => self.video_bytes,
            PreviewSizeLimitKind::Archive => self.archive_bytes,
            PreviewSizeLimitKind::Document => self.document_bytes,
            PreviewSizeLimitKind::Sqlite => self.sqlite_bytes,
        }
    }

    pub fn set_limit(&mut self, kind: PreviewSizeLimitKind, bytes: u64) {
        match kind {
            PreviewSizeLimitKind::Image => self.image_bytes = bytes,
            PreviewSizeLimitKind::Video => self.video_bytes = bytes,
            PreviewSizeLimitKind::Archive => self.archive_bytes = bytes,
            PreviewSizeLimitKind::Document => self.document_bytes = bytes,
            PreviewSizeLimitKind::Sqlite => self.sqlite_bytes = bytes,
        }
    }

    /// 迁移时用旧的全局单值上限同时填充全部类型。
    pub fn from_legacy_global_bytes(bytes: u64) -> Self {
        Self {
            image_bytes: bytes,
            video_bytes: bytes,
            archive_bytes: bytes,
            document_bytes: bytes,
            sqlite_bytes: bytes,
        }
    }
}

pub fn normalize_preview_directory_expand_levels(levels: u8) -> u8 {
    levels.clamp(
        MIN_PREVIEW_DIRECTORY_EXPAND_LEVELS,
        MAX_PREVIEW_DIRECTORY_EXPAND_LEVELS,
    )
}

/// 把用户输入规范成可匹配的后缀：去首尾空白、去前导点、转小写。
/// 含内部空白（如 "my ext"）永远无法命中真实文件名，直接拒绝。
/// 复合后缀（如 tar.gz）保留内部点。
pub fn normalize_preview_extension(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('.');
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return None;
    }
    Some(trimmed.to_lowercase())
}

/// 空格预览的分类型后缀规则：文本、音频列表是「额外当作文本/音频的后缀」，
/// 默认空，这两类主要由类型库判定；其余类型的列表完全决定该类型识别哪些
/// 后缀（替换式）。匹配按文件名 `ends_with` 进行，天然覆盖 tar.gz
/// 这类复合后缀；大小写不敏感，与各渲染器行为保持一致。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewExtensionRules {
    pub text: Vec<String>,
    pub image: Vec<String>,
    pub video: Vec<String>,
    pub audio: Vec<String>,
    pub sqlite: Vec<String>,
    pub archive: Vec<String>,
    pub document: Vec<String>,
}

impl PreviewExtensionRules {
    pub fn default_rules() -> Self {
        Self {
            text: Vec::new(),
            image: default_extensions(&DEFAULT_PREVIEW_IMAGE_EXTENSIONS),
            video: default_extensions(&DEFAULT_PREVIEW_VIDEO_EXTENSIONS),
            audio: Vec::new(),
            sqlite: default_extensions(&DEFAULT_PREVIEW_SQLITE_EXTENSIONS),
            archive: default_extensions(&DEFAULT_PREVIEW_ARCHIVE_EXTENSIONS),
            document: default_extensions(&DEFAULT_PREVIEW_DOCUMENT_EXTENSIONS),
        }
    }

    pub fn matches(&self, kind: PreviewFileSizeKind, path: &Path) -> bool {
        extensions_match(self.list(kind), path)
    }

    pub fn list(&self, kind: PreviewFileSizeKind) -> &Vec<String> {
        match kind {
            PreviewFileSizeKind::Text => &self.text,
            PreviewFileSizeKind::Image => &self.image,
            PreviewFileSizeKind::Video => &self.video,
            PreviewFileSizeKind::Audio => &self.audio,
            PreviewFileSizeKind::Archive => &self.archive,
            PreviewFileSizeKind::Document => &self.document,
            PreviewFileSizeKind::Sqlite => &self.sqlite,
        }
    }

    pub fn list_mut(&mut self, kind: PreviewFileSizeKind) -> &mut Vec<String> {
        match kind {
            PreviewFileSizeKind::Text => &mut self.text,
            PreviewFileSizeKind::Image => &mut self.image,
            PreviewFileSizeKind::Video => &mut self.video,
            PreviewFileSizeKind::Audio => &mut self.audio,
            PreviewFileSizeKind::Archive => &mut self.archive,
            PreviewFileSizeKind::Document => &mut self.document,
            PreviewFileSizeKind::Sqlite => &mut self.sqlite,
        }
    }

    pub fn set_list(&mut self, kind: PreviewFileSizeKind, extensions: Vec<String>) {
        *self.list_mut(kind) = extensions;
    }

    pub fn default_list(kind: PreviewFileSizeKind) -> Vec<String> {
        let builtin: &[&str] = match kind {
            PreviewFileSizeKind::Text | PreviewFileSizeKind::Audio => &[],
            PreviewFileSizeKind::Image => &DEFAULT_PREVIEW_IMAGE_EXTENSIONS,
            PreviewFileSizeKind::Video => &DEFAULT_PREVIEW_VIDEO_EXTENSIONS,
            PreviewFileSizeKind::Archive => &DEFAULT_PREVIEW_ARCHIVE_EXTENSIONS,
            PreviewFileSizeKind::Document => &DEFAULT_PREVIEW_DOCUMENT_EXTENSIONS,
            PreviewFileSizeKind::Sqlite => &DEFAULT_PREVIEW_SQLITE_EXTENSIONS,
        };
        default_extensions(builtin)
    }
}

fn default_extensions(builtin: &[&str]) -> Vec<String> {
    builtin
        .iter()
        .map(|extension| (*extension).to_owned())
        .collect()
}

fn extensions_match(extensions: &[String], path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let file_name = file_name.to_lowercase();
    extensions
        .iter()
        .any(|candidate| file_name.ends_with(&format!(".{candidate}")))
}

pub const DEFAULT_MEDIA_PREVIEW_VOLUME: f32 = 1.0;

/// 存成整数百分比：设置文档里好读，也免得浮点误差让「没改」看起来像改了。
pub fn media_preview_volume_from_stored(percent: Option<u8>) -> f32 {
    percent.map_or(DEFAULT_MEDIA_PREVIEW_VOLUME, |percent| {
        f32::from(percent.min(100)) / 100.0
    })
}

pub fn media_preview_volume_percent(volume: f32) -> u8 {
    // 先夹到 0–1 再乘，结果一定在 0–100 之内。
    (volume.clamp(0.0, 1.0) * 100.0).round() as u8
}

pub fn preview_size_limit_mib(bytes: u64) -> u64 {
    bytes.div_ceil(PREVIEW_FILE_SIZE_UNIT_BYTES)
}

pub fn preview_size_limit_bytes_from_mib(mib: u64) -> Option<u64> {
    mib.checked_mul(PREVIEW_FILE_SIZE_UNIT_BYTES)
}

/// 解析后的预览偏好（域类型），两个宿主并轨消费；默认值见
/// `PreviewPreferences::default_preferences`。
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewPreferences {
    pub size_limits: PreviewFileSizeLimits,
    pub extension_rules: PreviewExtensionRules,
    pub directory_expand_levels: u8,
    pub text_view: TextViewSettings,
    /// 音频、视频预览共用的音量（0.0–1.0）。
    pub media_volume: f32,
}

impl PreviewPreferences {
    /// 未存储任何偏好时的完整默认：六/七类默认上限 + 内置默认后缀表 +
    /// 默认展开层级；portal 读配置失败也回落到这里。
    pub fn default_preferences() -> Self {
        Self {
            size_limits: PreviewFileSizeLimits::with_default_limits(),
            extension_rules: PreviewExtensionRules::default_rules(),
            directory_expand_levels: DEFAULT_PREVIEW_DIRECTORY_EXPAND_LEVELS,
            text_view: TextViewSettings::default(),
            media_volume: DEFAULT_MEDIA_PREVIEW_VOLUME,
        }
    }
}

/// 存储形态 → 分类型大小上限。迁移语义（与 app-ui 历史行为逐键一致）：
/// 分类型键优先；缺失类型由 legacy 全局 max_preview_file_bytes 种子；
/// 两者皆无回退 fallback。
pub fn preview_size_limits_from_stored(
    stored: &StoredPreviewPreferences,
    fallback: PreviewFileSizeLimits,
) -> PreviewFileSizeLimits {
    let legacy_global_limit = stored.max_preview_file_bytes;
    let resolve = |stored_bytes: Option<u64>, fallback_bytes: u64| {
        stored_bytes
            .or(legacy_global_limit)
            .unwrap_or(fallback_bytes)
    };
    // 文本、音频不再有上限：旧的 preview_text_size_bytes / preview_audio_size_bytes 读到也不用。
    PreviewFileSizeLimits {
        image_bytes: resolve(stored.preview_image_size_bytes, fallback.image_bytes),
        video_bytes: resolve(stored.preview_video_size_bytes, fallback.video_bytes),
        archive_bytes: resolve(stored.preview_archive_size_bytes, fallback.archive_bytes),
        document_bytes: resolve(stored.preview_document_size_bytes, fallback.document_bytes),
        sqlite_bytes: resolve(stored.preview_sqlite_size_bytes, fallback.sqlite_bytes),
    }
}

/// 存储形态 → 分类型后缀规则。每类型独立回退：None 是旧版本数据或缺失
/// 类型；空列表是用户显式清空，必须原样保留。
pub fn preview_extension_rules_from_stored(
    stored: &StoredPreviewPreferences,
    fallback: &PreviewExtensionRules,
) -> PreviewExtensionRules {
    let legacy_rules = stored.preview_extension_rules.as_ref();
    let extra_text = extra_extensions_from_stored(
        &stored.preview_extra_text_extensions,
        legacy_rules.and_then(|rules| rules.text.as_ref()),
        &LEGACY_DEFAULT_PREVIEW_TEXT_EXTENSIONS,
        &fallback.text,
    );
    let extra_audio = extra_extensions_from_stored(
        &stored.preview_extra_audio_extensions,
        legacy_rules.and_then(|rules| rules.audio.as_ref()),
        &LEGACY_DEFAULT_PREVIEW_AUDIO_EXTENSIONS,
        &fallback.audio,
    );
    let Some(stored_rules) = legacy_rules else {
        return PreviewExtensionRules {
            text: extra_text,
            audio: extra_audio,
            ..fallback.clone()
        };
    };
    let per_type = |stored: &Option<Vec<String>>, default_extensions: &Vec<String>| {
        stored.clone().unwrap_or_else(|| default_extensions.clone())
    };
    PreviewExtensionRules {
        text: extra_text,
        image: per_type(&stored_rules.image, &fallback.image),
        video: per_type(&stored_rules.video, &fallback.video),
        audio: extra_audio,
        sqlite: per_type(&stored_rules.sqlite, &fallback.sqlite),
        archive: per_type(&stored_rules.archive, &fallback.archive),
        document: per_type(&stored_rules.document, &fallback.document),
    }
}

/// 额外后缀（文本、音频）：新键说了算；新键还没写过时从旧的替换式表迁移一次——
/// 去掉旧内置默认值，只留用户自己加的。迁移结果随下次保存写进新键，之后用户
/// 再加 `txt` / `mp3` 也不会被过滤。
fn extra_extensions_from_stored(
    saved: &Option<Vec<String>>,
    legacy: Option<&Vec<String>>,
    legacy_defaults: &[&str],
    fallback: &[String],
) -> Vec<String> {
    if let Some(extra) = saved {
        return extra.clone();
    }
    let Some(legacy) = legacy else {
        return fallback.to_vec();
    };
    legacy
        .iter()
        .filter(|extension| !legacy_defaults.contains(&extension.as_str()))
        .cloned()
        .collect()
}

/// 存储形态 → 目录展开层级：缺省回退 fallback，存值越界时归一化夹住。
pub fn preview_directory_expand_levels_from_stored(
    stored: &StoredPreviewPreferences,
    fallback: u8,
) -> u8 {
    stored
        .preview_directory_expand_levels
        .map_or(fallback, normalize_preview_directory_expand_levels)
}

/// 存储形态 → 域类型全量解析（默认值兜底），portal 与 app-ui 迁移路径
/// 共用同一语义。
pub fn preview_preferences_from_stored(stored: &StoredPreviewPreferences) -> PreviewPreferences {
    let defaults = PreviewPreferences::default_preferences();
    PreviewPreferences {
        size_limits: preview_size_limits_from_stored(stored, defaults.size_limits),
        extension_rules: preview_extension_rules_from_stored(stored, &defaults.extension_rules),
        directory_expand_levels: preview_directory_expand_levels_from_stored(
            stored,
            defaults.directory_expand_levels,
        ),
        text_view: TextViewSettings::from_stored(
            stored.text_preview_zoom_percent,
            stored.text_preview_wrap,
        ),
        media_volume: media_preview_volume_from_stored(stored.media_preview_volume_percent),
    }
}

#[cfg(test)]
mod stored_preferences_migration_tests {
    use super::*;

    /// 下沉前（平铺字段时代）整份用户偏好文档样本：仅含无 serde 缺省的
    /// 必需键 + 全部 preview 键，字段名与旧设置文档逐字一致。
    const LEGACY_DOCUMENT: &str = r#"{
        "network_list_thumbnail_downloads_enabled": false,
        "max_preview_file_bytes": 4194304,
        "preview_text_size_bytes": null,
        "preview_image_size_bytes": 1048576,
        "preview_video_size_bytes": null,
        "preview_audio_size_bytes": 5242880,
        "preview_archive_size_bytes": null,
        "preview_document_size_bytes": null,
        "preview_sqlite_size_bytes": null,
        "preview_extension_rules": {
            "text": ["txt", "log"],
            "image": [],
            "video": null,
            "audio": ["flac"],
            "sqlite": null,
            "archive": null,
            "document": null
        },
        "preview_directory_expand_levels": 2,
        "show_hidden_files": true,
        "sidebar_width": 200.0,
        "sidebar_favorites": null,
        "network_connections": [],
        "terminal_emulator": "automatic",
        "file_operation_verification": "basic_metadata",
        "browser_view_mode": "columns",
        "startup_location": "home",
        "startup_custom_directory": {"encoding": "unix_bytes", "bytes": []},
        "save_view_state": false,
        "shortcuts": []
    }"#;

    #[test]
    fn legacy_document_resolves_per_kind_limits_with_legacy_global_seeding() {
        // 整份设置文档里只取 preview 段解析（loader 的视角）。
        let stored: StoredPreviewPreferences = serde_json::from_str(LEGACY_DOCUMENT)
            .expect("legacy document parses as preview segment");

        let limits =
            preview_size_limits_from_stored(&stored, PreviewFileSizeLimits::with_default_limits());
        // null（缺失）类型吃 legacy 全局值，显式分类型键覆盖全局值。
        assert_eq!(limits.image_bytes, 1048576);
        assert_eq!(limits.video_bytes, 4194304);
        assert_eq!(limits.archive_bytes, 4194304);
        assert_eq!(limits.document_bytes, 4194304);
        assert_eq!(limits.sqlite_bytes, 4194304);

        let preferences = preview_preferences_from_stored(&stored);
        assert_eq!(
            preferences.size_limits,
            preview_size_limits_from_stored(&stored, PreviewFileSizeLimits::with_default_limits())
        );
        assert_eq!(preferences.directory_expand_levels, 2);
    }

    #[test]
    fn extension_rules_fallback_matrix_preserves_empty_lists() {
        let defaults = PreviewExtensionRules::default_rules();

        // 整段缺失：全部回退默认表。
        let missing = StoredPreviewPreferences::default();
        assert_eq!(
            preview_extension_rules_from_stored(&missing, &defaults),
            defaults
        );

        // 段存在但单类型 None：该类型回退默认，空列表是用户显式清空。
        let stored = StoredPreviewPreferences {
            preview_extension_rules: Some(StoredPreviewExtensionRules {
                image: Some(Vec::new()),
                ..StoredPreviewExtensionRules::default()
            }),
            ..StoredPreviewPreferences::default()
        };
        let rules = preview_extension_rules_from_stored(&stored, &defaults);
        assert_eq!(rules.text, Vec::<String>::new());
        assert_eq!(rules.image, Vec::<String>::new());
        assert_eq!(rules.video, defaults.video);
        assert_eq!(rules.audio, defaults.audio);
        assert_eq!(rules.sqlite, defaults.sqlite);
        assert_eq!(rules.archive, defaults.archive);
        assert_eq!(rules.document, defaults.document);
    }

    #[test]
    fn legacy_text_list_migrates_once_keeping_only_user_additions() {
        let defaults = PreviewExtensionRules::default_rules();
        let mut legacy_text = LEGACY_DEFAULT_PREVIEW_TEXT_EXTENSIONS
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect::<Vec<_>>();
        legacy_text.extend(["nfo".to_owned(), "srt".to_owned()]);
        let legacy = StoredPreviewPreferences {
            preview_extension_rules: Some(StoredPreviewExtensionRules {
                text: Some(legacy_text),
                ..StoredPreviewExtensionRules::default()
            }),
            ..StoredPreviewPreferences::default()
        };

        // 旧默认（含 ts）去掉，只剩用户自己加的。
        let rules = preview_extension_rules_from_stored(&legacy, &defaults);
        assert_eq!(rules.text, vec!["nfo".to_owned(), "srt".to_owned()]);

        // 新键写过之后旧键不再参与：用户后来自己加回 txt 也保留。
        let saved = StoredPreviewPreferences {
            preview_extra_text_extensions: Some(vec!["txt".to_owned()]),
            ..legacy
        };
        let rules = preview_extension_rules_from_stored(&saved, &defaults);
        assert_eq!(rules.text, vec!["txt".to_owned()]);

        // 什么都没存：默认空。
        let rules =
            preview_extension_rules_from_stored(&StoredPreviewPreferences::default(), &defaults);
        assert!(rules.text.is_empty());
    }

    #[test]
    fn legacy_audio_list_migrates_once_keeping_only_user_additions() {
        let defaults = PreviewExtensionRules::default_rules();
        let mut legacy_audio = LEGACY_DEFAULT_PREVIEW_AUDIO_EXTENSIONS
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect::<Vec<_>>();
        legacy_audio.push("dsf".to_owned());
        let legacy = StoredPreviewPreferences {
            preview_extension_rules: Some(StoredPreviewExtensionRules {
                audio: Some(legacy_audio),
                ..StoredPreviewExtensionRules::default()
            }),
            ..StoredPreviewPreferences::default()
        };

        // 旧内置 7 个去掉，只剩用户自己加的；其余类型不受影响。
        let rules = preview_extension_rules_from_stored(&legacy, &defaults);
        assert_eq!(rules.audio, vec!["dsf".to_owned()]);
        assert_eq!(rules.video, defaults.video);

        // 新键写过之后旧键不再参与：用户清空也原样保留。
        let saved = StoredPreviewPreferences {
            preview_extra_audio_extensions: Some(Vec::new()),
            ..legacy
        };
        let rules = preview_extension_rules_from_stored(&saved, &defaults);
        assert!(rules.audio.is_empty());

        // 什么都没存：默认空。
        let rules =
            preview_extension_rules_from_stored(&StoredPreviewPreferences::default(), &defaults);
        assert!(rules.audio.is_empty());
    }

    #[test]
    fn media_volume_round_trips_as_a_percentage() {
        assert_eq!(media_preview_volume_from_stored(None), 1.0);
        assert_eq!(media_preview_volume_from_stored(Some(35)), 0.35);
        assert_eq!(media_preview_volume_from_stored(Some(250)), 1.0);
        assert_eq!(media_preview_volume_percent(0.354), 35);
        assert_eq!(media_preview_volume_percent(1.5), 100);
        assert_eq!(media_preview_volume_percent(-0.2), 0);
    }

    #[test]
    fn directory_expand_levels_fallback_and_clamp() {
        let fallback = 1_u8;
        let levels = |stored: Option<u8>| {
            preview_directory_expand_levels_from_stored(
                &StoredPreviewPreferences {
                    preview_directory_expand_levels: stored,
                    ..StoredPreviewPreferences::default()
                },
                fallback,
            )
        };
        assert_eq!(levels(None), 1);
        assert_eq!(levels(Some(0)), 0);
        assert_eq!(levels(Some(3)), 3);
        // 越界存值归一化夹住，与 app-ui 历史行为一致。
        assert_eq!(levels(Some(9)), 3);
    }
}
