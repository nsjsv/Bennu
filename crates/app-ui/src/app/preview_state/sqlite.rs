//! SQLite 预览交互已迁 bennu-preview 的 PreviewEngine（engine/sqlite.rs，
//! impl 块逐字节保真）；此处保留同名薄转发：Task 返回值经
//! `.map(Message::Preview)` 包装，update.rs 调用点零改动。
//! active_sqlite_preview_mut / clear_sqlite_preview /
//! update_sqlite_tables_resize_drag 的签名不含宿主消息，经 FileBrowser
//! 的 Deref 垫片直接命中引擎同名方法，无需转发。

use iced::Task;

use super::FileBrowser;
use crate::model::{Message, SqlitePreviewMessage};

impl FileBrowser {
    pub(in crate::app) fn handle_sqlite_preview_message(
        &mut self,
        message: SqlitePreviewMessage,
    ) -> Task<Message> {
        self.preview_engine
            .handle_sqlite_preview_message(message)
            .map(Message::Preview)
    }

    /// 拖动起点的横坐标来自宿主指针状态（引擎不持有宿主输入）。
    pub(in crate::app) fn start_sqlite_tables_resize_drag(&mut self) -> Task<Message> {
        self.preview_engine
            .start_sqlite_tables_resize_drag(self.cursor_position.x)
            .map(Message::Preview)
    }

    pub(in crate::app) fn finish_sqlite_tables_resize_drag(&mut self) -> Task<Message> {
        self.preview_engine
            .finish_sqlite_tables_resize_drag()
            .map(Message::Preview)
    }
}
