//! 超级输入框的浮层：上面是输入框（有模式时最前面是模式的胶囊），下面是结果行，命令模式再往下是输出。
//! 行、选中、建议都由模型算好，这里只画。

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use iced::widget::text::Wrapping;
use iced::widget::{button, container, mouse_area, rich_text, row, span, stack, text_input};
use iced::widget::{Column, Space};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Theme};

use crate::app::{super_box_input_id, FileBrowser, SuperBoxMessage};
use crate::formatting::{format_middle_ellipsized_text, format_system_time};
use crate::model::Message;
use crate::super_box::{
    read_history, BoxMode, BoxRow, Choices, Exit, FindStatus, NewEntry, RunEnd, RunState, SuperBox,
};
use crate::terminal_panel::session::TerminalSession;
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::{normal_button, text_field, text_field_style};
use bennu_theme::icons::IconSymbol;
use bennu_theme::metrics::{CONTROL_LINE_HEIGHT, CONTROL_PADDING_Y, FIELD_PADDING_X};
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
/// 输出不超过这么多行就直接排开，再多就放进滚动区。跑的时候显示最后这么多行。
const OUTPUT_LINES_WITHOUT_SCROLLING: usize = 12;
const OUTPUT_SCROLL_HEIGHT: f32 = 240.0;

/// 框的留白，和结果面板（输出所在）的左右留白。
const BOX_PADDING: f32 = 8.0;
const PANEL_PADDING: [f32; 2] = [4.0, 8.0];

/// 框里输出那一栏放得下几个等宽字：命令的终端就这么宽，输出在框里不再折行。
pub(crate) fn super_box_output_columns() -> usize {
    let width = SUPER_BOX_WIDTH - 2.0 * (BOX_PADDING + PANEL_PADDING[1]);
    (width / (CAPTION_SIZE * bennu_theme::font_families::monospace_advance_em())).floor() as usize
}

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
    let context = RowContext {
        home: &browser.home_dir,
        folder: browser.tab().current_directory(),
        selected_items: browser.tab().selected_paths.len(),
    };
    let mut content = Column::new().push(input_field(super_box)).spacing(6);
    if let Some(choices) = super_box.choices() {
        content = content.push(choice_strip(choices));
    }
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
            RunState::Running => {
                running_panel(&run.command, browser.hidden_terminal_session(run.session))
            }
            RunState::Ended(end) => end_panel(None, end),
        });
    } else if let Some((BoxMode::Shell, command)) =
        super_box.selected_row().and_then(|row| match row {
            BoxRow::History(entry) => Some(read_history(entry)),
            _ => None,
        })
    {
        if let Some(past) = browser.super_box_commands.past(command) {
            let at = UNIX_EPOCH + Duration::from_millis(past.at_ms.max(0) as u64);
            content = content.push(end_panel(
                Some(tr!("Last run %1", format_system_time(at))),
                &past.end,
            ));
        }
    }
    container(content)
        .padding(BOX_PADDING)
        .width(Length::Fixed(SUPER_BOX_WIDTH))
        .style(context_menu_style)
        .into()
}

/// 输入框是一个整体：有模式时最前面是它的胶囊，后面是打的字，字后面淡淡的是建议（→ 接受）。
fn input_field(super_box: &SuperBox) -> Element<'_, Message> {
    let mode = super_box.mode();
    let font = (mode == BoxMode::Shell).then_some(Font::MONOSPACE);
    let mut input = text_field(&placeholder(mode), super_box.input())
        .id(super_box_input_id())
        .on_input(|text| message(SuperBoxMessage::InputChanged(text)))
        .style(bare_field_style)
        .width(Length::Fill);
    if let Some(font) = font {
        input = input.font(font);
    }
    // 建议叠在输入框上面：前面那截是框里的字，同字体同排字，透明着只用来占出一样的宽；后面那截是
    // 建议。字长过输入框时输入框会往左滚，那时建议已经在框外，被裁掉。
    let mut faded: iced::widget::text::Rich<'_, (), Message> = rich_text([
        span(super_box.input()).color(Color::TRANSPARENT),
        span(super_box.suggestion().unwrap_or("")),
    ])
    .line_height(iced::widget::text::LineHeight::Absolute(
        CONTROL_LINE_HEIGHT.into(),
    ))
    .wrapping(Wrapping::None)
    .style(|theme: &Theme| iced::widget::text::Style {
        color: Some(muted_text_color(theme)),
    });
    if let Some(font) = font {
        faded = faded.font(font);
    }
    let typed = stack![
        input,
        container(faded)
            .padding([CONTROL_PADDING_Y, FIELD_PADDING_X])
            .width(Length::Fill)
            .clip(true)
    ];
    // 胶囊出现、消失，有没有建议，输入框前后的控件都不能换种类：2026-11-20 虚拟机里胶囊那一格从
    // 空白换成胶囊时，输入框的焦点跟着丢了，`n:` 后面打的字全没进去。没有胶囊时那一格是同样嵌套的
    // 空胶囊。
    let chip = match mode {
        BoxMode::Search => container(container(readable_text(String::new()))),
        mode => container(mode_chip(mode)).padding(iced::Padding::ZERO.left(6)),
    };
    container(row![chip, typed].align_y(Alignment::Center))
        .style(field_frame_style)
        .into()
}

