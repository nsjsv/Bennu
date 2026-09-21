use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use thiserror::Error;
use tokio::process::Command;

use crate::desktop_entries::{desktop_entry_name, desktop_entry_path, read_desktop_entry};

const XDG_MIME: &str = "xdg-mime";
const GIO: &str = "gio";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWithApplication {
    pub desktop_id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWithApplicationList {
    pub path: PathBuf,
    pub mime_type: String,
    pub applications: Vec<OpenWithApplication>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenWithLaunchMode {
    OpenOnce,
    SetAsDefault,
}

#[derive(Debug, Error)]
pub enum OpenWithError {
    #[error("could not query MIME type for {path:?}: {source}")]
    MimeQuerySpawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("xdg-mime failed to query MIME type for {path:?} with status {status}")]
    MimeQueryFailed { path: PathBuf, status: ExitStatus },
    #[error("could not determine MIME type for {path:?}")]
    MimeTypeUnavailable { path: PathBuf },
    #[error("could not list applications for {path:?} ({mime_type}): {source}")]
    ApplicationQuerySpawn {
        path: PathBuf,
        mime_type: String,
        #[source]
        source: std::io::Error,
    },
    #[error("gio failed to list applications for {path:?} ({mime_type}) with status {status}")]
    ApplicationQueryFailed {
        path: PathBuf,
        mime_type: String,
        status: ExitStatus,
    },
    #[error("no applications found for {path:?} ({mime_type})")]
    NoApplications { path: PathBuf, mime_type: String },
    #[error("could not find desktop entry for {desktop_id} to open {path:?}")]
    DesktopEntryUnavailable { path: PathBuf, desktop_id: String },
    #[error("could not set {desktop_id} as default application for {mime_type}: {source}")]
    SetDefaultSpawn {
        mime_type: String,
        desktop_id: String,
        #[source]
        source: std::io::Error,
    },
    #[error("gio failed to set {desktop_id} as default application for {mime_type} with status {status}")]
    SetDefaultFailed {
        mime_type: String,
        desktop_id: String,
        status: ExitStatus,
    },
    #[error("could not open {path:?} with {desktop_id}: {source}")]
    LaunchSpawn {
        path: PathBuf,
        desktop_id: String,
        #[source]
        source: std::io::Error,
    },
    #[error("gio failed to open {path:?} with {desktop_id} with status {status}")]
    LaunchFailed {
        path: PathBuf,
        desktop_id: String,
        status: ExitStatus,
    },
}

pub async fn open_with_applications(
    path: impl AsRef<Path>,
) -> Result<OpenWithApplicationList, OpenWithError> {
    let path = path.as_ref().to_path_buf();
    let mime_type = query_file_mime_type(&path).await?;
    let parsed = query_gio_mime_applications(&path, &mime_type).await?;
    let mut applications = Vec::new();
    for desktop_id in parsed.desktop_ids {
        let Some(desktop_entry) = read_desktop_entry(&desktop_id).await else {
            continue;
        };
        applications.push(OpenWithApplication {
            name: desktop_entry_name(&desktop_entry.text)
                .unwrap_or_else(|| desktop_id.trim_end_matches(".desktop").to_owned()),
            is_default: parsed.default_desktop_id.as_deref() == Some(desktop_id.as_str()),
            desktop_id,
        });
    }

    if applications.is_empty() {
        return Err(OpenWithError::NoApplications { path, mime_type });
    }

    Ok(OpenWithApplicationList {
        path,
        mime_type,
        applications,
    })
}

pub async fn open_path_with_application(
    path: impl AsRef<Path>,
    desktop_id: impl Into<String>,
    launch_mode: OpenWithLaunchMode,
) -> Result<(), OpenWithError> {
    let path = path.as_ref().to_path_buf();
    let desktop_id = desktop_id.into();
    if launch_mode == OpenWithLaunchMode::SetAsDefault {
        let mime_type = query_file_mime_type(&path).await?;
        set_default_application(&mime_type, &desktop_id).await?;
    }
    launch_application(&path, &desktop_id).await
}

async fn query_file_mime_type(path: &Path) -> Result<String, OpenWithError> {
    let output = Command::new(XDG_MIME)
        .arg("query")
        .arg("filetype")
        .arg(path.as_os_str())
        .output()
        .await
        .map_err(|source| OpenWithError::MimeQuerySpawn {
            path: path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(OpenWithError::MimeQueryFailed {
            path: path.to_path_buf(),
            status: output.status,
        });
    }

    let mime_type = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if mime_type.is_empty() {
        Err(OpenWithError::MimeTypeUnavailable {
            path: path.to_path_buf(),
        })
    } else {
        Ok(mime_type)
    }
}

async fn query_gio_mime_applications(
    path: &Path,
    mime_type: &str,
) -> Result<ParsedGioMimeApplications, OpenWithError> {
    let output = Command::new(GIO)
        // `gio mime` 输出是面向人的文本，固定 locale 后解析规则才可测试。
        .env("LC_ALL", "C")
        .arg("mime")
        .arg(mime_type)
        .output()
        .await
        .map_err(|source| OpenWithError::ApplicationQuerySpawn {
            path: path.to_path_buf(),
            mime_type: mime_type.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(OpenWithError::ApplicationQueryFailed {
            path: path.to_path_buf(),
            mime_type: mime_type.to_owned(),
            status: output.status,
        });
    }

    Ok(parse_gio_mime_applications(
        String::from_utf8_lossy(&output.stdout).as_ref(),
    ))
}

async fn set_default_application(mime_type: &str, desktop_id: &str) -> Result<(), OpenWithError> {
    let status = Command::new(GIO)
        .arg("mime")
        .arg(mime_type)
        .arg(desktop_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|source| OpenWithError::SetDefaultSpawn {
            mime_type: mime_type.to_owned(),
            desktop_id: desktop_id.to_owned(),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(OpenWithError::SetDefaultFailed {
            mime_type: mime_type.to_owned(),
            desktop_id: desktop_id.to_owned(),
            status,
        })
    }
}

async fn launch_application(path: &Path, desktop_id: &str) -> Result<(), OpenWithError> {
    let desktop_file_path = desktop_entry_path(desktop_id).await.ok_or_else(|| {
        OpenWithError::DesktopEntryUnavailable {
            path: path.to_path_buf(),
            desktop_id: desktop_id.to_owned(),
        }
    })?;
    let status = gio_launch_command(path, &desktop_file_path)
        .status()
        .await
        .map_err(|source| OpenWithError::LaunchSpawn {
            path: path.to_path_buf(),
            desktop_id: desktop_id.to_owned(),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(OpenWithError::LaunchFailed {
            path: path.to_path_buf(),
            desktop_id: desktop_id.to_owned(),
            status,
        })
    }
}

fn gio_launch_command(path: &Path, desktop_file_path: &Path) -> Command {
    let mut command = Command::new(GIO);
    command
        .arg("launch")
        .arg(desktop_file_path.as_os_str())
        .arg(path.as_os_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedGioMimeApplications {
    default_desktop_id: Option<String>,
    desktop_ids: Vec<String>,
}

fn parse_gio_mime_applications(output: &str) -> ParsedGioMimeApplications {
    let mut default_desktop_id = None;
    let mut listed_desktop_ids = Vec::new();
    let mut in_application_section = false;

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((prefix, value)) = line.split_once(':') {
            if prefix.starts_with("Default application for") {
                let value = value.trim();
                if !value.is_empty() {
                    default_desktop_id = Some(value.to_owned());
                }
                in_application_section = false;
                continue;
            }
        }
        if line == "Registered applications:" || line == "Recommended applications:" {
            in_application_section = true;
            continue;
        }
        if line.ends_with(':') {
            in_application_section = false;
            continue;
        }
        if in_application_section {
            listed_desktop_ids.push(line.to_owned());
        }
    }

    let mut seen = HashSet::new();
    let mut desktop_ids = Vec::new();
    if let Some(default_desktop_id) = &default_desktop_id {
        if seen.insert(default_desktop_id.clone()) {
            desktop_ids.push(default_desktop_id.clone());
        }
    }
    for desktop_id in listed_desktop_ids {
        if seen.insert(desktop_id.clone()) {
            desktop_ids.push(desktop_id);
        }
    }

    ParsedGioMimeApplications {
        default_desktop_id,
        desktop_ids,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use super::*;

    #[test]
    fn gio_mime_parser_orders_default_then_unique_applications() {
        let output = r#"
Default application for "text/plain": vim.desktop
Registered applications:
	vim.desktop
	nvim.desktop
	dev.zed.Zed.desktop
Recommended applications:
	nvim.desktop
	emacs.desktop
"#;

        let parsed = parse_gio_mime_applications(output);

        assert_eq!(parsed.default_desktop_id.as_deref(), Some("vim.desktop"));
        assert_eq!(
            parsed.desktop_ids,
            [
                "vim.desktop",
                "nvim.desktop",
                "dev.zed.Zed.desktop",
                "emacs.desktop",
            ]
        );
    }

    #[test]
    fn gio_mime_parser_accepts_no_default() {
        let output = r#"
No default applications for "application/x-example"
Registered applications:
	example.desktop
"#;

        let parsed = parse_gio_mime_applications(output);

        assert_eq!(parsed.default_desktop_id, None);
        assert_eq!(parsed.desktop_ids, ["example.desktop"]);
    }

    #[test]
    fn gio_launch_command_passes_path_without_shell() {
        let command = gio_launch_command(
            Path::new("/tmp/example file.txt"),
            Path::new("/usr/share/applications/editor.desktop"),
        );
        let command = command.as_std();
        let arguments = command.get_args().map(OsString::from).collect::<Vec<_>>();

        assert_eq!(command.get_program(), OsStr::new(GIO));
        assert_eq!(
            arguments,
            [
                OsString::from("launch"),
                OsString::from("/usr/share/applications/editor.desktop"),
                OsString::from("/tmp/example file.txt"),
            ]
        );
    }
}
