//! 超级输入框的浮层：上面是模式按钮和输入框，下面是结果行，命令模式再往下是输出。行和选中都由模型
//! 算好，这里只画。

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use iced::widget::{button, container, mouse_area, row, Column, Space};
use iced::{Alignment, Element, Font, Length, Theme};

use crate::app::{super_box_input_id, FileBrowser, SuperBoxMessage};
use crate::formatting::{format_middle_ellipsized_text, format_system_time};
use crate::model::Message;
use crate::super_box::{
    read_history, BoxMode, BoxRow, FindStatus, NewEntry, RunEnd, RunState, SuperBox,
};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{normal_button, text_field, tooltip_bubble};
use bennu_theme::icons::IconSymbol;
use bennu_theme::styles::{
    context_menu_highlighted_item_button_style, context_menu_item_button_style, context_menu_style,
};
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;

use super::{themed_icon, IconTone, MENU_ICON_SIZE};
use crate::appearance::muted_text_color;

pub(super) const SUPER_BOX_WIDTH: f32 = 560.0;
/// 框的上沿离内容区顶部的距离。
pub(super) const SUPER_BOX_TOP: f32 = 12.0;
const PATH_MAX_CHARS: usize = 64;
/// 输出不超过这么多行就直接排开，再多就放进滚动区。
const OUTPUT_LINES_WITHOUT_SCROLLING: usize = 12;
const OUTPUT_SCROLL_HEIGHT: f32 = 240.0;

fn message(message: SuperBoxMessage) -> Message {
    Message::SuperBox(message)
}

/// 画一行要知道的主窗口情况。
struct RowContext<'a> {
    home: &'a Path,
    /// 命令在哪个文件夹里执行。
    folder: Option<&'a Path>,
    /// 主窗口里选中了几项（命令模式说明它们会作为参数）。
    selected_items: usize,
}

pub(super) fn super_box_panel<'a>(
    browser: &'a FileBrowser,
    super_box: &'a SuperBox,
) -> Element<'a, Message> {
    let mode = super_box.mode();
    let context = RowContext {
        home: &browser.home_dir,
        folder: browser.tab().current_directory(),
        selected_items: browser.tab().selected_paths.len(),
    };
    let mode_button = iced::widget::tooltip(
        normal_button(readable_text(mode.label())).on_press(message(SuperBoxMessage::ModePressed)),
        tooltip_bubble(tr!("Tab switches the mode")),
        iced::widget::tooltip::Position::Bottom,
    );
    let mut input = text_field(&placeholder(mode), super_box.input())
        .id(super_box_input_id())
        .on_input(|text| message(SuperBoxMessage::InputChanged(text)))
        .width(Length::Fill);
    if mode == BoxMode::Shell {
        input = input.font(Font::MONOSPACE);
    }
    let mut content = Column::new()
        .push(
            row![mode_button, input]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .spacing(6);
    if let Some(notice) = super_box.notice() {
        content = content.push(error_text(notice.to_owned()));
    }
    let rows = super_box.rows();
    if rows.is_empty() {
        content = content.push(muted(empty_hint(super_box)));
    }
    for (index, row) in rows.iter().enumerate() {
        content = content.push(result_row(
            super_box,
            index,
            row,
            super_box.selected() == Some(index),
            &context,
        ));
    }
    // 命令模式：这次跑的那条的结果；没跑过时，选中的那条历史上次的结果。
    if let Some(run) = super_box.run() {
        content = content.push(match &run.state {
            RunState::Running => running_line(&run.command),
            RunState::Ended(end) => end_panel(tr!("Finished, exit status 0"), end),
        });
    } else if let (BoxMode::Shell, Some(BoxRow::History(entry))) = (mode, super_box.selected_row())
    {
        if let Some(past) = browser.super_box_commands.past(read_history(entry).1) {
            let at = UNIX_EPOCH + Duration::from_millis(past.at_ms.max(0) as u64);
            content = content.push(end_panel(
                tr!("Last run %1", format_system_time(at)),
                &past.end,
            ));
        }
    }
    container(content)
        .padding(8)
        .width(Length::Fixed(SUPER_BOX_WIDTH))
        .style(context_menu_style)
        .into()
}

