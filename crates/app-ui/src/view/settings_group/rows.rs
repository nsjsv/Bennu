//! 卡片行的骨架：每种行一个函数，统一内边距、最矮高度、标题与附注的排法。
//! 行的种类是不同概念（开关、输入、跳转……），所以拆成不同函数，不用种类参数。

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, Column, Row, Space};
use iced::{Alignment, Element, Length, Padding, Theme};

use crate::model::Message;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{
    destructive_button_style, icon_button, on_escape, segmented_choice_row, switch, text_field,
    SegmentedChoice,
};
use bennu_theme::icons::{rotated_chevron_right_view, themed_icon, IconSymbol, IconTone};
use bennu_theme::metrics::{ROW_MIN_HEIGHT, ROW_PADDING_X, ROW_PADDING_Y};
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;

use super::{card_row_button_style, RowNote, SettingsRow};
use crate::appearance::muted_text_color;

const ROW_ICON_SIZE: f32 = 14.0;
const LABEL_SPACING: f32 = 2.0;
const ROW_CONTENT_SPACING: f32 = 12.0;

/// 输入行：上面小字标题，下面是输入框（框的左边缘与其它行的标题对齐）；
/// 草稿和已保存值不同时框右边出现 ✓。
/// 点别处草稿保留；在输入框里按 Esc 恢复成已保存的值。
pub(in crate::view) struct EntryRow<'a> {
    pub(in crate::view) title: String,
    /// 已生效的值；“添加”一类的输入行是空串。
    pub(in crate::view) saved: String,
    pub(in crate::view) draft: &'a str,
    pub(in crate::view) placeholder: String,
    pub(in crate::view) on_input: Box<dyn Fn(String) -> Message + 'a>,
    pub(in crate::view) submit: Message,
    pub(in crate::view) note: Option<RowNote>,
    /// 值后面的单位（“MiB”）。
    pub(in crate::view) unit: Option<String>,
    /// 最右边的图标按钮（删除、选择目录）。
    pub(in crate::view) trailing: Vec<Element<'a, Message>>,
}

pub(in crate::view) fn switch_row(
    title: String,
    note: Option<RowNote>,
    is_on: bool,
    message: Message,
) -> SettingsRow<'static> {
    clickable(
        row![labels(title, note), switch(is_on)]
            .spacing(ROW_CONTENT_SPACING)
            .align_y(Alignment::Center),
        message,
    )
}

/// 右边放一个控件（下拉框、按钮、图标按钮）的行。
pub(in crate::view) fn control_row<'a>(
    title: String,
    note: Option<RowNote>,
    control: impl Into<Element<'a, Message>>,
) -> SettingsRow<'a> {
    framed(
        row![labels(title, note), control.into()]
            .spacing(ROW_CONTENT_SPACING)
            .align_y(Alignment::Center),
    )
}

/// 分段选择放在标题下面占整行。
pub(in crate::view) fn choice_row(
    title: String,
    note: Option<RowNote>,
    choices: Vec<SegmentedChoice<Message>>,
) -> SettingsRow<'static> {
    framed(
        column![labels(title, note), segmented_choice_row(choices)]
            .spacing(8)
            .padding(Padding::default().top(4.0).bottom(4.0)),
    )
}

pub(in crate::view) fn entry_row(entry: EntryRow<'_>) -> SettingsRow<'_> {
    let EntryRow {
        title,
        saved,
        draft,
        placeholder,
        on_input,
        submit,
        note,
        unit,
        trailing,
    } = entry;
    let dirty = entry_is_dirty(&saved, draft);
    let revert = on_input(saved);
    let field = text_field(&placeholder, draft)
        .on_input(on_input)
        .on_submit(submit.clone())
        .width(Length::Fill);

    // 单位、✓ 和图标按钮都跟在输入框那一行，与框对齐。
    let mut field_line = Row::new()
        .push(on_escape(field, revert))
        .spacing(6)
        .align_y(Alignment::Center);
    if let Some(unit) = unit {
        field_line = field_line.push(caption(unit, muted_text_color));
    }
    if dirty {
        field_line = field_line.push(save_button(submit));
    }
    for element in trailing {
        field_line = field_line.push(element);
    }
    let mut value = column![caption(title, muted_text_color), field_line]
        .spacing(4)
        .width(Length::Fill);
    if let Some(note) = note {
        value = value.push(note_text(note));
    }
    framed(value)
}

pub(in crate::view) fn navigation_row(
    title: String,
    note: Option<RowNote>,
    message: Message,
) -> SettingsRow<'static> {
    trailing_icon_row(title, note, IconSymbol::ChevronRight, message)
}

