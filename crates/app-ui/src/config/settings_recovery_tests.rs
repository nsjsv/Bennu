use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use file_operation_store::{StoredUserPreferences, TaskQueueStore, USER_PREFERENCES_KEY};
use serde_json::{json, Value};

use super::app_config::{self, default_app_config};
use super::renderer_probe_cache;
use super::settings_recovery::ResetSettings;
use super::user_config_loading::load_user_config_from_sources;
use super::*;

/// 数据库里放一行原始设置，模拟坏数据或新版本写下的值。
fn database_with_preferences(directory: &Path, payload: &str) -> PathBuf {
    let database = directory.join("state.sqlite");
    TaskQueueStore::new(&database).expect("create store");
    rusqlite::Connection::open(&database)
        .expect("open database")
        .execute(
            "INSERT INTO user_preferences (preference_key, payload_json, updated_at_ms)
             VALUES (?1, ?2, 1)",
            rusqlite::params![USER_PREFERENCES_KEY, payload],
        )
        .expect("insert preferences");
    database
}

/// 以程序自己的默认设置为底（存储层的默认值不全是程序认得的值）。
fn default_payload_with(changes: Value) -> String {
    let defaults = default_user_config().user_preferences().to_stored();
    let mut payload = serde_json::to_value(defaults).unwrap();
    let object = payload.as_object_mut().unwrap();
    for (key, value) in changes.as_object().unwrap() {
        object.insert(key.clone(), value.clone());
    }
    payload.to_string()
}

fn restored(recovery: Option<SettingsRecovery>) -> (ResetSettings, PathBuf) {
    match recovery {
        Some(SettingsRecovery::Restored {
            source: SettingsSource::Preferences,
            reset,
            backup,
        }) => (reset, backup),
        other => panic!("expected restored settings, got {other:?}"),
    }
}

fn stored_row_is_readable(database: &Path) -> bool {
    TaskQueueStore::new(database)
        .unwrap()
        .read_user_preferences(&StoredUserPreferences::default())
        .unwrap()
        .is_some_and(|read| read.unreadable.is_none())
}

#[test]
fn wholly_unreadable_settings_are_backed_up_and_reset() {
    let directory = tempfile::tempdir().unwrap();
    let database = database_with_preferences(directory.path(), "{ not json");

    let loaded = load_user_config_from_sources(default_app_config(), &database, None);

    let (reset, backup) = restored(loaded.recovery);
    assert_eq!(reset, ResetSettings::All);
    assert_eq!(fs::read_to_string(backup).unwrap(), "{ not json");
    assert_eq!(
        loaded.config.user_preferences(),
        default_user_config().user_preferences()
    );
    assert!(stored_row_is_readable(&database));
}

#[test]
fn one_unreadable_key_is_reset_and_the_rest_is_kept() {
    let directory = tempfile::tempdir().unwrap();
    let payload = default_payload_with(json!({
        "show_hidden_files": "yes",
        "sidebar_width": 300.0,
    }));
    let database = database_with_preferences(directory.path(), &payload);

    let loaded = load_user_config_from_sources(default_app_config(), &database, None);

    let (reset, backup) = restored(loaded.recovery);
    assert_eq!(
        reset,
        ResetSettings::Keys(vec!["show_hidden_files".to_owned()])
    );
    assert_eq!(fs::read_to_string(backup).unwrap(), payload);
    assert_eq!(loaded.config.sidebar_width, 300.0);
    assert!(stored_row_is_readable(&database));
}

#[test]
fn a_value_this_version_does_not_know_is_reported_and_backed_up() {
    let directory = tempfile::tempdir().unwrap();
    let payload = default_payload_with(json!({
        "terminal_emulator": "terminal-from-the-future",
        "sidebar_width": 300.0,
    }));
    let database = database_with_preferences(directory.path(), &payload);

    let loaded = load_user_config_from_sources(default_app_config(), &database, None);

    let (reset, backup) = restored(loaded.recovery);
    assert_eq!(
        reset,
        ResetSettings::Keys(vec!["terminal_emulator".to_owned()])
    );
    assert_eq!(fs::read_to_string(backup).unwrap(), payload);
    assert_eq!(loaded.config.sidebar_width, 300.0);
}

#[test]
fn intact_settings_need_no_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let payload = default_payload_with(json!({"sidebar_width": 300.0}));
    let database = database_with_preferences(directory.path(), &payload);

    let loaded = load_user_config_from_sources(default_app_config(), &database, None);

    assert_eq!(loaded.recovery, None);
    assert_eq!(loaded.config.sidebar_width, 300.0);
}

