//! 文件名被放进一段要交给另一层命令行的文字里时的检查。Bennu 动作的 `run`
//! （`sh -c 'ffmpeg -i {file}'`）和服务菜单的 `Exec`（`bash -c "mpv \"%f\""`）共用这一条规则：
//! 第一层从不把文件名当成代码，但那段文字到了下一层 shell 会再被解析一遍。

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// 路径会被交给另一层命令行，里面却有能被当成命令的字符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsafeName {
    pub(crate) path: PathBuf,
    pub(crate) character: char,
}

impl UnsafeName {
    pub(crate) fn reason(&self) -> String {
        let character = match self.character {
            '\n' => tr!("a line break"),
            character if character.is_ascii_control() => tr!("a control character"),
            character => format!("“{character}”"),
        };
        tr!(
            "%1 contains %2, and the command hands the name on to another command line, so it was not run",
            self.path.display(),
            character
        )
    }
}

/// 在下一层 shell 里能开始新命令、做命令替换、重定向或者闭合引号的字符。空格、括号、`'`
/// 这些常见文件名字符不在里面：没有这些字符配合，它们开不了新命令。
pub(crate) fn unsafe_character(value: &OsStr) -> Option<char> {
    value
        .as_bytes()
        .iter()
        .find(|byte| b"$`\"\\;&|<>".contains(byte) || byte.is_ascii_control())
        .map(|byte| char::from(*byte))
}
