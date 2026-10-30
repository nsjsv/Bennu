use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use file_operation_store::{StoredUserPreferences, TaskQueueStore, USER_PREFERENCES_KEY};
use serde_json::{json, Value};

use super::app_config::default_app_config;
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
    // 比存储形态：配色存成不带透明度的 #RRGGBB，内存里的默认配色存不回原样。
    assert_eq!(
        loaded.config.user_preferences().to_stored(),
        default_user_config().user_preferences().to_stored()
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
