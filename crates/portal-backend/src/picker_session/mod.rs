//! 每个选择窗口的核心状态机：目录导航（含历史栈）、条目选择、目录原地
//! 展开（访达列表语义）、SaveFile 命名与覆盖确认、地址栏编辑。不触碰
//! iced 类型，效果以 `SessionEffect` 输出由上层翻译执行，保证逻辑可
//! 脱离窗口环境单测。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::oneshot;

use file_core::entry::{DirectoryEntry, FileKind};
use file_core::scan::{scan_directory, ScanOptions};

use crate::dbus_file_chooser::PickerResolution;
use crate::filter::PickerFilter;
use crate::picker_request::{FilterRule, PickerKind, PickerRequestSpec};

/// 目录内容加载状态（根列表与展开节点共用）。
pub(crate) enum DirectoryListing {
    Pending,
    Ready(Vec<DirectoryEntry>),
    Failed(String),
}

/// 状态机对外请求的效果。
pub(crate) enum SessionEffect {
    None,
    /// 需要扫描该目录（进入/上级/面包屑/展开/初始；结果按路径回填）。
    ScanDirectory(PathBuf),
    /// 用户确认，携带选中路径。
    Confirmed(Vec<PathBuf>),
    /// 用户取消（Esc / 取消按钮）。
    Dismissed,
}

/// 视图事件。
#[derive(Debug, Clone)]
pub(crate) enum SessionMessage {
    ScanReady(Box<DirectoryScanResult>),
    EntryClicked {
        index: usize,
        ctrl: bool,
        shift: bool,
    },
    EntryDoubleClicked {
        index: usize,
    },
    /// 目录行的展开/收起开关（访达列表语义）。
    EntryExpandToggled {
        index: usize,
    },
    NavigateUp,
    NavigateBack,
    NavigateForward,
    BreadcrumbActivated {
        ancestor: usize,
    },
    FilterSelected {
        rule: usize,
    },
    NameInputChanged(String),
    ConfirmPressed,
    DismissPressed,
    OverwriteDeclined,
    /// 下拉选择值不在已知规则中（视图与状态不一致的兜底，正常不可达）。
    FilterSelectionIgnored,
    /// 指针悬停变化；None = 离开所有行。索引以当前扁平化行列表为准。
    EntryHovered {
        index: Option<usize>,
    },
    /// Ctrl+A 全选可选中条目（仅 OpenFile 多选模式有意义）。
    SelectAllPressed,
    /// 点击地址栏空白处进入路径编辑（草稿预填当前目录）。
    AddressEditingStarted,
    AddressEditChanged(String),
    AddressEditingSubmitted,
    AddressEditingCancelled,
}

/// 扫描完成载荷：携带扫描目标，供会话把结果路由回根列表或展开节点。
#[derive(Debug, Clone)]
pub(crate) struct DirectoryScanResult {
    pub(crate) directory: PathBuf,
    pub(crate) outcome: Result<DirectoryScanOutcome, String>,
}

/// 扫描成功内容（避免 session 依赖 file-core 错误类型细节）。
#[derive(Debug, Clone)]
pub(crate) struct DirectoryScanOutcome {
    pub(crate) entries: Vec<DirectoryEntry>,
}

/// 扁平化后的可见行：根条目与已展开子级按深度排列。
pub(crate) struct PickerRow {
    pub(crate) entry: DirectoryEntry,
    pub(crate) depth: usize,
    pub(crate) expanded: bool,
}

/// 一个已展开目录的子内容（访达语义：子级按父深度 +1 缩进）。
struct ExpansionState {
    listing: DirectoryListing,
}

pub(crate) struct PickerSession {
    request_path: String,
    kind: PickerKind,
    accept_label: Option<String>,
    filters: Vec<FilterRule>,
    active_filter: PickerFilter,
    directory: PathBuf,
    listing: DirectoryListing,
    /// 已展开目录：路径 → 子内容。导航切换目录时整体清空。
    expansions: HashMap<PathBuf, ExpansionState>,
    /// 扁平化可见行（根 + 展开子级），`refresh_rows` 是唯一重建点。
    rows: Vec<PickerRow>,
    selection: Vec<usize>,
    selection_anchor: Option<usize>,
    /// 指针悬停的行索引；导航/刷新后必须重新验证或清空。
    hovered_index: Option<usize>,
    name_input: String,
    /// SaveFile 二次确认目标：非 None 时确认按钮变为"覆盖"。
    overwrite_target: Option<PathBuf>,
    /// 地址栏编辑草稿；None = 面包屑态。
    address_edit: Option<String>,
    /// 目录访问历史；`history_position` 指向当前目录（侧键后退/前进）。
    history: Vec<PathBuf>,
    history_position: usize,
    reply: Option<oneshot::Sender<PickerResolution>>,
}

