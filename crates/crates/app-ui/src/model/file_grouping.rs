use std::path::Path;
use std::time::SystemTime;

use file_core::SortDirection;
use time::OffsetDateTime;

use crate::config::UiLanguage;
use crate::formatting::{format_file_size, format_file_size_compact};
use crate::localization::translate;
use crate::model::BrowserViewMode;

/// 分组方式菜单入口的视图门控：分组只在列表/大图视图生效，
/// 多栏视图既不应用分组也不提供入口。打开菜单时按 pane 当时的
/// 视图模式求值一次，结果存进菜单状态，渲染层只读结果。
pub(crate) fn file_grouping_entry_visible(view_mode: BrowserViewMode) -> bool {
    matches!(view_mode, BrowserViewMode::List | BrowserViewMode::Icons)
}

#[path = "file_grouping/pinyin_initial_table.rs"]
mod pinyin_initial_table;

/// 大小分组的桶数上限；与访达一致，目录内文件再多也不超过 5 个大小段。
pub(crate) const MAX_FILE_SIZE_GROUPS: usize = 5;

/// 分组维度（七项对齐访达）。全局一份设置，列表/大图共用，多栏不生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FileGroupingMode {
    None,
    NameInitial,
    Kind,
    Size,
    ModifiedTime,
    CreatedTime,
    AccessedTime,
}

impl FileGroupingMode {
    /// 菜单文案的英文 key；None 的语义是"无分组"，文案即 "None"。
    /// 中文词条由 localization::exact_translation 在接入菜单时补齐。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::NameInitial => "Name",
            Self::Kind => "Kind",
            Self::Size => "Size",
            Self::ModifiedTime => "Date Modified",
            Self::CreatedTime => "Date Created",
            Self::AccessedTime => "Date Accessed",
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::NameInitial => "name_initial",
            Self::Kind => "kind",
            Self::Size => "size",
            Self::ModifiedTime => "date_modified",
            Self::CreatedTime => "date_created",
            Self::AccessedTime => "date_accessed",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "name_initial" => Some(Self::NameInitial),
            "kind" => Some(Self::Kind),
            "size" => Some(Self::Size),
            "date_modified" => Some(Self::ModifiedTime),
            "date_created" => Some(Self::CreatedTime),
            "date_accessed" => Some(Self::AccessedTime),
            _ => None,
        }
    }
}

/// 一个组的稳定身份：显示标题 + 索引栏短标签 + 组间排序秩。
/// 标题与短标签都是构造时已本地化的成品文案；秩为升序方向的组序，
/// 降序时由 `partition_files_into_groups` 反转组序列（组内不动）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileGroupDescriptor {
    pub(crate) title: String,
    pub(crate) index_label: String,
    pub(crate) rank: usize,
}

/// 类型大类（访达风格，不按扩展名细拆）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KindCategory {
    Images,
    Videos,
    Audio,
    Documents,
    Archives,
    Applications,
    Other,
}

impl KindCategory {
    /// 升序方向的组序；Other 是扩展名未知的兜底组，固定排最末。
    fn rank(self) -> usize {
        match self {
            Self::Images => 0,
            Self::Videos => 1,
            Self::Audio => 2,
            Self::Documents => 3,
            Self::Archives => 4,
            Self::Applications => 5,
            Self::Other => 6,
        }
    }

    fn title(self, language: UiLanguage) -> String {
        let key = match self {
            Self::Images => "Images",
            Self::Videos => "Videos",
            Self::Audio => "Audio",
            Self::Documents => "Documents",
            Self::Archives => "Archives",
            Self::Applications => "Applications",
            Self::Other => "Other",
        };
        translate(language, key).into_owned()
    }
}

/// 日期时段桶（修改/创建/访问三个日期维度共用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateBucket {
    Today,
    Yesterday,
    ThisWeek,
    ThisMonth,
    /// 当年更早的日历月（1..=12）。
    Month(u8),
    Earlier,
}

