// 文件分组纯函数层的单元测试；与实现分文件以守住单文件 800 行约束。
use std::time::Duration;

use file_core::SortDirection;
use time::Month as TimeMonth;

use super::*;

fn system_time(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> SystemTime {
    let date = time::Date::from_calendar_date(year, TimeMonth::try_from(month).unwrap(), day)
        .expect("valid test date");
    let datetime = date.midnight().assume_utc()
        + Duration::from_secs(u64::from(hour) * 3600 + u64::from(minute) * 60);
    SystemTime::UNIX_EPOCH + Duration::from_secs(datetime.unix_timestamp() as u64)
}

fn name_initial_key(name: &str) -> FileGroupKey {
    FileGroupKey::NameInitial(name_initial_group_key(name))
}

#[test]
fn name_initial_maps_ascii_letters_uppercase() {
    assert_eq!(name_initial_group_key("apple.txt"), 'A');
    assert_eq!(name_initial_group_key("zebra"), 'Z');
    assert_eq!(name_initial_group_key("File"), 'F');
}

#[test]
fn name_initial_maps_common_chinese_by_pinyin() {
    assert_eq!(name_initial_group_key("中文.txt"), 'Z');
    assert_eq!(name_initial_group_key("北京"), 'B');
    assert_eq!(name_initial_group_key("你好"), 'N');
    assert_eq!(name_initial_group_key("图片.png"), 'T');
    assert_eq!(name_initial_group_key("文件"), 'W');
}

#[test]
fn name_initial_falls_back_to_hash_for_digits_symbols_and_rare_chars() {
    assert_eq!(name_initial_group_key("1password"), '#');
    assert_eq!(name_initial_group_key("_cache"), '#');
    assert_eq!(name_initial_group_key("（括号）.txt"), '#');
    // CJK 扩展 A 区生僻字，不在 GB2312 一级字库表内。
    assert_eq!(name_initial_group_key("\u{3400}"), '#');
    assert_eq!(name_initial_group_key(""), '#');
}

#[test]
fn name_initial_ranks_hash_before_letters() {
    assert_eq!(name_initial_key("#").rank(), 0);
    assert_eq!(name_initial_key("a").rank(), 1);
    assert_eq!(name_initial_key("z").rank(), 26);
}

#[test]
fn kind_category_maps_by_extension_case_insensitively() {
    assert_eq!(kind_category_group_key("photo.JPG"), KindCategory::Images);
    assert_eq!(kind_category_group_key("icon.svg"), KindCategory::Images);
    assert_eq!(kind_category_group_key("movie.MP4"), KindCategory::Videos);
    assert_eq!(kind_category_group_key("clip.mkv"), KindCategory::Videos);
    assert_eq!(kind_category_group_key("song.flac"), KindCategory::Audio);
    assert_eq!(kind_category_group_key("voice.opus"), KindCategory::Audio);
    assert_eq!(
        kind_category_group_key("report.pdf"),
        KindCategory::Documents
    );
    assert_eq!(kind_category_group_key("notes.md"), KindCategory::Documents);
    assert_eq!(
        kind_category_group_key("backup.tar.gz"),
        KindCategory::Archives
    );
    assert_eq!(kind_category_group_key("bundle.7z"), KindCategory::Archives);
    assert_eq!(
        kind_category_group_key("tool.AppImage"),
        KindCategory::Applications
    );
    assert_eq!(
        kind_category_group_key("run.sh"),
        KindCategory::Applications
    );
}

#[test]
fn kind_category_falls_back_to_other_for_unknown_and_missing_extensions() {
    assert_eq!(kind_category_group_key("Makefile"), KindCategory::Other);
    assert_eq!(kind_category_group_key("data.xyz123"), KindCategory::Other);
    assert_eq!(kind_category_group_key(".gitignore"), KindCategory::Other);
    assert_eq!(KindCategory::Other.rank(), 6);
}

#[test]
fn date_buckets_cover_fixed_now_boundaries() {
    let now = system_time(2024, 4, 15, 12, 0);

    // 今天（含当日任意时刻）与未来时间戳（时钟偏差）。
    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 15, 0, 0)), now),
        DateBucket::Today
    );
    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 15, 23, 30)), now),
        DateBucket::Today
    );
    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 16, 8, 0)), now),
        DateBucket::Today
    );

    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 14, 23, 0)), now),
        DateBucket::Yesterday
    );
    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 14, 0, 0)), now),
        DateBucket::Yesterday
    );

    // 过去 7 天窗口：日历日差 2..=6。
    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 13, 12, 0)), now),
        DateBucket::ThisWeek
    );
    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 9, 0, 0)), now),
        DateBucket::ThisWeek
    );

    // 过去 30 天窗口：日历日差 7..=29。
    assert_eq!(
        date_group_key(Some(system_time(2024, 4, 8, 12, 0)), now),
        DateBucket::ThisMonth
    );
    assert_eq!(
        date_group_key(Some(system_time(2024, 3, 17, 12, 0)), now),
        DateBucket::ThisMonth
    );

    // 当年更早按日历月；边界日差 30 归月桶。
    assert_eq!(
        date_group_key(Some(system_time(2024, 3, 16, 12, 0)), now),
        DateBucket::Month(3)
    );
    assert_eq!(
        date_group_key(Some(system_time(2024, 1, 1, 0, 0)), now),
        DateBucket::Month(1)
    );

    // 往年与元数据缺失。
    assert_eq!(
        date_group_key(Some(system_time(2023, 12, 31, 12, 0)), now),
        DateBucket::Earlier
    );
    assert_eq!(date_group_key(None, now), DateBucket::Earlier);
}

