use super::*;

#[test]
fn playback_durations_are_written_as_clock_times() {
    assert_eq!(format_duration(std::time::Duration::from_secs(65)), "1:05");
    assert_eq!(
        format_duration(std::time::Duration::from_secs(3661)),
        "1:01:01"
    );
}

#[test]
fn file_sizes_are_written_the_kde_way() {
    for (bytes, regular, compact) in [
        (0, "0 B", "0B"),
        (1023, "1023 B", "1023B"),
        (1024, "1.0 KiB", "1.0K"),
        (15_000_000, "14.3 MiB", "14.3M"),
        (47_185_920, "45.0 MiB", "45.0M"),
        (1 << 40, "1.0 TiB", "1.0T"),
        (u64::MAX, "16.0 EiB", "16.0E"),
    ] {
        assert_eq!(format_file_size(bytes), regular);
        assert_eq!(format_file_size_compact(bytes), compact);
    }
}

#[test]
fn detects_chinese_from_lang_prefix() {
    let language = detect_system_language_with(|key| match key {
        "LANG" => Some("zh_CN.UTF-8".to_owned()),
        _ => None,
    });
    assert_eq!(language, UiLanguage::Chinese);
}

#[test]
fn detects_chinese_from_language_fallback() {
    let language = detect_system_language_with(|key| match key {
        "LANGUAGE" => Some("en_US:zh_CN".to_owned()),
        _ => None,
    });
    assert_eq!(language, UiLanguage::Chinese);
}

#[test]
fn defaults_to_english_for_non_chinese_locale() {
    let language = detect_system_language_with(|key| match key {
        "LANG" => Some("en_US.UTF-8".to_owned()),
        _ => None,
    });
    assert_eq!(language, UiLanguage::English);
}
