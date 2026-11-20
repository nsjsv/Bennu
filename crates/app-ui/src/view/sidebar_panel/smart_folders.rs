//! 侧栏的“智能文件夹”一组：存下的搜索（`f:` 的写法），点开就是那次搜索，右键移除。一个都没有时
//! 不显示这一组。

use iced::alignment::Vertical;
use iced::widget::{button, container, mouse_area, Column};
use iced::{Element, Length};

use bennu_theme::icons::IconSymbol;

use super::{sidebar_label, sidebar_row_button_style, sidebar_section_label};
use super::{SIDEBAR_ROW_HEIGHT, SIDEBAR_ROW_PADDING_X, SIDEBAR_ROW_SPACING};
use crate::app::FileBrowser;
use crate::model::Message;
use crate::view::IconTone;

pub(super) fn smart_folders_section(browser: &FileBrowser) -> Option<Element<'_, Message>> {
    let folders = &browser.user_config().smart_folders;
    if folders.is_empty() {
        return None;
    }
    let mut section = Column::new()
        .spacing(SIDEBAR_ROW_SPACING)
        .push(sidebar_section_label(tr!("Smart Folders")));
    for query in folders {
        section = section.push(smart_folder_row(query));
    }
    Some(section.into())
}

fn smart_folder_row(query: &str) -> Element<'_, Message> {
    let item = button(
        container(sidebar_label(IconSymbol::Search, query, IconTone::Normal))
            .height(Length::Fill)
            .align_y(Vertical::Center),
    )
    .on_press(Message::SmartFolderOpened(query.to_owned()))
    .padding([0.0, SIDEBAR_ROW_PADDING_X])
    .height(Length::Fixed(SIDEBAR_ROW_HEIGHT))
    .width(Length::Fill)
    .style(sidebar_row_button_style(false));
    mouse_area(item)
        .on_right_press(Message::SmartFolderRightClicked(query.to_owned()))
        .into()
}
