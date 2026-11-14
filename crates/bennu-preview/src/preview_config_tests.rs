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
    let stored: StoredPreviewPreferences =
        serde_json::from_str(LEGACY_DOCUMENT).expect("legacy document parses as preview segment");

    let limits =
        preview_size_limits_from_stored(&stored, PreviewFileSizeLimits::with_default_limits());
    // null（缺失）类型吃 legacy 全局值，显式分类型键覆盖全局值。
    assert_eq!(limits.image_bytes, 1048576);
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
fn untouched_legacy_archive_list_gains_the_new_tar_suffixes() {
    let defaults = PreviewExtensionRules::default_rules();
    let stored_archive = |extensions: &[&str]| StoredPreviewPreferences {
        preview_extension_rules: Some(StoredPreviewExtensionRules {
            archive: Some(
                extensions
                    .iter()
                    .map(|extension| (*extension).to_owned())
                    .collect(),
            ),
            ..StoredPreviewExtensionRules::default()
        }),
        ..StoredPreviewPreferences::default()
    };

    let rules = preview_extension_rules_from_stored(
        &stored_archive(&LEGACY_DEFAULT_PREVIEW_ARCHIVE_EXTENSIONS),
        &defaults,
    );
    assert_eq!(rules.archive, defaults.archive);
    assert!(rules.archive.contains(&"tar.zst".to_owned()));

    // 用户改过的表原样保留。
    let rules = preview_extension_rules_from_stored(&stored_archive(&["zip", "cbz"]), &defaults);
    assert_eq!(rules.archive, vec!["zip".to_owned(), "cbz".to_owned()]);
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
fn legacy_video_list_migrates_once_keeping_only_user_additions() {
    let defaults = PreviewExtensionRules::default_rules();
    let mut legacy_video = LEGACY_DEFAULT_PREVIEW_VIDEO_EXTENSIONS
        .iter()
        .map(|extension| (*extension).to_owned())
        .collect::<Vec<_>>();
    legacy_video.push("dav".to_owned());
    let legacy = StoredPreviewPreferences {
        preview_extension_rules: Some(StoredPreviewExtensionRules {
            video: Some(legacy_video),
            ..StoredPreviewExtensionRules::default()
        }),
        ..StoredPreviewPreferences::default()
    };

    // 旧内置 6 个去掉，只剩用户自己加的。
    let rules = preview_extension_rules_from_stored(&legacy, &defaults);
    assert_eq!(rules.video, vec!["dav".to_owned()]);

    // 新键写过之后旧键不再参与。
    let saved = StoredPreviewPreferences {
        preview_extra_video_extensions: Some(vec!["mp4".to_owned()]),
        ..legacy
    };
    let rules = preview_extension_rules_from_stored(&saved, &defaults);
    assert_eq!(rules.video, vec!["mp4".to_owned()]);

    // 什么都没存：默认空。
    let rules =
        preview_extension_rules_from_stored(&StoredPreviewPreferences::default(), &defaults);
    assert!(rules.video.is_empty());
}

#[test]
fn end_action_cycles_through_what_the_host_offers() {
    use AudioEndAction::{Next, Repeat, Stop};
    let main_app = [Stop, Repeat, Next];
    assert_eq!(Stop.after(&main_app), Repeat);
    assert_eq!(Repeat.after(&main_app), Next);
    assert_eq!(Next.after(&main_app), Stop);
    let dialog = [Stop, Repeat];
    assert_eq!(Repeat.after(&dialog), Stop);
    assert_eq!(Next.within(&dialog), Stop);
    assert_eq!(Next.after(&dialog), Stop);
    for action in main_app {
        assert_eq!(
            AudioEndAction::from_stored(Some(action.stored_name())),
            action
        );
    }
    assert_eq!(AudioEndAction::from_stored(Some("shuffle")), Stop);
    assert_eq!(AudioEndAction::from_stored(None), Stop);
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