impl DateBucket {
    /// 升序方向的组序 = 时间先后（旧→新）。周/月桶是"最近 N 天"的
    /// 滚动窗口，必然晚于当年更早的日历月，故秩排在全部 Month 之后。
    fn rank(self) -> usize {
        match self {
            Self::Earlier => 0,
            Self::Month(month) => month as usize,
            Self::ThisMonth => 13,
            Self::ThisWeek => 14,
            Self::Yesterday => 15,
            Self::Today => 16,
        }
    }

    fn title(self, language: UiLanguage) -> String {
        match self {
            Self::Today => translate(language, "Today").into_owned(),
            Self::Yesterday => translate(language, "Yesterday").into_owned(),
            Self::ThisWeek => translate(language, "This Week").into_owned(),
            Self::ThisMonth => translate(language, "This Month").into_owned(),
            Self::Earlier => translate(language, "Earlier").into_owned(),
            Self::Month(month) => month_group_title(language, month),
        }
    }
}

/// "今年8月"这类动态月份文案不走词条查表（动态拼串查不到词条），
/// 在构造组头时直接出成品：英文用月名，中文用"今年N月"。
fn month_group_title(language: UiLanguage, month: u8) -> String {
    match language {
        UiLanguage::English => ENGLISH_MONTHS[(month - 1) as usize].to_owned(),
        UiLanguage::Chinese => format!("今年{month}月"),
    }
}

/// 索引栏的月份短格式：宽度受限，英文取月名前三字母（Aug），中文用
/// "8月"。与组头标题共用同一张月名表，两个展示位不会漂移。
fn month_group_index_label(language: UiLanguage, month: u8) -> String {
    match language {
        UiLanguage::English => ENGLISH_MONTHS[(month - 1) as usize][..3].to_owned(),
        UiLanguage::Chinese => format!("{month}月"),
    }
}

const ENGLISH_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// 分组键的统一形态：每个维度的键都自带确定组秩与组标题的全部信息，
/// 因此划分函数不感知具体维度。Size 键携带桶边界是因为标题要显示
/// 实际大小范围，而桶是调用方按当前目录文件集合预计算的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileGroupKey {
    /// 'A'..='Z' 或兜底 '#'。
    NameInitial(char),
    Kind(KindCategory),
    /// bucket_count 是本次划分的桶总数：索引栏短标签的"首桶 ≤ 上界 /
    /// 末桶 ≥ 下界"语义需要知道自身是不是端点桶（端点桶外的尺寸也归
    /// 它，范围标注才真实）；同一次划分内恒定，不影响键的同一性。
    Size {
        bucket_index: usize,
        bucket_count: usize,
        min_bytes: u64,
        max_bytes: u64,
    },
    Date(DateBucket),
}

impl FileGroupKey {
    fn descriptor(self, language: UiLanguage) -> FileGroupDescriptor {
        FileGroupDescriptor {
            title: self.title(language),
            index_label: self.index_label(language),
            rank: self.rank(),
        }
    }

    fn rank(self) -> usize {
        match self {
            Self::NameInitial(initial) => {
                if initial == '#' {
                    0
                } else {
                    initial as usize - 'A' as usize + 1
                }
            }
            Self::Kind(category) => category.rank(),
            Self::Size { bucket_index, .. } => bucket_index,
            Self::Date(bucket) => bucket.rank(),
        }
    }

    fn title(self, language: UiLanguage) -> String {
        match self {
            Self::NameInitial(initial) => initial.to_string(),
            Self::Kind(category) => category.title(language),
            Self::Size {
                min_bytes,
                max_bytes,
                ..
            } => {
                // 尺寸单位是通用符号不走本地化；单值组（lo==hi）不重复展示。
                if min_bytes == max_bytes {
                    format_file_size(min_bytes)
                } else {
                    format!(
                        "{} ~ {}",
                        format_file_size(min_bytes),
                        format_file_size(max_bytes)
                    )
                }
            }
            Self::Date(bucket) => bucket.title(language),
        }
    }

