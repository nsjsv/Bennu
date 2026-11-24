use iced::widget::{column, container, row, scrollable, text, Column};
use iced::{Alignment, Element, Length};

use crate::app::right_preview_panel::PANEL_CONTENT_PADDING;
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::model::Message;
use crate::operation_queue_view::{clear_finished_button, operation_task_list};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::normal_button;
use bennu_theme::typography::BOLD;

/// 任务面板：上面是正在跑和刚做完的任务（跟右下角弹出的列表同一套行），下面是能撤销的操作，
/// 最新的在上。撤销只能按顺序：点第 n 条 = 连按 n 次 Ctrl+Z。
pub(super) fn task_log_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let mut tasks_header = row![heading(tr!("Tasks"))].align_y(Alignment::Center);
    if let Some(clear) = clear_finished_button(&browser.operation_queue, &browser.transfer_tasks) {
        tasks_header = tasks_header.push(clear);
    }
    let mut history = Column::new().spacing(6);
    let summaries = browser.undo_summaries();
    if summaries.is_empty() {
        history = history.push(readable_text(tr!("Nothing to undo")).style(|theme| {
            iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }
        }));
    }
    for (index, summary) in summaries.into_iter().enumerate() {
        let label = if index == 0 {
            tr!("Undo")
        } else {
            tr!("Undo to Here")
        };
        history = history.push(
            row![
                text(summary).width(Length::Fill),
                normal_button(readable_text(label))
                    .on_press(Message::UndoFileOperations(index + 1)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    let content = column![
        tasks_header,
        operation_task_list(
            &browser.operation_queue,
            &browser.transfer_tasks,
            &browser.action_runs,
            browser.operation_progress_animation_frame,
        ),
        heading(tr!("History")),
        history,
    ]
    .spacing(10)
    .width(Length::Fill);
    container(scrollable(content))
        .padding(PANEL_CONTENT_PADDING)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn heading(label: String) -> Element<'static, Message> {
    container(text(label).font(BOLD)).width(Length::Fill).into()
}
