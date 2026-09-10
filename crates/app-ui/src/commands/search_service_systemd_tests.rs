use std::num::NonZeroU32;
use std::path::PathBuf;

use file_search::SearchRuntimeIdentity;

use super::{
    expected_user_fragment, SearchUnitSnapshot, UnitActiveState, PACKAGED_RELEASE_EXEC_START_PATH,
    PACKAGED_RELEASE_FRAGMENT_PATH,
};

#[test]
fn packaged_release_definition_has_no_override_guidance() {
    let description = release_snapshot(
        PACKAGED_RELEASE_FRAGMENT_PATH,
        "",
        PACKAGED_RELEASE_EXEC_START_PATH,
    )
    .description();

    assert!(description.contains("FragmentPath=/usr/lib/systemd/user/bennu-search.service"));
    assert!(description.contains("DropInPaths=<none>"));
    assert!(!description.contains("unit source guidance"));
    assert!(!description.contains("user unit definition"));
}

#[test]
fn release_user_fragment_identifies_the_actual_override_source() {
    let fragment_path = expected_user_fragment(SearchRuntimeIdentity::Release).unwrap();
    let fragment_text = fragment_path.to_string_lossy();
    let description =
        release_snapshot(&fragment_text, "", PACKAGED_RELEASE_EXEC_START_PATH).description();

    assert!(description.contains(fragment_text.as_ref()));
    assert!(description.contains("a user unit definition or drop-in overrides"));
    assert!(description.contains("systemctl --user daemon-reload"));
}

#[test]
fn packaged_fragment_with_drop_in_names_the_drop_in_conflict() {
    let drop_in_path =
        "/home/test/.config/systemd/user/bennu-search.service.d/override.conf";
    let description = release_snapshot(
        PACKAGED_RELEASE_FRAGMENT_PATH,
        drop_in_path,
        PACKAGED_RELEASE_EXEC_START_PATH,
    )
    .description();

    assert!(description.contains(drop_in_path));
    assert!(description.contains("a user unit definition or drop-in overrides"));
}

#[test]
fn unexpected_exec_start_recommends_reinstalling_the_bundle_not_removing_an_override() {
    let unexpected_exec = "/opt/old-bennu/bennu-searchd";
    let description =
        release_snapshot(PACKAGED_RELEASE_FRAGMENT_PATH, "", unexpected_exec).description();

    assert!(description.contains(unexpected_exec));
    assert!(description.contains("FragmentPath/ExecStartPath do not match the current bundle"));
    assert!(description.contains("reinstall the current Bennu package"));
    assert!(!description.contains("a user unit definition or drop-in overrides"));
}

fn release_snapshot(
    fragment_path: &str,
    drop_in_paths: &str,
    exec_start_path: &str,
) -> SearchUnitSnapshot {
    SearchUnitSnapshot {
        runtime_identity: SearchRuntimeIdentity::Release,
        active_state: UnitActiveState::Active,
        sub_state: "running".to_owned(),
        main_pid: NonZeroU32::new(42),
        control_group: Some(PathBuf::from("/user.slice/search.service")),
        memory_high: 512_000_000,
        memory_max: 640_000_000,
        memory_swap_max: 0,
        service_slice: "background.slice".to_owned(),
        cpu_quota_per_sec_usec: "infinity".to_owned(),
        service_result: "success".to_owned(),
        exec_main_status: 0,
        restart_count: 0,
        fragment_path: fragment_path.to_owned(),
        drop_in_paths: drop_in_paths.to_owned(),
        exec_start_path: exec_start_path.to_owned(),
    }
}
