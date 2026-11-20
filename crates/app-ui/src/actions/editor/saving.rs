//! 写动作文件：先在同一个文件夹里写临时文件、落盘，再一步换上去。中途失败原文件不变，新文件
//! 永远不覆盖已有的文件。会读写盘，只在后台线程调用。
//!
//! 检查“文件还是打开时那样”和换上去之间有一个很短的窗口，别的程序恰好这时写入会被换掉；
//! 文本编辑器也是这样，接受。

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use file_core::{entry_name_candidates, EntryNameKind};
use tempfile::NamedTempFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaveTarget {
    /// 新动作：在这个文件夹里按名字起文件名，重名时编号。
    New { folder: PathBuf, name: String },
    /// 已有的文件：现在的内容必须还是 `expected`。`None` = 这里必须没有文件
    /// （在别处被删了，用户选了“仍然保存”）。
    Existing {
        path: PathBuf,
        expected: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    Saved {
        path: PathBuf,
        text: String,
    },
    /// 文件在别处改过了（`found` 是现在的内容，被删了是 `None`），没有写。
    Changed {
        found: Option<String>,
    },
    Failed(String),
}

pub(crate) fn save(target: SaveTarget, text: String) -> SaveOutcome {
    let written = match &target {
        SaveTarget::New { folder, name } => write_new(folder, name, &text),
        SaveTarget::Existing { path, expected } => replace(path, expected.as_deref(), &text),
    };
    match written {
        Ok(Written::Saved(path)) => SaveOutcome::Saved { path, text },
        Ok(Written::Changed(found)) => SaveOutcome::Changed { found },
        Err(error) => SaveOutcome::Failed(error.to_string()),
    }
}

enum Written {
    Saved(PathBuf),
    Changed(Option<String>),
}

/// 动作名 → 文件名（不含 `.toml`）：`/` 和控制字符换成空格，去掉开头的点（不做隐藏文件），
/// 太长时截短（文件名最多 255 字节）。什么都不剩时叫“动作”。
pub(crate) fn file_stem_for(name: &str) -> String {
    const LONGEST_STEM: usize = 200;
    let cleaned = name
        .chars()
        .map(|character| {
            if character == '/' || character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let mut stem = cleaned.trim().trim_start_matches('.').trim().to_owned();
    if stem.len() > LONGEST_STEM {
        let mut end = LONGEST_STEM;
        while !stem.is_char_boundary(end) {
            end -= 1;
        }
        stem.truncate(end);
        stem.truncate(stem.trim_end().len());
    }
    if stem.is_empty() {
        tr!("Action")
    } else {
        stem
    }
}

/// 临时文件建在 `folder` 里，名字是 `.tmpXXXXXX`：没有 `.toml` 后缀，动作库的监视不会去读它。
/// 出错时 `NamedTempFile` 自己删掉它（只删自己的临时文件）。
fn temporary_with(folder: &Path, text: &str) -> io::Result<NamedTempFile> {
    let mut temporary = NamedTempFile::new_in(folder)?;
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    Ok(temporary)
}

fn write_new(folder: &Path, name: &str, text: &str) -> io::Result<Written> {
    // 临时文件默认 0600：动作能执行命令，只给自己看。
    let mut temporary = temporary_with(folder, text)?;
    let file_name = format!("{}.toml", file_stem_for(name));
    for candidate in entry_name_candidates(OsStr::new(&file_name), EntryNameKind::File) {
        let path = folder.join(candidate);
        match temporary.persist_noclobber(&path) {
            Ok(_) => return Ok(Written::Saved(path)),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                temporary = error.file;
            }
            Err(error) => return Err(error.error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        tr!("There is no free file name for %1", file_name),
    ))
}

/// 符号链接（dotfiles 管理工具常这么做）写到它指向的文件，链接留着；权限照原来的。
fn replace(path: &Path, expected: Option<&str>, text: &str) -> io::Result<Written> {
    let real = match std::fs::canonicalize(path) {
        Ok(real) => real,
        Err(error) if error.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error),
    };
    let found = match std::fs::read_to_string(&real) {
        Ok(found) => Some(found),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if found.as_deref() != expected {
        return Ok(Written::Changed(found));
    }
    let folder = real.parent().expect("an action file is inside a folder");
    let temporary = temporary_with(folder, text)?;
    if expected.is_none() {
        return match temporary.persist_noclobber(&real) {
            Ok(_) => Ok(Written::Saved(path.to_path_buf())),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                Ok(Written::Changed(std::fs::read_to_string(&real).ok()))
            }
            Err(error) => Err(error.error),
        };
    }
    temporary
        .as_file()
        .set_permissions(std::fs::metadata(&real)?.permissions())?;
    temporary.persist(&real).map_err(|error| error.error)?;
    Ok(Written::Saved(path.to_path_buf()))
}

#[cfg(test)]
#[path = "saving_tests.rs"]
mod tests;
