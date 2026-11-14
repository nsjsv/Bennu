use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bennu_localization::tr;
use file_core::mime::{MimeDatabase, NameType, TEXT_PLAIN};
use file_core::{
    list_archive_members_with_format, scan_directory, sniff_archive_extraction_format,
    ArchiveListingEntry, DirectoryEntry, FileKind, ScanOptions,
};

use crate::animated_image_preview::is_animated_image_preview_path;
use crate::audio_preview::{probe_audio_preview, AudioProbeError};
use crate::ffmpeg_install::ffmpeg_is_installed;
use crate::formatting::format_file_size;
use crate::preview::{
    AudioPreviewDetails, PreviewContent, PreviewTreeDirectoryChildren, PreviewTreeEntry,
};
use crate::preview_config::{PreviewExtensionRules, PreviewFileSizeKind, PreviewSizeLimitKind};
use crate::sqlite_preview::load_sqlite_preview;
use crate::text_preview_loading::load_initial_text_preview;

pub const PREVIEW_ARCHIVE_ENTRY_LIMIT: usize = 500;

/// 所有预览入口（本地、远程缓存、动图、文档、文件选择器）共用的超限提示。
pub fn file_too_large_to_preview(size: u64, maximum: u64) -> String {
    tr!(
        "File is too large to preview (%1). Maximum preview size is %2.",
        format_file_size(size),
        format_file_size(maximum)
    )
}

pub async fn load_preview(
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
            match classify_preview_path(&path, rules) {
                Some(PreviewPathKind::Archive) => load_archive_preview(path).await,
                Some(PreviewPathKind::Audio) => load_audio_preview(path).await,
                Some(PreviewPathKind::Video) => load_video_preview(path).await,
                Some(PreviewPathKind::Sqlite) => {
                    load_sqlite_preview(path).await.map(PreviewContent::Sqlite)
                }
                Some(PreviewPathKind::Text) => load_initial_text_preview(path, rules).await,
                // 文档、图片各有专门的加载命令，不经过这里；未分类的文件在宿主门禁就被拦下。
                Some(
                    PreviewPathKind::Document
                    | PreviewPathKind::Image
                    | PreviewPathKind::AnimatedImage,
                )
                | None => Err(unsupported_preview_kind()),
            }
        }
        FileKind::Symlink | FileKind::Other => Err(unsupported_preview_kind()),
    }
}

fn unsupported_preview_kind() -> String {
    tr!("Preview is only available for directories, archives, audio files, images, and text files")
}

async fn reject_file_over_preview_limit(path: &Path, max_file_bytes: u64) -> Result<(), String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("could not inspect preview file: {error}"))?;
    if max_file_bytes == 0 || metadata.len() <= max_file_bytes {
        return Ok(());
    }

    Err(file_too_large_to_preview(metadata.len(), max_file_bytes))
}

/// 预览路径的单一事实源分类：分支顺序必须镜像
/// `start_classified_preview` 的真实 dispatch；新增预览类型时
/// 分类器与 dispatch 必须同步扩展，禁止在调用点各自内联判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewPathKind {
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
    /// 适用哪一类大小上限；文本按需读取、音频边读边放，没有上限。
    pub fn size_limit_kind(self) -> Option<PreviewSizeLimitKind> {
        match self {
            PreviewPathKind::Document => Some(PreviewSizeLimitKind::Document),
            PreviewPathKind::Archive => Some(PreviewSizeLimitKind::Archive),
            PreviewPathKind::Sqlite => Some(PreviewSizeLimitKind::Sqlite),
            PreviewPathKind::AnimatedImage | PreviewPathKind::Image => {
                Some(PreviewSizeLimitKind::Image)
            }
            PreviewPathKind::Video => Some(PreviewSizeLimitKind::Video),
            PreviewPathKind::Audio | PreviewPathKind::Text => None,
        }
    }

    /// 要先把远程挂载上的文件整个下载到本地缓存才能预览（解码器、转换器只认本地文件）。
    /// 文本按需读挂载路径；音频播放和读信息也是对挂载路径边读边用，都不下载。
    pub fn needs_local_copy(self) -> bool {
        !matches!(self, PreviewPathKind::Text | PreviewPathKind::Audio)
    }
}

/// 返回 `None` 表示文件不属于任何可预览类型。文档、归档、SQLite、图片、
/// 视频的后缀列表完全决定该类型识别哪些后缀（替换式）；图片列表中的 gif
/// 走动图渲染。音频：额外音频后缀直接算，其余按类型库只看名字。
/// 文本是兜底：额外文本后缀直接算，其余按类型库只看名字——名字确定是
/// 非文本类型的不预览，名字定不下来的先放行，打开时看内容定论
/// （`text_preview_loading::inspect_text_file`）。
pub fn classify_preview_path(
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
    if rules.matches(Kind::Audio, path) || file_core::is_audio_by_name(path) {
        return Some(PreviewPathKind::Audio);
    }
    if rules.matches(Kind::Text, path) || may_be_text_by_name(path) {
        return Some(PreviewPathKind::Text);
    }
    None
}

fn may_be_text_by_name(path: &Path) -> bool {
    let Some(file_name) = path.file_name() else {
        return false;
    };
    let types = MimeDatabase::system();
    match types.type_from_name(file_name) {
        NameType::Known(mime) => types.is_subclass(&mime, TEXT_PLAIN),
        NameType::Candidates(candidates) => candidates
            .iter()
            .any(|candidate| types.is_subclass(candidate, TEXT_PLAIN)),
        NameType::Unknown => true,
    }
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

pub async fn load_directory_preview_children(
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

async fn load_audio_preview(path: PathBuf) -> Result<PreviewContent, String> {
    let len = tokio::fs::metadata(&path)
        .await
        .map_err(|error| tr!("Could not read this audio file: %1", error.to_string()))?
        .len();
    match probe_audio_preview(path.clone()).await {
        Ok(probe) => Ok(PreviewContent::Audio {
            path,
            details: Box::new(AudioPreviewDetails { len, probe }),
            cover: None,
            lyrics: None,
        }),
        Err(AudioProbeError::FfmpegMissing) => Ok(PreviewContent::FfmpegRequired { path }),
        Err(AudioProbeError::Failed(reason)) => {
            Err(tr!("Could not read this audio file: %1", reason))
        }
    }
}

async fn load_video_preview(path: PathBuf) -> Result<PreviewContent, String> {
    if !ffmpeg_is_installed().await {
        return Ok(PreviewContent::FfmpegRequired { path });
    }
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

#[cfg(test)]
mod tests;