fn placeholder(mode: BoxMode) -> String {
    match mode {
        BoxMode::Search => tr!("Find a feature, action, setting or favorite"),
        BoxMode::Files => tr!("Find files in the index"),
        BoxMode::Path => tr!("Go to a path; ~ is your home folder"),
        BoxMode::New => tr!("Name of the new file; end it with / for a folder"),
        BoxMode::Shell => tr!("A command for your shell"),
    }
}

/// 没有结果时的一行：框是空的就说这个模式能做什么，打了字就说正在找或者没找到。
fn empty_hint(super_box: &SuperBox) -> String {
    let mode = super_box.mode();
    if !super_box.input().trim().is_empty() {
        return match super_box.find_status() {
            FindStatus::Searching => tr!("Searching…"),
            FindStatus::Idle | FindStatus::Unavailable(_) => tr!("Nothing matches"),
        };
    }
    match mode {
        BoxMode::Files => tr!(
            "Type what to look for. type:, time: and size: narrow it down, for example type:images time:7d size:>10m. Ctrl+Enter opens the search page."
        ),
        BoxMode::Shell => tr!(
            "Type a command. It runs in this folder with your shell, the selected items are its arguments (\"$@\", in fish $argv). Ctrl+Enter runs it in the terminal panel."
        ),
        BoxMode::Search => tr!(
            "Type to find what to do. Tab switches to going to a path or making a new item; g: and n: do it too."
        ),
        BoxMode::Path => tr!("Type a path, or start with / or ~ in search."),
        BoxMode::New => tr!("Type a name; end it with / to make a folder"),
    }
}

/// 一行的内容：图标、名字（命令用等宽字）、名字后面的灰字、最右边的灰字。
struct RowParts {
    icon: Option<IconSymbol>,
    title: String,
    monospace: bool,
    detail: Option<String>,
    hint: Option<String>,
}

impl RowParts {
    fn plain(icon: IconSymbol, title: String) -> Self {
        Self {
            icon: Some(icon),
            title,
            monospace: false,
            detail: None,
            hint: None,
        }
    }
}

fn row_parts(super_box: &SuperBox, row: &BoxRow, context: &RowContext<'_>) -> RowParts {
    match row {
        BoxRow::Entry(entry_index) => {
            let entry = super_box.entry(*entry_index);
            let mut hint = entry.kind.label();
            if let Some(key) = &entry.key_hint {
                hint = format!("{hint} · {key}");
            }
            RowParts {
                icon: None,
                title: entry.title.clone(),
                monospace: false,
                detail: entry.detail.clone(),
                hint: Some(hint),
            }
        }
        BoxRow::Found(found_index) => {
            let hit = super_box.found(*found_index);
            let icon = match hit.kind {
                file_search::SearchFileKind::Directory => IconSymbol::Folder,
                _ => IconSymbol::File,
            };
            RowParts {
                detail: hit
                    .path
                    .parent()
                    .map(|parent| short_path(parent, context.home)),
                ..RowParts::plain(icon, hit.display_name.clone())
            }
        }
        BoxRow::Run(command) => {
            let mut hint = match context.folder {
                Some(folder) => tr!("In %1", short_path(folder, context.home)),
                None => String::new(),
            };
            if context.selected_items > 0 {
                hint = format!(
                    "{hint} · {}",
                    tr!("%1 selected as arguments", context.selected_items)
                );
            }
            RowParts {
                monospace: true,
                hint: Some(hint),
                ..RowParts::plain(IconSymbol::Terminal, format!("$ {command}"))
            }
        }
        // 模式按钮已经说明了是哪种，前缀不再写。
        BoxRow::History(entry) => RowParts {
            monospace: super_box.mode() == BoxMode::Shell,
            ..RowParts::plain(IconSymbol::RotateCcw, read_history(entry).1.to_owned())
        },
        BoxRow::JumpTo(path) => RowParts::plain(
            IconSymbol::ArrowRight,
            tr!("Go to %1", short_path(path, context.home)),
        ),
        BoxRow::JumpSuggestion(path) => {
            RowParts::plain(IconSymbol::Folder, short_path(path, context.home))
        }
        BoxRow::Create(NewEntry::File(name)) => {
            RowParts::plain(IconSymbol::Plus, tr!("New file “%1”", name))
        }
        BoxRow::Create(NewEntry::Folder(name)) => {
            RowParts::plain(IconSymbol::Plus, tr!("New folder “%1”", name))
        }
        BoxRow::Problem(reason) => RowParts::plain(IconSymbol::TriangleAlert, reason.clone()),
    }
}

