use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone};
use file_search::{
    normalize_extension_tokens, normalize_size_range, MimePattern, SearchEntryTypeRule,
    SearchFileKind, SearchFilters, SearchMatchMode, SearchTextScope, SizeRange, TimeRange,
    SIZE_UNIT_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchEntryTypePreset {
    Spreadsheets,
    Video,
    Images,
    Text,
    Documents,
    Folders,
    Audio,
    Pdf,
    Files,
    Archives,
    Links,
}

impl SearchEntryTypePreset {
    pub(crate) const COMMON: [Self; 8] = [
        Self::Spreadsheets,
        Self::Video,
        Self::Images,
        Self::Text,
        Self::Documents,
        Self::Folders,
        Self::Audio,
        Self::Pdf,
    ];
    pub(crate) const MORE: [Self; 3] = [Self::Files, Self::Archives, Self::Links];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Spreadsheets => "Spreadsheets",
            Self::Video => "Video",
            Self::Images => "Images",
            Self::Text => "Text",
            Self::Documents => "Documents",
            Self::Folders => "Folders",
            Self::Audio => "Audio",
            Self::Pdf => "PDF",
            Self::Files => "Files",
            Self::Archives => "Archives",
            Self::Links => "Links",
        }
    }

    fn query_rules(self) -> Vec<SearchEntryTypeRule> {
        let kind = |value| SearchEntryTypeRule::Kind(value);
        let exact = |value: &str| SearchEntryTypeRule::Mime(MimePattern::Exact(value.to_owned()));
        let prefix = |value: &str| SearchEntryTypeRule::Mime(MimePattern::Prefix(value.to_owned()));
        match self {
            Self::Spreadsheets => vec![
                exact("application/vnd.ms-excel"),
                exact("application/vnd.oasis.opendocument.spreadsheet"),
                exact("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
            ],
            Self::Video => vec![prefix("video/")],
            Self::Images => vec![prefix("image/")],
            Self::Text => vec![prefix("text/")],
            Self::Documents => vec![
                prefix("text/"),
                exact("application/pdf"),
                exact("application/rtf"),
                exact("application/msword"),
                exact("application/vnd.ms-excel"),
                exact("application/vnd.ms-powerpoint"),
                exact("application/vnd.oasis.opendocument.text"),
                exact("application/vnd.oasis.opendocument.spreadsheet"),
                exact("application/vnd.oasis.opendocument.presentation"),
                exact("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
                exact("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
                exact("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
            ],
            Self::Folders => vec![kind(SearchFileKind::Directory)],
            Self::Audio => vec![prefix("audio/")],
            Self::Pdf => vec![exact("application/pdf")],
            Self::Files => vec![kind(SearchFileKind::File)],
            Self::Archives => vec![
                exact("application/zip"),
                exact("application/x-7z-compressed"),
                exact("application/vnd.rar"),
                exact("application/x-rar-compressed"),
                exact("application/x-tar"),
                exact("application/gzip"),
                exact("application/x-bzip2"),
                exact("application/x-xz"),
                exact("application/zstd"),
            ],
            Self::Links => vec![kind(SearchFileKind::Symlink)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchDateField {
    Accessed,
    Modified,
    Created,
}

impl SearchDateField {
    pub(crate) const ALL: [Self; 3] = [Self::Accessed, Self::Modified, Self::Created];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Accessed => "Accessed",
            Self::Modified => "Modified",
            Self::Created => "Created",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchDatePreset {
    Any,
    Today,
    Yesterday,
    PastSevenDays,
    PastThirtyDays,
    PastYear,
}

impl SearchDatePreset {
    pub(crate) const ALL: [Self; 6] = [
        Self::Any,
        Self::Today,
        Self::Yesterday,
        Self::PastSevenDays,
        Self::PastThirtyDays,
        Self::PastYear,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Any => "Any time",
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::PastSevenDays => "Past 7 days",
            Self::PastThirtyDays => "Past 30 days",
            Self::PastYear => "Past year",
        }
    }
}

/// 大小档位：与资源管理器档位一致，边界语义统一为 `[min, max)`。
/// 档位 → 区间映射只存在于 `range()`，单位 1024 进制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchSizePreset {
    Any,
    Empty,
    Tiny,
    Small,
    Medium,
    Large,
    Huge,
    Gigantic,
    Custom,
}

impl SearchSizePreset {
    pub(crate) const ALL: [Self; 9] = [
        Self::Any,
        Self::Empty,
        Self::Tiny,
        Self::Small,
        Self::Medium,
        Self::Large,
        Self::Huge,
        Self::Gigantic,
        Self::Custom,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Any => "Any size",
            Self::Empty => "Empty (0 B)",
            Self::Tiny => "Tiny (0-16 KB)",
            Self::Small => "Small (16 KB-1 MB)",
            Self::Medium => "Medium (1-128 MB)",
            Self::Large => "Large (128 MB-1 GB)",
            Self::Huge => "Huge (1-4 GB)",
            Self::Gigantic => "Gigantic (>4 GB)",
            Self::Custom => "Custom",
        }
    }

    /// 唯一的档位 → 区间映射；Custom 由查询构造时按输入文本解析，不经此表。
    pub(crate) fn range(self) -> Option<SizeRange> {
        let kb = SIZE_UNIT_BYTES;
        let mb = kb * kb;
        let gb = mb * kb;
        match self {
            Self::Any | Self::Custom => None,
            // 「空」= 仅 size == 0，用 [0, 1) 表达，无特判。
            Self::Empty => Some(SizeRange {
                min_bytes: 0,
                max_bytes: Some(1),
            }),
            Self::Tiny => Some(SizeRange {
                min_bytes: 0,
                max_bytes: Some(16 * kb),
            }),
            Self::Small => Some(SizeRange {
                min_bytes: 16 * kb,
                max_bytes: Some(mb),
            }),
            Self::Medium => Some(SizeRange {
                min_bytes: mb,
                max_bytes: Some(128 * mb),
            }),
            Self::Large => Some(SizeRange {
                min_bytes: 128 * mb,
                max_bytes: Some(gb),
            }),
            Self::Huge => Some(SizeRange {
                min_bytes: gb,
                max_bytes: Some(4 * gb),
            }),
            Self::Gigantic => Some(SizeRange {
                min_bytes: 4 * gb,
                max_bytes: None,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchFilterPresetState {
    pub(crate) selected_entry_types: Vec<SearchEntryTypePreset>,
    pub(crate) text_scope: SearchTextScope,
    pub(crate) match_mode: SearchMatchMode,
    pub(crate) date_field: SearchDateField,
    pub(crate) date_preset: SearchDatePreset,
    pub(crate) size_preset: SearchSizePreset,
    pub(crate) custom_extensions: String,
    pub(crate) custom_extensions_open: bool,
    pub(crate) custom_size_min: String,
    pub(crate) custom_size_max: String,
}

impl Default for SearchFilterPresetState {
    fn default() -> Self {
        Self {
            selected_entry_types: Vec::new(),
            text_scope: SearchTextScope::NameAndContent,
            match_mode: SearchMatchMode::Plain,
            date_field: SearchDateField::Modified,
            date_preset: SearchDatePreset::Any,
            size_preset: SearchSizePreset::Any,
            custom_extensions: String::new(),
            custom_extensions_open: false,
            custom_size_min: String::new(),
            custom_size_max: String::new(),
        }
    }
}

impl SearchFilterPresetState {
    pub(crate) fn toggle_entry_type(&mut self, entry_type: SearchEntryTypePreset) {
        if let Some(index) = self
            .selected_entry_types
            .iter()
            .position(|selected| *selected == entry_type)
        {
            self.selected_entry_types.remove(index);
        } else {
            self.selected_entry_types.push(entry_type);
        }
    }

    pub(crate) fn entry_type_is_selected(&self, entry_type: SearchEntryTypePreset) -> bool {
        self.selected_entry_types.contains(&entry_type)
    }

    pub(crate) fn toggle_custom_extensions(&mut self) {
        self.custom_extensions_open = !self.custom_extensions_open;
        if !self.custom_extensions_open {
            self.custom_extensions.clear();
        }
    }

    /// 按钮高亮语义（产品确认）：面板展开且当前输入可解析出至少一个后缀。
    pub(crate) fn custom_extensions_are_active(&self) -> bool {
        self.custom_extensions_open
            && normalize_extension_tokens(&self.custom_extensions)
                .is_ok_and(|tokens| !tokens.is_empty())
    }

    /// 选择档位即切换：离开 Custom 时收起输入行并清空两侧输入（进入时必为空）。
    pub(crate) fn select_size_preset(&mut self, preset: SearchSizePreset) {
        if preset != SearchSizePreset::Custom {
            self.custom_size_min.clear();
            self.custom_size_max.clear();
        }
        self.size_preset = preset;
    }

    /// 空闲态判断用的大小条件意图：Custom 档下任一侧输入非空即算意图。
    /// 不问当前文本能否解析出合法区间——非法输入必须照常进入 query 构造，
    /// 由 `query_filters_at` 的共享解析在 restart 边界 reject（PRD：非法输入
    /// 拒绝提交并显示报错）；若按"可解析"判定，非法输入会静默回落空闲态并
    /// 吞掉报错。两侧全空 = 不限，不算意图，维持无查询空闲态。
    pub(crate) fn custom_size_has_intent(&self) -> bool {
        self.size_preset == SearchSizePreset::Custom
            && (!self.custom_size_min.trim().is_empty() || !self.custom_size_max.trim().is_empty())
    }

    pub(crate) fn selected_more_type_count(&self) -> usize {
        SearchEntryTypePreset::MORE
            .iter()
            .filter(|entry_type| self.entry_type_is_selected(**entry_type))
            .count()
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn query_filters_at<Tz>(&self, now: DateTime<Tz>) -> Result<SearchFilters, String>
    where
        Tz: TimeZone,
    {
        let mut entry_type_rules = Vec::new();
        for entry_type in &self.selected_entry_types {
            for rule in entry_type.query_rules() {
                if !entry_type_rules.contains(&rule) {
                    entry_type_rules.push(rule);
                }
            }
        }
        let date_range = date_range_at(self.date_preset, now)?;
        let extensions = normalize_extension_tokens(&self.custom_extensions)?;
        let size = match self.size_preset {
            SearchSizePreset::Custom => {
                normalize_size_range(&self.custom_size_min, &self.custom_size_max)?
            }
            preset => preset.range(),
        };
        let (accessed, modified, created) = match (self.date_field, date_range) {
            (_, None) => (None, None, None),
            (SearchDateField::Accessed, range) => (range, None, None),
            (SearchDateField::Modified, range) => (None, range, None),
            (SearchDateField::Created, range) => (None, None, range),
        };
        Ok(SearchFilters {
            entry_type_rules,
            modified,
            accessed,
            created,
            extensions,
            size,
        })
    }
}

fn date_range_at<Tz>(
    preset: SearchDatePreset,
    now: DateTime<Tz>,
) -> Result<Option<TimeRange>, String>
where
    Tz: TimeZone,
{
    let end_ms = now.timestamp_millis();
    let start_ms = match preset {
        SearchDatePreset::Any => return Ok(None),
        SearchDatePreset::PastSevenDays => (now - Duration::days(7)).timestamp_millis(),
        SearchDatePreset::PastThirtyDays => (now - Duration::days(30)).timestamp_millis(),
        SearchDatePreset::PastYear => (now - Duration::days(365)).timestamp_millis(),
        SearchDatePreset::Today => local_calendar_start(&now, now.date_naive())?,
        SearchDatePreset::Yesterday => {
            let today = now.date_naive();
            let yesterday = today
                .pred_opt()
                .ok_or_else(|| "previous local calendar day is unavailable".to_owned())?;
            let today_start_ms = local_calendar_start(&now, today)?;
            return Ok(Some(TimeRange {
                start_ms: local_calendar_start(&now, yesterday)?,
                end_ms: today_start_ms.saturating_sub(1),
            }));
        }
    };
    Ok(Some(TimeRange { start_ms, end_ms }))
}

fn local_calendar_start<Tz>(now: &DateTime<Tz>, date: NaiveDate) -> Result<i64, String>
where
    Tz: TimeZone,
{
    now.timezone()
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
        .earliest()
        .map(|start| start.timestamp_millis())
        .ok_or_else(|| format!("local calendar boundary is unavailable for {date}"))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};
    use chrono_tz::America::New_York;

    use super::*;

    #[test]
    fn entry_types_compose_across_kind_and_mime_without_mutating_each_other() {
        let mut presets = SearchFilterPresetState::default();
        presets.toggle_entry_type(SearchEntryTypePreset::Folders);
        presets.toggle_entry_type(SearchEntryTypePreset::Images);
        presets.toggle_entry_type(SearchEntryTypePreset::Pdf);

        let filters = presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .unwrap();

        assert!(filters
            .entry_type_rules
            .contains(&SearchEntryTypeRule::Kind(SearchFileKind::Directory)));
        assert!(filters
            .entry_type_rules
            .contains(&SearchEntryTypeRule::Mime(MimePattern::Prefix(
                "image/".to_owned()
            ))));
        assert!(filters
            .entry_type_rules
            .contains(&SearchEntryTypeRule::Mime(MimePattern::Exact(
                "application/pdf".to_owned()
            ))));
        presets.toggle_entry_type(SearchEntryTypePreset::Images);
        assert!(!presets.entry_type_is_selected(SearchEntryTypePreset::Images));
        assert!(presets.entry_type_is_selected(SearchEntryTypePreset::Folders));
    }

    #[test]
    fn overlapping_type_presets_deduplicate_query_rules() {
        let mut presets = SearchFilterPresetState::default();
        for entry_type in [
            SearchEntryTypePreset::Documents,
            SearchEntryTypePreset::Spreadsheets,
            SearchEntryTypePreset::Text,
            SearchEntryTypePreset::Pdf,
        ] {
            presets.toggle_entry_type(entry_type);
        }

        let filters = presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .unwrap();

        for expected in [
            SearchEntryTypeRule::Mime(MimePattern::Prefix("text/".to_owned())),
            SearchEntryTypeRule::Mime(MimePattern::Exact("application/pdf".to_owned())),
            SearchEntryTypeRule::Mime(MimePattern::Exact("application/vnd.ms-excel".to_owned())),
        ] {
            assert_eq!(
                filters
                    .entry_type_rules
                    .iter()
                    .filter(|rule| **rule == expected)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn every_entry_type_preset_maps_to_an_expected_query_rule() {
        let cases = [
            (
                SearchEntryTypePreset::Spreadsheets,
                SearchEntryTypeRule::Mime(MimePattern::Exact(
                    "application/vnd.ms-excel".to_owned(),
                )),
            ),
            (
                SearchEntryTypePreset::Video,
                SearchEntryTypeRule::Mime(MimePattern::Prefix("video/".to_owned())),
            ),
            (
                SearchEntryTypePreset::Images,
                SearchEntryTypeRule::Mime(MimePattern::Prefix("image/".to_owned())),
            ),
            (
                SearchEntryTypePreset::Text,
                SearchEntryTypeRule::Mime(MimePattern::Prefix("text/".to_owned())),
            ),
            (
                SearchEntryTypePreset::Documents,
                SearchEntryTypeRule::Mime(MimePattern::Exact("application/pdf".to_owned())),
            ),
            (
                SearchEntryTypePreset::Folders,
                SearchEntryTypeRule::Kind(SearchFileKind::Directory),
            ),
            (
                SearchEntryTypePreset::Audio,
                SearchEntryTypeRule::Mime(MimePattern::Prefix("audio/".to_owned())),
            ),
            (
                SearchEntryTypePreset::Pdf,
                SearchEntryTypeRule::Mime(MimePattern::Exact("application/pdf".to_owned())),
            ),
            (
                SearchEntryTypePreset::Files,
                SearchEntryTypeRule::Kind(SearchFileKind::File),
            ),
            (
                SearchEntryTypePreset::Archives,
                SearchEntryTypeRule::Mime(MimePattern::Exact("application/zip".to_owned())),
            ),
            (
                SearchEntryTypePreset::Links,
                SearchEntryTypeRule::Kind(SearchFileKind::Symlink),
            ),
        ];

        for (preset, expected_rule) in cases {
            assert!(preset.query_rules().contains(&expected_rule), "{preset:?}");
        }
    }

    #[test]
    fn selected_date_field_is_the_only_time_constraint() {
        let now = New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        for date_field in SearchDateField::ALL {
            let presets = SearchFilterPresetState {
                date_field,
                date_preset: SearchDatePreset::Today,
                ..SearchFilterPresetState::default()
            };
            let filters = presets.query_filters_at(now).unwrap();

            assert_eq!(
                filters.accessed.is_some(),
                date_field == SearchDateField::Accessed
            );
            assert_eq!(
                filters.modified.is_some(),
                date_field == SearchDateField::Modified
            );
            assert_eq!(
                filters.created.is_some(),
                date_field == SearchDateField::Created
            );
        }
    }

    #[test]
    fn reset_restores_one_default_query_state() {
        let mut presets = SearchFilterPresetState {
            selected_entry_types: vec![SearchEntryTypePreset::Folders],
            text_scope: SearchTextScope::NameOnly,
            match_mode: SearchMatchMode::Regex,
            date_field: SearchDateField::Created,
            date_preset: SearchDatePreset::PastYear,
            custom_extensions: ".PDF".to_owned(),
            custom_extensions_open: true,
            ..SearchFilterPresetState::default()
        };

        presets.reset();

        assert!(presets.is_default());
        let filters = presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .unwrap();
        assert!(filters.entry_type_rules.is_empty());
        assert!(filters.modified.is_none());
        assert!(filters.accessed.is_none());
        assert!(filters.created.is_none());
    }

    #[test]
    fn today_uses_local_midnight_across_a_dst_transition() {
        let now = New_York.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
        let range = date_range_at(SearchDatePreset::Today, now)
            .unwrap()
            .unwrap();

        assert_eq!(
            range.end_ms - range.start_ms,
            Duration::hours(11).num_milliseconds()
        );
    }

    #[test]
    fn yesterday_uses_the_complete_local_calendar_day_across_dst() {
        let now = New_York.with_ymd_and_hms(2026, 3, 9, 12, 0, 0).unwrap();
        let expected_start = New_York.with_ymd_and_hms(2026, 3, 8, 0, 0, 0).unwrap();
        let expected_end = New_York
            .with_ymd_and_hms(2026, 3, 9, 0, 0, 0)
            .unwrap()
            .timestamp_millis()
            - 1;
        let range = date_range_at(SearchDatePreset::Yesterday, now)
            .unwrap()
            .unwrap();

        assert_eq!(range.start_ms, expected_start.timestamp_millis());
        assert_eq!(range.end_ms, expected_end);
    }

    #[test]
    fn rolling_presets_use_fixed_elapsed_duration_across_dst() {
        let now = New_York.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();
        for (preset, days) in [
            (SearchDatePreset::PastSevenDays, 7),
            (SearchDatePreset::PastThirtyDays, 30),
            (SearchDatePreset::PastYear, 365),
        ] {
            let range = date_range_at(preset, now).unwrap().unwrap();
            assert_eq!(
                range.end_ms - range.start_ms,
                Duration::days(days).num_milliseconds()
            );
        }
    }

    #[test]
    fn custom_extensions_flow_into_query_filters_and_toggle_clears_them() {
        let mut presets = SearchFilterPresetState::default();
        presets.custom_extensions = ".PDF docx".to_owned();
        presets.custom_extensions_open = true;
        assert!(presets.custom_extensions_are_active());

        let filters = presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .unwrap();
        assert_eq!(
            filters.extensions,
            vec!["pdf".to_owned(), "docx".to_owned()]
        );

        presets.toggle_custom_extensions();
        assert!(presets.custom_extensions.is_empty());
        assert!(!presets.custom_extensions_are_active());
        assert!(presets.is_default());
    }

    #[test]
    fn invalid_custom_extensions_reject_the_query_filters() {
        let mut presets = SearchFilterPresetState::default();
        presets.custom_extensions = "pd*f".to_owned();
        presets.custom_extensions_open = true;
        assert!(!presets.custom_extensions_are_active());
        assert!(presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .is_err());
    }

    #[test]
    fn size_presets_map_to_half_open_resource_manager_bounds() {
        let kb = SIZE_UNIT_BYTES;
        let mb = kb * kb;
        let gb = mb * kb;
        let expected = [
            (SearchSizePreset::Any, None),
            (
                SearchSizePreset::Empty,
                Some(SizeRange {
                    min_bytes: 0,
                    max_bytes: Some(1),
                }),
            ),
            (
                SearchSizePreset::Tiny,
                Some(SizeRange {
                    min_bytes: 0,
                    max_bytes: Some(16 * kb),
                }),
            ),
            (
                SearchSizePreset::Small,
                Some(SizeRange {
                    min_bytes: 16 * kb,
                    max_bytes: Some(mb),
                }),
            ),
            (
                SearchSizePreset::Medium,
                Some(SizeRange {
                    min_bytes: mb,
                    max_bytes: Some(128 * mb),
                }),
            ),
            (
                SearchSizePreset::Large,
                Some(SizeRange {
                    min_bytes: 128 * mb,
                    max_bytes: Some(gb),
                }),
            ),
            (
                SearchSizePreset::Huge,
                Some(SizeRange {
                    min_bytes: gb,
                    max_bytes: Some(4 * gb),
                }),
            ),
            (
                SearchSizePreset::Gigantic,
                Some(SizeRange {
                    min_bytes: 4 * gb,
                    max_bytes: None,
                }),
            ),
        ];
        for (preset, range) in expected {
            assert_eq!(preset.range(), range, "{preset:?}");
            // 档位直接写入查询过滤器（Custom 除外）。
            let presets = SearchFilterPresetState {
                size_preset: preset,
                ..SearchFilterPresetState::default()
            };
            let filters = presets
                .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
                .unwrap();
            assert_eq!(filters.size, range, "{preset:?}");
        }
    }

    #[test]
    fn custom_size_requires_custom_preset_and_parses_both_sides() {
        let mut presets = SearchFilterPresetState::default();
        presets.size_preset = SearchSizePreset::Custom;
        // 全空 = 不限：不算意图，维持空闲态，查询不带大小条件。
        assert!(!presets.custom_size_has_intent());
        let filters = presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .unwrap();
        assert!(filters.size.is_none());

        // 任一侧有输入即构成意图，可解析时写入归一化区间。
        presets.custom_size_min = "16kb".to_owned();
        assert!(presets.custom_size_has_intent());
        let filters = presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .unwrap();
        assert_eq!(
            filters.size,
            Some(SizeRange {
                min_bytes: 16 * SIZE_UNIT_BYTES,
                max_bytes: None,
            })
        );

        // 非法输入（最小值大于最大值、未知单位）沿 reject 路径拒绝；
        // 意图仍在，保证 restart 不静默回落空闲态、报错可见。
        presets.custom_size_max = "1kb".to_owned();
        assert!(presets.custom_size_has_intent());
        assert!(presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .is_err());
        presets.custom_size_min = "5 TB".to_owned();
        presets.custom_size_max = String::new();
        assert!(presets.custom_size_has_intent());
        assert!(presets
            .query_filters_at(New_York.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap())
            .is_err());
    }

    #[test]
    fn leaving_custom_size_preset_clears_inputs_and_reset_restores_any() {
        let mut presets = SearchFilterPresetState {
            size_preset: SearchSizePreset::Custom,
            custom_size_min: "1MB".to_owned(),
            custom_size_max: "2GB".to_owned(),
            ..SearchFilterPresetState::default()
        };
        assert!(presets.custom_size_has_intent());

        // 从 Custom 切走：清空两侧输入并回到普通档位。
        presets.select_size_preset(SearchSizePreset::Medium);
        assert!(presets.custom_size_min.is_empty());
        assert!(presets.custom_size_max.is_empty());
        assert_eq!(presets.size_preset, SearchSizePreset::Medium);
        assert!(!presets.custom_size_has_intent());

        // 切到 Custom：输入行保持为空（展开即空）。
        presets.select_size_preset(SearchSizePreset::Custom);
        assert!(presets.custom_size_min.is_empty());
        assert!(presets.custom_size_max.is_empty());
        assert!(!presets.custom_size_has_intent());

        // reset 恢复任意大小并清空输入。
        presets.custom_size_min = "1MB".to_owned();
        presets.reset();
        assert!(presets.is_default());
        assert_eq!(presets.size_preset, SearchSizePreset::Any);
    }
}
