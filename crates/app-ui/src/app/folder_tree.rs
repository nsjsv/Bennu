//! 文件夹树面板接进程序：面板显示着才有树（关掉就整棵丢掉，不读也不看着）；`update` 出口跟着
//! 活动窗格的当前目录走；列表在后台线程读；展开的文件夹交给窗格同一套目录监视。

use std::path::{Path, PathBuf};

use iced::widget::scrollable::AbsoluteOffset;
use iced::Task;

use super::FileBrowser;
use crate::folder_tree::{tree_root, FolderNode, FolderTree, ListingRequest};
use crate::model::{DockMenuPanelSection, FileDropLayoutState, FileDropTarget, Message};
use crate::right_dock::DockPanel;

#[derive(Debug, Clone)]
pub(crate) enum FolderTreeMessage {
    Listed {
        directory: PathBuf,
        request: u64,
        /// None：读不了。
        folders: Option<Vec<FolderNode>>,
    },
    ExpanderPressed(PathBuf),
    Scrolled {
        offset: f32,
        height: f32,
    },
    MenuRequested,
    LimitToHomeToggled,
}

pub(crate) fn folder_tree_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("folder-tree")
}

impl FileBrowser {
    pub(super) fn handle_folder_tree_message(
        &mut self,
        message: FolderTreeMessage,
    ) -> Task<Message> {
        match message {
            FolderTreeMessage::Listed {
                directory,
                request,
                folders,
            } => {
                if let Some(tree) = self.folder_tree.as_mut() {
                    tree.accept_listing(&directory, request, folders);
                }
            }
            FolderTreeMessage::ExpanderPressed(directory) => {
                if let Some(request) = self
                    .folder_tree
                    .as_mut()
                    .and_then(|tree| tree.toggle(&directory))
                {
                    return list_folders(request);
                }
            }
            FolderTreeMessage::Scrolled { offset, height } => {
                if let Some(tree) = self.folder_tree.as_mut() {
                    tree.scroll_offset = offset;
                    tree.viewport_height = height;
                }
            }
            FolderTreeMessage::MenuRequested => {
                self.context_menu = Some(self.right_dock_context_menu(Some(
                    DockMenuPanelSection::FolderTree {
                        limit_to_home: self.user_config.folder_tree_limit_to_home,
                    },
                )));
            }
            FolderTreeMessage::LimitToHomeToggled => {
                self.context_menu = None;
                self.user_config.folder_tree_limit_to_home =
                    !self.user_config.folder_tree_limit_to_home;
                return self.persist_user_preferences_command();
            }
        }
        Task::none()
    }

    /// `update` 出口：面板没显示就丢掉整棵树；显示着就按当前目录定根、展开到它、隐藏文件开关
    /// 换了重列，当前目录那一行出来后滚到它。
    pub(super) fn sync_folder_tree(&mut self) -> Task<Message> {
        if !self.right_dock.layout().shows(DockPanel::FolderTree) {
            self.folder_tree = None;
            return Task::none();
        }
        let include_hidden = self.options.include_hidden;
        // 搜索结果、回收站、压缩包里面没有能跟的文件夹：树停在原来的地方。
        let followed = self
            .tab()
            .current_directory()
            .filter(|directory| {
                file_core::archive_path_identity(directory)
                    == file_core::ArchivePathIdentity::RealFile
            })
            .map(Path::to_path_buf);
        let limit_to_home = self.user_config.folder_tree_limit_to_home;
        let root = match (&followed, &self.folder_tree) {
            (Some(directory), _) => tree_root(directory, &self.home_dir, limit_to_home),
            (None, Some(tree)) => tree.root().to_path_buf(),
            (None, None) => tree_root(&self.home_dir, &self.home_dir, limit_to_home),
        };
        let tree = match &mut self.folder_tree {
            Some(tree) if tree.root() == root => tree,
            slot => slot.insert(FolderTree::new(root, include_hidden)),
        };
        let mut requests = Vec::from_iter(tree.list_root());
        if tree.include_hidden() != include_hidden {
            requests.extend(tree.relist_all(include_hidden));
        }
        if let Some(directory) = followed.filter(|directory| tree.current() != Some(directory)) {
            requests.extend(tree.follow(&directory));
        }
        let scroll = tree
            .take_reveal()
            .and_then(|row| tree.scroll_target_for_row(row))
            .map(|y| {
                iced::widget::operation::scroll_to(
                    folder_tree_scroll_id(),
                    AbsoluteOffset { x: 0.0, y },
                )
            });
        Task::batch(requests.into_iter().map(list_folders).chain(scroll))
    }

    /// 窗格同一套目录监视报来的变化：树看着这个文件夹时按它重列这一层。
    pub(super) fn accept_folder_tree_changes(
        &mut self,
        changes: &file_core::DirectoryEntryChanges,
    ) -> Task<Message> {
        self.folder_tree
            .as_mut()
            .and_then(|tree| tree.accept_changes(changes))
            .map_or_else(Task::none, list_folders)
    }

    /// 拖文件时指针停在树里的哪个文件夹上（画落点高亮）：从拖放会话现算，不另存一份状态。
    pub(crate) fn folder_tree_drop_target(&self) -> Option<&Path> {
        let session = self.file_drop_session.as_ref()?;
        let position = session.position?;
        let FileDropLayoutState::Ready {
            hit_test_bounds, ..
        } = &session.layout
        else {
            return None;
        };
        let Some(FileDropTarget::Directory(target)) = &session.hovered_target else {
            return None;
        };
        hit_test_bounds
            .folder_tree_directories
            .iter()
            .find(|(directory, row)| directory == target && row.contains(position))
            .map(|(directory, _)| directory.as_path())
    }

    /// 树要看着的文件夹（面板没显示时没有）。
    pub(super) fn folder_tree_watched_directories(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.folder_tree
            .iter()
            .flat_map(|tree| tree.watched_directories().map(Path::to_path_buf))
    }
}

fn list_folders(request: ListingRequest) -> Task<Message> {
    let ListingRequest {
        directory,
        request,
        include_hidden,
    } = request;
    Task::perform(
        async move {
            let reading = directory.clone();
            let folders =
                tokio::task::spawn_blocking(move || read_folders(&reading, include_hidden))
                    .await
                    .ok()
                    .flatten();
            (directory, folders)
        },
        move |(directory, folders)| {
            Message::FolderTree(FolderTreeMessage::Listed {
                directory,
                request,
                folders,
            })
        },
    )
}

/// 列一层的文件夹（指向文件夹的链接也算），按文件区按名字排的顺序。只看目录项的类型，不逐个
/// stat（链接除外），几万个文件的文件夹也快。
fn read_folders(directory: &Path, include_hidden: bool) -> Option<Vec<FolderNode>> {
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(directory).ok()?.flatten() {
        let name = entry.file_name();
        if !include_hidden && file_core::is_hidden_name(&name) {
            continue;
        }
        let is_folder = match entry.file_type() {
            Ok(kind) if kind.is_dir() => true,
            Ok(kind) if kind.is_symlink() => {
                std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir())
            }
            _ => false,
        };
        if is_folder {
            folders.push((name, entry.path()));
        }
    }
    folders.sort_by(|(left, _), (right, _)| file_core::compare_names(left, right));
    Some(
        folders
            .into_iter()
            .map(|(name, path)| FolderNode {
                name: name.to_string_lossy().into_owned(),
                path,
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "folder_tree_tests.rs"]
mod tests;
