use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bennu_localization::tr;
use file_core::mime::{MimeDatabase, NameType, TEXT_PLAIN};
use file_core::{
    list_archive_members_with_format, scan_directory, sniff_archive_extraction_format,
    ArchiveListingEntry, DirectoryEntry, ExternalTool, FileError, FileKind, MediaByName,
    ScanOptions,
};

use crate::audio_preview::{probe_audio_preview, AudioProbeError};
use crate::folder_preview::FolderPreview;
use crate::formatting::format_file_size;
use crate::preview::{
    ArchiveSummary, AudioPreviewDetails, PreviewContent, PreviewTreeEntry, TreeViewport,
    VideoPreviewDetails,
};
use crate::preview_config::{PreviewExtensionRules, PreviewFileSizeKind, PreviewSizeLimitKind};
use crate::sqlite_preview::load_sqlite_preview;
use crate::text_preview_loading::{load_initial_text_preview, TextOpening};
use crate::video_preview::{
    find_external_subtitles, probe_video_preview, subtitle_choices, VideoProbeError,
};

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
                Some(PreviewPathKind::Text) => {
                    match load_initial_text_preview(path.clone(), rules).await? {
                        TextOpening::Text(opened) => Ok(PreviewContent::Text { path, opened }),
                        TextOpening::Video => load_video_preview(path).await,
                    }
                }
                // 文档、图片各有专门的加载命令，不经过这里；未分类的文件在宿主门禁就被拦下。
                Some(PreviewPathKind::Document | PreviewPathKind::Image) | None => {
                    Err(unsupported_preview_kind())
                }
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
    /// 静态图和动图：是不是动图打开时看内容（尺寸探测）。
    Image,
    Video,
    Audio,
    /// 文本，或名字定不下来的文件：打开时看内容，内容是视频就按视频打开。
    Text,
}

impl PreviewPathKind {
    /// 适用哪一类大小上限；文本按需读取、音频视频边读边放、压缩包只列目录，没有上限。
    pub fn size_limit_kind(self) -> Option<PreviewSizeLimitKind> {
        match self {
            PreviewPathKind::Document => Some(PreviewSizeLimitKind::Document),
            PreviewPathKind::Sqlite => Some(PreviewSizeLimitKind::Sqlite),
            PreviewPathKind::Image => Some(PreviewSizeLimitKind::Image),
            PreviewPathKind::Video
            | PreviewPathKind::Audio
            | PreviewPathKind::Text
            | PreviewPathKind::Archive => None,
        }
    }

    /// 要先把远程挂载上的文件整个下载到本地缓存才能预览（解码器、转换器只认本地文件）。
    /// 文本按需读挂载路径；音视频播放和读信息也是 ffmpeg 对挂载路径边读边用；压缩包由
    /// 7-Zip/tar 直接读挂载路径列目录（zip、7z 只读目录区），都不下载。
    pub fn needs_local_copy(self) -> bool {
        !matches!(
            self,
            PreviewPathKind::Text
                | PreviewPathKind::Audio
                | PreviewPathKind::Video
                | PreviewPathKind::Archive
        )
    }
}