#[test]
fn date_buckets_cross_year_boundaries() {
    let january_now = system_time(2024, 1, 10, 12, 0);
    // 30 天滚动窗口优先于日历月：12 月末条目在 1 月初仍归 ThisMonth。
    assert_eq!(
        date_group_key(Some(system_time(2023, 12, 20, 12, 0)), january_now),
        DateBucket::ThisMonth
    );
    // 窗口之外的往年条目 → Earlier。
    assert_eq!(
        date_group_key(Some(system_time(2023, 11, 1, 12, 0)), january_now),
        DateBucket::Earlier
    );
    // 2 月回看 1 月：30 天窗口内归 ThisMonth。
    let february_now = system_time(2024, 2, 5, 12, 0);
    assert_eq!(
        date_group_key(Some(system_time(2024, 1, 20, 12, 0)), february_now),
        DateBucket::ThisMonth
    );
}

#[test]
fn date_bucket_ranks_run_oldest_to_newest() {
    assert!(DateBucket::Earlier.rank() < DateBucket::Month(1).rank());
    assert!(DateBucket::Month(1).rank() < DateBucket::Month(12).rank());
    assert!(DateBucket::Month(12).rank() < DateBucket::ThisMonth.rank());
    assert!(DateBucket::ThisMonth.rank() < DateBucket::ThisWeek.rank());
    assert!(DateBucket::ThisWeek.rank() < DateBucket::Yesterday.rank());
    assert!(DateBucket::Yesterday.rank() < DateBucket::Today.rank());
}

#[test]
fn date_bucket_titles_use_localized_finished_text() {
    let english = UiLanguage::English;
    assert_eq!(DateBucket::Today.title(english), "Today");
    assert_eq!(DateBucket::Earlier.title(english), "Earlier");
    assert_eq!(DateBucket::Month(8).title(english), "August");
    assert_eq!(DateBucket::Month(1).title(UiLanguage::Chinese), "今年1月");
    assert_eq!(DateBucket::Month(12).title(UiLanguage::Chinese), "今年12月");
}

#[test]
fn size_buckets_handle_trivial_inputs() {
    assert!(dynamic_size_buckets(&[]).is_empty());
    // 全部等大 → 1 组。
    assert_eq!(dynamic_size_buckets(&[10, 10, 10]), vec![(10, 10)]);
    // 不同尺寸 ≤ 5 → 每尺寸一组；相等大小的文件不切开。
    assert_eq!(
        dynamic_size_buckets(&[1, 2, 3, 4, 5]),
        vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]
    );
    assert_eq!(dynamic_size_buckets(&[5, 5, 9]), vec![(5, 5), (9, 9)]);
}

#[test]
fn size_buckets_split_by_count_up_to_five() {
    let sizes: Vec<u64> = (1..=12).collect();
    assert_eq!(
        dynamic_size_buckets(&sizes),
        vec![(1, 2), (3, 5), (6, 7), (8, 10), (11, 12)]
    );
    // 100 个不同尺寸 → 恰好 5 组，数量大致均分。
    let large: Vec<u64> = (1..=100).collect();
    let buckets = dynamic_size_buckets(&large);
    assert_eq!(buckets.len(), MAX_FILE_SIZE_GROUPS);
    let per_bucket: Vec<usize> = buckets
        .iter()
        .map(|&(lo, hi)| (hi - lo + 1) as usize)
        .collect();
    assert_eq!(per_bucket, vec![20, 20, 20, 20, 20]);
}