/// 外框照输入框聚焦时的样子画：框开着时它的输入框一直有焦点。
fn field_frame_style(theme: &Theme) -> container::Style {
    let field = text_field_style(theme, text_input::Status::Focused { is_hovered: false });
    container::Style {
        background: Some(field.background),
        border: field.border,
        ..container::Style::default()
    }
}

/// 外框里的输入框自己不画底和边。
fn bare_field_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let field = text_field_style(theme, status);
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            width: 0.0,
            ..field.border
        },
        ..field
    }
}

/// 模式的胶囊：输入框最前面、混在一起的历史行前面。
fn mode_chip(mode: BoxMode) -> Element<'static, Message> {
    container(
        readable_text(mode.label())
            .size(CAPTION_SIZE)
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(ui_colors(theme).primary),
            }),
    )
    .padding([1, 8])
    .style(|theme: &Theme| chip_style(theme, 0.16))
    .into()
}

fn chip_style(theme: &Theme, strength: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color {
            a: strength,
            ..ui_colors(theme).primary
        })),
        border: Border {
            radius: 999.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// 最多列出这么多个候选，再多写“…”。
const CHOICES_SHOWN: usize = 40;

/// Tab 补不动时列出来的候选，再按 Tab 换到的那个高亮。
fn choice_strip(choices: &Choices) -> Element<'_, Message> {
    let mut strip = row![].spacing(4);
    for (index, candidate) in choices.candidates.iter().take(CHOICES_SHOWN).enumerate() {
        let name = if candidate.is_dir {
            format!("{}/", candidate.name)
        } else {
            candidate.name.clone()
        };
        let chosen = choices.chosen == Some(index);
        strip = strip.push(
            container(readable_text(name).size(CAPTION_SIZE))
                .padding([1, 6])
                .style(move |theme: &Theme| chip_style(theme, if chosen { 0.24 } else { 0.0 })),
        );
    }
    if choices.candidates.len() > CHOICES_SHOWN {
        strip = strip.push(muted("…".to_owned()));
    }
    container(strip.wrap().vertical_spacing(4))
        .padding(PANEL_PADDING)
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
            "Type a command. It runs in this folder with your shell, the selected items are its arguments (\"$@\", in fish $argv). Tab completes, → takes the faded suggestion. Full-screen programs move to the terminal panel; Ctrl+Enter runs the command there."
        ),
        BoxMode::Search => tr!(
            "Type to find what to do. Shift+Tab switches to files, a path, a new item or a command; f:, g:, n: and s: do it too."
        ),
        BoxMode::Path => tr!(
            "Type a path, or start with / or ~ in search. Tab completes it, → takes the faded suggestion."
        ),
        BoxMode::New => tr!("Type a name; end it with / to make a folder"),
    }
}

/// 一行的内容：图标、模式胶囊（混在一起的历史）、名字（命令用等宽字）、名字后面的灰字、最右边的灰字。
struct RowParts {
    icon: Option<IconSymbol>,
    chip: Option<BoxMode>,
    title: String,
    monospace: bool,
    detail: Option<String>,
    hint: Option<String>,
}

