use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use file_core::{
    list_archive_members_with_format, scan_directory, sniff_archive_extraction_format,
    ArchiveListingEntry, DirectoryEntry, FileKind, ScanOptions,
};

use crate::animated_image_preview::is_animated_image_preview_path;
use crate::audio_preview::inspect_audio_preview_metadata;
use crate::config::{PreviewExtensionRules, PreviewFileSizeKind};
use crate::formatting::format_file_size;
use crate::model::{PreviewContent, PreviewTreeDirectoryChildren, PreviewTreeEntry};
use crate::sqlite_preview::load_sqlite_preview;
use crate::text_preview_loading::load_initial_text_preview;

pub(crate) const PREVIEW_ARCHIVE_ENTRY_LIMIT: usize = 500;

const SUPPORTED_ARCHIVE_FORMAT_MESSAGE: &str =
    "Archive preview supports .zip, .tar, .tar.gz, .tgz, .7z, and .rar files";

pub(crate) async fn load_preview(
    path: PathBuf,
    kind: FileKind,
    rules: &PreviewExtensionRules,
    options: ScanOptions,
    max_file_bytes: u64,
) -> Result<PreviewContent, String> {
    match kind {
        FileKind::Directory => load_directory_preview(path, options).await,
        FileKind::File => {
            reject_file_over_preview_limit(&path, max_file_bytes).await?;
            if rules.matches(PreviewFileSizeKind::Archive, &path) {
                load_archive_preview(path).await
            } else if is_recognized_unsupported_archive_path(&path) {
                Err(format!(
                    "This archive format is not supported yet. {SUPPORTED_ARCHIVE_FORMAT_MESSAGE}"
                ))
            } else if rules.matches(PreviewFileSizeKind::Audio, &path) {
                load_audio_preview(path).await
            } else if rules.matches(PreviewFileSizeKind::Video, &path) {
                load_video_preview(path).await
            } else if rules.matches(PreviewFileSizeKind::Sqlite, &path) {
                load_sqlite_preview(path).await.map(PreviewContent::Sqlite)
            } else {
                load_text_preview(path).await
            }
        }
        FileKind::Symlink | FileKind::Other => {
            Err("Preview is only available for directories, archives, audio files, images, and UTF-8 text files".to_owned())
        }
    }
}

async fn reject_file_over_preview_limit(path: &Path, max_file_bytes: u64) -> Result<(), String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("could not inspect preview file: {error}"))?;
    if max_file_bytes == 0 || metadata.len() <= max_file_bytes {
        return Ok(());
    }

    Err(format!(
        "File is too large to preview ({}). Maximum preview size is {}.",
        format_file_size(metadata.len()),
        format_file_size(max_file_bytes)
    ))
}

/// 预览路径的单一事实源分类：分支顺序必须镜像
/// `start_classified_preview` 的真实 dispatch；新增预览类型时
/// 分类器与 dispatch 必须同步扩展，禁止在调用点各自内联判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewPathKind {
    Document,
    Archive,
    Sqlite,
    AnimatedImage,
    Image,
    Video,
    Audio,
    Text,
}

impl PreviewPathKind {
    pub(crate) fn file_size_kind(self) -> PreviewFileSizeKind {
        match self {
            PreviewPathKind::Document => PreviewFileSizeKind::Document,
            PreviewPathKind::Archive => PreviewFileSizeKind::Archive,
            PreviewPathKind::Sqlite => PreviewFileSizeKind::Sqlite,
            PreviewPathKind::AnimatedImage | PreviewPathKind::Image => PreviewFileSizeKind::Image,
            PreviewPathKind::Video => PreviewFileSizeKind::Video,
            PreviewPathKind::Audio => PreviewFileSizeKind::Audio,
            PreviewPathKind::Text => PreviewFileSizeKind::Text,
        }
    }
}

