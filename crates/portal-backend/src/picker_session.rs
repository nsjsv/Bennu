//! 每个选择窗口的核心状态机：目录导航、条目选择、SaveFile 命名与覆盖
//! 确认。不触碰 iced 类型，效果以 `SessionEffect` 输出由上层翻译执行，
//! 保证逻辑可脱离窗口环境单测。

use std::path::{Path, PathBuf};

use tokio::sync::oneshot;

use file_core::entry::{DirectoryEntry, FileKind};
use file_core::scan::{scan_directory, ScanOptions};

use crate::dbus_file_chooser::PickerResolution;
use crate::filter::PickerFilter;
use crate::picker_request::{FilterRule, PickerKind, PickerRequestSpec};

/// 目录内容加载状态。
pub(crate) enum DirectoryListing {
    Pending,
    Ready(Vec<DirectoryEntry>),
    Failed(String),
}

/// 状态机对外请求的效果。
pub(crate) enum SessionEffect {
    None,
    /// 需要扫描该目录（进入/上级/面包屑/初始）。
    ScanDirectory(PathBuf),
    /// 用户确认，携带选中路径。
    Confirmed(Vec<PathBuf>),
    /// 用户取消（Esc / 取消按钮）。
    Dismissed,
}

/// 视图事件。
#[derive(Debug, Clone)]
pub(crate) enum SessionMessage {
    ScanReady(Box<Result<DirectoryScanOutcome, String>>),
    EntryClicked { index: usize, ctrl: bool, shift: bool },
    EntryDoubleClicked { index: usize },
    NavigateUp,
    BreadcrumbActivated { ancestor: usize },
    FilterSelected { rule: usize },
    NameInputChanged(String),
    ConfirmPressed,
    DismissPressed,
    OverwriteDeclined,
    /// 下拉选择值不在已知规则中（视图与状态不一致的兜底，正常不可达）。
    FilterSelectionIgnored,
}

/// 扫描完成载荷（避免 session 依赖 file-core 错误类型细节）。
#[derive(Debug, Clone)]
pub(crate) struct DirectoryScanOutcome {
    pub(crate) path: PathBuf,
    pub(crate) entries: Vec<DirectoryEntry>,
}

pub(crate) struct PickerSession {
    request_path: String,
    kind: PickerKind,
    accept_label: Option<String>,
    filters: Vec<FilterRule>,
    active_filter: PickerFilter,
    directory: PathBuf,
    listing: DirectoryListing,
    /// 可见条目 = 扫描结果按当前过滤规则筛选。
    visible: Vec<DirectoryEntry>,
    selection: Vec<usize>,
    selection_anchor: Option<usize>,
    name_input: String,
    /// SaveFile 二次确认目标：非 None 时确认按钮变为"覆盖"。
    overwrite_target: Option<PathBuf>,
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
            visible: Vec::new(),
            selection: Vec::new(),
            selection_anchor: None,
            name_input,
            overwrite_target: None,
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

    pub(crate) fn visible(&self) -> &[DirectoryEntry] {
        &self.visible
    }

