//! 文件夹预览：根下的条目树（列出过内容的文件夹带着孩子）、每个文件夹多大、选中哪行、
//! 读子文件夹排队。每棵子树只走一遍：没列出过内容的文件夹自己测（多线程测量），
//! 列出过的由孩子推出来；展开且列出过的文件夹和根由宿主订阅监视，变了就重读对账。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use file_core::{
    DirectoryEntry, DirectoryUsage, FileKind, HiddenEntries, ScanOptions, UsageMeasurement,
    UsageProgress, UsageSnapshot,
};
use iced::Task;

use crate::preview::TreeViewport;
use crate::preview_loading::load_directory_preview_children;
use crate::preview_message::PreviewMessage;

/// 同时最多读几个子文件夹：自动展开时根下几百个子文件夹排队读，不一起压到磁盘上。
const LISTINGS_AT_ONCE: usize = 4;

#[derive(Debug, Clone)]
pub enum FolderPreviewMessage {
    /// 点了文件夹前面的箭头。
    Toggled(PathBuf),
    /// 点了一行。
    Selected(PathBuf),
    /// 独立预览窗口里的方向键、回车；滚动区 id 用来把选中行滚进视口。
    Key(FolderKey, iced::widget::Id),
    /// 读完一个文件夹（`generation` 不是当前会话就丢掉）。
    Listed {
        generation: u64,
        directory: PathBuf,
        outcome: Result<Vec<DirectoryEntry>, String>,
    },
    /// 监视着的文件夹里有变化。
    Changed { generation: u64, directory: PathBuf },
    /// 有测量在跑时的节拍：读一遍各测量的数。
    MeasurementTick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKey {
    Up,
    Down,
    Expand,
    Collapse,
    Open,
}

/// 一个文件夹（或根）里面的用量；`settled` 是里面都算完了（读不了的也算完，记在
/// `unreadable` 里）。没算完时行上先空着，总计显示「正在计算」和当前的部分数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderUsage {
    pub usage: DirectoryUsage,
    pub settled: bool,
}

#[derive(Debug, Clone)]
pub struct FolderNode {
    pub path: PathBuf,
    pub name: String,
    pub kind: FileKind,
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// 只有文件夹有。
    pub folder: Option<Box<FolderState>>,
}

#[derive(Debug, Clone, Default)]
pub struct FolderState {
    pub expanded: bool,
    /// 箭头转到哪了（0 收起，1 展开），动画用。
    pub rotation: f32,
    pub listing: FolderListing,
    /// 没列出过内容时它自己的测量；算完就收起来放进 `measured`。
    measurement: Option<Arc<UsageMeasurement>>,
    measured: Option<UsageSnapshot>,
    pub usage: FolderUsage,
}

#[derive(Debug, Clone, Default)]
pub enum FolderListing {
    #[default]
    NotLoaded,
    /// 排着或正在读。
    Waiting,
    Loaded(Vec<FolderNode>),
    Failed(String),
}