#[test]
fn an_unopenable_database_uses_defaults_and_blocks_saving() {
    let directory = tempfile::tempdir().unwrap();
    // 数据库路径是个目录：打不开。
    let loaded = load_user_config_from_sources(default_app_config(), directory.path(), None);

    let recovery = loaded.recovery.expect("recovery");
    assert!(matches!(
        recovery,
        SettingsRecovery::Unavailable {
            source: SettingsSource::Preferences,
            ..
        }
    ));
    assert!(matches!(
        SettingsPersistence::for_source(&[recovery], SettingsSource::Preferences),
        SettingsPersistence::Blocked(_)
    ));
}

/// 数据库文件只读：照样能读，写不进去。
fn make_read_only(database: &Path) {
    fs::set_permissions(database, fs::Permissions::from_mode(0o444)).unwrap();
}

fn stored_raw_payload(database: &Path) -> String {
    TaskQueueStore::new(database)
        .unwrap()
        .read_user_preferences(&StoredUserPreferences::default())
        .unwrap()
        .expect("preferences row")
        .raw_payload
}

#[test]
fn damaged_settings_that_cannot_be_backed_up_are_left_alone_and_block_saving() {
    let database_directory = tempfile::tempdir().unwrap();
    let database = database_with_preferences(database_directory.path(), "{ not json");
    // 备份写在程序给的路径旁边，SQLite 的日志文件跟着链接写在真实文件旁边：
    // 链接所在目录只读时，库照常读写，备份写不进去。
    let link_directory = tempfile::tempdir().unwrap();
    let linked_database = link_directory.path().join("state.sqlite");
    std::os::unix::fs::symlink(&database, &linked_database).unwrap();
    fs::set_permissions(link_directory.path(), fs::Permissions::from_mode(0o555)).unwrap();

    let loaded = load_user_config_from_sources(default_app_config(), &linked_database, None);

    fs::set_permissions(link_directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let recovery = loaded.recovery.expect("recovery");
    assert!(
        matches!(
            recovery,
            SettingsRecovery::NotBackedUp {
                source: SettingsSource::Preferences,
                ..
            }
        ),
        "{recovery:?}"
    );
    assert!(matches!(
        SettingsPersistence::for_source(&[recovery], SettingsSource::Preferences),
        SettingsPersistence::Blocked(_)
    ));
    assert_eq!(stored_raw_payload(&database), "{ not json");
}

#[test]
fn damaged_settings_that_were_backed_up_but_not_rewritten_still_allow_saving() {
    let directory = tempfile::tempdir().unwrap();
    let database = database_with_preferences(directory.path(), "{ not json");
    make_read_only(&database);

    let loaded = load_user_config_from_sources(default_app_config(), &database, None);

    let (reset, backup) = restored(loaded.recovery.clone());
    assert_eq!(reset, ResetSettings::All);
    assert_eq!(fs::read_to_string(backup).unwrap(), "{ not json");
    assert_eq!(
        SettingsPersistence::for_source(
            &[loaded.recovery.expect("recovery")],
            SettingsSource::Preferences
        ),
        SettingsPersistence::Writable
    );
    assert_eq!(stored_raw_payload(&database), "{ not json");
}

#[test]
fn settings_that_could_not_be_migrated_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    TaskQueueStore::new(&database).expect("create store");
    make_read_only(&database);

    let loaded = load_user_config_from_sources(default_app_config(), &database, None);

    assert!(matches!(
        loaded.recovery,
        Some(SettingsRecovery::MigrationNotSaved { .. })
    ));
}

#[test]
fn a_restored_recovery_does_not_block_saving() {
    let recovery = SettingsRecovery::Restored {
        source: SettingsSource::Preferences,
        reset: ResetSettings::All,
        backup: PathBuf::from("/data/user-preferences-1.json"),
    };

    assert_eq!(
        SettingsPersistence::for_source(&[recovery], SettingsSource::Preferences),
        SettingsPersistence::Writable
    );
}

#[test]
fn early_readers_use_readable_settings_and_leave_damage_for_startup() {
    let directory = tempfile::tempdir().unwrap();
    let payload = default_payload_with(json!({
        "theme_mode": "dark",
        "launch_window_policy": file_operation_store::LAUNCH_WINDOW_POLICY_MERGE_INTO_EXISTING,
        "show_hidden_files": "yes",
    }));
    let database = database_with_preferences(directory.path(), &payload);

    let preferences = read_user_preferences_without_repair(&database).unwrap();

    assert_eq!(preferences.theme_mode, bennu_theme::ThemeMode::Dark);
    assert_eq!(
        preferences.launch_window_policy,
        LaunchWindowPolicy::MergeIntoExisting
    );
    assert_eq!(stored_raw_payload(&database), payload);
    let backups = fs::read_dir(directory.path())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("user-preferences-")
        })
        .count();
    assert_eq!(backups, 0);
}

