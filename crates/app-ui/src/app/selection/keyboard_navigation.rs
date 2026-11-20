use std::path::{Path, PathBuf};

use bennu_preview::preview_loading::{classify_preview_path, PreviewPathKind};
use file_core::{is_supported_archive_path, FileKind};
use iced::Task;

use super::super::{
    global_error::GlobalErrorSource, right_preview_panel::PreviewLoadSurface, FileBrowser,
};
use crate::commands::{
    image_preview_dimensions_command, load_expanded_directory_command, open_file_command,
    open_terminal_command,
};
use crate::model::{
    BrowserLocation, DirectoryExpansionLoadContext, ExpandedDirectory, ExpandedDirectoryStatus,
    ImagePreviewViewport, Message, NavigationMode, PreviewState, PreviewWindowProfile,
};

impl FileBrowser {
    pub(crate) fn activate_selected_path(&mut self) -> Task<Message> {
        if !self.file_browser_content_shortcuts_enabled() {
            return Task::none();
        }
        let Some(path) = self.tab().selected.clone() else {
            return Task::none();
        };
        self.activate_path(path)
    }

    pub(crate) fn request_preview(&mut self) -> Task<Message> {
        // 空格 toggle 的对象是键盘选中项;悬停不作门控——纯键盘操作没有
        // hovered_entry,若沿用“悬停才开”会让键盘用户永远开不了预览。
        if self.standalone_preview_session_active()
            && self.preview_engine.preview_shown_path.as_deref() == self.tab().selected.as_deref()
        {
            self.context_menu = None;
            return self.close_preview_window();
        }

        self.open_preview()
    }

    /// Space toggle 的不变量：独立预览窗口会话正在活跃显示。preview 会话
    /// 状态与右侧停靠面板共享（面板会话同样置 preview 非空），单看
    /// preview.is_some() 会把面板加载误判成“独立窗口已显示”，导致面板
    /// 开着时按 Space 打不开独立窗口；补上窗口存在性即回到原语义。
    fn standalone_preview_session_active(&self) -> bool {
        self.preview_engine.preview_window.is_some() && self.preview_engine.preview.is_some()
    }

    pub(crate) fn open_path(&mut self, path: PathBuf) -> Task<Message> {
        self.context_menu = None;
        self.activate_path(path)
    }

    pub(crate) fn activate_path(&mut self, path: PathBuf) -> Task<Message> {
        if self.tab().location == BrowserLocation::Trash {
            if !self.tab().selected_paths.contains(&path) {
                self.select_path(path);
            } else if self.tab().selected.as_deref() != Some(path.as_path()) {
                self.focus_path(path);
            }
            return self.restore_selected();
        }
        if let Some(dropped) = self.drop_missing_search_result(&path) {
            return dropped;
        }

        match self.entry_kind(&path) {
            // 压缩包视作目录：真实目录中的归档条目 kind 是 File，但
            // 双击/Enter 的动作是进入浏览；解压入口统一收在右键菜单。
            Some(FileKind::Directory) => self.navigate_to(path, NavigationMode::RecordHistory),
            Some(_) | None if is_supported_archive_path(&path) => {
                self.navigate_to(path, NavigationMode::RecordHistory)
            }
            Some(_) | None => open_file_command(path, self.terminal_emulator),
        }
    }

    pub(crate) fn open_terminal_here(&mut self, directory: PathBuf) -> Task<Message> {
        self.context_menu = None;
        if self.tab().location == BrowserLocation::Trash {
            return Task::none();
        }
        open_terminal_command(directory, self.terminal_emulator)
    }

    pub(crate) fn open_column_for_directory(&mut self, path: PathBuf) -> Task<Message> {
        if !self.tab().location.expands_entries_in_place() {
            return Task::none();
        }

        if !self.entry_acts_as_directory(&path) {
            return Task::none();
        }
        self.set_deepest_open_column_directory(Some(path.clone()));
        let load = self.load_column(path);
        Task::batch([
            load,
            self.focus_latest_column(),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ])
    }

