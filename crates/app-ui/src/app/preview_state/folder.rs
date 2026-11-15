//! 文件夹预览里双击/回车打开一项：文件夹（和能当文件夹浏览的压缩包）让主窗口进入，
//! 其余用默认程序打开；独立预览窗口里打开后关掉预览窗口。不走 `activate_path`：它按
//! 主列表里的条目查类型，预览里深处的条目不在主列表。

use std::path::PathBuf;

use file_core::{is_supported_archive_path, FileKind};
use iced::Task;

use super::super::right_preview_panel::PreviewLoadSurface;
use super::FileBrowser;
use crate::commands::open_file_command;
use crate::model::{Message, NavigationMode};

impl FileBrowser {
    pub(in crate::app) fn open_folder_preview_entry(
        &mut self,
        path: PathBuf,
        kind: FileKind,
    ) -> Task<Message> {
        let close = match self.preview_engine.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => self.close_preview_window(),
            PreviewLoadSurface::RightDockedPanel => Task::none(),
        };
        let open = if kind == FileKind::Directory || is_supported_archive_path(&path) {
            self.navigate_to(path, NavigationMode::RecordHistory)
        } else {
            open_file_command(path, self.terminal_emulator)
        };
        Task::batch([close, open])
    }
}
