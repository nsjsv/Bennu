use std::ffi::OsString;
use std::process::ExitStatus;

use thiserror::Error;
use tokio::process::Command;

const NOTIFY_SEND_COMMAND: &str = "notify-send";
const NOTIFICATION_APP_NAME: &str = "Bennu";
const NOTIFICATION_ICON_NAME: &str = "system-file-manager";

#[derive(Debug, Error)]
pub enum DesktopNotificationError {
    #[error("could not start notify-send: {source}")]
    Spawn {
        #[source]
        source: std::io::Error,
    },
    #[error("notify-send failed with status {status}")]
    Failed { status: ExitStatus },
}

pub async fn publish_desktop_notification(
    summary: &str,
    body: &str,
) -> Result<(), DesktopNotificationError> {
    let status = desktop_notification_command(summary, body)
        .status()
        .await
        .map_err(|source| DesktopNotificationError::Spawn { source })?;
    if status.success() {
        Ok(())
    } else {
        Err(DesktopNotificationError::Failed { status })
    }
}

fn desktop_notification_command(summary: &str, body: &str) -> Command {
    let mut command = Command::new(NOTIFY_SEND_COMMAND);
    command.args(desktop_notification_arguments(summary, body));
    command
}

fn desktop_notification_arguments(summary: &str, body: &str) -> Vec<OsString> {
    vec![
        OsString::from("--app-name"),
        OsString::from(NOTIFICATION_APP_NAME),
        OsString::from("--icon"),
        OsString::from(NOTIFICATION_ICON_NAME),
        OsString::from("--urgency"),
        OsString::from("normal"),
        OsString::from("--"),
        OsString::from(escape_xdg_markup(summary)),
        OsString::from(escape_xdg_markup(body)),
    ]
}

fn escape_xdg_markup(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    fn argument_text(arguments: Vec<OsString>) -> Vec<String> {
        arguments
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn desktop_notification_arguments_end_options_before_user_text() {
        let arguments = argument_text(desktop_notification_arguments(
            "-Copy <completed>",
            "name & notes",
        ));

        assert_eq!(
            arguments,
            vec![
                "--app-name",
                "Bennu",
                "--icon",
                "system-file-manager",
                "--urgency",
                "normal",
                "--",
                "-Copy &lt;completed&gt;",
                "name &amp; notes",
            ]
        );
    }

    #[test]
    fn desktop_notification_markup_escape_preserves_unicode() {
        assert_eq!(
            escape_xdg_markup("项目 <报告> & 说明"),
            "项目 &lt;报告&gt; &amp; 说明"
        );
    }

    #[test]
    fn desktop_notification_error_keeps_spawn_source() {
        let error = DesktopNotificationError::Spawn {
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "notify-send missing"),
        };

        assert!(matches!(
            error,
            DesktopNotificationError::Spawn { source }
                if source.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn desktop_notification_error_keeps_failed_status() {
        let status = ExitStatus::from_raw(1 << 8);
        let error = DesktopNotificationError::Failed { status };

        assert!(
            matches!(error, DesktopNotificationError::Failed { status } if status.code() == Some(1))
        );
    }
}