    /// 多栏视图里一路展开到 `folder`（它成为最后一栏），再选中 `selection`。`folder` 在当前第一栏
    /// 下面时第一栏不动；不在时从两者共同的上级开始（第一栏换成它）。要去的地方是点名的：
    /// 不显示隐藏文件时，路上的隐藏目录和选中的隐藏条目也列在各自的上一级栏里。
    pub(in crate::app) fn reveal_in_columns(
        &mut self,
        folder: PathBuf,
        selection: Option<PathBuf>,
    ) -> Task<Message> {
        let current = self.tab().current_directory().map(Path::to_path_buf);
        let root = match &current {
            Some(current) if folder.starts_with(current) => current.clone(),
            Some(current) => common_ancestor(current, &folder),
            None => folder.clone(),
        };
        let mut tasks = Vec::new();
        if current.as_ref() != Some(&root) {
            tasks.push(self.navigate_to(root.clone(), NavigationMode::RecordHistory));
        }
        // 先换点名再开栏：要重读的上级和新开的栏都按新规则列。
        tasks.push(self.reveal_path(selection.clone().unwrap_or_else(|| folder.clone())));
        let mut chain = folder
            .ancestors()
            .take_while(|directory| *directory != root)
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        chain.reverse();
        for directory in &chain {
            tasks.push(self.load_column(directory.clone()));
        }
        self.set_deepest_open_column_directory(chain.last().cloned());
        if let Some(selection) = selection {
            self.select_path(selection);
        }
        tasks.extend([
            self.focus_latest_column(),
            self.request_browser_session_save(),
            self.reveal_address_bar_current_segment(self.active_pane_id()),
        ]);
        Task::batch(tasks)
    }

    /// 让这一栏显示出来：已经有了就展开，没有就建一份“加载中”并去读。
    fn load_column(&mut self, path: PathBuf) -> Task<Message> {
        if let Some(expanded) = self.tab_mut().expanded_directories.get_mut(&path) {
            expanded.is_expanded = true;
            expanded.is_collapsing = false;
            expanded.animation_progress = 1.0;
            return Task::none();
        }

        let mut expanded = ExpandedDirectory {
            entries: Default::default(),
            directory_discovery: None,
            status: ExpandedDirectoryStatus::Loading,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 0,
            load_context: None,
            load_cancel: None,
            directory_order_phase: crate::model::DirectoryOrderPhase::Ready {
                field: file_core::SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        };
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            DirectoryExpansionLoadContext::BrowserTree {
                pane_id: self.active_pane_id(),
            },
            path.clone(),
            &mut expanded,
        );
        self.tab_mut().expanded_directories.insert(path, expanded);
        load_expanded_directory_command(
            request,
            self.listing_options(self.active_pane_id()),
            cancellation,
        )
    }

    pub(super) fn open_preview(&mut self) -> Task<Message> {
        self.context_menu = None;
        // 包内成员按空格完全无动作:虚拟路径读不了文件,加载必然失败
        // 并弹「could not read image」全局错误;按只读浏览语义静默忽略。
        // 归档文件本身是 BrowsingRoot 但仍是真实文件,归档预览照常发起。
        if self.tab().selected.as_deref().is_some_and(|path| {
            file_core::archive_path_identity(path) == file_core::ArchivePathIdentity::InsideArchive
        }) {
            return Task::none();
        }
        // 空格驱动的加载会话面向独立窗口:异步回流时窗口尺寸/聚焦动作
        // 全部照旧;面板发起的会话经 sync_right_preview_panel 另行标注。
        self.preview_engine.preview_load_surface = PreviewLoadSurface::StandaloneWindow;
        // 内部“先关后开”会复位固定状态；切换预览内容时必须保留用户设定的固定。
        let pinned = self.preview_engine.preview_window_pinned;
        let close_window_command = self.close_preview_window();

        // 悬停在条目上但没有选中项：无操作。
        let Some(path) = self.tab().selected.clone() else {
            return Task::none();
        };
        close_window_command.chain(self.present_preview_target(path, pinned))
    }