/// 打开外部链接的行。
pub(in crate::view) fn link_row(
    title: String,
    note: Option<RowNote>,
    message: Message,
) -> SettingsRow<'static> {
    trailing_icon_row(title, note, IconSymbol::Link, message)
}

/// 整行就是一个按钮；`None` = 暂时不可点（比如操作正在进行）。
pub(in crate::view) fn action_row(
    title: String,
    note: Option<RowNote>,
    message: Option<Message>,
) -> SettingsRow<'static> {
    styled_button_row(labels(title, note), message, card_row_button_style())
}

/// 二次确认态的危险操作：整行红底。
pub(in crate::view) fn destructive_action_row(
    title: String,
    note: Option<RowNote>,
    message: Option<Message>,
) -> SettingsRow<'static> {
    styled_button_row(labels(title, note), message, destructive_button_style)
}

/// 只有标题和附注的说明行（诊断结论、状态说明）。
pub(in crate::view) fn label_row(title: String, note: Option<RowNote>) -> SettingsRow<'static> {
    framed(labels(title, note))
}

/// 左边标题，右边灰色的值。值长（属性窗口里的路径）时在右边那块里折行、需要时从字中间断，
/// 标题不让位也不被挤成一列。
pub(in crate::view) fn value_row(title: String, value: String) -> SettingsRow<'static> {
    framed(
        row![
            readable_text(title).wrapping(Wrapping::None),
            readable_text(value)
                .size(CAPTION_SIZE)
                .width(Length::Fill)
                .align_x(Horizontal::Right)
                .wrapping(Wrapping::WordOrGlyph)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text_color(theme)),
                }),
        ]
        .spacing(ROW_CONTENT_SPACING)
        .align_y(Alignment::Center),
    )
}

/// 可折叠的行：头部整行可点；折叠时头部右边是摘要，展开时下面是内容。
pub(in crate::view) fn expander_row<'a>(
    title: String,
    summary: impl Into<Element<'a, Message>>,
    toggle: Message,
    body: Option<Element<'a, Message>>,
) -> SettingsRow<'a> {
    let rotation = if body.is_some() { 90.0 } else { 0.0 };
    let mut header = row![
        rotated_chevron_right_view(rotation, ROW_ICON_SIZE)
            .style(bennu_theme::icons::icon_tone_style(IconTone::Normal)),
        readable_text(title).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if body.is_none() {
        header = header.push(summary.into());
    }
    let SettingsRow(header) = clickable(header, toggle);
    let Some(body) = body else {
        return SettingsRow(header);
    };
    SettingsRow(
        column![
            header,
            container(body)
                .padding(Padding {
                    top: 0.0,
                    right: ROW_PADDING_X,
                    bottom: ROW_PADDING_Y * 2.0,
                    left: ROW_PADDING_X,
                })
                .width(Length::Fill),
        ]
        .into(),
    )
}

/// 整行一段说明：空状态、加载中、出错原因。
pub(in crate::view) fn notice_row(note: RowNote) -> SettingsRow<'static> {
    let text = match note {
        RowNote::Hint(text) => readable_text(text)
            .width(Length::Fill)
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }),
        RowNote::Error(text) => readable_text(text).width(Length::Fill).style(error_text),
    };
    framed(text)
}

/// 自带布局的内容（草图定稿的编辑器、配色网格）：只套统一的行内边距。
pub(in crate::view) fn custom_row<'a>(content: impl Into<Element<'a, Message>>) -> SettingsRow<'a> {
    framed(content)
}

fn save_button(submit: Message) -> Element<'static, Message> {
    icon_button(themed_icon(
        IconSymbol::Check,
        IconTone::Normal,
        ROW_ICON_SIZE,
    ))
    .on_press(submit)
    .into()
}

