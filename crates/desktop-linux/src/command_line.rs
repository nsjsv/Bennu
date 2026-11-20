//! 按 shell 的引号规则把一行命令切成参数，用的是 GLib 的 `g_shell_parse_argv`（GIO 启动桌面文件的
//! `Exec` 也是这样切的），但不经过 shell 执行：切好的参数原样交给要启动的程序，所以没有管道、
//! 重定向、变量展开这些 shell 功能，文件名里的特殊字符也不会被当成命令。

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CommandLineError(String);

/// 空命令、引号没有配对时报错，原因用 GLib 的说法。
pub fn split_command_line(command_line: &str) -> Result<Vec<String>, CommandLineError> {
    let arguments = gio::glib::shell_parse_argv(command_line)
        .map_err(|error| CommandLineError(error.message().to_owned()))?;
    // 输入是 UTF-8，切出来的每一段都是它的一部分。
    Ok(arguments
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_group_and_operators_stay_plain_arguments() {
        assert_eq!(
            split_command_line(r#"ffmpeg -i "my video.mp4" 'out {stem}.wav' | wc"#).unwrap(),
            ["ffmpeg", "-i", "my video.mp4", "out {stem}.wav", "|", "wc"]
        );
        assert_eq!(
            split_command_line(r#"echo a\ b "c\"d" $HOME"#).unwrap(),
            ["echo", "a b", r#"c"d"#, "$HOME"]
        );
    }

    #[test]
    fn empty_and_unbalanced_commands_are_rejected() {
        assert!(split_command_line("   ").is_err());
        assert!(split_command_line(r#"echo "open"#).is_err());
    }
}