#[test]
fn an_unreadable_legacy_config_is_not_migrated_and_blocks_saving() {
    let directory = tempfile::tempdir().unwrap();
    let config_dir = directory.path().join("config");
    // config.toml 是个目录：读不了。
    fs::create_dir_all(config_dir.join(CONFIG_FILE_NAME)).unwrap();
    let database = directory.path().join("state.sqlite");

    let loaded = load_user_config_from_sources(default_app_config(), &database, Some(&config_dir));

    assert!(matches!(
        loaded.recovery,
        Some(SettingsRecovery::Unavailable {
            source: SettingsSource::Preferences,
            ..
        })
    ));
    let stored = TaskQueueStore::new(&database)
        .unwrap()
        .read_user_preferences(&StoredUserPreferences::default())
        .unwrap();
    assert!(stored.is_none(), "next launch migrates again");
}

fn config_file_with(directory: &Path, content: &str) -> PathBuf {
    let path = directory.join(CONFIG_FILE_NAME);
    fs::write(&path, content).unwrap();
    path
}

fn config_backups(directory: &Path) -> Vec<PathBuf> {
    let mut backups = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("config.toml.") && name.ends_with(".bak")
            })
        })
        .collect::<Vec<_>>();
    backups.sort();
    backups
}

fn config_document(path: &Path) -> toml::Table {
    fs::read_to_string(path).unwrap().parse().unwrap()
}

fn app_config_restored(recovery: Option<SettingsRecovery>) -> (ResetSettings, PathBuf) {
    match recovery {
        Some(SettingsRecovery::Restored {
            source: SettingsSource::AppConfig,
            reset,
            backup,
        }) => (reset, backup),
        other => panic!("expected restored config.toml, got {other:?}"),
    }
}

fn probe_cache_record() -> RendererProbeCacheRecord {
    RendererProbeCacheRecord {
        version: 1,
        backend: "gl".to_owned(),
        rendering_gpu_preference: "gpu".to_owned(),
        wgpu_power_preference: Some("high".to_owned()),
        mesa_vulkan_device_select: None,
        vulkan_loader_driver_select: None,
        display_gpu_device_select: None,
    }
}

#[test]
fn an_unparseable_config_file_is_backed_up_and_reset() {
    let directory = tempfile::tempdir().unwrap();
    let original = "rendering_backend = \"gpu\"\n[[broken";
    let path = config_file_with(directory.path(), original);

    let loaded = app_config::load_and_repair_app_config_at(&path);

    let (reset, backup) = app_config_restored(loaded.recovery);
    assert_eq!(reset, ResetSettings::All);
    assert_eq!(config_backups(directory.path()), [backup.clone()]);
    assert_eq!(fs::read_to_string(backup).unwrap(), original);
    assert_eq!(loaded.config, default_app_config());
    let reloaded = app_config::load_and_repair_app_config_at(&path);
    assert_eq!(reloaded.recovery, None);
}

#[test]
fn an_unreadable_config_key_is_reset_and_the_rest_is_kept() {
    let directory = tempfile::tempdir().unwrap();
    let original = "thumbnail_cache_dir = \"/tmp/thumbs\"\n\
                    search_content_indexing_enabled = false\n\
                    rendering_backend = \"quantum\"\n\
                    search_max_extract_bytes = -5\n\
                    from_a_newer_version = 7\n";
    let path = config_file_with(directory.path(), original);

    let loaded = app_config::load_and_repair_app_config_at(&path);

    let (reset, backup) = app_config_restored(loaded.recovery);
    assert_eq!(
        reset,
        ResetSettings::Keys(vec![
            "rendering_backend".to_owned(),
            "search_max_extract_bytes".to_owned()
        ])
    );
    assert_eq!(fs::read_to_string(backup).unwrap(), original);
    assert!(!loaded.config.search_content_indexing_enabled);
    assert_eq!(
        loaded.config.rendering_gpu_preference,
        default_app_config().rendering_gpu_preference
    );
    let reloaded = app_config::load_and_repair_app_config_at(&path);
    assert_eq!(reloaded.recovery, None);
    assert_eq!(reloaded.config, loaded.config);
    assert_eq!(
        config_document(&path)["from_a_newer_version"].as_integer(),
        Some(7)
    );
    // 已删掉的旧键当作不认识的键原样保留。
    assert_eq!(
        config_document(&path)["thumbnail_cache_dir"].as_str(),
        Some("/tmp/thumbs")
    );
}

