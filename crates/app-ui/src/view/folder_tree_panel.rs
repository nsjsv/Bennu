use iced::widget::{button, container, mouse_area, row, scrollable, Column, Space};
use iced::{Alignment, Element, Length, Theme};

use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use crate::app::folder_tree::{folder_tree_scroll_id, FolderTreeMessage};
use crate::app::FileBrowser;
use crate::appearance::icon_svg_style;
use crate::file_drag_hit_test_bounds::FileDragHitTestMarker;
use crate::file_drag_hit_test_marker::track_file_drag_hit_test_marker;
use crate::folder_tree::{Expander, FolderRow, FOLDER_ROW_HEIGHT};
use crate::model::Message;
use bennu_theme::controls::full_name_bubble;
use bennu_theme::icons::{rotated_chevron_right_view, IconSymbol};
use bennu_theme::measured_text::measured_middle_ellipsized_text_with_hint;
use bennu_theme::styles::sidebar_item_button_style;
use bennu_theme::typography::BODY_SIZE;

const INDENT_PER_LEVEL: f32 = 14.0;
const EXPANDER_WIDTH: f32 = 18.0;
const EXPANDER_ICON_SIZE: f32 = 12.0;
const ROW_PADDING_X: f32 = 6.0;
const PANEL_PADDING: f32 = 6.0;
/// 视口上下各多画几行：滚得快时边上不露白。
const OVERSCAN_ROWS: usize = 4;

/// 文件夹树：只画视口里的那些行（上下用空白顶住总高度），展开很多层、成千上万个文件夹也
/// 一样快。当前目录那一行是选中样子，拖文件停在上面的那一行是悬停样子。右键是停靠区菜单
/// 多一项「只限主目录」。
pub(super) fn folder_tree_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let Some(tree) = &browser.folder_tree else {
        return Space::new().into();
    };
    let rows = tree.rows();
    let first = ((tree.scroll_offset / FOLDER_ROW_HEIGHT) as usize)
        .saturating_sub(OVERSCAN_ROWS)
        .min(rows.len());
    let shown = (tree.viewport_height / FOLDER_ROW_HEIGHT).ceil() as usize + OVERSCAN_ROWS * 2;
    let last = (first + shown).min(rows.len());
    let current = browser.tab().current_directory();
    let drop_target = browser.folder_tree_drop_target();
    let pane = browser.active_pane_id();

    let mut list = Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(first as f32 * FOLDER_ROW_HEIGHT)));
    for folder in &rows[first..last] {
        let selected = current == Some(folder.path);
        let drop_hovered = drop_target == Some(folder.path);
        list = list.push(folder_row(folder, selected, drop_hovered, pane));
    }
    list = list.push(Space::new().height(Length::Fixed(
        (rows.len() - last) as f32 * FOLDER_ROW_HEIGHT,
    )));
    let content = scrollable(container(list).padding([0.0, PANEL_PADDING]))
        .id(folder_tree_scroll_id())
        .on_scroll(|viewport| {
            Message::FolderTree(FolderTreeMessage::Scrolled {
                offset: viewport.absolute_offset().y,
                height: viewport.bounds().height,
            })
        })
        .width(Length::Fill)
        .height(Length::Fill);
    mouse_area(container(content).padding([PANEL_PADDING, 0.0]))
        .on_right_press(Message::FolderTree(FolderTreeMessage::MenuRequested))
        .into()
}

/// 一行：缩进、展开箭头（没有子文件夹时空着）、文件夹图标、名字（放不下中间省略，停一会儿
/// 显示全名）。点一下当前窗格跳过去，中键在新标签里开。
fn folder_row<'a>(
    folder: &FolderRow<'a>,
    selected: bool,
    drop_hovered: bool,
    pane: crate::model::BrowserPaneId,
) -> Element<'a, Message> {
    let path = folder.path.to_path_buf();
    let tone = if selected {
        IconTone::Selected
    } else {
        IconTone::Normal
    };
    let content = row![
        Space::new().width(Length::Fixed(folder.depth as f32 * INDENT_PER_LEVEL)),
        expander(folder.expander, &path),
        themed_icon(IconSymbol::Folder, tone, MENU_ICON_SIZE),
        measured_middle_ellipsized_text_with_hint(
            folder.name.to_owned(),
            BODY_SIZE,
            full_name_bubble(folder.name.to_owned()),
        ),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let item = button(content)
        .on_press(Message::NavigateTo(path.clone()))
        .padding([0.0, ROW_PADDING_X])
        .width(Length::Fill)
        .height(Length::Fixed(FOLDER_ROW_HEIGHT))
        .style(move |theme: &Theme, status| {
            // 拖文件停在上面的那一行和鼠标悬停画得一样。
            let status = if drop_hovered {
                button::Status::Hovered
            } else {
                status
            };
            sidebar_item_button_style(theme, status, selected)
        });
    let item = mouse_area(item)
        .on_middle_press(Message::OpenDirectoryFromMiddleClick(pane, path.clone()))
        .on_release(Message::DragSelectionFinished);
    track_file_drag_hit_test_marker(
        item,
        FileDragHitTestMarker::FolderTreeDirectory { directory: path },
    )
}

fn expander(expander: Expander, path: &std::path::Path) -> Element<'static, Message> {
    let rotation = match expander {
        Expander::Empty => {
            return Space::new().width(Length::Fixed(EXPANDER_WIDTH)).into();
        }
        Expander::Collapsed => 0.0,
        Expander::Expanded | Expander::Loading => 90.0,
    };
    mouse_area(
        container(rotated_chevron_right_view(rotation, EXPANDER_ICON_SIZE).style(icon_svg_style()))
            .center_x(Length::Fixed(EXPANDER_WIDTH))
            .center_y(Length::Fixed(FOLDER_ROW_HEIGHT)),
    )
    .on_press(Message::FolderTree(FolderTreeMessage::ExpanderPressed(
        path.to_path_buf(),
    )))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}