/// 看得见的一行：条目，或展开的文件夹下面的一行状态（在读、读不了）。
#[derive(Debug, Clone, Copy)]
pub enum FolderRow<'a> {
    Entry { node: &'a FolderNode, depth: usize },
    Reading { depth: usize },
    Failed { depth: usize, error: &'a str },
}

/// 键盘处理的结果：要不要打开某项、选中的是第几行（好滚进视口）。
#[derive(Debug, Default)]
pub struct FolderKeyOutcome {
    pub task: Task<PreviewMessage>,
    pub open: Option<(PathBuf, FileKind)>,
    pub selected_row: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct FolderPreview {
    root: PathBuf,
    generation: u64,
    options: ScanOptions,
    expand_levels: u8,
    entries: Vec<FolderNode>,
    selected: Option<PathBuf>,
    pub viewport: TreeViewport,
    summary: FolderUsage,
    measuring: bool,
    waiting: VecDeque<PathBuf>,
    /// 正在读的；句柄丢掉即中止（切走预览时）。
    reading: Vec<(PathBuf, iced::task::Handle)>,
}

impl FolderPreview {
    /// 根的条目（扫描结果，已按 `options` 排好序）；会话从 [`Self::begin`] 开始。
    pub fn new(root: PathBuf, listing: Vec<DirectoryEntry>, options: ScanOptions) -> Self {
        Self {
            root,
            generation: 0,
            options,
            expand_levels: 0,
            entries: listing.into_iter().map(node_from).collect(),
            selected: None,
            viewport: TreeViewport::default(),
            summary: FolderUsage::default(),
            measuring: false,
            waiting: VecDeque::new(),
            reading: Vec::new(),
        }
    }

    /// 定下会话身份和自动展开层数，按层数展开或开始测量根下的文件夹。
    pub fn begin(&mut self, generation: u64, expand_levels: u8) -> Task<PreviewMessage> {
        self.generation = generation;
        self.expand_levels = expand_levels;
        let plan = self.new_folder_plan();
        for node in &mut self.entries {
            plan.prepare(node, 0, &mut self.waiting);
        }
        self.refresh_usage();
        self.start_listings()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn entries(&self) -> &[FolderNode] {
        &self.entries
    }

    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn summary(&self) -> FolderUsage {
        self.summary
    }

    /// 有测量在跑：宿主据此挂测量节拍。
    pub fn is_measuring(&self) -> bool {
        self.measuring
    }

    /// 要监视的文件夹：根，加上展开且列出过内容的。
    pub fn watched_directories(&self) -> Vec<PathBuf> {
        let mut directories = vec![self.root.clone()];
        collect_watched(&self.entries, &mut directories);
        directories
    }

    pub fn visible_rows(&self) -> Vec<FolderRow<'_>> {
        let mut rows = Vec::new();
        push_visible_rows(&self.entries, 0, &mut rows);
        rows
    }

    pub fn toggle(&mut self, path: &Path) -> Task<PreviewMessage> {
        let Some(state) = self.folder_state_mut(path) else {
            return Task::none();
        };
        state.expanded = !state.expanded;
        if state.expanded {
            // 收起期间没在监视，展开时重读一遍对账。
            if !matches!(state.listing, FolderListing::Loaded(_)) {
                state.listing = FolderListing::Waiting;
            }
            self.request_listing(path.to_path_buf());
        }
        self.refresh_usage();
        self.start_listings()
    }

    pub fn select(&mut self, path: PathBuf) {
        self.selected = Some(path);
    }

    /// 监视到变化：把这个文件夹排进去重读。
    pub fn directory_changed(
        &mut self,
        generation: u64,
        directory: PathBuf,
    ) -> Task<PreviewMessage> {
        if generation != self.generation {
            return Task::none();
        }
        self.request_listing(directory);
        self.start_listings()
    }

    pub fn accept_listing(
        &mut self,
        generation: u64,
        directory: PathBuf,
        outcome: Result<Vec<DirectoryEntry>, String>,
    ) -> Task<PreviewMessage> {
        if generation != self.generation {
            return Task::none();
        }
        if let Some(index) = self.reading.iter().position(|(path, _)| *path == directory) {
            self.reading.swap_remove(index);
        }
        let plan = self.new_folder_plan();
        let mut waiting = Vec::new();
        if directory == self.root {
            // 根读不了（多半刚被删掉）就留着旧的，主窗口自己会处理。
            if let Ok(listing) = outcome {
                let old = std::mem::take(&mut self.entries);
                self.entries = plan.reconcile(old, listing, 0, &mut waiting);
            }
        } else if let Some(depth) = self.depth_of(&directory) {
            if let Some(state) = self.folder_state_mut(&directory) {
                match outcome {
                    Ok(listing) => {
                        let old = match std::mem::take(&mut state.listing) {
                            FolderListing::Loaded(children) => children,
                            _ => Vec::new(),
                        };
                        // 第一次列出内容：自己的测量若还在跑就停掉，孩子各自测，同一棵子树
                        // 不走两遍；已算完的留着，孩子没算完前先显示它。
                        if state.measurement.take().is_some() {
                            state.measured = None;
                        }
                        state.listing = FolderListing::Loaded(plan.reconcile(
                            old,
                            listing,
                            depth + 1,
                            &mut waiting,
                        ));
                    }
                    Err(error) => state.listing = FolderListing::Failed(error),
                }
            }
        }
        self.waiting.extend(waiting);
        self.refresh_usage();
        self.start_listings()
    }

    /// 测量节拍：读各测量的数，算完的收起来。
    pub fn poll_measurements(&mut self) {
        poll_nodes(&mut self.entries);
        self.refresh_usage();
    }

    pub fn press_key(&mut self, key: FolderKey) -> FolderKeyOutcome {
        let entries = self.visible_entry_paths();
        let current = self
            .selected
            .as_ref()
            .and_then(|selected| entries.iter().position(|(path, _, _)| path == selected));
        let mut outcome = FolderKeyOutcome::default();
        match (key, current) {
            (FolderKey::Up | FolderKey::Down, None) => {
                if let Some((path, _, _)) = entries.first() {
                    self.selected = Some(path.clone());
                }
            }
            (FolderKey::Up, Some(index)) => {
                self.selected = Some(entries[index.saturating_sub(1)].0.clone());
            }
            (FolderKey::Down, Some(index)) => {
                self.selected = Some(entries[(index + 1).min(entries.len() - 1)].0.clone());
            }
            (FolderKey::Expand, Some(index)) => {
                let (path, kind, expanded) = entries[index].clone();
                match (kind, expanded) {
                    (FileKind::Directory, false) => outcome.task = self.toggle(&path),
                    (FileKind::Directory, true) => {
                        if let Some((child, _, _)) = entries
                            .get(index + 1)
                            .filter(|(child, _, _)| child.parent() == Some(path.as_path()))
                        {
                            self.selected = Some(child.clone());
                        }
                    }
                    _ => {}
                }
            }
            (FolderKey::Collapse, Some(index)) => {
                let (path, kind, expanded) = entries[index].clone();
                if kind == FileKind::Directory && expanded {
                    outcome.task = self.toggle(&path);
                } else if let Some(parent) = path.parent().filter(|parent| *parent != self.root) {
                    self.selected = Some(parent.to_path_buf());
                }
            }
            (FolderKey::Open, Some(index)) => {
                let (path, kind, _) = entries[index].clone();
                outcome.open = Some((path, kind));
            }
            (FolderKey::Expand | FolderKey::Collapse | FolderKey::Open, None) => {}
        }
        outcome.selected_row = self.selected_row();
        outcome
    }

    /// 选中行在所有看得见的行（含状态行）里排第几。
    pub fn selected_row(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        self.visible_rows()
            .iter()
            .position(|row| matches!(row, FolderRow::Entry { node, .. } if node.path == selected))
    }

    /// 箭头动画：每个文件夹往展开/收起转一步；还有在转的返回 true。
    pub fn advance_rotation(&mut self, step: f32) -> bool {
        advance_rotation(&mut self.entries, step)
    }

    pub fn rotation_is_active(&self) -> bool {
        rotation_is_active(&self.entries)
    }

    fn visible_entry_paths(&self) -> Vec<(PathBuf, FileKind, bool)> {
        self.visible_rows()
            .into_iter()
            .filter_map(|row| match row {
                FolderRow::Entry { node, .. } => Some((
                    node.path.clone(),
                    node.kind,
                    node.folder.as_ref().is_some_and(|state| state.expanded),
                )),
                FolderRow::Reading { .. } | FolderRow::Failed { .. } => None,
            })
            .collect()
    }

    fn new_folder_plan(&self) -> NewFolderPlan {
        NewFolderPlan {
            expand_levels: usize::from(self.expand_levels),
            hidden: if self.options.include_hidden {
                HiddenEntries::Counted
            } else {
                HiddenEntries::Skipped
            },
        }
    }

    /// 根的孩子是第 0 层。
    fn depth_of(&self, path: &Path) -> Option<usize> {
        let relative = path.strip_prefix(&self.root).ok()?;
        relative.components().count().checked_sub(1)
    }

    fn folder_state_mut(&mut self, path: &Path) -> Option<&mut FolderState> {
        let relative = path.strip_prefix(&self.root).ok()?;
        let mut nodes = &mut self.entries;
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let node = nodes
                .iter_mut()
                .find(|node| node.path.file_name() == Some(component.as_os_str()))?;
            let state = node.folder.as_deref_mut()?;
            if components.peek().is_none() {
                return Some(state);
            }
            let FolderListing::Loaded(children) = &mut state.listing else {
                return None;
            };
            nodes = children;
        }
        None
    }

    fn request_listing(&mut self, directory: PathBuf) {
        if !self.waiting.contains(&directory) {
            self.waiting.push_back(directory);
        }
    }

    fn start_listings(&mut self) -> Task<PreviewMessage> {
        let mut tasks = Vec::new();
        while self.reading.len() < LISTINGS_AT_ONCE {
            let Some(directory) = self.waiting.pop_front() else {
                break;
            };
            // 排着的时候被删掉的文件夹就不读了。
            if directory != self.root && self.folder_state_mut(&directory).is_none() {
                continue;
            }
            let (task, handle) =
                listing_command(self.generation, directory.clone(), self.options.clone())
                    .abortable();
            self.reading.push((directory, handle.abort_on_drop()));
            tasks.push(task);
        }
        Task::batch(tasks)
    }

    fn refresh_usage(&mut self) {
        let mut measuring = false;
        self.summary = usage_of(&mut self.entries, &mut measuring);
        self.measuring = measuring;
    }
}

/// 新出现的文件夹怎么起步：层数够浅就自动展开（排队读），否则自己测量。
struct NewFolderPlan {
    expand_levels: usize,
    hidden: HiddenEntries,
}

impl NewFolderPlan {
    fn prepare(&self, node: &mut FolderNode, depth: usize, waiting: &mut impl Extend<PathBuf>) {
        let Some(state) = node.folder.as_deref_mut() else {
            return;
        };
        if depth < self.expand_levels {
            state.expanded = true;
            state.listing = FolderListing::Waiting;
            waiting.extend([node.path.clone()]);
        } else {
            state.measurement = Some(Arc::new(UsageMeasurement::start(
                node.path.clone(),
                self.hidden,
            )));
        }
    }

    /// 按路径对账：还在的保留展开状态、子树、测量，只换大小、时间；没了的连同测量
    /// 一起丢掉；新出现的按 [`Self::prepare`] 起步。顺序跟扫描结果。
    fn reconcile(
        &self,
        old: Vec<FolderNode>,
        listing: Vec<DirectoryEntry>,
        depth: usize,
        waiting: &mut Vec<PathBuf>,
    ) -> Vec<FolderNode> {
        let mut kept: HashMap<PathBuf, FolderNode> = old
            .into_iter()
            .map(|node| (node.path.clone(), node))
            .collect();
        listing
            .into_iter()
            .map(|entry| {
                let fresh = node_from(entry);
                match kept.remove(&fresh.path) {
                    Some(mut node) if node.kind == fresh.kind => {
                        node.name = fresh.name;
                        node.len = fresh.len;
                        node.modified = fresh.modified;
                        node
                    }
                    _ => {
                        let mut node = fresh;
                        self.prepare(&mut node, depth, waiting);
                        node
                    }
                }
            })
            .collect()
    }
}

fn node_from(entry: DirectoryEntry) -> FolderNode {
    let kind = entry.kind;
    FolderNode {
        name: entry.name().to_string_lossy().into_owned(),
        len: entry.metadata.len,
        modified: entry.metadata.modified,
        path: entry.path().to_path_buf(),
        kind,
        folder: (kind == FileKind::Directory).then(Box::default),
    }
}

fn listing_command(
    generation: u64,
    directory: PathBuf,
    options: ScanOptions,
) -> Task<PreviewMessage> {
    let listed = directory.clone();
    Task::perform(
        load_directory_preview_children(directory, options),
        move |outcome| {
            PreviewMessage::FolderPreview(FolderPreviewMessage::Listed {
                generation,
                directory: listed.clone(),
                outcome,
            })
        },
    )
}

/// 一层孩子加起来，顺带把每个文件夹自己的用量写回去。
fn usage_of(nodes: &mut [FolderNode], measuring: &mut bool) -> FolderUsage {
    let mut total = FolderUsage {
        usage: DirectoryUsage::default(),
        settled: true,
    };
    for node in nodes {
        total.usage.bytes = total.usage.bytes.saturating_add(node.len);
        let Some(state) = node.folder.as_deref_mut() else {
            total.usage.files += 1;
            continue;
        };
        total.usage.folders += 1;
        let inner = folder_usage(state, measuring);
        state.usage = inner;
        total.usage = add_usage(total.usage, inner.usage);
        total.settled &= inner.settled;
    }
    total
}

fn folder_usage(state: &mut FolderState, measuring: &mut bool) -> FolderUsage {
    *measuring |= state.measurement.is_some();
    let derived = match &mut state.listing {
        FolderListing::Loaded(children) => Some(usage_of(children, measuring)),
        _ => None,
    };
    let measured = state
        .measured
        .as_ref()
        .map(|snapshot| match &snapshot.progress {
            UsageProgress::Running => FolderUsage {
                usage: snapshot.usage,
                settled: false,
            },
            UsageProgress::Finished => FolderUsage {
                usage: snapshot.usage,
                settled: true,
            },
            UsageProgress::Failed(_) => unreadable_usage(),
        });
    match (derived, measured) {
        (Some(derived), _) if derived.settled => derived,
        (_, Some(measured)) if measured.settled => measured,
        (Some(derived), _) => derived,
        (None, Some(measured)) => measured,
        (None, None) if matches!(state.listing, FolderListing::Failed(_)) => unreadable_usage(),
        (None, None) => FolderUsage::default(),
    }
}

fn unreadable_usage() -> FolderUsage {
    FolderUsage {
        usage: DirectoryUsage {
            unreadable: 1,
            ..DirectoryUsage::default()
        },
        settled: true,
    }
}

fn add_usage(left: DirectoryUsage, right: DirectoryUsage) -> DirectoryUsage {
    DirectoryUsage {
        files: left.files + right.files,
        folders: left.folders + right.folders,
        bytes: left.bytes.saturating_add(right.bytes),
        disk_bytes: left.disk_bytes.saturating_add(right.disk_bytes),
        unreadable: left.unreadable + right.unreadable,
    }
}

fn poll_nodes(nodes: &mut [FolderNode]) {
    for state in nodes
        .iter_mut()
        .filter_map(|node| node.folder.as_deref_mut())
    {
        if let Some(measurement) = &state.measurement {
            let snapshot = measurement.snapshot();
            if snapshot.progress != UsageProgress::Running {
                state.measurement = None;
            }
            state.measured = Some(snapshot);
        }
        if let FolderListing::Loaded(children) = &mut state.listing {
            poll_nodes(children);
        }
    }
}

fn collect_watched(nodes: &[FolderNode], directories: &mut Vec<PathBuf>) {
    for node in nodes {
        let Some(state) = node.folder.as_deref() else {
            continue;
        };
        if let (true, FolderListing::Loaded(children)) = (state.expanded, &state.listing) {
            directories.push(node.path.clone());
            collect_watched(children, directories);
        }
    }
}

fn push_visible_rows<'a>(nodes: &'a [FolderNode], depth: usize, rows: &mut Vec<FolderRow<'a>>) {
    for node in nodes {
        rows.push(FolderRow::Entry { node, depth });
        let Some(state) = node.folder.as_deref().filter(|state| state.expanded) else {
            continue;
        };
        match &state.listing {
            FolderListing::Loaded(children) => push_visible_rows(children, depth + 1, rows),
            FolderListing::NotLoaded | FolderListing::Waiting => {
                rows.push(FolderRow::Reading { depth: depth + 1 });
            }
            FolderListing::Failed(error) => rows.push(FolderRow::Failed {
                depth: depth + 1,
                error,
            }),
        }
    }
}

fn advance_rotation(nodes: &mut [FolderNode], step: f32) -> bool {
    let mut active = false;
    for state in nodes
        .iter_mut()
        .filter_map(|node| node.folder.as_deref_mut())
    {
        let target = if state.expanded { 1.0 } else { 0.0 };
        state.rotation = if state.rotation < target {
            (state.rotation + step).min(target)
        } else {
            (state.rotation - step).max(target)
        };
        active |= state.rotation != target;
        if let FolderListing::Loaded(children) = &mut state.listing {
            active |= advance_rotation(children, step);
        }
    }
    active
}

fn rotation_is_active(nodes: &[FolderNode]) -> bool {
    nodes
        .iter()
        .filter_map(|node| node.folder.as_deref())
        .any(|state| {
            let target = if state.expanded { 1.0 } else { 0.0 };
            state.rotation != target
                || matches!(&state.listing, FolderListing::Loaded(children) if rotation_is_active(children))
        })
}

#[cfg(test)]
#[path = "folder_preview/tests.rs"]
mod tests;