impl PickerSession {
    pub(crate) fn new(
        invocation_spec: &PickerRequestSpec,
        request_path: String,
        start_directory: PathBuf,
        reply: oneshot::Sender<PickerResolution>,
    ) -> Self {
        let PickerRequestSpec {
            kind,
            accept_label,
            filters,
            active_filter,
            ..
        } = invocation_spec;
        let active_filter = active_filter
            .and_then(|index| filters.get(index))
            .map(|rule| PickerFilter::from_rules(vec![rule.clone()]))
            .unwrap_or_else(|| {
                if filters.is_empty() {
                    PickerFilter::unconstrained()
                } else {
                    PickerFilter::from_rules(filters[..1].to_vec())
                }
            });
        let name_input = match kind {
            PickerKind::SaveFile { default_name } => default_name.clone().unwrap_or_default(),
            _ => String::new(),
        };
        PickerSession {
            request_path,
            kind: kind.clone(),
            accept_label: accept_label.clone(),
            filters: filters.clone(),
            active_filter,
            directory: start_directory.clone(),
            listing: DirectoryListing::Pending,
            expansions: HashMap::new(),
            rows: Vec::new(),
            selection: Vec::new(),
            selection_anchor: None,
            hovered_index: None,
            name_input,
            overwrite_target: None,
            address_edit: None,
            history: vec![start_directory],
            history_position: 0,
            reply: Some(reply),
        }
    }

