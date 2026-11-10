//! 发送前准备：把要发的条目展开成文件（文件夹里的带 `文件夹名/相对路径`，同官方），
//! 逐个算 sha256（prepare-upload 里就要带上，所以得先算完）。
//!
//! 只记元数据，不把内容读进内存；几台设备共用一份（`Arc`），要 PIN 重发也不用重算。

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 准备好的一批文件。读不了的不会让整批失败，记在 `unreadable` 里随结果报给用户。
#[derive(Debug)]
pub struct PreparedFiles {
    pub(crate) files: Vec<PreparedFile>,
    pub(crate) unreadable: Vec<FileFailure>,
}

impl PreparedFiles {
    /// 准备本身出了错（后台任务崩了）：整批算读不了，带上原因。
    pub fn unreadable(reason: String) -> Self {
        Self {
            files: Vec::new(),
            unreadable: vec![FileFailure {
                file_name: String::new(),
                reason,
            }],
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreparedFile {
    /// 发给对方的名字：单个文件是文件名，文件夹里的是 `文件夹名/相对路径`。
    pub(crate) file_name: String,
    pub(crate) path: PathBuf,
    /// 算校验时实际读到的字节数（与声明的 sha256 对应）。
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

/// 一个没发出去的文件：名字同发给对方的名字，原因是英文（日志与界面都显示它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFailure {
    pub file_name: String,
    pub reason: String,
}

/// 校验进度（按字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
}

const HASH_BUFFER: usize = 1024 * 1024;

/// 展开并算校验；进度通道满时丢进度，不拖慢计算。
/// 取消了（每个条目、每读 1 MiB 查一次）返回 None。
pub async fn prepare_files(
    items: Vec<PathBuf>,
    progress: mpsc::Sender<PrepareProgress>,
    cancel: CancellationToken,
) -> Option<PreparedFiles> {
    tokio::task::spawn_blocking(move || prepare_blocking(items, &progress, &cancel))
        .await
        .unwrap_or_else(|error| {
            Some(PreparedFiles::unreadable(format!(
                "preparing the files failed: {error}"
            )))
        })
}

/// 要发的一个文件：名字、路径、枚举时的大小（只用来算总进度）。
struct FoundFile {
    file_name: String,
    path: PathBuf,
    size: u64,
}

fn prepare_blocking(
    items: Vec<PathBuf>,
    progress: &mpsc::Sender<PrepareProgress>,
    cancel: &CancellationToken,
) -> Option<PreparedFiles> {
    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    for item in items {
        if cancel.is_cancelled() {
            return None;
        }
        let Some(name) = item
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            unreadable.push(FileFailure {
                file_name: item.display().to_string(),
                reason: "not a file or folder".to_owned(),
            });
            continue;
        };
        // 用户选的条目本身跟着链接走：选的就是它指向的东西。
        match std::fs::metadata(&item) {
            Ok(metadata) if metadata.is_dir() => {
                collect_directory(&item, &name, &mut found, &mut unreadable);
            }
            Ok(metadata) if metadata.is_file() => found.push(FoundFile {
                file_name: name,
                path: item,
                size: metadata.len(),
            }),
            Ok(_) => unreadable.push(FileFailure {
                file_name: name,
                reason: "not a regular file".to_owned(),
            }),
            Err(error) => unreadable.push(FileFailure {
                file_name: name,
                reason: error.to_string(),
            }),
        }
    }

    let bytes_total = found.iter().map(|file| file.size).sum();
    let mut bytes_done = 0u64;
    let mut files = Vec::with_capacity(found.len());
    for file in found {
        let before = bytes_done;
        let hashed = hash_file(&file.path, |read| {
            bytes_done += read;
            let _ = progress.try_send(PrepareProgress {
                bytes_done: bytes_done.min(bytes_total),
                bytes_total,
            });
            !cancel.is_cancelled()
        });
        if cancel.is_cancelled() {
            return None;
        }
        // 文件在枚举后变了大小：进度按枚举时的大小走完，不倒退也不超出。
        bytes_done = before + file.size;
        match hashed {
            Ok((sha256, size)) => files.push(PreparedFile {
                file_name: file.file_name,
                path: file.path,
                size,
                sha256,
            }),
            Err(error) => unreadable.push(FileFailure {
                file_name: file.file_name,
                reason: error.to_string(),
            }),
        }
    }
    let _ = progress.try_send(PrepareProgress {
        bytes_done: bytes_total,
        bytes_total,
    });
    Some(PreparedFiles { files, unreadable })
}

/// 文件夹里按名字顺序深度优先。指向文件的链接发内容；指向文件夹的链接不进（免得
/// 绕圈或把整个家目录发出去）；坏链接、管道、设备文件跳过。读不了的子文件夹记失败。
fn collect_directory(
    directory: &Path,
    prefix: &str,
    found: &mut Vec<FoundFile>,
    unreadable: &mut Vec<FileFailure>,
) {
    let read_failure = |error: std::io::Error| FileFailure {
        file_name: prefix.to_owned(),
        reason: error.to_string(),
    };
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => return unreadable.push(read_failure(error)),
    };
    let mut entries = match entries.collect::<std::io::Result<Vec<_>>>() {
        Ok(entries) => entries,
        Err(error) => return unreadable.push(read_failure(error)),
    };
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_directory(&path, &name, found, unreadable);
            continue;
        }
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => found.push(FoundFile {
                file_name: name,
                path,
                size: metadata.len(),
            }),
            Ok(_) => {}
            Err(_) if file_type.is_symlink() => {}
            Err(error) => unreadable.push(FileFailure {
                file_name: name,
                reason: error.to_string(),
            }),
        }
    }
}