#[test]
fn size_buckets_never_cut_equal_sizes() {
    // 等值段横跨切点时整段归前一组，段内文件不被拆开。
    let sizes = [1, 1, 1, 1, 1, 1, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!(
        dynamic_size_buckets(&sizes),
        vec![(1, 1), (2, 3), (4, 5), (6, 8)]
    );
    // 尾部等值段延伸到数组末尾时并入最后一组。
    let tail_equal = [1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9];
    let buckets = dynamic_size_buckets(&tail_equal);
    assert_eq!(buckets.last(), Some(&(8, 9)));
}

#[test]
fn size_bucket_index_locates_containing_bucket() {
    let buckets = dynamic_size_buckets(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    assert_eq!(size_group_bucket_index(&buckets, 1), 0);
    assert_eq!(size_group_bucket_index(&buckets, 5), 1);
    assert_eq!(size_group_bucket_index(&buckets, 12), 4);
    // 未知大小（占位行）归最末兜底组。
    assert_eq!(size_group_bucket_index(&buckets, u64::MAX), 4);
}

#[test]
fn size_key_title_shows_range_or_single_value() {
    let english = UiLanguage::English;
    let single = FileGroupKey::Size {
        bucket_index: 0,
        bucket_count: 1,
        min_bytes: 2048,
        max_bytes: 2048,
    };
    assert_eq!(single.title(english), "2.0 KB");
    let range = FileGroupKey::Size {
        bucket_index: 1,
        bucket_count: 3,
        min_bytes: 1024,
        max_bytes: 45 * 1024 * 1024,
    };
    assert_eq!(range.title(english), "1.0 KB ~ 45 MB");
}

#[test]
fn partition_preserves_within_group_order_and_sorts_groups_by_rank() {
    let names = ["apple", "banana", "avocado", "cherry", "1file", "apricot"];
    let sections = partition_files_into_groups(
        &names,
        FileGroupingContext {
            direction: SortDirection::Ascending,
            language: UiLanguage::English,
        },
        |name| name_initial_key(name),
    );
    let titles: Vec<&str> = sections
        .iter()
        .map(|section| section.descriptor.title.as_str())
        .collect();
    assert_eq!(titles, vec!["#", "A", "B", "C"]);

    let a_items: Vec<&str> = sections[1].items.iter().map(|name| **name).collect();
    assert_eq!(a_items, vec!["apple", "avocado", "apricot"]);
}

#[test]
fn partition_reverses_group_order_for_descending_but_keeps_items() {
    let names = ["apple", "banana", "cherry", "1file"];
    let sections = partition_files_into_groups(
        &names,
        FileGroupingContext {
            direction: SortDirection::Descending,
            language: UiLanguage::English,
        },
        |name| name_initial_key(name),
    );
    let titles: Vec<&str> = sections
        .iter()
        .map(|section| section.descriptor.title.as_str())
        .collect();
    assert_eq!(titles, vec!["C", "B", "A", "#"]);
    // 组内条目保持输入序：降序只反转组序列。
    assert_eq!(sections[2].items.len(), 1);
    assert_eq!(*sections[2].items[0], "apple");
}

#[test]
fn partition_stays_stable_for_equal_keys_and_empty_input() {
    let names = ["a1", "a2", "a3"];
    let sections = partition_files_into_groups(
        &names,
        FileGroupingContext {
            direction: SortDirection::Ascending,
            language: UiLanguage::English,
        },
        |name| name_initial_key(name),
    );
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].items.len(), 3);
    assert_eq!(*sections[0].items[0], "a1");
    assert_eq!(*sections[0].items[2], "a3");

    let empty: [&str; 0] = [];
    let sections = partition_files_into_groups(
        &empty,
        FileGroupingContext {
            direction: SortDirection::Ascending,
            language: UiLanguage::English,
        },
        |name| name_initial_key(name),
    );
    assert!(sections.is_empty());
}