    /// 索引栏短标签：与标题同源收窄——字母/类别/日常时段直接用标题，
    /// 月份用短格式，大小用压缩单位与端点语义（首桶 ≤ 上界、末桶
    /// ≥ 下界、中间桶 lo~hi、单值组只写单值）。
    fn index_label(self, language: UiLanguage) -> String {
        match self {
            Self::NameInitial(initial) => initial.to_string(),
            Self::Kind(category) => category.title(language),
            Self::Size {
                bucket_index,
                bucket_count,
                min_bytes,
                max_bytes,
            } => {
                let lo = format_file_size_compact(min_bytes);
                let hi = format_file_size_compact(max_bytes);
                if min_bytes == max_bytes {
                    lo
                } else if bucket_index == 0 {
                    format!("≤{hi}")
                } else if bucket_index + 1 == bucket_count {
                    format!("≥{lo}")
                } else {
                    format!("{lo}~{hi}")
                }
            }
            Self::Date(DateBucket::Month(month)) => month_group_index_label(language, month),
            Self::Date(bucket) => bucket.title(language),
        }
    }
}

/// 名称首字母分组键：ASCII 字母→A..Z；常用简体字→拼音首字母（内置
/// GB2312 一级字库表，统一成大写与 ASCII 组对齐）；数字/符号/生僻字/
/// 空名→`#` 兜底。
pub(crate) fn name_initial_group_key(name: &str) -> char {
    match name.chars().next() {
        Some(first) if first.is_ascii_alphabetic() => first.to_ascii_uppercase(),
        Some(first) => pinyin_initial_table::gb2312_pinyin_initial(first)
            .map(|initial| initial.to_ascii_uppercase())
            .unwrap_or('#'),
        None => '#',
    }
}

/// 类型大类分组键（按扩展名映射）。目录不进此函数（调用方保证只喂文件）。
/// 扩展名清单与预览后缀表（config.rs DEFAULT_PREVIEW_*）刻意不共用：
/// 两者语义不同（能否预览 vs 归哪个大类），成员随各自需求独立演化。
pub(crate) fn kind_category_group_key(name: &str) -> KindCategory {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some(
            "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "apng" | "gif" | "webp" | "bmp" | "svg"
            | "heic" | "heif" | "avif" | "tif" | "tiff" | "ico" | "psd",
        ) => KindCategory::Images,
        Some(
            "mp4" | "m4v" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "ts" | "m2ts" | "mts"
            | "mpg" | "mpeg" | "ogv" | "3gp" | "asf" | "rm" | "rmvb" | "vob" | "f4v" | "divx",
        ) => KindCategory::Videos,
        Some(
            "mp3" | "flac" | "wav" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma" | "aiff"
            | "aif" | "alac" | "ape" | "mka" | "mid" | "midi" | "amr" | "dsf",
        ) => KindCategory::Audio,
        Some(
            "pdf" | "doc" | "docx" | "dot" | "xls" | "xlsx" | "xlsm" | "csv" | "tsv" | "ppt"
            | "pptx" | "txt" | "md" | "markdown" | "rtf" | "odt" | "ods" | "odp" | "epub" | "mobi"
            | "azw" | "azw3" | "fb2" | "tex" | "log" | "html" | "htm" | "xml" | "json" | "yaml"
            | "yml" | "toml" | "ini" | "conf",
        ) => KindCategory::Documents,
        Some(
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "tbz" | "tbz2" | "xz" | "txz" | "7z" | "rar"
            | "zst" | "lz4" | "br" | "z" | "lz" | "jar" | "iso" | "dmg" | "cab",
        ) => KindCategory::Archives,
        Some(
            "app" | "exe" | "msi" | "bat" | "cmd" | "deb" | "rpm" | "appimage" | "flatpak"
            | "desktop" | "apk" | "sh" | "bin" | "dll" | "so",
        ) => KindCategory::Applications,
        _ => KindCategory::Other,
    }
}