#[test]
fn a_missing_config_file_uses_defaults_without_a_notice() {
    let directory = tempfile::tempdir().unwrap();

    let loaded =
        app_config::load_and_repair_app_config_at(&directory.path().join(CONFIG_FILE_NAME));

    assert_eq!(loaded.recovery, None);
    assert_eq!(loaded.config, default_app_config());
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
}

#[test]
fn an_unreadable_config_file_uses_defaults_and_blocks_saving() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(CONFIG_FILE_NAME);
    // config.toml 是个目录：读不了。
    fs::create_dir(&path).unwrap();

    let loaded = app_config::load_and_repair_app_config_at(&path);

    let recovery = loaded.recovery.expect("recovery");
    assert!(matches!(
        recovery,
        SettingsRecovery::Unavailable {
            source: SettingsSource::AppConfig,
            ..
        }
    ));
    assert!(matches!(
        SettingsPersistence::for_source(&[recovery], SettingsSource::AppConfig),
        SettingsPersistence::Blocked(_)
    ));
}

#[test]
fn a_config_file_that_cannot_be_backed_up_is_left_alone_and_blocks_saving() {
    let directory = tempfile::tempdir().unwrap();
    let path = config_file_with(directory.path(), "[[broken");
    // 备份写在同一目录：目录只读时写不进去。
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o555)).unwrap();

    let loaded = app_config::load_and_repair_app_config_at(&path);

    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let recovery = loaded.recovery.expect("recovery");
    assert!(matches!(
        recovery,
        SettingsRecovery::NotBackedUp {
            source: SettingsSource::AppConfig,
            ..
        }
    ));
    assert!(matches!(
        SettingsPersistence::for_source(&[recovery], SettingsSource::AppConfig),
        SettingsPersistence::Blocked(_)
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "[[broken");
}

#[test]
fn saving_the_config_keeps_keys_this_version_does_not_know() {
    let directory = tempfile::tempdir().unwrap();
    let path = config_file_with(
        directory.path(),
        "from_a_newer_version = \"kept\"\n[some_tool]\nanswer = 42\n",
    );

    app_config::save_app_config_at(&path, &default_app_config()).unwrap();

    let document = config_document(&path);
    assert_eq!(document["from_a_newer_version"].as_str(), Some("kept"));
    assert_eq!(document["some_tool"]["answer"].as_integer(), Some(42));
    assert_eq!(app_config::load_app_config_at(&path), default_app_config());
}

#[test]
fn saving_over_an_unparseable_config_file_backs_it_up_first() {
    let directory = tempfile::tempdir().unwrap();
    let path = config_file_with(directory.path(), "[[broken");

    app_config::save_app_config_at(&path, &default_app_config()).unwrap();

    let backups = config_backups(directory.path());
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "[[broken");
    assert_eq!(app_config::load_app_config_at(&path), default_app_config());
}

#[test]
fn the_probe_cache_never_rewrites_an_unparseable_config_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = config_file_with(directory.path(), "[[broken");

    let saved = renderer_probe_cache::save_renderer_probe_cache_at(&path, &probe_cache_record());
    let cleared = renderer_probe_cache::clear_renderer_probe_cache_at(&path);

    assert!(saved.is_err());
    assert!(cleared.is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "[[broken");
    assert!(config_backups(directory.path()).is_empty());
}

#[test]
fn the_probe_cache_writes_only_its_own_section() {
    let directory = tempfile::tempdir().unwrap();
    let path = config_file_with(
        directory.path(),
        "rendering_backend = \"quantum\"\nfrom_a_newer_version = 7\n",
    );

    renderer_probe_cache::save_renderer_probe_cache_at(&path, &probe_cache_record()).unwrap();

    // 读不懂的用户项原样留给启动修复备份。
    let document = config_document(&path);
    assert_eq!(document["rendering_backend"].as_str(), Some("quantum"));
    assert_eq!(document["from_a_newer_version"].as_integer(), Some(7));
    assert_eq!(
        renderer_probe_cache::load_renderer_probe_cache_at(&path),
        Some(probe_cache_record())
    );

    renderer_probe_cache::clear_renderer_probe_cache_at(&path).unwrap();

    let document = config_document(&path);
    assert!(!document.contains_key("renderer_probe_cache"));
    assert_eq!(document["rendering_backend"].as_str(), Some("quantum"));
}