    pub(crate) fn selection(&self) -> &[usize] {
        &self.selection
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
    pub(crate) fn begin(self: &mut Self) -> SessionEffect {
        SessionEffect::ScanDirectory(self.directory.clone())
    }

    pub(crate) fn update(&mut self, message: SessionMessage) -> SessionEffect {
        match message {
            SessionMessage::ScanReady(outcome) => self.apply_scan(*outcome),
            SessionMessage::EntryClicked { index, ctrl, shift } => {
                self.click_entry(index, ctrl, shift);
                SessionEffect::None
            }
            SessionMessage::EntryDoubleClicked { index } => self.activate_entry(index),
            SessionMessage::NavigateUp => self.navigate_up(),
            SessionMessage::BreadcrumbActivated { ancestor } => {
                self.navigate_breadcrumb(ancestor)
            }
            SessionMessage::FilterSelected { rule } => {
                if let Some(rule) = self.filters.get(rule) {
                    self.active_filter = PickerFilter::from_rules(vec![rule.clone()]);
                    self.refresh_visible();
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
        }
    }

    /// 窗口被外部关闭（用户点 X 或 Request.Close）：未回复则视为取消。
    pub(crate) fn window_closed(mut self) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(PickerResolution::Cancelled);
        }
    }

    fn apply_scan(&mut self, outcome: Result<DirectoryScanOutcome, String>) -> SessionEffect {
        match outcome {
            Ok(scan) if scan.path == self.directory => {
                self.listing = DirectoryListing::Ready(scan.entries);
                self.selection.clear();
                self.selection_anchor = None;
                self.refresh_visible();
            }
            Ok(_) => {}
            Err(details) => {
                self.listing = DirectoryListing::Failed(details);
                self.selection.clear();
                self.refresh_visible();
            }
        }
        SessionEffect::None
    }

    fn refresh_visible(&mut self) {
        let rules_ok = self.active_filter.clone();
        if let DirectoryListing::Ready(entries) = &self.listing {
            self.visible = entries
                .iter()
                .filter(|entry| {
                    rules_ok.entry_allowed(
                        &entry.name.to_string_lossy(),
                        entry.kind,
                        entry.is_hidden,
                    )
                })
                .cloned()
                .collect();
        } else {
            self.visible.clear();
        }
        // 选中集必须在可见层内保持有效。
        self.selection.retain(|&index| index < self.visible.len());
    }

    fn click_entry(&mut self, index: usize, ctrl: bool, shift: bool) {
        let multiple = matches!(self.kind, PickerKind::OpenFile { multiple: true, .. });
        match self.kind {
            PickerKind::SaveFile { .. } => {
                // SaveFile：文件点击=取其名；目录不进选中集。
                if let Some(entry) = self.visible.get(index) {
                    if entry.kind == FileKind::File {
                        self.name_input = entry.name.to_string_lossy().into_owned();
                        self.overwrite_target = None;
                    }
                }
                return;
            }
            PickerKind::OpenFile { directory, .. } => {
                if let Some(entry) = self.visible.get(index) {
                    let selectable = if directory {
                        entry.kind == FileKind::Directory
                    } else {
                        entry.kind == FileKind::File
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
        // 选择变化后 SaveFile 不受影响；OpenFile 清空覆盖语义不适用。
    }

    fn activate_entry(&mut self, index: usize) -> SessionEffect {
        let Some(entry) = self.visible.get(index) else {
            return SessionEffect::None;
        };
        if entry.kind == FileKind::Directory {
            self.directory = entry.path.clone();
            self.listing = DirectoryListing::Pending;
            self.visible.clear();
            self.selection.clear();
            self.selection_anchor = None;
            SessionEffect::ScanDirectory(self.directory.clone())
        } else {
            // 文件双击 = 选中并立即确认（桌面选择器惯例）。
            match self.kind {
                PickerKind::OpenFile { multiple: false, .. } => {
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
                    self.name_input = entry.name.to_string_lossy().into_owned();
                    self.overwrite_target = None;
                    SessionEffect::None
                }
            }
        }
    }

    fn navigate_up(&mut self) -> SessionEffect {
        let Some(parent) = self.directory.parent().map(Path::to_path_buf) else {
            return SessionEffect::None;
        };
        if parent == self.directory {
            return SessionEffect::None;
        }
        self.directory = parent;
        self.listing = DirectoryListing::Pending;
        self.visible.clear();
        self.selection.clear();
        self.selection_anchor = None;
        SessionEffect::ScanDirectory(self.directory.clone())
    }

    fn navigate_breadcrumb(&mut self, ancestor: usize) -> SessionEffect {
        let ancestors = breadcrumb_chain(&self.directory);
        let Some(target) = ancestors.get(ancestor) else {
            return SessionEffect::None;
        };
        self.directory = target.clone();
        self.listing = DirectoryListing::Pending;
        self.visible.clear();
        self.selection.clear();
        self.selection_anchor = None;
        SessionEffect::ScanDirectory(self.directory.clone())
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
                    .filter_map(|&index| self.visible.get(index))
                    .map(|entry| entry.path.clone())
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
pub(crate) async fn scan_listing(
    directory: PathBuf,
) -> Result<DirectoryScanOutcome, String> {
    let scan = scan_directory(&directory, ScanOptions::default())
        .await
        .map_err(|error| error.to_string())?;
    Ok(DirectoryScanOutcome {
        path: scan.path,
        entries: scan.entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker_request::FilePattern;
    use std::fs;

    fn spec(kind: PickerKind, filters: Vec<FilterRule>) -> PickerRequestSpec {
        PickerRequestSpec {
            kind,
            accept_label: None,
            filters,
            active_filter: None,
            start_folder: None,
        }
    }

    fn session(kind: PickerKind) -> (PickerSession, oneshot::Receiver<PickerResolution>) {
        let (reply, receiver) = oneshot::channel();
        let base = tempfile::tempdir().unwrap();
        let session = PickerSession::new(
            &spec(kind.clone(), Vec::new()),
            "/req/test".to_string(),
            base.keep(),
            reply,
        );
        (session, receiver)
    }

    fn seeded_listing(session: &mut PickerSession, names: &[(&str, FileKind)]) {
        let entries = names
            .iter()
            .map(|&(name, kind)| {
                DirectoryEntry::new(
                    PathBuf::from(name),
                    kind,
                    file_core::entry::EntryMetadata::default(),
                    name.starts_with('.'),
                    false,
                    false,
                )
            })
            .collect();
        session.apply_scan(Ok(DirectoryScanOutcome {
            path: session.directory.clone(),
            entries,
        }));
    }

    #[test]
    fn breadcrumb_chain_lists_ancestors_root_first() {
        let chain = breadcrumb_chain(Path::new("/home/u/Downloads"));
        assert_eq!(chain.len(), 4);
        assert_eq!(chain[0], Path::new("/"));
        assert_eq!(chain[3], Path::new("/home/u/Downloads"));
    }

    #[test]
    fn single_click_selects_and_double_click_confirms_file() {
        let (mut session, receiver) = session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        seeded_listing(
            &mut session,
            &[("a.txt", FileKind::File), ("dir", FileKind::Directory)],
        );
        session.update(SessionMessage::EntryClicked { index: 0, ctrl: false, shift: false });
        assert_eq!(session.selection(), &[0]);

        let effect = session.update(SessionMessage::EntryDoubleClicked { index: 0 });
        assert!(matches!(effect, SessionEffect::Confirmed(_)));
        assert!(matches!(
            receiver.blocking_recv().unwrap(),
            PickerResolution::Confirmed(paths) if paths == vec![PathBuf::from("a.txt")]
        ));
    }

    #[test]
    fn directory_mode_rejects_file_selection() {
        let (mut session, _receiver) = session(PickerKind::OpenFile {
            multiple: false,
            directory: true,
        });
        seeded_listing(
            &mut session,
            &[("a.txt", FileKind::File), ("dir", FileKind::Directory)],
        );
        session.update(SessionMessage::EntryClicked { index: 0, ctrl: false, shift: false });
        assert!(session.selection().is_empty());
        session.update(SessionMessage::EntryClicked { index: 1, ctrl: false, shift: false });
        assert_eq!(session.selection(), &[1]);
    }

    #[test]
    fn ctrl_click_accumulates_selection() {
        let (mut session, _receiver) = session(PickerKind::OpenFile {
            multiple: true,
            directory: false,
        });
        seeded_listing(
            &mut session,
            &[
                ("a", FileKind::File),
                ("b", FileKind::File),
                ("c", FileKind::File),
            ],
        );
        session.update(SessionMessage::EntryClicked { index: 0, ctrl: true, shift: false });
        session.update(SessionMessage::EntryClicked { index: 2, ctrl: true, shift: false });
        assert_eq!(session.selection(), &[0, 2]);
        session.update(SessionMessage::EntryClicked { index: 0, ctrl: true, shift: false });
        assert_eq!(session.selection(), &[2]);
    }

    #[test]
    fn shift_click_selects_range() {
        let (mut session, _receiver) = session(PickerKind::OpenFile {
            multiple: true,
            directory: false,
        });
        seeded_listing(
            &mut session,
            &[
                ("a", FileKind::File),
                ("b", FileKind::File),
                ("c", FileKind::File),
                ("d", FileKind::File),
            ],
        );
        session.update(SessionMessage::EntryClicked { index: 1, ctrl: false, shift: false });
        session.update(SessionMessage::EntryClicked { index: 3, ctrl: false, shift: true });
        assert_eq!(session.selection(), &[1, 2, 3]);
    }

    #[test]
    fn save_file_requires_name_and_confirms_target() {
        let (mut session, receiver) = session(PickerKind::SaveFile {
            default_name: Some("报告.txt".to_string()),
        });
        assert_eq!(session.name_input(), "报告.txt");
        assert!(session.can_confirm());

        session.update(SessionMessage::NameInputChanged("   ".to_string()));
        assert!(!session.can_confirm());

        session.update(SessionMessage::NameInputChanged("新名字.md".to_string()));
        let effect = session.update(SessionMessage::ConfirmPressed);
        assert!(matches!(effect, SessionEffect::Confirmed(_)));
        assert!(matches!(
            receiver.blocking_recv().unwrap(),
            PickerResolution::Confirmed(paths) if paths.len() == 1 && paths[0].ends_with("新名字.md")
        ));
    }

    #[test]
    fn save_file_existing_target_requires_overwrite_step() {
        let existing = tempfile::tempdir().unwrap();
        let existing_file = existing.path().join("已存在.txt");
        fs::write(&existing_file, "x").unwrap();

        let (reply, receiver) = oneshot::channel();
        let mut session = PickerSession::new(
            &spec(
                PickerKind::SaveFile { default_name: Some("已存在.txt".to_string()) },
                Vec::new(),
            ),
            "/req/test".to_string(),
            existing.path().to_path_buf(),
            reply,
        );
        let effect = session.update(SessionMessage::ConfirmPressed);
        // 第一步：进入覆盖确认，不立即完成。
        assert!(matches!(effect, SessionEffect::None));
        assert_eq!(session.overwrite_target(), Some(existing_file.as_path()));
        assert_eq!(session.accept_button_label(), "覆盖");

        let effect = session.update(SessionMessage::ConfirmPressed);
        assert!(matches!(effect, SessionEffect::Confirmed(_)));
        assert!(matches!(
            receiver.blocking_recv().unwrap(),
            PickerResolution::Confirmed(paths) if paths == vec![existing_file]
        ));
    }

    #[test]
    fn save_file_entry_click_adopts_file_name() {
        let (mut session, _receiver) = session(PickerKind::SaveFile { default_name: None });
        seeded_listing(
            &mut session,
            &[("模板.txt", FileKind::File), ("dir", FileKind::Directory)],
        );
        session.update(SessionMessage::EntryClicked { index: 0, ctrl: false, shift: false });
        assert_eq!(session.name_input(), "模板.txt");
        // 目录点击不改名字
        session.update(SessionMessage::EntryClicked { index: 1, ctrl: false, shift: false });
        assert_eq!(session.name_input(), "模板.txt");
    }

    #[test]
    fn double_click_directory_navigates() {
        let (mut session, _receiver) = session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        let base = tempfile::tempdir().unwrap();
        let child = base.path().join("child");
        fs::create_dir(&child).unwrap();
        seeded_listing(
            &mut session,
            &[("child", FileKind::Directory), ("a", FileKind::File)],
        );
        // seeded listing 的路径是假名；改为真实路径验证导航。
        session.apply_scan(Ok(DirectoryScanOutcome {
            path: session.directory().to_path_buf(),
            entries: vec![DirectoryEntry::new(
                child.clone(),
                FileKind::Directory,
                file_core::entry::EntryMetadata::default(),
                false,
                false,
                false,
            )],
        }));
        let effect = session.update(SessionMessage::EntryDoubleClicked { index: 0 });
        assert!(matches!(effect, SessionEffect::ScanDirectory(target) if target == child));
    }

    #[test]
    fn filter_rule_hides_non_matching_files_but_keeps_directories() {
        let (reply, _receiver) = oneshot::channel();
        let base = tempfile::tempdir().unwrap();
        let filter = vec![FilterRule {
            name: "PNG".to_string(),
            patterns: vec![FilePattern::Glob("*.png".to_string())],
        }];
        let mut session = PickerSession::new(
            &spec(
                PickerKind::OpenFile { multiple: false, directory: false },
                filter,
            ),
            "/req/test".to_string(),
            base.keep(),
            reply,
        );
        seeded_listing(
            &mut session,
            &[
                ("a.png", FileKind::File),
                ("b.jpg", FileKind::File),
                ("dir", FileKind::Directory),
            ],
        );
        assert_eq!(session.visible().len(), 2);
        assert_eq!(session.visible()[0].name, "a.png");
        assert_eq!(session.visible()[1].name, "dir");
    }

    #[test]
    fn dismiss_cancels_reply() {
        let (mut session, receiver) = session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        let effect = session.update(SessionMessage::DismissPressed);
        assert!(matches!(effect, SessionEffect::Dismissed));
        assert!(matches!(
            receiver.blocking_recv().unwrap(),
            PickerResolution::Cancelled
        ));
    }

    #[test]
    fn external_window_close_without_reply_counts_as_cancel() {
        let (session, receiver) = session(PickerKind::OpenFile {
            multiple: false,
            directory: false,
        });
        session.window_closed();
        assert!(matches!(
            receiver.blocking_recv().unwrap(),
            PickerResolution::Cancelled
        ));
    }
}