fn trailing_icon_row(
    title: String,
    note: Option<RowNote>,
    icon: IconSymbol,
    message: Message,
) -> SettingsRow<'static> {
    clickable(
        row![
            labels(title, note),
            themed_icon(icon, IconTone::Normal, ROW_ICON_SIZE),
        ]
        .spacing(ROW_CONTENT_SPACING)
        .align_y(Alignment::Center),
        message,
    )
}

fn labels(title: String, note: Option<RowNote>) -> Column<'static, Message> {
    let mut labels = column![readable_text(title).width(Length::Fill)]
        .spacing(LABEL_SPACING)
        .width(Length::Fill);
    if let Some(note) = note {
        labels = labels.push(note_text(note));
    }
    labels
}

fn note_text(note: RowNote) -> Element<'static, Message> {
    match note {
        RowNote::Hint(text) => caption(text, muted_text_color),
        RowNote::Error(text) => readable_text(text)
            .size(CAPTION_SIZE)
            .width(Length::Fill)
            .style(error_text)
            .into(),
    }
}

fn caption(text: String, color: fn(&Theme) -> iced::Color) -> Element<'static, Message> {
    readable_text(text)
        .size(CAPTION_SIZE)
        .style(move |theme: &Theme| iced::widget::text::Style {
            color: Some(color(theme)),
        })
        .into()
}

fn error_text(theme: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(ui_colors(theme).error),
    }
}

/// 行里放一根撑子：单行的行也至少 `ROW_MIN_HEIGHT` 高，有没有控件都一样。
fn with_min_height<'a>(content: impl Into<Element<'a, Message>>) -> Row<'a, Message> {
    row![
        container(content)
            .width(Length::Fill)
            .align_y(Vertical::Center),
        row_height_strut(),
    ]
    .align_y(Alignment::Center)
}

/// 撑子宽 1 而不是 0：iced 的 Row::push 把宽或高为 0 的子元素当“空”直接丢掉
/// （`Size::is_void`），0 宽的撑子根本进不了布局，行就塌回文字高度。
fn row_height_strut() -> Space {
    Space::new()
        .width(Length::Fixed(1.0))
        .height(Length::Fixed(ROW_MIN_HEIGHT - 2.0 * ROW_PADDING_Y))
}

fn framed<'a>(content: impl Into<Element<'a, Message>>) -> SettingsRow<'a> {
    framed_with_padding(content, Padding::from([ROW_PADDING_Y, ROW_PADDING_X]))
}

fn framed_with_padding<'a>(
    content: impl Into<Element<'a, Message>>,
    padding: Padding,
) -> SettingsRow<'a> {
    SettingsRow(
        container(with_min_height(content))
            .padding(padding)
            .width(Length::Fill)
            .into(),
    )
}

fn clickable<'a>(content: impl Into<Element<'a, Message>>, message: Message) -> SettingsRow<'a> {
    styled_button_row(content, Some(message), card_row_button_style())
}

fn styled_button_row<'a>(
    content: impl Into<Element<'a, Message>>,
    message: Option<Message>,
    style: fn(&Theme, button::Status) -> button::Style,
) -> SettingsRow<'a> {
    SettingsRow(
        button(with_min_height(content))
            .on_press_maybe(message)
            .padding([ROW_PADDING_Y, ROW_PADDING_X])
            .width(Length::Fill)
            .style(style)
            .into(),
    )
}

/// 草稿和已保存值不同才算“没保存”。
fn entry_is_dirty(saved: &str, draft: &str) -> bool {
    saved != draft
}

#[cfg(test)]
mod tests {
    use super::{entry_is_dirty, row_height_strut};
    use iced::advanced::Widget;

    #[test]
    fn row_height_strut_survives_row_push() {
        let strut = row_height_strut();
        let hint = Widget::<crate::model::Message, iced::Theme, iced::Renderer>::size_hint(&strut);
        assert!(!hint.is_void(), "iced Row drops void children");
    }

    #[test]
    fn untouched_entry_is_clean() {
        assert!(!entry_is_dirty("53317", "53317"));
        assert!(!entry_is_dirty("", ""));
    }

    #[test]
    fn edited_entry_is_dirty_until_it_matches_again() {
        assert!(entry_is_dirty("53317", "5331"));
        assert!(entry_is_dirty("", "/data"));
    }
}
