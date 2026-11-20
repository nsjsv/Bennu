//! 文件模式（`f:`）：框里的字分成要找的词和筛选。筛选的选项就是搜索页下拉框里那些，读出来的就是
//! 搜索页用的那个 `SearchFilterPresetState`；真正的查询条件由它的 `query_filters_at` 算，和搜索页
//! 一套，写错的大小也在那里报。Ctrl+Enter 把同一个状态交给搜索页。

use chrono::Local;
use file_search::SearchFilters;

use crate::model::{
    SearchDatePreset, SearchEntryTypePreset, SearchFilterPresetState, SearchSizePreset,
};

use super::mode::fold;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FindQuery {
    pub(crate) terms: String,
    /// 交给搜索页的筛选。
    pub(crate) filters: SearchFilterPresetState,
    /// 拿去查索引的条件（读的时候用“现在”算好）。
    pub(crate) search_filters: SearchFilters,
}

impl FindQuery {
    /// 搜索模式顺带查的：整段字都是名字，不读筛选。
    pub(crate) fn names(text: &str) -> Self {
        Self {
            terms: text.to_owned(),
            filters: SearchFilterPresetState::default(),
            search_filters: SearchFilters::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterKey {
    Type,
    Time,
    Size,
}

impl FilterKey {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "类型" | "type" => Self::Type,
            "时间" | "time" | "date" => Self::Time,
            "大小" | "size" => Self::Size,
            _ => return None,
        })
    }
}

/// 英文和中文的写法，界面是什么语言都认（键也是两种都认）；当前语言的说法是 `label()`。
fn type_keywords(preset: SearchEntryTypePreset) -> &'static [&'static str] {
    match preset {
        SearchEntryTypePreset::Spreadsheets => &["spreadsheets", "表格"],
        SearchEntryTypePreset::Video => &["video", "视频"],
        SearchEntryTypePreset::Images => &["images", "图片"],
        SearchEntryTypePreset::Text => &["text", "文本"],
        SearchEntryTypePreset::Documents => &["documents", "文档"],
        SearchEntryTypePreset::Folders => &["folders", "文件夹"],
        SearchEntryTypePreset::Audio => &["audio", "音频"],
        SearchEntryTypePreset::Pdf => &["pdf"],
        SearchEntryTypePreset::Files => &["files", "文件"],
        SearchEntryTypePreset::Archives => &["archives", "压缩包"],
        SearchEntryTypePreset::Links => &["links", "链接"],
    }
}

/// 英文和中文的短写法，界面是什么语言都认；当前语言的完整说法是 `label()`。
fn date_keywords(preset: SearchDatePreset) -> &'static [&'static str] {
    match preset {
        SearchDatePreset::Any => &[],
        SearchDatePreset::Today => &["today", "今天"],
        SearchDatePreset::Yesterday => &["yesterday", "昨天"],
        SearchDatePreset::PastSevenDays => &["7d", "7天", "week"],
        SearchDatePreset::PastThirtyDays => &["30d", "30天", "month"],
        SearchDatePreset::PastYear => &["1y", "一年", "year"],
    }
}

/// 比较用：折叠大小写和全角，去掉空格（“近 7 天”和“近7天”一样）。
fn squeezed(text: &str) -> String {
    fold(text)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// `键:值`，键是认得的筛选名时才算筛选，其余的字（比如 `C:`）都是要找的词。
fn split_filter(word: &str) -> Option<(FilterKey, &str)> {
    let (at, colon) = word
        .char_indices()
        .find(|(_, character)| matches!(character, ':' | '：'))?;
    let key = FilterKey::named(&fold(&word[..at]))?;
    Some((key, &word[at + colon.len_utf8()..]))
}

pub(crate) fn read_find_query(text: &str) -> Result<FindQuery, String> {
    let mut terms = Vec::new();
    let mut filters = SearchFilterPresetState::default();
    for word in text.split_whitespace() {
        let Some((key, value)) = split_filter(word) else {
            terms.push(word);
            continue;
        };
        match key {
            FilterKey::Type => {
                for part in value.split([',', '，']).filter(|part| !part.is_empty()) {
                    let preset = type_preset(part)?;
                    if !filters.entry_type_is_selected(preset) {
                        filters.toggle_entry_type(preset);
                    }
                }
            }
            FilterKey::Time => filters.date_preset = date_preset(value)?,
            FilterKey::Size => {
                let (min, max) = size_bounds(value)?;
                filters.select_size_preset(SearchSizePreset::Custom);
                filters.custom_size_min = min.to_owned();
                filters.custom_size_max = max.to_owned();
            }
        }
    }
    let search_filters = filters.query_filters_at(Local::now())?;
    Ok(FindQuery {
        terms: terms.join(" "),
        filters,
        search_filters,
    })
}

fn type_preset(value: &str) -> Result<SearchEntryTypePreset, String> {
    let wanted = squeezed(value);
    SearchEntryTypePreset::ALL
        .into_iter()
        .find(|preset| {
            squeezed(&preset.label()) == wanted || type_keywords(*preset).contains(&wanted.as_str())
        })
        .ok_or_else(|| {
            let choices = SearchEntryTypePreset::ALL
                .into_iter()
                .map(SearchEntryTypePreset::label)
                .collect::<Vec<_>>()
                .join(" ");
            tr!("Unknown type “%1”. You can use: %2", value, choices)
        })
}

fn date_preset(value: &str) -> Result<SearchDatePreset, String> {
    let wanted = squeezed(value);
    SearchDatePreset::ALL
        .into_iter()
        .filter(|preset| *preset != SearchDatePreset::Any)
        .find(|preset| {
            squeezed(&preset.label()) == wanted || date_keywords(*preset).contains(&wanted.as_str())
        })
        .ok_or_else(|| {
            tr!(
                "Unknown time “%1”. You can use: %2",
                value,
                "today yesterday 7d 30d 1y"
            )
        })
}

/// `>10m` 至少、`<1g` 不到、`10m-1g` 之间。单位由搜索页的大小读法认。
fn size_bounds(value: &str) -> Result<(&str, &str), String> {
    if let Some(min) = value.strip_prefix('>') {
        return Ok((min, ""));
    }
    if let Some(max) = value.strip_prefix('<') {
        return Ok(("", max));
    }
    value
        .split_once('-')
        .filter(|(min, max)| !min.is_empty() && !max.is_empty())
        .ok_or_else(|| tr!("Write sizes like >10m, <1g or 10m-1g"))
}