fn result_row<'a>(
    super_box: &'a SuperBox,
    index: usize,
    row: &'a BoxRow,
    selected: bool,
    context: &RowContext<'_>,
) -> Element<'a, Message> {
    if let BoxRow::Problem(reason) = row {
        return error_text(reason.clone());
    }
    let parts = row_parts(super_box, row, context);
    let mut line = row![].spacing(8).align_y(Alignment::Center);
    line = match parts.icon {
        Some(icon) => line.push(themed_icon(icon, IconTone::Normal, MENU_ICON_SIZE)),
        None => line.push(Space::new().width(Length::Fixed(MENU_ICON_SIZE))),
    };
    let title = readable_text(parts.title);
    let title = if parts.monospace {
        title.font(Font::MONOSPACE)
    } else {
        title
    };
    let mut label = row![title].spacing(8).align_y(Alignment::Center);
    if let Some(detail) = parts.detail {
        label = label.push(muted(format_middle_ellipsized_text(
            &detail,
            PATH_MAX_CHARS,
        )));
    }
    line = line.push(container(label).width(Length::Fill));
    if let Some(hint) = parts.hint.filter(|hint| !hint.is_empty()) {
        line = line.push(muted(hint));
    }
    let style = if selected {
        context_menu_highlighted_item_button_style()
    } else {
        context_menu_item_button_style()
    };
    mouse_area(
        button(line)
            .on_press(message(SuperBoxMessage::RowPressed(index)))
            .padding([6, 8])
            .width(Length::Fill)
            .style(style),
    )
    .on_enter(message(SuperBoxMessage::RowHovered(index)))
    .into()
}

/// 正在跑：命令和“停止”。
fn running_line(command: &str) -> Element<'static, Message> {
    container(
        row![
            readable_text(tr!("Running %1", command))
                .font(Font::MONOSPACE)
                .width(Length::Fill),
            normal_button(readable_text(tr!("Stop")))
                .on_press(message(SuperBoxMessage::StopPressed)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .into()
}

/// 跑完的结果：标题一行（这次跑完 / 上次的时间），下面是输出的末尾或失败原因。
fn end_panel(heading: String, end: &RunEnd) -> Element<'_, Message> {
    let mut panel = Column::new().push(muted(heading)).spacing(4);
    match end {
        RunEnd::Failed(reason) => panel = panel.push(error_text(reason.clone())),
        RunEnd::Finished(output) if !output.text.trim().is_empty() => {
            // 借用框里的字，不每帧复制；输出里常有中文文件名，按复杂文字排版。
            let text = iced::widget::text(output.text.as_str())
                .shaping(iced::widget::text::Shaping::Advanced)
                .size(CAPTION_SIZE)
                .font(Font::MONOSPACE);
            // 短的直接排开；长的放进固定高度的滚动区，停在最后面（看末尾）。
            panel = if output.lines <= OUTPUT_LINES_WITHOUT_SCROLLING {
                panel.push(text)
            } else {
                panel.push(
                    iced::widget::scrollable(text)
                        .anchor_bottom()
                        .height(Length::Fixed(OUTPUT_SCROLL_HEIGHT))
                        .width(Length::Fill),
                )
            };
        }
        RunEnd::Finished(_) => {}
    }
    container(panel).padding([4, 8]).into()
}

/// 主目录下的路径写成 `~/…`，太长时从中间省略。
fn short_path(path: &Path, home: &Path) -> String {
    let shown = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    format_middle_ellipsized_text(&shown, PATH_MAX_CHARS)
}

fn muted(text: String) -> Element<'static, Message> {
    readable_text(text)
        .size(CAPTION_SIZE)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        })
        .into()
}

fn error_text(text: String) -> Element<'static, Message> {
    container(
        readable_text(text)
            .size(CAPTION_SIZE)
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(ui_colors(theme).error),
            }),
    )
    .padding([4, 8])
    .into()
}