/// 返回 `None` 表示文件不属于任何可预览类型。每个类型的后缀列表
/// 完全决定该类型识别哪些后缀（替换式）；图片列表中的 gif 走动图渲染。
pub(crate) fn classify_preview_path(
    path: &Path,
    rules: &PreviewExtensionRules,
) -> Option<PreviewPathKind> {
    use PreviewFileSizeKind as Kind;

    if rules.matches(Kind::Document, path) {
        return Some(PreviewPathKind::Document);
    }
    if rules.matches(Kind::Archive, path) {
        return Some(PreviewPathKind::Archive);
    }
    if rules.matches(Kind::Sqlite, path) {
        return Some(PreviewPathKind::Sqlite);
    }
    if rules.matches(Kind::Image, path) {
        if is_animated_image_preview_path(path) {
            return Some(PreviewPathKind::AnimatedImage);
        }
        return Some(PreviewPathKind::Image);
    }
    if rules.matches(Kind::Video, path) {
        return Some(PreviewPathKind::Video);
    }
    if rules.matches(Kind::Audio, path) {
        return Some(PreviewPathKind::Audio);
    }
    if rules.matches(Kind::Text, path) {
        return Some(PreviewPathKind::Text);
    }
    None
}

async fn load_directory_preview(
    path: PathBuf,
    options: ScanOptions,
) -> Result<PreviewContent, String> {
    let directory_entries = load_directory_preview_children(path, options).await?;
    let entries = directory_entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| PreviewTreeEntry::from_directory_entry(index, entry, 0, None))
        .collect();

    Ok(PreviewContent::Directory { entries })
}

pub(crate) async fn load_directory_preview_children(
    path: PathBuf,
    options: ScanOptions,
) -> Result<Vec<DirectoryEntry>, String> {
    scan_directory(path, options)
        .await
        .map(|scan| scan.entries)
        .map_err(|error| error.to_string())
}

async fn load_archive_preview(path: PathBuf) -> Result<PreviewContent, String> {
    // 自定义归档后缀无法从扩展名推断解压后端，按文件头嗅探决定。
    let format = sniff_archive_extraction_format(&path).await;
    let members = list_archive_members_with_format(path, format)
        .await
        .map_err(|error| error.to_string())?;
    let mut tree_builder = ArchiveTreeBuilder::new();
    for member in members {
        tree_builder.insert_member(member);
    }
    let archive_preview = tree_builder.finish(PREVIEW_ARCHIVE_ENTRY_LIMIT);

    Ok(PreviewContent::Archive {
        entries: archive_preview.entries,
    })
}

