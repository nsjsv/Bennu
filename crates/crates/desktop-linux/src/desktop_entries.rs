use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

const DEFAULT_XDG_DATA_DIRS: &str = "/usr/local/share:/usr/share";

pub(crate) struct DesktopEntry {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
}

pub(crate) async fn read_desktop_entry(desktop_id: &str) -> Option<DesktopEntry> {
    for path in desktop_entry_search_paths(desktop_id) {
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            return Some(DesktopEntry { path, text });
        }
    }
    None
}

pub(crate) async fn read_desktop_entry_text(desktop_id: &str) -> Option<String> {
    read_desktop_entry(desktop_id)
        .await
        .map(|desktop_entry| desktop_entry.text)
}

pub(crate) async fn desktop_entry_path(desktop_id: &str) -> Option<PathBuf> {
    read_desktop_entry(desktop_id)
        .await
        .map(|desktop_entry| desktop_entry.path)
}

pub(crate) fn desktop_entry_name(desktop_entry: &str) -> Option<String> {
    read_desktop_entry_key(desktop_entry, "Name")
}

pub(crate) fn desktop_entry_requires_terminal(desktop_entry: &str) -> bool {
    read_desktop_entry_key(desktop_entry, "Terminal")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn desktop_entry_search_paths(desktop_id: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(data_home) = xdg_data_home() {
        paths.push(data_home.join("applications").join(desktop_id));
    }

    let data_dirs = env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from(DEFAULT_XDG_DATA_DIRS));
    paths.extend(
        env::split_paths(&data_dirs).map(|data_dir| data_dir.join("applications").join(desktop_id)),
    );

    paths
}

fn xdg_data_home() -> Option<PathBuf> {
    env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

fn read_desktop_entry_key(desktop_entry: &str, requested_key: &str) -> Option<String> {
    let mut in_desktop_entry_group = false;
    for raw_line in desktop_entry.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == requested_key {
            return Some(value.trim().to_owned());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_requires_terminal_reads_desktop_entry_group() {
        let desktop_entry = r#"
[Desktop Entry]
Name=Vim
Terminal=true
Exec=vim %F

[Desktop Action New]
Terminal=false
"#;

        assert!(desktop_entry_requires_terminal(desktop_entry));
    }

    #[test]
    fn desktop_entry_requires_terminal_ignores_other_groups() {
        let desktop_entry = r#"
[Desktop Action New]
Terminal=true

[Desktop Entry]
Name=Graphical Editor
Terminal=false
Exec=editor %F
"#;

        assert!(!desktop_entry_requires_terminal(desktop_entry));
    }

    #[test]
    fn desktop_entry_name_reads_only_desktop_entry_group() {
        let desktop_entry = r#"
[Desktop Action New]
Name=New Window

[Desktop Entry]
Name=Editor
Exec=editor %F
"#;

        assert_eq!(desktop_entry_name(desktop_entry).as_deref(), Some("Editor"));
    }
}
