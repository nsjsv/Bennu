//! 归档虚拟路径解析：把「归档文件 + 包内路径」编码为普通 `PathBuf`
//! （如 `/home/u/docs.zip/photos/1.png`）的唯一权威解析层。
//!
//! 无歧义依据：真实文件系统中归档文件是文件而非目录，其子路径不可能
//! 真实存在，因此包含归档段的路径必然是虚拟浏览路径。所有需要区分
//! 「真实路径 / 归档浏览路径」的边界（目录发现、元数据、监听、打开、
//! 门控）都必须经由本模块的判定函数，禁止在调用方自行切路径。

use std::path::{Path, PathBuf};

use crate::archive_extraction::archive_extraction_format_for_path;

pub(crate) mod discovery;
pub(crate) mod extract;
pub(crate) mod temp_store;
mod tree;

pub(crate) use discovery::discover_archive_directory;
pub use extract::{
    extract_archive_members_with_controls_and_progress, ArchiveMemberExtractionRequest,
};

/// 智能解压判定：包内所有成员是否都在同一个根目录下。
/// 单根返回根目录名；散文件/多顶层条目/空包返回 `None`。
pub async fn single_root_member_name(archive: &Path) -> Result<Option<String>, crate::FileError> {
    let format = archive_extraction_format_for_path(archive).ok_or(
        crate::FileError::Unsupported("archive format is not supported for listing"),
    )?;
    let members =
        crate::archive_listing::list_archive_members_with_format(archive.to_path_buf(), format)
            .await?;
    let tree = tree::ArchiveMemberTree::build(members);
    Ok(tree
        .single_root_name()
        .map(|name| name.to_string_lossy().into_owned()))
}

/// 把包内成员（或文件夹子树）物化为可直接打开/预览的临时文件，
/// 返回真实文件路径。加密 zip 未提供密码时返回 `ArchivePasswordRequired`。
pub async fn materialize_archive_member_for_open(
    path: &Path,
    password: Option<crate::ArchivePassword>,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<PathBuf, crate::FileError> {
    let resolved = resolve_archive_virtual_path(path).ok_or(crate::FileError::InvalidInput {
        path: path.to_path_buf(),
        message: "path is not inside an archive".to_owned(),
    })?;
    extract::materialize_member_for_open(&resolved, password.as_ref(), cancellation).await
}

/// 路径的归档身份。全系统的门控与分流都消费这一个枚举，
/// 避免各入口对同一路径重复解释。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivePathIdentity {
    /// 与归档无关的真实路径。
    RealFile,
    /// 路径恰是归档文件本身，作为包根目录打开的浏览入口。
    /// 该路径同时仍是真实文件；写操作由真实文件系统自然拒绝。
    BrowsingRoot,
    /// 路径位于归档内部（含嵌套归档内部），只读浏览语义。
    InsideArchive,
}

/// 解析成功的虚拟路径：归档边界链 + 最内层归档内的成员相对路径。
///
/// `boundaries` 首段必须是真实存在的归档文件；后续段是包内嵌套归档
/// 成员（仅按扩展名切分，存在性由目录发现阶段的成员树查证）。
/// `inner` 为空表示包根本身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedArchivePath {
    pub(crate) boundaries: Vec<PathBuf>,
    pub(crate) inner: PathBuf,
}

/// 解析虚拟归档路径；非虚拟路径返回 `None`。
///
/// 首个「真实存在且为文件、扩展名命中受支持归档」的前缀即归档边界；
/// 其后的段按扩展名继续切出嵌套边界，剩余段为最内层成员路径。
/// 本函数只做逐前缀 stat 与字符串切分，不做包内成员查证。
pub(crate) fn resolve_archive_virtual_path(path: &Path) -> Option<ResolvedArchivePath> {
    let boundary = first_archive_file_prefix(path)?;
    let remainder = path
        .strip_prefix(&boundary)
        .expect("boundary is a prefix of the given path");

    let mut boundaries = vec![boundary.clone()];
    let mut nested_virtual = PathBuf::new();
    let mut inner = PathBuf::new();
    for segment in remainder.iter() {
        nested_virtual.push(segment);
        if segment_is_archive_boundary(segment) {
            // 嵌套边界记录为「最外层归档 + 包内成员路径」的虚拟路径，
            // 提取流程按此从外层包内定位内层归档成员。
            boundaries.push(boundary.join(&nested_virtual));
            nested_virtual.clear();
            inner.clear();
        } else {
            inner.push(segment);
        }
    }

    Some(ResolvedArchivePath { boundaries, inner })
}

/// 路径的归档身份判定。详见 [`ArchivePathIdentity`]。
pub fn archive_path_identity(path: &Path) -> ArchivePathIdentity {
    match resolve_archive_virtual_path(path) {
        None => ArchivePathIdentity::RealFile,
        Some(resolved) => {
            if resolved.inner.as_os_str().is_empty()
                && resolved.boundaries.len() == 1
                && resolved.boundaries[0] == path
            {
                ArchivePathIdentity::BrowsingRoot
            } else {
                ArchivePathIdentity::InsideArchive
            }
        }
    }
}

