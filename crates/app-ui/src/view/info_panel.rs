use iced::widget::{container, mouse_area, row, scrollable, text, Column};
use iced::{Element, Length};

use crate::app::info_panel::InfoPanelMessage;
use crate::app::info_panel_rows::InfoValueRow;
use crate::app::right_preview_panel::PANEL_CONTENT_PADDING;
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::formatting::format_middle_ellipsized_text;
use crate::info_panel::InfoField;
use crate::keyed_rows::keyed_rows;
use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::selectable_text;
use bennu_theme::typography::{BOLD, CAPTION_SIZE};

/// 标签那一列的宽度；值在右边，长了折行。
const LABEL_WIDTH: f32 = 84.0;
const TITLE_MAX_CHARS: usize = 64;
/// 标题和各行之间的竖向间距。
const ROW_SPACING: f32 = 6.0;

pub(crate) fn info_field_label(field: InfoField) -> String {
    match field {
        InfoField::Type => tr!("Type"),
        InfoField::Size => tr!("Size"),
        InfoField::DiskSize => tr!("Size on disk"),
        InfoField::Location => tr!("Location"),
        InfoField::LinkTarget => tr!("Points to"),
        InfoField::Modified => tr!("Modified"),
        InfoField::Created => tr!("Created"),
        InfoField::Accessed => tr!("Accessed"),
        InfoField::StatusChanged => tr!("Status Changed"),
        InfoField::Permissions => tr!("Permissions"),
        InfoField::Owner => tr!("Owner"),
        InfoField::Details => tr!("Details"),
        InfoField::Pages => tr!("Pages"),
    }
}

/// 信息面板：单个条目是名字 + 字段表；多选是个数和大小；没选中是当前文件夹。右键多一段
/// 「显示的信息」（面板里自己吃掉右键，外面的停靠区菜单收不到）。文字都在状态里
/// （`InfoPanelState::rows`），这里只画。
pub(super) fn info_panel_content(browser: &FileBrowser) -> Element<'_, Message> {
    let rows = browser.info_panel.rows();
    let body = match rows.title() {
        None => Column::new().push(readable_text(tr!("Select an item to see its details"))),
        Some(name) => Column::new().push(title(name)).push(keyed_rows(
            rows.values().iter().map(|row| (row.key, info_row(row))),
        )),
    };
    let content = container(scrollable(body.width(Length::Fill)))
        .padding(iced::Padding {
            top: 8.0,
            bottom: 8.0,
            left: PANEL_CONTENT_PADDING,
            right: PANEL_CONTENT_PADDING,
        })
        .width(Length::Fill)
        .height(Length::Fill);
    mouse_area(content)
        .on_right_press(Message::InfoPanel(InfoPanelMessage::MenuRequested))
        .into()
}

fn title(name: &str) -> Element<'static, Message> {
    text(format_middle_ellipsized_text(name, TITLE_MAX_CHARS))
        .font(BOLD)
        .into()
}

/// 标签是普通文字，值是可选中的文字。行间距做成每行上面的留白而不是列的间距：`keyed_rows`
/// 自己是一整块，不带间距。
fn info_row(row: &InfoValueRow) -> Element<'_, Message> {
    let key = row.key;
    row![
        container(
            readable_text(row.label.clone())
                .size(CAPTION_SIZE)
                .style(|theme| iced::widget::text::Style {
                    color: Some(muted_text_color(theme)),
                })
        )
        .width(Length::Fixed(LABEL_WIDTH)),
        selectable_text(&row.value, move |action| {
            Message::InfoPanel(InfoPanelMessage::ValueAction {
                row_key: key,
                action,
            })
        }),
    ]
    .spacing(8)
    .padding(iced::Padding {
        top: ROW_SPACING,
        ..iced::Padding::ZERO
    })
    .width(Length::Fill)
    .into()
}
