//! 收到的文件落到哪：fileName 可以带相对路径（官方发文件夹时是 `照片/2024/a.jpg`）。
//!
//! 这是信任边界：fileName 来自任意同网设备，直接拼接会写出接收目录之外。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 拆成路径段：`\` 当 `/`，去掉空段与 `.`；有 `..` 就整个拒收（合法的文件名里不会有）。
pub(super) fn file_name_segments(raw: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    for segment in raw.split(['/', '\\']) {
        let segment = segment.trim();
        match segment {
            "" | "." => continue,
            ".." => return None,
            segment if segment.contains('\0') => return None,
            segment => segments.push(segment.to_owned()),
        }
    }
    (!segments.is_empty()).then_some(segments)
}

/// 单个文件直接放接收目录（重名加 " (n)"）；带文件夹的放进这次会话给顶层文件夹定下
/// 的目录：接收目录里已有同名的就整个改名"照片 (1)"，不合并；同一次发送只定一次，
/// 所以 `folders` 归会话所有，调用方在会话锁内调用。
pub(super) async fn resolve_destination(
    download_dir: &Path,
    folders: &mut HashMap<String, PathBuf>,
    segments: &[String],
) -> std::io::Result<PathBuf> {
    let (file_name, directories) = segments
        .split_last()
        .expect("file_name_segments never returns an empty list");
    let Some((top, middle)) = directories.split_first() else {
        return unique_destination(download_dir, file_name).await;
    };
    let top_directory = match folders.get(top) {
        Some(directory) => directory.clone(),
        None => {
            let directory = unique_directory(download_dir, top).await?;
            folders.insert(top.clone(), directory.clone());
            directory
        }
    };
    let directory = middle
        .iter()
        .fold(top_directory, |directory, segment| directory.join(segment));
    unique_destination(&directory, file_name).await
}

/// 重名规则：`name.ext` 已存在则 `name (1).ext`、`name (2).ext` 依次探测（R8）。
/// create_new 保证并发会话写入同名文件时不会互相覆盖。
async fn unique_destination(dir: &Path, file_name: &str) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    let extension = path.extension().and_then(|s| s.to_str());
    for index in 0..10_000u32 {
        let candidate = match (index, extension) {
            (0, _) => dir.join(file_name),
            (n, Some(ext)) => dir.join(format!("{stem} ({n}).{ext}")),
            (n, None) => dir.join(format!("{stem} ({n})")),
        };
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(too_many_collisions())
}

/// 顶层文件夹占名：`create_dir` 原子，两次发送同名文件夹不会写进同一个目录。
async fn unique_directory(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    for index in 0..10_000u32 {
        let candidate = match index {
            0 => dir.join(name),
            n => dir.join(format!("{name} ({n})")),
        };
        match tokio::fs::create_dir(&candidate).await {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(too_many_collisions())
}

fn too_many_collisions() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many name collisions",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(raw: &str) -> Option<Vec<String>> {
        file_name_segments(raw)
    }

    #[test]
    fn names_keep_their_folders_but_never_leave_the_receive_directory() {
        assert_eq!(segments("a.txt"), Some(vec!["a.txt".to_owned()]));
        assert_eq!(
            segments("照片\\2024/./a.jpg"),
            Some(vec!["照片".into(), "2024".into(), "a.jpg".into()])
        );
        // 绝对路径的前导 `/` 只是空段：仍落在接收目录里。
        assert_eq!(
            segments("/etc/passwd"),
            Some(vec!["etc".into(), "passwd".into()])
        );
        assert_eq!(segments("../evil.txt"), None);
        assert_eq!(segments("a/../../evil.txt"), None);
        assert_eq!(segments(" / . /"), None);
    }

    #[tokio::test]
    async fn a_folder_that_already_exists_is_renamed_once_per_session() {
        let download = tempfile::tempdir().unwrap();
        std::fs::create_dir(download.path().join("照片")).unwrap();
        let mut folders = HashMap::new();
        let path = |raw: &str| file_name_segments(raw).unwrap();

        let first = resolve_destination(download.path(), &mut folders, &path("照片/a.jpg"))
            .await
            .unwrap();
        let second = resolve_destination(download.path(), &mut folders, &path("照片/2024/b.jpg"))
            .await
            .unwrap();
        // 另一次发送的同名文件夹：再改一次名，不和上一次合并。
        let other = resolve_destination(download.path(), &mut HashMap::new(), &path("照片/a.jpg"))
            .await
            .unwrap();

        let renamed = download.path().join("照片 (1)");
        assert_eq!(first, renamed.join("a.jpg"));
        assert_eq!(second, renamed.join("2024").join("b.jpg"));
        assert_eq!(other, download.path().join("照片 (2)").join("a.jpg"));
    }
}