/// 会话恢复用：把包内/包根路径归一到最外层归档所在的真实目录。
/// 非虚拟路径返回 `None`（无需归一）。
pub fn real_directory_outside_archive(path: &Path) -> Option<PathBuf> {
    let resolved = resolve_archive_virtual_path(path)?;
    resolved.boundaries.first()?.parent().map(Path::to_path_buf)
}

/// 在路径前缀链中找到第一个「真实存在且为文件、扩展名命中归档」的前缀。
fn first_archive_file_prefix(path: &Path) -> Option<PathBuf> {
    let mut prefix = PathBuf::new();
    for segment in path.iter() {
        prefix.push(segment);
        let Ok(metadata) = std::fs::symlink_metadata(&prefix) else {
            // 前缀链断裂：后续更深的段不可能再指向真实文件。
            return None;
        };
        if metadata.is_dir() {
            continue;
        }
        if archive_extraction_format_for_path(&prefix).is_some() {
            return Some(prefix);
        }
    }
    None
}

/// 单个路径段是否按嵌套归档边界切分。只看扩展名；该段是否真为
/// 包内归档文件由成员树在目录发现阶段查证。
fn segment_is_archive_boundary(segment: &std::ffi::OsStr) -> bool {
    archive_extraction_format_for_path(Path::new(segment)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileError, FileKind};
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn write_sample_zip(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.add_directory("photos", SimpleFileOptions::default())
            .unwrap();
        zip.start_file("photos/1.txt", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn real_directory_and_missing_paths_are_not_virtual() {
        let workspace = tempfile::tempdir().unwrap();
        let plain = workspace.path().join("plain");
        std::fs::create_dir(&plain).unwrap();

        assert_eq!(archive_path_identity(&plain), ArchivePathIdentity::RealFile);
        assert_eq!(
            archive_path_identity(&workspace.path().join("missing.zip/inner")),
            ArchivePathIdentity::RealFile
        );
    }

    #[test]
    fn directory_named_like_archive_stays_real() {
        // 归档边界必须是真实文件：恰好叫 x.zip 的目录仍是真实路径。
        let workspace = tempfile::tempdir().unwrap();
        let fake_archive = workspace.path().join("fake.zip");
        std::fs::create_dir(&fake_archive).unwrap();

        assert_eq!(
            archive_path_identity(&fake_archive),
            ArchivePathIdentity::RealFile
        );
        assert_eq!(
            archive_path_identity(&fake_archive.join("inner")),
            ArchivePathIdentity::RealFile
        );
    }

    #[test]
    fn archive_file_is_browsing_root() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("docs.zip");
        write_sample_zip(&archive);

        assert_eq!(
            archive_path_identity(&archive),
            ArchivePathIdentity::BrowsingRoot
        );
    }

    #[test]
    fn paths_below_archive_are_inside_archive() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("docs.zip");
        write_sample_zip(&archive);

        assert_eq!(
            archive_path_identity(&archive.join("photos")),
            ArchivePathIdentity::InsideArchive
        );
        assert_eq!(
            archive_path_identity(&archive.join("photos/1.txt")),
            ArchivePathIdentity::InsideArchive
        );
    }

    #[test]
    fn nested_archive_segments_split_boundaries() {
        let workspace = tempfile::tempdir().unwrap();
        let outer = workspace.path().join("a.zip");
        write_sample_zip(&outer);

        let resolved = resolve_archive_virtual_path(&outer.join("inner/b.zip/x/c.txt"))
            .expect("nested archive path must resolve");
        assert_eq!(
            resolved.boundaries,
            vec![outer.clone(), outer.join("inner/b.zip")]
        );
        assert_eq!(resolved.inner, PathBuf::from("x/c.txt"));
        assert_eq!(
            archive_path_identity(&outer.join("inner/b.zip/x/c.txt")),
            ArchivePathIdentity::InsideArchive
        );
    }

    #[test]
    fn tgz_and_tar_gz_suffixes_are_boundaries() {
        let workspace = tempfile::tempdir().unwrap();
        for name in ["bundle.tar.gz", "bundle.tgz", "bundle.7z", "bundle.rar"] {
            let archive = workspace.path().join(name);
            std::fs::write(&archive, b"payload").unwrap();
            assert_eq!(
                archive_path_identity(&archive),
                ArchivePathIdentity::BrowsingRoot,
                "{name} must be a browsing root"
            );
            assert_eq!(
                archive_path_identity(&archive.join("inner")),
                ArchivePathIdentity::InsideArchive,
                "{name}/inner must be inside archive"
            );
        }
    }

    #[tokio::test]
    async fn scan_directory_lists_archive_root_and_inner_directory() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("docs.zip");
        write_sample_zip(&archive);

        let root_scan = crate::scan_directory(&archive, Default::default())
            .await
            .expect("archive root must scan as a directory");
        let root_names: Vec<&str> = root_scan
            .entries
            .iter()
            .map(|entry| entry.name().to_str().unwrap())
            .collect();
        assert_eq!(root_names, vec!["photos"]);
        assert_eq!(root_scan.entries[0].kind, FileKind::Directory);
        assert!(
            root_scan.entries[0].metadata.filesystem_availability
                == crate::DirectoryMetadataAvailability::Complete,
            "virtual entries must ship complete metadata without fs demand"
        );

        let inner_scan = crate::scan_directory(&archive.join("photos"), Default::default())
            .await
            .expect("archive inner directory must scan");
        let inner_names: Vec<&str> = inner_scan
            .entries
            .iter()
            .map(|entry| entry.name().to_str().unwrap())
            .collect();
        assert_eq!(inner_names, vec!["1.txt"]);
        assert_eq!(inner_scan.entries[0].kind, FileKind::File);
        assert_eq!(inner_scan.entries[0].metadata.len, 5);
    }

    #[tokio::test]
    async fn scan_directory_reports_missing_inner_directory_as_unavailable() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("docs.zip");
        write_sample_zip(&archive);

        let outcome = crate::scan_directory(&archive.join("missing"), Default::default()).await;
        assert!(matches!(
            outcome,
            Err(FileError::ReadDirectory { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[tokio::test]
    async fn nested_zip_member_materializes_and_browses() {
        let workspace = tempfile::tempdir().unwrap();
        let inner_path = workspace.path().join("inner.zip");
        write_sample_zip(&inner_path);

        let outer_path = workspace.path().join("outer.zip");
        let file = std::fs::File::create(&outer_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("inner.zip", options).unwrap();
        zip.write_all(&std::fs::read(&inner_path).unwrap()).unwrap();
        zip.finish().unwrap();

        let nested_root = outer_path.join("inner.zip");
        let scan = crate::scan_directory(&nested_root, Default::default())
            .await
            .expect("nested zip must materialize and scan");
        let names: Vec<&str> = scan
            .entries
            .iter()
            .map(|entry| entry.name().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["photos"]);
    }

    #[tokio::test]
    async fn single_root_member_name_reports_root_and_scatter() {
        let workspace = tempfile::tempdir().unwrap();

        let single_path = workspace.path().join("single.zip");
        let file = std::fs::File::create(&single_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.add_directory("root", options).unwrap();
        zip.start_file("root/a.txt", options).unwrap();
        zip.write_all(b"1").unwrap();
        zip.finish().unwrap();
        assert_eq!(
            single_root_member_name(&single_path).await.unwrap(),
            Some("root".to_owned())
        );

        let scattered_path = workspace.path().join("scattered.zip");
        let file = std::fs::File::create(&scattered_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("a.txt", options).unwrap();
        zip.write_all(b"1").unwrap();
        zip.add_directory("dir", options).unwrap();
        zip.finish().unwrap();
        assert_eq!(
            single_root_member_name(&scattered_path).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn member_extraction_lands_files_and_numbers_duplicates() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("docs.zip");
        write_sample_zip(&archive);

        let destination = workspace.path().join("out");
        let request =
            ArchiveMemberExtractionRequest::new(vec![archive.clone()], destination.clone());
        crate::extract_archive_members_with_controls_and_progress(
            request,
            crate::FileOperationControls::running(tokio_util::sync::CancellationToken::new()),
            |_| {},
        )
        .await
        .expect("member extraction must succeed");

        let extracted = destination.join("photos/1.txt");
        assert_eq!(std::fs::read(&extracted).unwrap(), b"hello");

        // 二次提取同一文件夹成员：绝不覆盖，自动编号。
        let request =
            ArchiveMemberExtractionRequest::new(vec![archive.clone()], destination.clone());
        crate::extract_archive_members_with_controls_and_progress(
            request,
            crate::FileOperationControls::running(tokio_util::sync::CancellationToken::new()),
            |_| {},
        )
        .await
        .expect("duplicate extraction must succeed");
        let numbered = destination.join("photos/1 (2).txt");
        assert_eq!(std::fs::read(&numbered).unwrap(), b"hello");
        assert_eq!(std::fs::read(&extracted).unwrap(), b"hello");
    }

    #[tokio::test]
    async fn materialize_member_for_open_extracts_to_temp() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("docs.zip");
        write_sample_zip(&archive);

        let member = archive.join("photos/1.txt");
        let temp_file = materialize_archive_member_for_open(
            &member,
            None,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("member materialization must succeed");
        assert_eq!(std::fs::read(&temp_file).unwrap(), b"hello");

        // 真实目录中不存在该路径：确认这是虚拟成员的临时物化。
        assert!(!member.exists());
    }
}
