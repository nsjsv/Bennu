use std::process::{Command, Output};

fn run_without_gui(arguments: &[&str]) -> Output {
    let root = tempfile::tempdir().expect("create isolated command-line root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_app-ui"));
    command
        .args(arguments)
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/bennu-test-bus",
        )
        .env("FILE_MANAGER_STARTUP_TRACE", "1")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("NIRI_SOCKET")
        .env_remove("SWAYSOCK")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE");
    command.output().expect("run app-ui command-line action")
}

#[test]
fn command_line_help_and_version_exit_before_desktop_activation() {
    let help = run_without_gui(&["--help"]);
    let version = run_without_gui(&["--version"]);

    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).starts_with("Usage: bennu"));
    assert!(help.stderr.is_empty());
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("bennu "));
    assert!(version.stderr.is_empty());
}

#[test]
fn command_line_usage_errors_exit_before_desktop_activation() {
    let unknown = run_without_gui(&["--unknown"]);
    let activation_with_path = run_without_gui(&["--activation-service", "/"]);
    let renderer_probe_with_path = run_without_gui(&["--renderer-probe", "/"]);
    let combined_hidden_modes = run_without_gui(&["--renderer-probe", "--activation-service"]);

    assert_eq!(unknown.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("--unknown"));
    assert_eq!(activation_with_path.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&activation_with_path.stderr)
        .contains("--activation-service does not accept path arguments"));
    assert_eq!(renderer_probe_with_path.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&renderer_probe_with_path.stderr)
        .contains("--renderer-probe does not accept path arguments"));
    assert_eq!(combined_hidden_modes.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&combined_hidden_modes.stderr)
        .contains("--activation-service and --renderer-probe cannot be used together"));
}
