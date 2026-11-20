//! 命令模式（`s:`）：交给用户的 shell 执行。命令是用户自己打的字，原样交给 `-c`；选中的项作为
//! 位置参数单独传进去，从不拼进命令里，名字里有什么都只是参数。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use file_operations::CommandInvocation;

/// POSIX 的 shell（bash、zsh、dash……）里 `-c` 后面第一个参数是 `$0`，选中项从 `$1` 起，`"$@"` 就是
/// 它们；fish 没有这个位置，参数直接进 `$argv`。
pub(crate) fn shell_invocation(
    shell: &Path,
    command: &str,
    items: &[PathBuf],
    folder: &Path,
) -> CommandInvocation {
    let mut arguments = vec![OsString::from("-c"), OsString::from(command)];
    if shell.file_name().is_none_or(|name| name != "fish") {
        arguments.push(OsString::from("bennu"));
    }
    arguments.extend(items.iter().map(|item| item.as_os_str().to_owned()));
    CommandInvocation {
        program: shell.as_os_str().to_owned(),
        arguments,
        working_directory: folder.to_path_buf(),
    }
}
