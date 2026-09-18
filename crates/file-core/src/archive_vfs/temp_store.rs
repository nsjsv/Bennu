//! 嵌套归档的临时物化缓存。
//!
//! 包内嵌套压缩包无法被归档后端直接读取，必须先提取为真实文件。
//! 物化结果按「最外层归档身份 + 边界链」缓存于系统临时目录：
//! 临时目录由操作系统管理生命周期（tmpfs/重启清理），会话内命中
//! 复用；最外层归档变更（mtime/size）后键失效自然重新物化。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio_util::sync::CancellationToken;

use crate::archive_vfs::extract::extract_member;
use crate::{archive_extraction_format_for_path, FileError};

const CACHE_DIRECTORY_NAME: &str = "file-manager-archive-vfs";

/// 物化边界链最内层归档为可直接读取的真实文件路径。
/// 单层边界（普通压缩包）原样返回，不经过缓存。
pub(crate) async fn materialize_innermost_archive(
    boundaries: &[PathBuf],
    cancellation: CancellationToken,
) -> Result<PathBuf, FileError> {
    let outermost = boundaries
        .first()
        .ok_or(FileError::Unsupported("archive boundary chain is empty"))?;
    if boundaries.len() == 1 {
        return Ok(outermost.clone());
    }

    let cache_key = compute_cache_key(outermost, boundaries)?;
    let cache_path = cache_file_path(&cache_key, boundaries);
    if tokio::fs::try_exists(&cache_path)
        .await
        .unwrap_or(false)
    {
        return Ok(cache_path);
    }

    tokio::fs::create_dir_all(cache_root())
        .await
        .map_err(|source| FileError::CreateDirectory {
            path: cache_root(),
            source,
        })?;

    // 自外向内逐层物化：第 i 层归档是第 i-1 层归档内的成员文件。
    let mut current_archive = outermost.clone();
    for index in 1..boundaries.len() {
        if cancellation.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        let member = boundaries[index]
            .strip_prefix(&boundaries[index - 1])
            .expect("boundary chain entries share the outermost prefix")
            .to_string_lossy()
            .into_owned();
        let is_innermost = index + 1 == boundaries.len();
        let target = if is_innermost {
            cache_path.clone()
        } else {
            let intermediate_key = compute_cache_key(outermost, &boundaries[..=index])?;
            cache_file_path(&intermediate_key, &boundaries[..=index])
        };
        if tokio::fs::try_exists(&target).await.unwrap_or(false) {
            current_archive = target;
            continue;
        }
        extract_member(&current_archive, &member, &target, None).await?;
        current_archive = target;
    }

    Ok(current_archive)
}

fn cache_root() -> PathBuf {
    std::env::temp_dir().join(CACHE_DIRECTORY_NAME)
}

/// 缓存键 = 最外层归档路径 + mtime + size + 完整边界链。归档内容
/// 变更必然改变 mtime/size，旧缓存键随之失效。
fn compute_cache_key(outermost: &Path, boundaries: &[PathBuf]) -> Result<String, FileError> {
    let metadata = std::fs::metadata(outermost).map_err(|source| FileError::Metadata {
        path: outermost.to_path_buf(),
        source,
    })?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time: SystemTime| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    let mut hasher = blake3::Hasher::new();
    hasher.update(outermost.as_os_str().as_encoded_bytes());
    hasher.update(&modified.to_le_bytes());
    hasher.update(&metadata.len().to_le_bytes());
    for boundary in boundaries {
        hasher.update(boundary.as_os_str().as_encoded_bytes());
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 缓存文件保留原归档扩展名，保证后续按扩展名的格式分派依然成立。
fn cache_file_path(cache_key: &str, boundaries: &[PathBuf]) -> PathBuf {
    let innermost = boundaries
        .last()
        .expect("boundary chain is never empty")
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("archive.zip");
    let extension = archive_extraction_format_for_path(Path::new(innermost))
        .map(|_| innermost.rsplit('.').next().unwrap_or("zip"))
        .unwrap_or("zip");
    cache_root().join(format!("{cache_key}.{extension}"))
}

/// 打开/预览用临时文件路径：`cache_root/open/<key>/<成员文件名>`。
/// 键含成员路径与最外层归档身份，同名成员互不串扰。
pub(crate) fn opened_member_path(boundaries: &[PathBuf], member: &Path) -> PathBuf {
    let file_name = member
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("member.bin");
    let mut hasher = blake3::Hasher::new();
    for boundary in boundaries {
        hasher.update(boundary.as_os_str().as_encoded_bytes());
    }
    hasher.update(member.as_os_str().as_encoded_bytes());
    let key = hasher.finalize().to_hex();
    cache_root().join("open").join(format!("{key}.{file_name}"))
}
