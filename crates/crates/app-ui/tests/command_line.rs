use std::process::{Command, Output};

fn run_app(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_app-ui"))
        .args(arguments)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("run app-ui binary")
}

#[test]
fn help_exits_successfully_without_a_display() {
    let output = run_app(&["--help"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage: bennu"));
    assert!(output.stderr.is_empty());
}

#[test]
fn version_exits_successfully_without_a_display() {
    let output = run_app(&["--version"]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("version stdout is UTF-8"),
        format!("bennu {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn unknown_option_returns_usage_exit_code_before_gui_startup() {
    let output = run_app(&["--unknown"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--unknown"));
    assert!(stderr.contains("bennu --help"));
}

#[test]
fn missing_path_returns_usage_exit_code_before_gui_startup() {
    let root = tempfile::tempdir().expect("create temp directory");
    let missing = root.path().join("missing");
    let output = Command::new(env!("CARGO_BIN_EXE_app-ui"))
        .arg(&missing)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("run app-ui binary");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot open"));
    assert!(stderr.contains("missing"));
}