/// 流式算 sha256（小写 hex，同官方），返回 (校验和, 读到的字节数)。
/// `on_read` 返回 false = 不算了（取消），返回 Interrupted。
fn hash_file(path: &Path, mut on_read: impl FnMut(u64) -> bool) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
        if !on_read(read as u64) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    async fn prepare(items: Vec<PathBuf>) -> PreparedFiles {
        let (progress, _) = mpsc::channel(64);
        prepare_files(items, progress, CancellationToken::new())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_canceled_preparation_gives_nothing() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let prepared = prepare_files(vec![file], mpsc::channel(4).0, cancel).await;

        assert!(prepared.is_none());
    }

    fn names(prepared: &PreparedFiles) -> Vec<&str> {
        prepared
            .files
            .iter()
            .map(|file| file.file_name.as_str())
            .collect()
    }

    #[tokio::test]
    async fn folders_become_files_named_by_their_path_inside_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("照片");
        std::fs::create_dir_all(folder.join("2024").join("空")).unwrap();
        std::fs::write(folder.join("b.txt"), b"b").unwrap();
        std::fs::write(folder.join("2024").join("a.txt"), b"a").unwrap();
        std::fs::write(folder.join(".hidden"), b"h").unwrap();
        let single = root.path().join("single.txt");
        std::fs::write(&single, b"hello").unwrap();

        let prepared = prepare(vec![folder, single]).await;

        assert_eq!(
            names(&prepared),
            [
                "照片/.hidden",
                "照片/2024/a.txt",
                "照片/b.txt",
                "single.txt"
            ]
        );
        let single = &prepared.files[3];
        assert_eq!(single.size, 5);
        assert_eq!(
            single.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(prepared.unreadable.is_empty());
    }

    #[tokio::test]
    async fn links_to_files_are_sent_while_links_to_folders_and_broken_links_are_not() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"s").unwrap();
        let folder = root.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(outside.join("target.txt"), b"t").unwrap();
        std::os::unix::fs::symlink(outside.join("target.txt"), folder.join("file-link")).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("folder-link")).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), folder.join("broken")).unwrap();

        let prepared = prepare(vec![folder]).await;

        assert_eq!(names(&prepared), ["folder/file-link"]);
        assert!(prepared.unreadable.is_empty());
    }

    #[tokio::test]
    async fn unreadable_entries_are_reported_and_the_rest_still_prepared() {
        if nix_is_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("folder");
        std::fs::create_dir_all(folder.join("locked")).unwrap();
        std::fs::write(folder.join("locked.txt"), b"x").unwrap();
        std::fs::write(folder.join("ok.txt"), b"ok").unwrap();
        std::fs::set_permissions(
            folder.join("locked.txt"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        std::fs::set_permissions(
            folder.join("locked"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let prepared = prepare(vec![folder.clone(), root.path().join("missing.txt")]).await;

        for path in [folder.join("locked.txt"), folder.join("locked")] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(names(&prepared), ["folder/ok.txt"]);
        let mut failed: Vec<_> = prepared
            .unreadable
            .iter()
            .map(|failure| failure.file_name.as_str())
            .collect();
        failed.sort_unstable();
        assert_eq!(
            failed,
            ["folder/locked", "folder/locked.txt", "missing.txt"]
        );
    }

    /// root 读得了 0 权限的文件，"读不了"的场景造不出来。
    fn nix_is_root() -> bool {
        std::fs::metadata("/proc/self")
            .map(|metadata| std::os::unix::fs::MetadataExt::uid(&metadata) == 0)
            .unwrap_or(false)
    }
}
