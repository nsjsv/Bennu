//! SQLite 预览交互在 bennu-preview 的 PreviewEngine（engine/sqlite.rs）；
//! 拖动起点的横坐标来自宿主指针状态，引擎不持有宿主输入。

use iced::Task;

use super::FileBrowser;
use crate::model::Message;

impl FileBrowser {
    pub(in crate::app) fn start_sqlite_tables_resize_drag(&mut self) -> Task<Message> {
        self.preview_engine
            .start_sqlite_tables_resize_drag(self.cursor_or_origin().x)
            .map(Message::Preview)
    }

    pub(in crate::app) fn finish_sqlite_tables_resize_drag(&mut self) -> Task<Message> {
        self.preview_engine
            .finish_sqlite_tables_resize_drag()
            .map(Message::Preview)
    }
}