/// 日期时段桶分组键；日界按 UTC 日历日，与列表时间列的 UTC 显示口径一致。
/// `now` 注入便于测试。元数据未加载（None）→ Earlier 兜底；时间戳落在
/// 未来（时钟偏差）按今天处理。桶定义：今天 / 昨天 / 过去 7 天内余下
/// 日历日 / 过去 30 天内余下日历日 / 当年更早按日历月 / 往年。
pub(crate) fn date_group_key(timestamp: Option<SystemTime>, now: SystemTime) -> DateBucket {
    let Some(timestamp) = timestamp else {
        return DateBucket::Earlier;
    };
    let now_date = OffsetDateTime::from(now).date();
    let timestamp_date = OffsetDateTime::from(timestamp).date();
    let day_difference = (now_date - timestamp_date).whole_days();
    match day_difference {
        i64::MIN..=0 => DateBucket::Today,
        1 => DateBucket::Yesterday,
        2..=6 => DateBucket::ThisWeek,
        7..=29 => DateBucket::ThisMonth,
        _ => {
            if timestamp_date.year() == now_date.year() {
                DateBucket::Month(u8::from(timestamp_date.month()))
            } else {
                DateBucket::Earlier
            }
        }
    }
}

/// 动态大小分桶：输入按升序的文件尺寸数组，均分文件数量切最多
/// [`MAX_FILE_SIZE_GROUPS`] 组；切点落在不同值之间（相等大小的文件
/// 不切开）；不同尺寸数 ≤ 上限时每个尺寸独占一组；全部等大 → 1 组；
/// 空数组 → 无桶。返回（lo, hi）字节区间，升序排列。
pub(crate) fn dynamic_size_buckets(ascending_sizes: &[u64]) -> Vec<(u64, u64)> {
    if ascending_sizes.is_empty() {
        return Vec::new();
    }
    debug_assert!(
        ascending_sizes
            .windows(2)
            .all(|window| window[0] <= window[1]),
        "调用契约：尺寸数组必须升序"
    );
    let distinct_count = {
        let mut count = 1;
        for window in ascending_sizes.windows(2) {
            if window[0] != window[1] {
                count += 1;
            }
        }
        count
    };
    if distinct_count <= MAX_FILE_SIZE_GROUPS {
        // 文件少（或尺寸重合）时不硬切：每个不同尺寸一组。
        let mut buckets = Vec::with_capacity(distinct_count);
        let mut current = ascending_sizes[0];
        buckets.push((current, current));
        for &size in &ascending_sizes[1..] {
            if size != current {
                buckets.push((size, size));
                current = size;
            }
        }
        return buckets;
    }

    // 目标切点按比例取整；若切点落在等值段内则右移到段尾，整段归前一组。
    let total = ascending_sizes.len();
    let mut boundaries: Vec<usize> = Vec::with_capacity(MAX_FILE_SIZE_GROUPS - 1);
    for part in 1..MAX_FILE_SIZE_GROUPS {
        let target = (part * total + MAX_FILE_SIZE_GROUPS / 2) / MAX_FILE_SIZE_GROUPS;
        if target == 0 || target >= total {
            continue;
        }
        let mut boundary = target;
        while boundary < total && ascending_sizes[boundary] == ascending_sizes[boundary - 1] {
            boundary += 1;
        }
        let collides_with_previous = boundaries
            .last()
            .is_some_and(|previous| *previous >= boundary);
        if boundary >= total || collides_with_previous {
            continue;
        }
        boundaries.push(boundary);
    }
    let mut buckets = Vec::with_capacity(boundaries.len() + 1);
    let mut start = 0;
    for boundary in &boundaries {
        buckets.push((ascending_sizes[start], ascending_sizes[boundary - 1]));
        start = *boundary;
    }
    buckets.push((ascending_sizes[start], ascending_sizes[total - 1]));
    buckets
}

/// 定位尺寸所属的桶下标（桶升序且连续，取最后一个 lo ≤ size 的桶）。
/// 占位行大小未知时传 u64::MAX 即归最末兜底组。
pub(crate) fn size_group_bucket_index(buckets: &[(u64, u64)], size_bytes: u64) -> usize {
    buckets
        .iter()
        .rposition(|&(lower, _)| lower <= size_bytes)
        .unwrap_or(0)
}