    /// 打开预览的后半段：登记目标、过门禁（不能预览、太大、远程先下载），再按类型加载。
    /// 空格先关窗再走这里；←/→ 换图在原来的窗口里直接走这里。
    pub(in crate::app) fn present_preview_target(
        &mut self,
        path: PathBuf,
        pinned: bool,
    ) -> Task<Message> {
        let kind = self.entry_kind(&path).unwrap_or(FileKind::Other);
        self.preview_engine.preview_shown_path = Some(path.clone());
        if kind == FileKind::File {
            // 门禁放在远程下载与大小检查之前：不可预览的远程文件不值得下载，
            // 也不应误报“文件太大”。
            let Some(classification) =
                classify_preview_path(&path, &self.user_config.preview_extension_rules)
            else {
                self.preview_engine.preview_window_pinned = pinned;
                return self.show_unpreviewable_file_preview();
            };
            if let Some(command) = self.reject_oversized_file_preview(&path) {
                return command;
            }
            // 文本按需读挂载路径，不先整个下载。
            if classification.needs_local_copy() && self.path_is_remote_mount(&path) {
                self.preview_engine.preview_window_pinned = pinned;
                return self.start_remote_preview_download(path);
            }
        }

        self.preview_engine.preview_window_pinned = pinned;
        self.open_preview_for_resolved_path(path, kind)
    }

    /// QuickLook 式占位：不可预览类型弹预览窗口显示错误，与“文件太大”同一表现。
    fn show_unpreviewable_file_preview(&mut self) -> Task<Message> {
        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.preview_engine.clear_preview();
        self.preview_engine.preview = Some(PreviewState::Error(bennu_localization::tr!(
            "No preview available for this file type."
        )));
        window_command
    }

    pub(in crate::app) fn open_preview_for_resolved_path(
        &mut self,
        path: PathBuf,
        kind: FileKind,
    ) -> Task<Message> {
        // 新预览会话不复用上一个文件的缩放/平移；同会话内的
        // 缩略图→原图替换不经过这里，视口得以保留。
        self.preview_engine.preview_image_viewport = ImagePreviewViewport::default();
        if kind == FileKind::File {
            // 预览会话的诞生边界：远程下载回流与本地入口共用这一道门，
            // 文本兜底同样受白名单约束。
            let classification =
                classify_preview_path(&path, &self.user_config.preview_extension_rules);
            let Some(classification) = classification else {
                return self.show_unpreviewable_file_preview();
            };
            return self.start_classified_preview(path, classification);
        }

        // 目录与未识别类型不走文件分类：目录预览由 load_preview 展开，
        // Symlink/Other 的错误仍由 load_preview 边界返回。
        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.preview_engine.clear_preview();
        self.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
        self.retract_global_error(GlobalErrorSource::Preview);
        let max_file_bytes = self.preview_file_size_limit_for(&path);
        Task::batch([
            window_command,
            self.preview_engine
                .load_preview(
                    path,
                    kind,
                    self.user_config.preview_extension_rules.clone(),
                    self.options.clone(),
                    max_file_bytes,
                )
                .map(Message::Preview),
            self.request_browser_session_save(),
        ])
    }