    pub(crate) fn request_path(&self) -> &str {
        &self.request_path
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn listing(&self) -> &DirectoryListing {
        &self.listing
    }

    pub(crate) fn rows(&self) -> &[PickerRow] {
        &self.rows
    }

    pub(crate) fn selection(&self) -> &[usize] {
        &self.selection
    }

    pub(crate) fn hovered_index(&self) -> Option<usize> {
        self.hovered_index
    }

    pub(crate) fn address_edit(&self) -> Option<&str> {
        self.address_edit.as_deref()
    }

    pub(crate) fn filters(&self) -> &[FilterRule] {
        &self.filters
    }

    pub(crate) fn active_filter_label(&self) -> String {
        self.active_filter.label()
    }

    pub(crate) fn kind(&self) -> &PickerKind {
        &self.kind
    }

    pub(crate) fn name_input(&self) -> &str {
        &self.name_input
    }

    pub(crate) fn overwrite_target(&self) -> Option<&Path> {
        self.overwrite_target.as_deref()
    }

    /// 确认按钮文案：调用方 accept_label 优先，缺省按模式定。
    pub(crate) fn accept_button_label(&self) -> String {
        if let Some(label) = &self.accept_label {
            return label.clone();
        }
        match &self.kind {
            PickerKind::OpenFile { .. } => "打开".to_string(),
            PickerKind::SaveFile { .. } => {
                if self.overwrite_target.is_some() {
                    "覆盖".to_string()
                } else {
                    "保存".to_string()
                }
            }
        }
    }

    /// 确认按钮是否可点。
    pub(crate) fn can_confirm(&self) -> bool {
        match &self.kind {
            PickerKind::OpenFile { .. } => !self.selection.is_empty(),
            PickerKind::SaveFile { .. } => !self.name_input.trim().is_empty(),
        }
    }

    /// 初始进入扫描。
    pub(crate) fn begin(&mut self) -> SessionEffect {
        SessionEffect::ScanDirectory(self.directory.clone())
    }

    pub(crate) fn update(&mut self, message: SessionMessage) -> SessionEffect {
        match message {
            SessionMessage::ScanReady(result) => {
                self.apply_scan(*result);
                SessionEffect::None
            }
            SessionMessage::EntryClicked { index, ctrl, shift } => {
                self.click_entry(index, ctrl, shift);
                SessionEffect::None
            }
            SessionMessage::EntryDoubleClicked { index } => self.activate_entry(index),
            SessionMessage::EntryExpandToggled { index } => self.toggle_expansion(index),
            SessionMessage::NavigateUp => self.navigate_up(),
            SessionMessage::NavigateBack => self.navigate_back(),
            SessionMessage::NavigateForward => self.navigate_forward(),
            SessionMessage::BreadcrumbActivated { ancestor } => self.navigate_breadcrumb(ancestor),
            SessionMessage::FilterSelected { rule } => {
                if let Some(rule) = self.filters.get(rule) {
                    self.active_filter = PickerFilter::from_rules(vec![rule.clone()]);
                    self.refresh_rows();
                }
                SessionEffect::None
            }
            SessionMessage::NameInputChanged(text) => {
                self.name_input = text;
                // 改名后覆盖确认失效，需对新名字重新判定。
                self.overwrite_target = None;
                SessionEffect::None
            }
            SessionMessage::ConfirmPressed => self.confirm(),
            SessionMessage::DismissPressed => self.finish_cancelled(),
            SessionMessage::OverwriteDeclined => {
                self.overwrite_target = None;
                SessionEffect::None
            }
            SessionMessage::FilterSelectionIgnored => SessionEffect::None,
            SessionMessage::EntryHovered { index } => {
                // 悬停索引必须落在当前行列表内，防止刷新后的越界高亮。
                self.hovered_index = index.filter(|&index| index < self.rows.len());
                SessionEffect::None
            }
            SessionMessage::SelectAllPressed => {
                self.select_all();
                SessionEffect::None
            }
            SessionMessage::AddressEditingStarted => {
                // 面包屑态才进入编辑；编辑态重复点击输入框不重置草稿。
                if self.address_edit.is_none() {
                    self.address_edit = Some(self.directory.to_string_lossy().into_owned());
                }
                SessionEffect::None
            }
            SessionMessage::AddressEditChanged(text) => {
                if self.address_edit.is_some() {
                    self.address_edit = Some(text);
                }
                SessionEffect::None
            }
            SessionMessage::AddressEditingSubmitted => self.submit_address_edit(),
            SessionMessage::AddressEditingCancelled => {
                self.address_edit = None;
                SessionEffect::None
            }
        }
    }

    /// 窗口被外部关闭（用户点 X 或 Request.Close）：未回复则视为取消。
    pub(crate) fn window_closed(mut self) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(PickerResolution::Cancelled);
        }
    }

    fn apply_scan(&mut self, result: DirectoryScanResult) {
        let DirectoryScanResult { directory, outcome } = result;
        if directory == self.directory {
            match outcome {
                Ok(scan) => {
                    self.listing = DirectoryListing::Ready(scan.entries);
                    self.selection.clear();
                    self.selection_anchor = None;
                }
                Err(details) => {
                    self.listing = DirectoryListing::Failed(details);
                    self.selection.clear();
                    self.selection_anchor = None;
                }
            }
            self.refresh_rows();
            return;
        }
        // 展开节点扫描：结果按路径回填；失败的展开自动收起。
        if let Some(state) = self.expansions.get_mut(&directory) {
            match outcome {
                Ok(scan) => state.listing = DirectoryListing::Ready(scan.entries),
                Err(_) => {
                    self.expansions.remove(&directory);
                }
            }
            self.refresh_rows();
        }
        // 既非根也非展开目标（已收起/已导航）：丢弃迟到结果。
    }

    /// 扁平化行列表的唯一重建点：根条目与展开子级在此统一过过滤规则，
    /// 选中/悬停索引在行数变化后重新验证。
    fn refresh_rows(&mut self) {
        self.rows.clear();
        if let DirectoryListing::Ready(entries) = &self.listing {
            let allowed: Vec<DirectoryEntry> = entries
                .iter()
                .filter(|entry| self.is_allowed(entry))
                .cloned()
                .collect();
            self.append_rows(&allowed, 0);
        }
        self.selection.retain(|&index| index < self.rows.len());
        self.hovered_index = self.hovered_index.filter(|&index| index < self.rows.len());
    }

    fn is_allowed(&self, entry: &DirectoryEntry) -> bool {
        self.active_filter
            .entry_allowed(&entry.name.to_string_lossy(), entry.kind, entry.is_hidden)
    }

    fn append_rows(&mut self, entries: &[DirectoryEntry], depth: usize) {
        for entry in entries {
            let expanded = self.expansions.contains_key(&entry.path);
            self.rows.push(PickerRow {
                entry: entry.clone(),
                depth,
                expanded,
            });
            if !expanded {
                continue;
            }
            let children =
                self.expansions
                    .get(&entry.path)
                    .and_then(|state| match &state.listing {
                        DirectoryListing::Ready(children) => Some(
                            children
                                .iter()
                                .filter(|child| self.is_allowed(child))
                                .cloned()
                                .collect::<Vec<_>>(),
                        ),
                        DirectoryListing::Pending | DirectoryListing::Failed(_) => None,
                    });
            if let Some(children) = children {
                self.append_rows(&children, depth + 1);
            }
        }
    }

    fn click_entry(&mut self, index: usize, ctrl: bool, shift: bool) {
        let multiple = matches!(self.kind, PickerKind::OpenFile { multiple: true, .. });
        match self.kind {
            PickerKind::SaveFile { .. } => {
                // SaveFile：文件点击=取其名；目录不进选中集。
                if let Some(row) = self.rows.get(index) {
                    if row.entry.kind == FileKind::File {
                        self.name_input = row.entry.name.to_string_lossy().into_owned();
                        self.overwrite_target = None;
                    }
                }
                return;
            }
            PickerKind::OpenFile { directory, .. } => {
                if let Some(row) = self.rows.get(index) {
                    let selectable = if directory {
                        row.entry.kind == FileKind::Directory
                    } else {
                        row.entry.kind == FileKind::File
                    };
                    if !selectable {
                        return;
                    }
                }
            }
        }
        if shift && multiple {
            if let Some(anchor) = self.selection_anchor {
                let (lo, hi) = (anchor.min(index), anchor.max(index));
                self.selection = (lo..=hi).collect();
            } else {
                self.selection = vec![index];
                self.selection_anchor = Some(index);
            }
        } else if ctrl && multiple {
            if let Some(position) = self.selection.iter().position(|&i| i == index) {
                self.selection.remove(position);
            } else {
                self.selection.push(index);
                self.selection.sort_unstable();
            }
            self.selection_anchor = Some(index);
        } else {
            self.selection = vec![index];
            self.selection_anchor = Some(index);
        }
    }

    fn activate_entry(&mut self, index: usize) -> SessionEffect {
        let Some(row) = self.rows.get(index) else {
            return SessionEffect::None;
        };
        if row.entry.kind == FileKind::Directory {
            return self.begin_navigation(row.entry.path.clone());
        }
        // 文件双击 = 选中并立即确认（桌面选择器惯例）。
        match self.kind {
            PickerKind::OpenFile {
                multiple: false, ..
            } => {
                self.selection = vec![index];
                self.confirm()
            }
            PickerKind::OpenFile { multiple: true, .. } => {
                if !self.selection.contains(&index) {
                    self.selection.push(index);
                    self.selection.sort_unstable();
                }
                SessionEffect::None
            }
            PickerKind::SaveFile { .. } => {
                self.name_input = row.entry.name.to_string_lossy().into_owned();
                self.overwrite_target = None;
                SessionEffect::None
            }
        }
    }

    /// 目录行的展开开关（访达列表语义）：加载中忽略重复点击；再次
    /// 点击收起。展开请求通过 `SessionEffect::ScanDirectory` 发出，
    /// 结果由 `apply_scan` 按路径回填。
    fn toggle_expansion(&mut self, index: usize) -> SessionEffect {
        let Some(row) = self.rows.get(index) else {
            return SessionEffect::None;
        };
        if row.entry.kind != FileKind::Directory {
            return SessionEffect::None;
        }
        let path = row.entry.path.clone();
        if let Some(state) = self.expansions.get(&path) {
            if matches!(state.listing, DirectoryListing::Pending) {
                return SessionEffect::None;
            }
            self.expansions.remove(&path);
            self.refresh_rows();
            return SessionEffect::None;
        }
        self.expansions.insert(
            path.clone(),
            ExpansionState {
                listing: DirectoryListing::Pending,
            },
        );
        self.refresh_rows();
        SessionEffect::ScanDirectory(path)
    }

    /// 进入目录：派生状态（选中、锚点、悬停、展开、地址编辑）的统一
    /// 清理点。历史栈由调用方负责记录。
    fn enter_directory(&mut self, directory: PathBuf) -> SessionEffect {
        self.directory = directory;
        self.listing = DirectoryListing::Pending;
        self.expansions.clear();
        self.rows.clear();
        self.selection.clear();
        self.selection_anchor = None;
        self.hovered_index = None;
        self.address_edit = None;
        SessionEffect::ScanDirectory(self.directory.clone())
    }

    /// 导航到新目录：记录历史（截断前进分支）并进入。
    fn begin_navigation(&mut self, directory: PathBuf) -> SessionEffect {
        if directory == self.directory {
            return SessionEffect::None;
        }
        self.history.truncate(self.history_position + 1);
        self.history.push(directory.clone());
        self.history_position = self.history.len() - 1;
        self.enter_directory(directory)
    }

    fn navigate_back(&mut self) -> SessionEffect {
        if self.history_position == 0 {
            return SessionEffect::None;
        }
        self.history_position -= 1;
        let target = self.history[self.history_position].clone();
        self.enter_directory(target)
    }

    fn navigate_forward(&mut self) -> SessionEffect {
        if self.history_position + 1 >= self.history.len() {
            return SessionEffect::None;
        }
        self.history_position += 1;
        let target = self.history[self.history_position].clone();
        self.enter_directory(target)
    }

    fn navigate_up(&mut self) -> SessionEffect {
        let Some(parent) = self.directory.parent().map(Path::to_path_buf) else {
            return SessionEffect::None;
        };
        if parent == self.directory {
            return SessionEffect::None;
        }
        self.begin_navigation(parent)
    }

    fn navigate_breadcrumb(&mut self, ancestor: usize) -> SessionEffect {
        let ancestors = breadcrumb_chain(&self.directory);
        let Some(target) = ancestors.get(ancestor) else {
            return SessionEffect::None;
        };
        self.begin_navigation(target.clone())
    }

    /// 提交地址栏编辑：空草稿=取消；绝对路径直接用，相对路径拼当前
    /// 目录（与主程序 `path_from_address_draft` 同规则）。
    fn submit_address_edit(&mut self) -> SessionEffect {
        let Some(draft) = self.address_edit.take() else {
            return SessionEffect::None;
        };
        let trimmed = draft.trim();
        if trimmed.is_empty() {
            return SessionEffect::None;
        }
        let path = PathBuf::from(trimmed);
        let target = if path.is_absolute() {
            path
        } else {
            self.directory.join(path)
        };
        self.begin_navigation(target)
    }

    /// Ctrl+A：仅 OpenFile 多选模式响应；按模式的可选类型圈定范围。
    fn select_all(&mut self) {
        let PickerKind::OpenFile {
            multiple: true,
            directory,
            ..
        } = &self.kind
        else {
            return;
        };
        self.selection = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                if *directory {
                    row.entry.kind == FileKind::Directory
                } else {
                    row.entry.kind == FileKind::File
                }
            })
            .map(|(index, _)| index)
            .collect();
        self.selection_anchor = self.selection.first().copied();
    }

    fn confirm(&mut self) -> SessionEffect {
        if self.overwrite_target.is_some() {
            return self.confirm_overwrite();
        }
        match &self.kind {
            PickerKind::OpenFile { .. } => {
                let mut paths: Vec<PathBuf> = self
                    .selection
                    .iter()
                    .filter_map(|&index| self.rows.get(index))
                    .map(|row| row.entry.path.clone())
                    .collect();
                paths.sort_unstable();
                paths.dedup();
                if paths.is_empty() {
                    return SessionEffect::None;
                }
                self.reply_confirmed(paths)
            }
            PickerKind::SaveFile { .. } => {
                let name = self.name_input.trim();
                if name.is_empty() {
                    return SessionEffect::None;
                }
                let target = self.directory.join(name);
                if target.exists() {
                    self.overwrite_target = Some(target);
                    return SessionEffect::None;
                }
                self.reply_confirmed(vec![target])
            }
        }
    }

    fn confirm_overwrite(&mut self) -> SessionEffect {
        let Some(target) = self.overwrite_target.take() else {
            return SessionEffect::None;
        };
        self.reply_confirmed(vec![target])
    }

    fn reply_confirmed(&mut self, paths: Vec<PathBuf>) -> SessionEffect {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(PickerResolution::Confirmed(paths.clone()));
        }
        SessionEffect::Confirmed(paths)
    }

    fn finish_cancelled(&mut self) -> SessionEffect {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(PickerResolution::Cancelled);
        }
        SessionEffect::Dismissed
    }
}

/// 面包屑链：`/home/u/Downloads` → `["/", "/home", "/home/u", "/home/u/Downloads"]`。
pub(crate) fn breadcrumb_chain(directory: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut current = Some(directory);
    while let Some(path) = current {
        chain.push(path.to_path_buf());
        current = path.parent();
    }
    chain.reverse();
    chain
}

/// 执行目录扫描（上层在 iced Task 里调用）。
pub(crate) async fn scan_listing(directory: PathBuf) -> Result<DirectoryScanOutcome, String> {
    let scan = scan_directory(&directory, ScanOptions::default())
        .await
        .map_err(|error| error.to_string())?;
    Ok(DirectoryScanOutcome {
        entries: scan.entries,
    })
}

#[cfg(test)]
mod tests;