#[test]
fn partition_carries_descriptor_rank_for_date_keys() {
    let now = system_time(2024, 4, 15, 12, 0);
    let timestamps = [
        (system_time(2024, 4, 15, 9, 0), "today-log"),
        (system_time(2023, 6, 1, 9, 0), "old-log"),
        (system_time(2024, 4, 10, 9, 0), "week-log"),
    ];
    let sections = partition_files_into_groups(
        &timestamps,
        FileGroupingContext {
            direction: SortDirection::Ascending,
            language: UiLanguage::English,
        },
        |(timestamp, _)| FileGroupKey::Date(date_group_key(Some(*timestamp), now)),
    );
    let titles: Vec<&str> = sections
        .iter()
        .map(|section| section.descriptor.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Earlier", "This Week", "Today"]);
    assert!(sections[0].descriptor.rank < sections[1].descriptor.rank);
    assert!(sections[1].descriptor.rank < sections[2].descriptor.rank);
}

#[test]
fn grouping_mode_config_values_roundtrip() {
    for mode in [
        FileGroupingMode::None,
        FileGroupingMode::NameInitial,
        FileGroupingMode::Kind,
        FileGroupingMode::Size,
        FileGroupingMode::ModifiedTime,
        FileGroupingMode::CreatedTime,
        FileGroupingMode::AccessedTime,
    ] {
        assert_eq!(
            FileGroupingMode::from_config_value(mode.config_value()),
            Some(mode)
        );
    }
    assert_eq!(FileGroupingMode::from_config_value("bogus"), None);
}

#[test]
fn grouping_mode_labels_use_menu_keys() {
    assert_eq!(FileGroupingMode::None.label(), "None");
    assert_eq!(FileGroupingMode::NameInitial.label(), "Name");
    assert_eq!(FileGroupingMode::Kind.label(), "Kind");
    assert_eq!(FileGroupingMode::Size.label(), "Size");
    assert_eq!(FileGroupingMode::ModifiedTime.label(), "Date Modified");
    assert_eq!(FileGroupingMode::CreatedTime.label(), "Date Created");
    assert_eq!(FileGroupingMode::AccessedTime.label(), "Date Accessed");
}

#[test]
fn grouping_menu_entry_is_listed_only_for_list_and_icons_views() {
    use crate::model::BrowserViewMode;

    assert!(file_grouping_entry_visible(BrowserViewMode::List));
    assert!(file_grouping_entry_visible(BrowserViewMode::Icons));
    // 多栏视图不应用分组,菜单也不提供入口。
    assert!(!file_grouping_entry_visible(BrowserViewMode::Columns));
}

/// 索引栏短标签的统一出口:键 → 描述符 → 短标签。
fn index_label_of(key: FileGroupKey, language: UiLanguage) -> String {
    key.descriptor(language).index_label
}

#[test]
fn index_labels_use_letters_for_name_initial_dimension() {
    assert_eq!(index_label_of(name_initial_key("apple.txt"), UiLanguage::English), "A");
    assert_eq!(index_label_of(name_initial_key("中文.txt"), UiLanguage::Chinese), "Z");
    assert_eq!(index_label_of(name_initial_key("1file"), UiLanguage::English), "#");
}

#[test]
fn index_labels_use_category_words_for_kind_dimension() {
    assert_eq!(
        index_label_of(
            FileGroupKey::Kind(kind_category_group_key("photo.jpg")),
            UiLanguage::English
        ),
        "Images"
    );
    assert_eq!(
        index_label_of(
            FileGroupKey::Kind(kind_category_group_key("photo.jpg")),
            UiLanguage::Chinese
        ),
        "图片"
    );
}

#[test]
fn index_labels_use_short_words_and_month_short_forms_for_dates() {
    let now = system_time(2024, 8, 15, 12, 0);
    let cases = [
        (system_time(2024, 8, 15, 9, 0), "Today", "今天"),
        (system_time(2024, 8, 14, 9, 0), "Yesterday", "昨天"),
        (system_time(2024, 8, 12, 9, 0), "This Week", "本周"),
        (system_time(2024, 8, 1, 9, 0), "This Month", "本月"),
        // 月份桶即短格式:英文取月名前三字母,中文"N月"。
        (system_time(2024, 3, 1, 9, 0), "Mar", "3月"),
        (system_time(2023, 3, 1, 9, 0), "Earlier", "更早"),
    ];
    for (timestamp, english, chinese) in cases {
        let key = FileGroupKey::Date(date_group_key(Some(timestamp), now));
        assert_eq!(index_label_of(key, UiLanguage::English), english);
        assert_eq!(index_label_of(key, UiLanguage::Chinese), chinese);
    }
    // 月份桶的短格式:英文取月名前三字母,中文"N月"。
    assert_eq!(
        index_label_of(FileGroupKey::Date(DateBucket::Month(8)), UiLanguage::English),
        "Aug"
    );
    assert_eq!(
        index_label_of(FileGroupKey::Date(DateBucket::Month(8)), UiLanguage::Chinese),
        "8月"
    );
}

#[test]
fn index_labels_compress_size_ranges_with_endpoint_semantics() {
    // 中间桶:lo~hi,单位压缩风格与常规大小显示一致(同压缩口径)。
    let middle = FileGroupKey::Size {
        bucket_index: 1,
        bucket_count: 3,
        min_bytes: 1_258_291,
        max_bytes: 47_185_920,
    };
    assert_eq!(index_label_of(middle, UiLanguage::English), "1.2M~45M");
    // 首桶 ≤ 上界;末桶 ≥ 下界;边界外尺寸也归属端点桶,标注真实。
    let first = FileGroupKey::Size {
        bucket_index: 0,
        bucket_count: 3,
        min_bytes: 1,
        max_bytes: 1_258_291,
    };
    assert_eq!(index_label_of(first, UiLanguage::English), "≤1.2M");
    let last = FileGroupKey::Size {
        bucket_index: 2,
        bucket_count: 3,
        min_bytes: 3_328_599_910,
        max_bytes: 9_000_000_000,
    };
    assert_eq!(index_label_of(last, UiLanguage::English), "≥3.1G");
    // 单值组只写单值。
    let single = FileGroupKey::Size {
        bucket_index: 1,
        bucket_count: 3,
        min_bytes: 1024,
        max_bytes: 1024,
    };
    assert_eq!(index_label_of(single, UiLanguage::English), "1.0K");
}

#[test]
fn compact_size_labels_share_compression_with_regular_size_display() {
    assert_eq!(crate::formatting::format_file_size_compact(512), "512B");
    assert_eq!(crate::formatting::format_file_size_compact(1_258_291), "1.2M");
    assert_eq!(crate::formatting::format_file_size(1_258_291), "1.2 MB");
    // 数值部分逐字一致,仅单位缩写与空格不同。
    assert_eq!(
        crate::formatting::format_file_size_compact(47_185_920),
        crate::formatting::format_file_size(47_185_920)
            .replace(" MB", "M")
    );
}

#[test]
fn rail_visibility_requires_at_least_two_groups() {
    assert!(!file_group_rail_visible(&[]));
    assert!(!file_group_rail_visible(&[FileGroupRailEntry {
        index_label: "A".to_owned(),
        top_offset: 0.0,
    }]));
    assert!(file_group_rail_visible(&[
        FileGroupRailEntry {
            index_label: "A".to_owned(),
            top_offset: 0.0,
        },
        FileGroupRailEntry {
            index_label: "B".to_owned(),
            top_offset: 32.0,
        },
    ]));
}

#[test]
fn active_group_follows_viewport_top_position() {
    let entries = [
        FileGroupRailEntry {
            index_label: "#".to_owned(),
            top_offset: 0.0,
        },
        FileGroupRailEntry {
            index_label: "A".to_owned(),
            top_offset: 80.0,
        },
        FileGroupRailEntry {
            index_label: "Z".to_owned(),
            top_offset: 200.0,
        },
    ];
    // 视口顶部落在目录置顶段(位置 0 之前语义按 0 处理)→ 第一组高亮。
    assert_eq!(active_file_group_index(&entries, -30.0), Some(0));
    assert_eq!(active_file_group_index(&entries, 79.0), Some(0));
    assert_eq!(active_file_group_index(&entries, 80.0), Some(1));
    assert_eq!(active_file_group_index(&entries, 999.0), Some(2));
}

#[test]
fn scroll_target_aligns_group_top_and_clamps_to_content() {
    // 表头留量加回:组头顶点对齐视口顶部。
    assert_eq!(
        file_group_scroll_target_offset(78.0, 32.0, 100_000.0, 400.0),
        110.0
    );
    // 目标越界时夹到内容尾部(内容高 - 视口高)。
    assert_eq!(
        file_group_scroll_target_offset(78.0, 32.0, 188.0, 100.0),
        88.0
    );
    // 内容塞得下时不产生滚动。
    assert_eq!(file_group_scroll_target_offset(0.0, 32.0, 80.0, 400.0), 0.0);
}
