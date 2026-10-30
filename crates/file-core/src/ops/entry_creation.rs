//! 按可用名创建条目：首选名被占用就按"名字 2"续编号。占用判定就是这一次原子
//! 创建（`create_dir`、`create_new`、`symlink`）返回的 `AlreadyExists`，不先查再建。

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::entry_naming::{entry_name_candidates, validate_entry_name, EntryNameKind};
use crate::FileError;

/// 在 `directory` 下的候选名上逐个调用 `create`，返回第一个创建成功的路径。
/// `name` 已经校验过；`creation_error` 把 `create` 的失败映射成对应的错误类型。
pub(crate) fn create_at_first_available_name(
    directory: &Path,
    name: &OsStr,
    kind: EntryNameKind,
    mut create: impl FnMut(&Path) -> io::Result<()>,
    creation_error: impl Fn(PathBuf, io::Error) -> FileError,
) -> Result<PathBuf, FileError> {
    for candidate in entry_name_candidates(name, kind) {
        let path = directory.join(candidate);
        match create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(creation_error(path, source)),
        }
    }
    Err(FileError::EntryNameCandidatesExhausted {
        directory: directory.to_path_buf(),
        base: name.to_os_string(),
    })
}

/// 首选路径拆成目录与名字，名字在这里（系统边界）校验一次。名字取最后一个路径
/// 成分而不是 `file_name()`：后者遇到 `..` 返回 None，会被报成"没有父目录"。
pub(crate) fn preferred_directory_and_name(preferred: &Path) -> Result<(&Path, &OsStr), FileError> {
    let last_component = preferred
        .components()
        .next_back()
        .map(|component| component.as_os_str());
    let (Some(directory), Some(name)) = (preferred.parent(), last_component) else {
        return Err(FileError::InvalidInput {
            path: preferred.to_path_buf(),
            message: "path has no parent directory".to_owned(),
        });
    };
    validate_entry_name(name).map_err(|reason| FileError::InvalidEntryName {
        name: name.to_os_string(),
        reason,
    })?;
    Ok((directory, name))
}

/// 在 `preferred` 处新建空目录，被占用则续编号；返回实际建成的路径。
pub async fn create_directory_at_available_name(preferred: PathBuf) -> Result<PathBuf, FileError> {
    run_blocking(move || {
        let (directory, name) = preferred_directory_and_name(&preferred)?;
        create_at_first_available_name(
            directory,
            name,
            EntryNameKind::Directory,
            |path| std::fs::create_dir(path),
            |path, source| FileError::CreateDirectory { path, source },
        )
    })
    .await
}

/// 在 `preferred` 处新建空文件，被占用则续编号；返回实际建成的路径。
pub async fn create_empty_file_at_available_name(preferred: PathBuf) -> Result<PathBuf, FileError> {
    create_file_with_contents_at_available_name(preferred, Vec::new()).await
}

/// 在 `preferred` 处新建文件并写入 `contents`，被占用则续编号；返回实际建成的路径。
pub async fn create_file_with_contents_at_available_name(
    preferred: PathBuf,
    contents: Vec<u8>,
) -> Result<PathBuf, FileError> {
    run_blocking(move || {
        let (directory, name) = preferred_directory_and_name(&preferred)?;
        create_at_first_available_name(
            directory,
            name,
            EntryNameKind::File,
            |path| {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                file.write_all(&contents)?;
                file.flush()
            },
            |path, source| FileError::CreateFile { path, source },
        )
    })
    .await
}

async fn run_blocking(
    operation: impl FnOnce() -> Result<PathBuf, FileError> + Send + 'static,
) -> Result<PathBuf, FileError> {
    match tokio::task::spawn_blocking(operation).await {
        Ok(result) => result,
        Err(join_error) if join_error.is_panic() => {
            std::panic::resume_unwind(join_error.into_panic())
        }
        // 阻塞任务只会在运行时关闭时被取消。
        Err(_) => Err(FileError::ApplicationStopping),
    }
}

#[cfg(test)]
#[path = "entry_creation_tests.rs"]
mod tests;