impl RowParts {
    fn plain(icon: IconSymbol, title: String) -> Self {
        Self {
            icon: Some(icon),
            chip: None,
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
                chip: None,
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
        // 前缀不写：有模式胶囊时它说明了是哪种；没有时（各种混在一起）每行前面带自己的胶囊。
        BoxRow::History(entry) => {
            let (mode, text) = read_history(entry);
            RowParts {
                chip: (super_box.mode() == BoxMode::Search && mode != BoxMode::Search)
                    .then_some(mode),
                monospace: mode == BoxMode::Shell,
                ..RowParts::plain(IconSymbol::RotateCcw, text.to_owned())
            }
        }
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
    let mut label = row![].spacing(8).align_y(Alignment::Center);
    if let Some(mode) = parts.chip {
        label = label.push(mode_chip(mode));
    }
    label = label.push(title);
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

/// 正在跑：命令、“转到终端”和“停止”，下面是它终端里最后几行（在等密码时能看到提示）。
fn running_panel<'a>(command: &str, session: Option<&TerminalSession>) -> Element<'a, Message> {
    let mut panel = Column::new()
        .push(
            row![
                readable_text(tr!("Running %1", command))
                    .font(Font::MONOSPACE)
                    .width(Length::Fill),
                normal_button(readable_text(tr!("Move to Terminal")))
                    .on_press(message(SuperBoxMessage::ShowInTerminalPressed)),
                normal_button(readable_text(tr!("Stop")))
                    .on_press(message(SuperBoxMessage::StopPressed)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .spacing(4);
    let live = session
        .map(|session| session.emulator.last_lines(OUTPUT_LINES_WITHOUT_SCROLLING))
        .unwrap_or_default();
    if !live.is_empty() {
        panel = panel.push(output_text(live));
    }
    container(panel).padding(PANEL_PADDING).into()
}

/// 跑完的结果：标题一行（怎么结束的，以前的还有时间），下面是输出的末尾。
fn end_panel(when: Option<String>, end: &RunEnd) -> Element<'_, Message> {
    let (ended, output) = match end {
        RunEnd::Exited {
            exit: Exit::Code(0),
            output,
        } => (Ok(tr!("Finished, exit status 0")), Some(output)),
        RunEnd::Exited {
            exit: Exit::Code(code),
            output,
        } => (Err(tr!("Exit status %1", code)), Some(output)),
        RunEnd::Exited {
            exit: Exit::Signal(name),
            output,
        } => (Err(tr!("Ended by signal: %1", name)), Some(output)),
        RunEnd::Failed(reason) => (Err(reason.clone()), None),
    };
    let heading = |ended: String| match &when {
        Some(when) => format!("{when} · {ended}"),
        None => ended,
    };
    let mut panel = Column::new()
        .push(match ended {
            Ok(ended) => muted(heading(ended)),
            Err(ended) => error_caption(heading(ended)),
        })
        .spacing(4);
    if let Some(output) = output.filter(|output| !output.text.trim().is_empty()) {
        // 借用框里的字，不每帧复制。短的直接排开；长的放进固定高度的滚动区，停在最后面（看末尾）。
        panel = if output.lines <= OUTPUT_LINES_WITHOUT_SCROLLING {
            panel.push(output_text(output.text.as_str()))
        } else {
            panel.push(
                iced::widget::scrollable(output_text(output.text.as_str()))
                    .anchor_bottom()
                    .height(Length::Fixed(OUTPUT_SCROLL_HEIGHT))
                    .width(Length::Fill),
            )
        };
    }
    container(panel).padding(PANEL_PADDING).into()
}

/// 命令的输出：等宽；里面常有中文文件名，按复杂文字排版。
fn output_text<'a>(
    text: impl iced::widget::text::IntoFragment<'a>,
) -> iced::widget::Text<'a, Theme> {
    iced::widget::text(text)
        .shaping(iced::widget::text::Shaping::Advanced)
        .size(CAPTION_SIZE)
        .font(Font::MONOSPACE)
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
    container(error_caption(text)).padding([4, 8]).into()
}

fn error_caption(text: String) -> Element<'static, Message> {
    readable_text(text)
        .size(CAPTION_SIZE)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_colors(theme).error),
        })
        .into()
}