    fn start_classified_preview(
        &mut self,
        path: PathBuf,
        classification: PreviewPathKind,
    ) -> Task<Message> {
        match classification {
            PreviewPathKind::Document => self.start_document_preview(path),
            // 是不是动图由尺寸探测看内容决定（accept_image_preview_dimensions）。
            PreviewPathKind::Image => {
                self.preview_engine.show_image_loading(path.clone());
                self.retract_global_error(GlobalErrorSource::Preview);
                let generation = self.preview_engine.next_original_image_preview_generation();
                Task::batch([
                    image_preview_dimensions_command(path, generation),
                    self.request_browser_session_save(),
                ])
            }
            PreviewPathKind::Video => {
                self.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
                self.retract_global_error(GlobalErrorSource::Preview);
                let max_file_bytes = self.preview_file_size_limit_for(&path);
                self.preview_engine
                    .load_preview(
                        path,
                        FileKind::File,
                        self.user_config.preview_extension_rules.clone(),
                        self.options.clone(),
                        max_file_bytes,
                    )
                    .map(Message::Preview)
            }
            PreviewPathKind::Audio => {
                let window_command =
                    self.preview_window_presentation_command(PreviewWindowProfile::Audio);
                self.preview_engine.clear_preview();
                self.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
                self.retract_global_error(GlobalErrorSource::Preview);
                let max_file_bytes = self.preview_file_size_limit_for(&path);
                Task::batch([
                    window_command,
                    self.preview_engine
                        .load_preview(
                            path,
                            FileKind::File,
                            self.user_config.preview_extension_rules.clone(),
                            self.options.clone(),
                            max_file_bytes,
                        )
                        .map(Message::Preview),
                    self.request_browser_session_save(),
                ])
            }
            PreviewPathKind::Archive | PreviewPathKind::Sqlite | PreviewPathKind::Text => {
                let window_command =
                    self.preview_window_presentation_command(PreviewWindowProfile::Regular);
                self.preview_engine.clear_preview();
                self.preview_engine.preview = Some(PreviewState::Loading(path.clone()));
                self.retract_global_error(GlobalErrorSource::Preview);
                let max_file_bytes = self.preview_file_size_limit_for(&path);
                Task::batch([
                    window_command,
                    self.preview_engine
                        .load_preview(
                            path,
                            FileKind::File,
                            self.user_config.preview_extension_rules.clone(),
                            self.options.clone(),
                            max_file_bytes,
                        )
                        .map(Message::Preview),
                    self.request_browser_session_save(),
                ])
            }
        }
    }

    fn reject_oversized_file_preview(&mut self, path: &Path) -> Option<Task<Message>> {
        let entry = self.entry_for_path(path)?;
        let max_bytes = self.preview_file_size_limit_for(path);
        let file_bytes = entry.metadata().len;
        if max_bytes == 0 || file_bytes <= max_bytes {
            return None;
        }

        let window_command =
            self.preview_window_presentation_command(PreviewWindowProfile::Regular);
        self.preview_engine.clear_preview();
        self.preview_engine.preview = Some(PreviewState::Error(
            bennu_preview::preview_loading::file_too_large_to_preview(file_bytes, max_bytes),
        ));
        Some(window_command)
    }
}

/// 两个路径最深的共同上级（绝对路径至少有 `/`）。
fn common_ancestor(left: &Path, right: &Path) -> PathBuf {
    left.ancestors()
        .find(|ancestor| right.starts_with(ancestor))
        .unwrap_or(Path::new("/"))
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use file_core::{DirectoryEntry, EntryMetadata, FileKind};

    use super::*;
    use crate::config;

    #[test]
    fn space_preview_opens_archive_root_but_not_inner_members() {
        // 回归:e2e5b29d 起 BrowsingRoot 被 `!= RealFile` 误拦,空格选中
        // 压缩包完全无反应。归档文件本身仍是真实文件,照常进预览;
        // 只有包内成员(InsideArchive)保持静默无动作。身份判定逐前缀
        // stat 真实文件系统,须用 tempdir 里的真 .zip(内容不校验)。
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("bundle.zip");
        std::fs::write(&archive, b"stub").unwrap();

        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.tab_mut().location = BrowserLocation::Directory(workspace.path().to_path_buf());
        browser.tab_mut().entries = vec![DirectoryEntry::new(
            archive.clone(),
            FileKind::File,
            EntryMetadata::default(),
            false,
            false,
            false,
        )]
        .into();
        browser.select_path(archive.clone());
        drop(browser.request_preview());

        assert!(
            matches!(browser.preview_engine.preview, Some(PreviewState::Loading(ref path)) if path == &archive),
            "归档文件本身应进入 Loading 预览会话"
        );

        // 包内成员:虚拟路径读不了文件,静默无动作,不改变预览状态。
        let (mut inner_browser, _) = FileBrowser::new(config::default_user_config());
        inner_browser.select_path(archive.join("inner.txt"));
        drop(inner_browser.request_preview());

        assert!(
            inner_browser.preview_engine.preview.is_none(),
            "包内成员按空格应保持静默无动作"
        );
    }
}
