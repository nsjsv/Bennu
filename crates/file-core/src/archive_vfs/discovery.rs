//! 归档虚拟目录发现：把「列出包内目录子项」合成为与真实目录同一
//! 形状的 `DirectoryDiscovery`。
//!
//! 元数据来自归档头并在条目构造处预填 Complete，后续 demand 自然
//! 短路；排序复用真实目录同一套 `sort_discovered_entry_indices`，
//! 隐藏过滤复用 `is_hidden_name` 权威。包内无外部变更源，不产生
//! discovery hint 批次（合同允许丢弃任意批次）。

use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::directory_metadata::{DirectoryMetadataResolver, DiscoveredDirectoryEntry};
use crate::{
    archive_extraction_format_for_path, archive_listing::list_archive_members_with_format,
    archive_vfs::temp_store, is_hidden_name, sort_discovered_entry_indices,
    ArchiveExtractionFormat, DirectoryDiscovery, FileError, ScanOptions,
};

use super::tree::ArchiveMemberTree;
use super::ResolvedArchivePath;

/// 列出虚拟目录 `virtual_directory` 的子项并组装权威发现结果。
pub(crate) async fn discover_archive_directory(
    virtual_directory: &Path,
    resolved: &ResolvedArchivePath,
    options: ScanOptions,
    cancellation: CancellationToken,
) -> Result<DirectoryDiscovery, FileError> {
    if cancellation.is_cancelled() {
        return Err(FileError::Cancelled);
    }

    let innermost_archive =
        temp_store::materialize_innermost_archive(&resolved.boundaries, cancellation.clone())
            .await?;
    let format = archive_format_for_materialized(&innermost_archive, resolved)?;
    let members = list_archive_members_with_format(innermost_archive, format).await?;
    let tree = ArchiveMemberTree::build(members);
    let children = tree.children_of(&resolved.inner).ok_or_else(|| {
        // 与真实目录语义对齐：目标不是目录（或不存在）按
        // ReadDirectory/NotFound 上报，让 UI 归类为「目录不可用」。
        FileError::ReadDirectory {
            path: virtual_directory.to_path_buf(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        }
    })?;

    let mut entries = Vec::with_capacity(children.len());
    for (name, info) in children {
        let is_hidden = is_hidden_name(&name);
        if is_hidden && !options.include_hidden {
            continue;
        }
        entries.push(DiscoveredDirectoryEntry::with_archive_member_metadata(
            virtual_directory.join(&name),
            name,
            info.kind,
            is_hidden,
            info.len,
            info.modified,
        ));
    }

    let order = Arc::new(sort_discovered_entry_indices(&entries, &options));
    let entries = Arc::new(entries);
    let metadata_resolver = DirectoryMetadataResolver::new(
        virtual_directory.to_path_buf(),
        Arc::clone(&entries),
    );

    Ok(DirectoryDiscovery {
        path: virtual_directory.to_path_buf(),
        entries,
        order,
        metadata_resolver,
        warnings: Vec::new(),
    })
}

/// 物化后的归档文件沿用原扩展名；单层边界直接按原路径判定格式。
/// 扩展名与内容不符时由列成员环节报错，与既有预览管道一致。
fn archive_format_for_materialized(
    materialized: &Path,
    resolved: &ResolvedArchivePath,
) -> Result<ArchiveExtractionFormat, FileError> {
    archive_extraction_format_for_path(materialized)
        .or_else(|| {
            resolved
                .boundaries
                .first()
                .and_then(archive_extraction_format_for_path)
        })
        .ok_or(FileError::Unsupported(
            "archive format is not supported for directory discovery",
        ))
}