fn is_recognized_unsupported_archive_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["xz", "bz2", "zst", "deb", "rpm"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

async fn load_audio_preview(path: PathBuf) -> Result<PreviewContent, String> {
    let preview_path = path.clone();
    let metadata = inspect_audio_preview_metadata(path).await?;

    Ok(PreviewContent::Audio {
        path: preview_path,
        duration: metadata.duration,
        len: metadata.len,
    })
}

async fn load_video_preview(path: PathBuf) -> Result<PreviewContent, String> {
    Ok(PreviewContent::Video {
        path,
        frame: None,
        width: 0,
        height: 0,
        duration: None,
    })
}

struct ArchivePreview {
    entries: Vec<PreviewTreeEntry>,
}

struct ArchiveTreeBuilder {
    nodes: Vec<ArchiveTreeNode>,
    children_by_parent: Vec<BTreeMap<String, usize>>,
    root_children: BTreeMap<String, usize>,
}

impl ArchiveTreeBuilder {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            children_by_parent: Vec::new(),
            root_children: BTreeMap::new(),
        }
    }

    fn insert_member(&mut self, member: ArchiveListingEntry) {
        let segments = archive_path_segments(&member.path);
        if segments.is_empty() {
            return;
        }

        let mut parent = None;
        for (depth, segment) in segments.iter().enumerate() {
            let is_leaf = depth + 1 == segments.len();
            let kind = if is_leaf {
                member.kind
            } else {
                FileKind::Directory
            };
            parent = Some(self.insert_segment(parent, segment, kind));
        }
    }

    fn insert_segment(&mut self, parent: Option<usize>, name: &str, kind: FileKind) -> usize {
        if let Some(existing_index) = self.child_index(parent, name) {
            if kind == FileKind::Directory {
                self.nodes[existing_index].kind = FileKind::Directory;
            }
            return existing_index;
        }

        let node_index = self.nodes.len();
        self.nodes.push(ArchiveTreeNode {
            name: name.to_owned(),
            kind,
        });
        self.children_by_parent.push(BTreeMap::new());
        self.children_mut(parent)
            .insert(name.to_owned(), node_index);
        node_index
    }

    fn child_index(&self, parent: Option<usize>, name: &str) -> Option<usize> {
        match parent {
            Some(parent_index) => self.children_by_parent[parent_index].get(name).copied(),
            None => self.root_children.get(name).copied(),
        }
    }

    fn children_mut(&mut self, parent: Option<usize>) -> &mut BTreeMap<String, usize> {
        match parent {
            Some(parent_index) => &mut self.children_by_parent[parent_index],
            None => &mut self.root_children,
        }
    }

    fn finish(self, limit: usize) -> ArchivePreview {
        let mut entries = Vec::new();
        let root_children = self.sorted_child_indices(None);
        for node_index in root_children {
            self.append_flattened_entry(node_index, None, 0, limit, &mut entries);
            if entries.len() >= limit {
                break;
            }
        }

        ArchivePreview { entries }
    }

    fn append_flattened_entry(
        &self,
        node_index: usize,
        parent: Option<usize>,
        depth: usize,
        limit: usize,
        entries: &mut Vec<PreviewTreeEntry>,
    ) {
        if entries.len() >= limit {
            return;
        }

        let node = &self.nodes[node_index];
        let entry_id = entries.len();
        entries.push(PreviewTreeEntry {
            id: entry_id,
            name: node.name.clone(),
            kind: node.kind,
            depth,
            parent,
            filesystem_path: None,
            directory_children: preview_archive_directory_children(node.kind),
            is_expanded: true,
            toggle_rotation_progress: if node.kind == FileKind::Directory {
                1.0
            } else {
                0.0
            },
        });

        if node.kind != FileKind::Directory {
            return;
        }

        for child_index in self.sorted_child_indices(Some(node_index)) {
            self.append_flattened_entry(child_index, Some(entry_id), depth + 1, limit, entries);
            if entries.len() >= limit {
                break;
            }
        }
    }

    fn sorted_child_indices(&self, parent: Option<usize>) -> Vec<usize> {
        let children = match parent {
            Some(parent_index) => &self.children_by_parent[parent_index],
            None => &self.root_children,
        };
        let mut indices = children.values().copied().collect::<Vec<_>>();
        indices.sort_by(|left, right| archive_node_order(&self.nodes[*left], &self.nodes[*right]));
        indices
    }
}

fn preview_archive_directory_children(kind: FileKind) -> Option<PreviewTreeDirectoryChildren> {
    (kind == FileKind::Directory).then_some(PreviewTreeDirectoryChildren::Loaded)
}

struct ArchiveTreeNode {
    name: String,
    kind: FileKind,
}

fn archive_node_order(left: &ArchiveTreeNode, right: &ArchiveTreeNode) -> std::cmp::Ordering {
    archive_kind_rank(left.kind)
        .cmp(&archive_kind_rank(right.kind))
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        .then_with(|| left.name.cmp(&right.name))
}

fn archive_kind_rank(kind: FileKind) -> u8 {
    match kind {
        FileKind::Directory => 0,
        FileKind::File => 1,
        FileKind::Symlink => 2,
        FileKind::Other => 3,
    }
}

fn archive_path_segments(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect()
}

async fn load_text_preview(path: PathBuf) -> Result<PreviewContent, String> {
    load_initial_text_preview(path).await
}

#[cfg(test)]
mod tests;