/// 划分入参：排序方向决定组序列是否反转；语言用于在构造时本地化组标题。
pub(crate) struct FileGroupingContext {
    pub(crate) direction: SortDirection,
    pub(crate) language: UiLanguage,
}

/// 一个已划分的组段：组身份 + 组内条目引用（保持输入序，不重排）。
pub(crate) struct FileGroupSection<'a, T> {
    pub(crate) descriptor: FileGroupDescriptor,
    pub(crate) items: Vec<&'a T>,
}

/// 稳定划分：对"已按当前排序排好"的条目序列按分组键切段，组内条目
/// 相对顺序不变（组内顺序沿用现有排序），组序列按秩升序；排序方向为
/// 降序时仅反转组序列。通过键提取闭包解耦 UI 类型，列表行流与网格
/// 流段共用同一实现。分组是纯视图概念，绝不触发目录重扫。
pub(crate) fn partition_files_into_groups<'a, T>(
    items: &'a [T],
    context: FileGroupingContext,
    group_key_of: impl Fn(&T) -> FileGroupKey,
) -> Vec<FileGroupSection<'a, T>> {
    // 组数受维度上限约束（≤27），线性查重即可，不值得引入哈希容器。
    let mut encountered: Vec<(FileGroupKey, Vec<usize>)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let key = group_key_of(item);
        match encountered
            .iter_mut()
            .find(|(existing, _)| *existing == key)
        {
            Some((_, members)) => members.push(index),
            None => encountered.push((key, vec![index])),
        }
    }
    // 稳定排序：同秩（同维度下不会出现）保持首次出现序，保证确定性。
    encountered.sort_by_key(|(key, _)| key.rank());
    let mut sections: Vec<FileGroupSection<T>> = encountered
        .into_iter()
        .map(|(key, members)| FileGroupSection {
            descriptor: key.descriptor(context.language),
            items: members.into_iter().map(|index| &items[index]).collect(),
        })
        .collect();
    if context.direction == SortDirection::Descending {
        sections.reverse();
    }
    sections
}

/// 分组索引栏的一个条目：组短标签 + 组头顶点在内容流中的纵向偏移。
/// 偏移由列表行流/网格流段的渲染同源数据派生，与渲染高度累计天然
/// 同口径——不存在第二份分组划分或高度计算。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileGroupRailEntry {
    pub(crate) index_label: String,
    pub(crate) top_offset: f32,
}

/// 索引栏可见门控：分组开启（行流/流段里有组头即开启）且组数 ≥2。
/// 单组时索引栏没有导航价值，None 模式时根本没有组头条目。
pub(crate) fn file_group_rail_visible(entries: &[FileGroupRailEntry]) -> bool {
    entries.len() >= 2
}

/// 当前可视区顶部所在组：`viewport_position` 是视口顶点在内容流里的
/// 纵向位置（列表为行流空间、网格为面板内容空间），取"顶点上方或
/// 顶点处"的最后一个组。顶点还落在分组区之前（目录置顶段）时无高亮。
pub(crate) fn active_file_group_index(
    entries: &[FileGroupRailEntry],
    viewport_position: f32,
) -> Option<usize> {
    let position = viewport_position.max(0.0);
    entries
        .iter()
        .rposition(|entry| entry.top_offset <= position + f32::EPSILON)
}

/// 索引栏点击/扫动的滚动目标：组头顶点对齐视口顶部，`header_allowance`
/// 是内容流里组头顶点之前的固定留量（列表为表头高，与 reveal/虚拟范围
/// 同一口径），并夹到内容可支撑的最大偏移内，视口越界自愈不被击穿。
pub(crate) fn file_group_scroll_target_offset(
    top_offset: f32,
    header_allowance: f32,
    content_height: f32,
    viewport_height: f32,
) -> f32 {
    let max_offset = (content_height - viewport_height).max(0.0);
    (top_offset + header_allowance).clamp(0.0, max_offset)
}

#[cfg(test)]
#[path = "file_grouping/file_grouping_tests.rs"]
mod file_grouping_tests;
