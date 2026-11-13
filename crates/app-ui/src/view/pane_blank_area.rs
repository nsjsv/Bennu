use iced::widget::mouse_area;
use iced::Element;

use crate::app::panes::BrowserPaneView;
use crate::model::{BrowserLocation, Message};

/// 窗格内容空白处的指针响应。目录的空白处是拖放落点，右键出目录菜单；回收站和搜索结果
/// 不是落点，不接松开（拖动由窗口级的松开收尾）；回收站右键出回收站菜单，搜索结果的
/// 空白处没有可做的事。
pub(crate) fn pane_blank_area<'a>(
    pane: &BrowserPaneView<'a>,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let area = mouse_area(content)
        .on_press(Message::BlankAreaPressed(pane.id))
        .on_enter(Message::ColumnBrowserCursorEntered(pane.id))
        .on_exit(Message::ColumnBrowserCursorExited(pane.id));
    match pane.location {
        BrowserLocation::Directory(directory) => area
            .on_release(Message::DropTargetReleased(pane.id, directory.clone()))
            .on_right_press(Message::BlankAreaRightClicked(pane.id, directory.clone())),
        BrowserLocation::Trash => area.on_right_press(Message::TrashBlankAreaRightClicked(pane.id)),
        BrowserLocation::Search(_) => area,
    }
    .into()
}
