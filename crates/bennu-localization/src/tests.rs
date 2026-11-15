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