/// 返回 `None` 表示文件不属于任何可预览类型。文档、归档、SQLite
/// 的后缀列表完全决定该类型识别哪些后缀（替换式）。图片、视频、音频：额外后缀直接算，
/// 其余按类型库只看名字；动图打开时看内容。
/// 文本是兜底：额外文本后缀直接算，其余按类型库只看名字——名字确定是
/// 非文本类型的不预览，名字定不下来的（如 `.ts`：电视录像 / Qt 翻译）先放行，
/// 打开时看内容定论，内容是视频就改按视频打开（`text_preview_loading::inspect_text_file`）。
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
    let media = file_core::media_by_name(path);
    if rules.matches(Kind::Image, path) || matches!(media, MediaByName::Image(_)) {
        return Some(PreviewPathKind::Image);
    }
    if rules.matches(Kind::Video, path) || media == MediaByName::Video {
        return Some(PreviewPathKind::Video);
    }
    if rules.matches(Kind::Audio, path) || media == MediaByName::Audio {
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
    let listing = load_directory_preview_children(path.clone(), options.clone()).await?;
    Ok(PreviewContent::Folder(Box::new(FolderPreview::new(
        path, listing, options,
    ))))
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
    let archive_bytes = tokio::fs::metadata(&path)
        .await
        .map_err(|error| error.to_string())?
        .len();
    // 自定义归档后缀无法从扩展名推断解压后端，按文件头嗅探决定。
    let format = sniff_archive_extraction_format(&path).await;
    let members = match list_archive_members_with_format(path.clone(), format).await {
        Ok(members) => members,
        Err(FileError::ToolMissing(tool)) => {
            return Ok(PreviewContent::ToolRequired { path, tool })
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut tree_builder = ArchiveTreeBuilder::new();
    for member in members {
        tree_builder.insert_member(member);
    }
    let (entries, mut summary) = tree_builder.finish();
    summary.archive_bytes = archive_bytes;
    Ok(PreviewContent::Archive {
        entries,
        summary,
        viewport: TreeViewport::default(),
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
        Err(AudioProbeError::FfmpegMissing) => Ok(PreviewContent::ToolRequired {
            path,
            tool: ExternalTool::Ffmpeg,
        }),
        Err(AudioProbeError::Failed(reason)) => {
            Err(tr!("Could not read this audio file: %1", reason))
        }
    }
}

/// 只有声音、没有画面（或只有封面图）的「视频」按音频预览打开。
async fn load_video_preview(path: PathBuf) -> Result<PreviewContent, String> {
    // 硬解探测在后台跑，和读视频信息并行；出帧不等它。
    crate::video_preview::start_hwaccel_detection();
    let len = tokio::fs::metadata(&path)
        .await
        .map_err(|error| tr!("Could not read this video file: %1", error.to_string()))?
        .len();
    match probe_video_preview(path.clone()).await {
        Ok(probe) if probe.video.is_none() => load_audio_preview(path).await,
        Ok(probe) => {
            let external = find_external_subtitles(path.clone()).await;
            let subtitles = subtitle_choices(external, &probe);
            Ok(PreviewContent::Video {
                path,
                details: Arc::new(VideoPreviewDetails {
                    len,
                    probe,
                    subtitles,
                }),
                frame: None,
            })
        }
        Err(VideoProbeError::FfmpegMissing) => Ok(PreviewContent::ToolRequired {
            path,
            tool: ExternalTool::Ffmpeg,
        }),
        Err(VideoProbeError::Failed(reason)) => {
            Err(tr!("Could not read this video file: %1", reason))
        }
    }
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
            let node = self.insert_segment(parent, segment, kind);
            if is_leaf {
                self.nodes[node].len = member.len;
                self.nodes[node].modified = member.modified;
            }
            parent = Some(node);
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
            parent,
            len: 0,
            modified: None,
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

    /// 摊平成树行（全部展开），文件夹大小是里面文件加起来；顺带数出总计。
    fn finish(mut self) -> (Vec<PreviewTreeEntry>, ArchiveSummary) {
        let mut summary = ArchiveSummary::default();
        for node in &mut self.nodes {
            if node.kind == FileKind::Directory {
                summary.folders += 1;
                node.len = 0;
            } else {
                summary.files += 1;
                summary.unpacked_bytes = summary.unpacked_bytes.saturating_add(node.len);
            }
        }
        // 子节点总是在父节点之后建的：倒着加一遍，文件夹就拿到整棵子树的大小。
        for index in (0..self.nodes.len()).rev() {
            if let Some(parent) = self.nodes[index].parent {
                let len = self.nodes[index].len;
                self.nodes[parent].len = self.nodes[parent].len.saturating_add(len);
            }
        }
        let mut entries = Vec::with_capacity(self.nodes.len());
        for node_index in self.sorted_child_indices(None) {
            self.append_flattened_entry(node_index, None, 0, &mut entries);
        }
        (entries, summary)
    }

    fn append_flattened_entry(
        &self,
        node_index: usize,
        parent: Option<usize>,
        depth: usize,
        entries: &mut Vec<PreviewTreeEntry>,
    ) {
        let node = &self.nodes[node_index];
        let entry_id = entries.len();
        entries.push(PreviewTreeEntry {
            id: entry_id,
            name: node.name.clone(),
            kind: node.kind,
            depth,
            parent,
            is_expanded: true,
            toggle_rotation_progress: if node.kind == FileKind::Directory {
                1.0
            } else {
                0.0
            },
            len: Some(node.len),
            modified: node.modified,
        });

        if node.kind != FileKind::Directory {
            return;
        }

        for child_index in self.sorted_child_indices(Some(node_index)) {
            self.append_flattened_entry(child_index, Some(entry_id), depth + 1, entries);
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

struct ArchiveTreeNode {
    name: String,
    kind: FileKind,
    parent: Option<usize>,
    len: u64,
    modified: Option<std::time::SystemTime>,
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
