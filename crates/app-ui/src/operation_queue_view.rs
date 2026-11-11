use iced::widget::{button, column, container, row, scrollable, svg, Svg};
use iced::{Alignment, Element, Length};

use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::app::transfer::{TransferTask, TransferTasks};
use crate::appearance::{
    context_menu_style, enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction,
    error_notification_style, operation_queue_indicator_button_style, path_suggestion_item_style,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::model::{Message, ScrollbarRegion, ScrollbarVisibility};
use crate::operation_queue_display::{
    file_operation_progress_text, FileOperationPathLines, OperationPathLines,
};
use crate::view::transfer_task_row::transfer_task_row;
use bennu_preview::operation_progress::{
    active_indeterminate_track_handle, static_indeterminate_track_handle,
};
use bennu_preview::panel_text::readable_text;
use bennu_theme::controls::normal_button;
use bennu_theme::controls::progress_bar;
use bennu_theme::typography::{BOLD, CAPTION_SIZE, TITLE_SIZE};
use file_operations::{FileOperationQueue, FileOperationStatus, FileOperationTask};

pub(crate) const OPERATION_QUEUE_PANEL_WIDTH: f32 = 360.0;
pub(crate) const OPERATION_QUEUE_PANEL_BOTTOM: f32 = 18.0;
pub(crate) const OPERATION_QUEUE_INDICATOR_SIZE: f32 = 30.0;
pub(crate) const OPERATION_QUEUE_INDICATOR_RIGHT: f32 = 14.0;
pub(crate) const OPERATION_QUEUE_INDICATOR_BOTTOM: f32 = 14.0;

const TASK_FILE_NAME_MAX_CHARS: usize = 24;
const TASK_PATH_MAX_CHARS: usize = 46;
const TASK_ERROR_MAX_CHARS: usize = 120;
const TASK_LIST_MAX_HEIGHT: f32 = 320.0;

pub(crate) fn operation_queue_indicator<'a>(
    queue: &'a FileOperationQueue,
    transfers: &TransferTasks,
    animation_frame: u8,
) -> Option<Element<'a, Message>> {
    let svg = Svg::new(svg::Handle::from_memory(
        indicator_svg(
            queue_indicator_ring(queue, transfers, animation_frame),
            queue_indicator_badge(queue, transfers),
        )
        .into_bytes(),
    ))
    .width(Length::Fixed(OPERATION_QUEUE_INDICATOR_SIZE))
    .height(Length::Fixed(OPERATION_QUEUE_INDICATOR_SIZE));

    Some(
        button(svg)
            .on_press(Message::FileOperationIndicatorPressed)
            .padding(0)
            .style(operation_queue_indicator_button_style())
            .into(),
    )
}

/// 面板里的一行：文件任务，或 LocalSend 收发（两边各记各的，按开始时间混排）。
enum PanelRow<'a> {
    File(&'a FileOperationTask),
    Transfer(&'a TransferTask),
}

impl PanelRow<'_> {
    fn started_at(&self) -> std::time::SystemTime {
        match self {
            Self::File(task) => task.enqueued_at,
            Self::Transfer(task) => task.started_at,
        }
    }
}

pub(crate) fn operation_queue_panel<'a>(
    queue: &'a FileOperationQueue,
    transfers: &'a TransferTasks,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<crate::model::ScrollbarViewport>,
    animation_frame: u8,
) -> Element<'a, Message> {
    let mut header = row![readable_text(tr!("Tasks"))
        .size(TITLE_SIZE)
        .font(BOLD)
        .width(Length::Fill)]
    .spacing(8)
    .align_y(Alignment::Center);
    if queue.has_terminal_tasks() || transfers.has_terminal() {
        header = header.push(task_control_button(
            tr!("Clear Finished"),
            Message::FileOperationClearFinishedRequested,
        ));
    }
    let rows = panel_rows(queue, transfers);
    let header = header.push(readable_text(tr!("%1 tasks", rows.len())));

    let mut tasks = column![].spacing(6);
    if rows.is_empty() {
        tasks = tasks.push(empty_queue_row());
    } else {
        for row in rows {
            tasks = tasks.push(match row {
                PanelRow::File(task) => operation_task_row(task, animation_frame),
                PanelRow::Transfer(task) => transfer_task_row(transfers, task, animation_frame),
            });
        }
    }

    let scroll_region = ScrollbarRegion::OperationQueue;
    let scroller = scrollable(smooth_scroll_content(tasks, scroll_region.clone()))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .on_scroll(scrollbar_on_scroll(scroll_region.clone(), |_| {
            Message::OperationQueueScrolled
        }));
    let scroller = container(scroller)
        .width(Length::Fill)
        .max_height(TASK_LIST_MAX_HEIGHT);
    let scroller = enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        6.0,
    );
    let content = column![header, scroller].spacing(10);

    container(content)
        .padding(12)
        .width(Length::Fixed(OPERATION_QUEUE_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

fn operation_queue_display_tasks(
    queue: &FileOperationQueue,
) -> impl Iterator<Item = &FileOperationTask> {
    queue.tasks().iter().rev()
}

/// 最新的在上面；同一时刻开始的保持各自原来的先后。
fn panel_rows<'a>(
    queue: &'a FileOperationQueue,
    transfers: &'a TransferTasks,
) -> Vec<PanelRow<'a>> {
    let mut rows: Vec<PanelRow<'a>> = operation_queue_display_tasks(queue)
        .map(PanelRow::File)
        .chain(transfers.panel_tasks().rev().map(PanelRow::Transfer))
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.started_at()));
    rows
}

fn operation_task_row(task: &FileOperationTask, animation_frame: u8) -> Element<'_, Message> {
    let path_lines = task.operation.path_lines();
    let title = row![
        readable_text(operation_title_text(task.operation.title(), &path_lines))
            .width(Length::Fill),
        readable_text(task.status_label()).size(CAPTION_SIZE),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let paths = column![
        readable_text(tr!(
            "Original: %1",
            format_middle_ellipsized_text(&path_lines.original_path, TASK_PATH_MAX_CHARS)
        ))
        .size(CAPTION_SIZE),
        readable_text(tr!(
            "Directory: %1",
            format_middle_ellipsized_text(&path_lines.directory_path, TASK_PATH_MAX_CHARS)
        ))
        .size(CAPTION_SIZE),
    ]
    .spacing(2)
    .width(Length::Fill);

    let progress: Element<'static, Message> = match task.progress.fraction() {
        Some(fraction) => progress_bar(0.0..=1.0, fraction)
            .girth(Length::Fixed(3.0))
            .into(),
        None if matches!(
            task.status,
            FileOperationStatus::Running | FileOperationStatus::Canceling
        ) =>
        {
            Svg::new(active_indeterminate_track_handle(animation_frame))
                .width(Length::Fill)
                .height(Length::Fixed(4.0))
                .into()
        }
        None => Svg::new(static_indeterminate_track_handle())
            .width(Length::Fill)
            .height(Length::Fixed(4.0))
            .into(),
    };

    let mut body = column![title, paths, progress]
        .spacing(4)
        .width(Length::Fill);

    if let Some(detail) = file_operation_progress_text(task) {
        body = body.push(readable_text(detail).size(CAPTION_SIZE).width(Length::Fill));
    }

    if let Some(warning) = task.completion_warning.as_deref() {
        body = body.push(
            readable_text(warning.to_owned())
                .size(CAPTION_SIZE)
                .width(Length::Fill),
        );
    }

    if let Some(error) = task.error.as_deref() {
        body = body.push(
            readable_text(format_middle_ellipsized_text(error, TASK_ERROR_MAX_CHARS))
                .size(CAPTION_SIZE)
                .width(Length::Fill),
        );
    }

    let content = column![body, operation_task_controls(task)]
        .spacing(6)
        .width(Length::Fill);
    let item = container(content).padding(8).width(Length::Fill);
    let item = if task.status == FileOperationStatus::Failed {
        item.style(error_notification_style)
    } else {
        item.style(path_suggestion_item_style)
    };

    item.into()
}

fn operation_title_text(title: String, path_lines: &FileOperationPathLines) -> String {
    let file_name = format_middle_ellipsized_text(&path_lines.file_name, TASK_FILE_NAME_MAX_CHARS);
    let more_count = path_lines.total_items.saturating_sub(1);
    if more_count == 0 {
        format!("{title} - {file_name}")
    } else {
        format!("{title} - {file_name} (+{more_count})")
    }
}

fn empty_queue_row() -> Element<'static, Message> {
    container(readable_text(tr!("No tasks")).width(Length::Fill))
        .padding(10)
        .width(Length::Fill)
        .style(path_suggestion_item_style)
        .into()
}

fn operation_task_controls(task: &FileOperationTask) -> Element<'static, Message> {
    let pause_label = match task.status {
        FileOperationStatus::Paused => tr!("Resume"),
        _ => tr!("Pause"),
    };
    let can_pause = matches!(
        task.status,
        FileOperationStatus::Running | FileOperationStatus::Paused
    ) && task.operation.supports_pause();
    let can_cancel = matches!(
        task.status,
        FileOperationStatus::Pending | FileOperationStatus::Running | FileOperationStatus::Paused
    );

    let mut controls = row![task_control_button(
        tr!("Copy Details"),
        Message::FileOperationDetailsCopyRequested(task.id),
    )]
    .spacing(6)
    .align_y(Alignment::Center);
    if can_pause {
        controls = controls.push(task_control_button(
            pause_label,
            Message::FileOperationPauseToggled(task.id),
        ));
    }
    if can_cancel {
        controls = controls.push(task_control_button(
            tr!("Cancel"),
            Message::FileOperationCancelRequested(task.id),
        ));
    }
    if task.status.is_terminal() {
        controls = controls.push(task_control_button(
            tr!("Clear"),
            Message::FileOperationClearRequested(task.id),
        ));
    }

    controls.into()
}

fn task_control_button(label: String, message: Message) -> Element<'static, Message> {
    normal_button(readable_text(label)).on_press(message).into()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum QueueIndicatorRing {
    Hidden,
    Determinate {
        fraction: f32,
        tone: QueueIndicatorTone,
    },
    ActiveIndeterminate {
        animation_frame: u8,
    },
    PausedIndeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueIndicatorTone {
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueIndicatorBadge {
    Hidden,
    Count(usize),
    Error,
    Warning,
    Paused,
}

fn queue_indicator_ring(
    queue: &FileOperationQueue,
    transfers: &TransferTasks,
    animation_frame: u8,
) -> QueueIndicatorRing {
    if let Some(fraction) = queue.indicator_progress() {
        return QueueIndicatorRing::Determinate {
            fraction,
            tone: if queue.is_active_paused() {
                QueueIndicatorTone::Paused
            } else {
                QueueIndicatorTone::Active
            },
        };
    }
    if queue.is_active_paused() {
        return QueueIndicatorRing::PausedIndeterminate;
    }
    // 文件任务没有确定进度时，进度环跟着 LocalSend 收发走。
    if !queue.has_active_task() {
        if let Some(fraction) = transfers.active_fraction() {
            return QueueIndicatorRing::Determinate {
                fraction,
                tone: QueueIndicatorTone::Active,
            };
        }
    }
    if queue.has_active_task() || transfers.has_active() {
        QueueIndicatorRing::ActiveIndeterminate { animation_frame }
    } else {
        QueueIndicatorRing::Hidden
    }
}

fn queue_indicator_badge(
    queue: &FileOperationQueue,
    transfers: &TransferTasks,
) -> QueueIndicatorBadge {
    let unread = queue.unread_count() + transfers.unread_count();
    if unread > 1 {
        QueueIndicatorBadge::Count(unread.min(99))
    } else if queue.has_unread_failed_task() || transfers.has_unread_failed() {
        QueueIndicatorBadge::Error
    } else if queue.has_unread_warning_task() {
        QueueIndicatorBadge::Warning
    } else if queue.is_active_paused() {
        QueueIndicatorBadge::Paused
    } else {
        QueueIndicatorBadge::Hidden
    }
}

fn indicator_svg(ring: QueueIndicatorRing, badge: QueueIndicatorBadge) -> String {
    let circumference = 2.0 * std::f32::consts::PI * 12.0;
    let progress_svg = match ring {
        QueueIndicatorRing::Hidden => String::new(),
        QueueIndicatorRing::Determinate { fraction, tone } => {
            let offset = circumference * (1.0 - fraction.clamp(0.0, 1.0));
            let stroke = match tone {
                QueueIndicatorTone::Active => "#3b82f6",
                QueueIndicatorTone::Paused => "#f59e0b",
            };
            format!(
                r#"<circle data-progress-kind="determinate" cx="15" cy="15" r="12" fill="none" stroke="{stroke}" stroke-width="2.6" stroke-linecap="round" stroke-dasharray="{circumference:.2}" stroke-dashoffset="{offset:.2}" transform="rotate(-90 15 15)"/>"#
            )
        }
        QueueIndicatorRing::ActiveIndeterminate { animation_frame } => {
            let dash_offset = animation_frame % 12;
            format!(
                r##"<circle data-progress-kind="indeterminate" cx="15" cy="15" r="12" fill="none" stroke="#3b82f6" stroke-width="2.6" stroke-linecap="round" stroke-dasharray="7 5" stroke-dashoffset="-{dash_offset}" transform="rotate(-90 15 15)"/>"##
            )
        }
        QueueIndicatorRing::PausedIndeterminate => r##"<circle data-progress-kind="indeterminate" cx="15" cy="15" r="12" fill="none" stroke="#f59e0b" stroke-width="2.6" stroke-linecap="round" stroke-dasharray="7 5" transform="rotate(-90 15 15)"/>"##.to_owned(),
    };

    let (badge_label, badge_fill) = match badge {
        QueueIndicatorBadge::Hidden => (None, "#2563eb"),
        QueueIndicatorBadge::Count(count) => (Some(count.to_string()), "#2563eb"),
        QueueIndicatorBadge::Error => (Some("!".to_owned()), "#ef4444"),
        QueueIndicatorBadge::Warning => (Some("!".to_owned()), "#f59e0b"),
        QueueIndicatorBadge::Paused => (Some("||".to_owned()), "#f59e0b"),
    };
    let badge_svg = badge_label
        .map(|label| {
            format!(
                r##"<circle cx="23" cy="7" r="5.2" fill="{badge_fill}"/>
<text x="23" y="9.8" text-anchor="middle" font-family="sans-serif" font-size="7.4" font-weight="700" fill="#ffffff">{label}</text>"##
            )
        })
        .unwrap_or_default();

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="30" viewBox="0 0 30 30">
<circle cx="15" cy="15" r="14" fill="#111827" fill-opacity="0.78"/>
<circle cx="15" cy="15" r="12" fill="none" stroke="#64748b" stroke-opacity="0.34" stroke-width="2.6"/>
{progress_svg}
<g transform="translate(6.4 6.4) scale(0.72)" fill="none" stroke="#f8fafc" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
<rect x="3" y="5" width="6" height="6" rx="1"/>
<path d="m3 17 2 2 4-4"/>
<path d="M13 6h8"/>
<path d="M13 12h8"/>
<path d="M13 18h8"/>
</g>
{badge_svg}
</svg>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_operations::test_persistence::queued_task_id;

    #[test]
    fn indicator_hides_progress_ring_for_completed_only_tasks() {
        let svg = indicator_svg(QueueIndicatorRing::Hidden, QueueIndicatorBadge::Count(1));

        assert!(!svg.contains("data-progress-kind"));
    }

    #[test]
    fn indicator_hides_badge_after_tasks_are_read() {
        let svg = indicator_svg(QueueIndicatorRing::Hidden, QueueIndicatorBadge::Hidden);

        assert!(!svg.contains("<text"));
    }

    #[test]
    fn active_indeterminate_indicator_has_no_completion_fraction() {
        let svg = indicator_svg(
            QueueIndicatorRing::ActiveIndeterminate { animation_frame: 0 },
            QueueIndicatorBadge::Hidden,
        );

        assert!(svg.contains(r#"data-progress-kind="indeterminate""#));
        assert!(svg.contains(r#"stroke-dasharray="7 5""#));
        assert!(!svg.contains("0.35"));
    }

    #[test]
    fn task_panel_displays_newest_first_without_changing_fifo_runner() {
        let mut queue = FileOperationQueue::new();
        let first_id = queued_task_id(queue.enqueue(
            file_operations::QueuedFileOperation::CreateDirectory {
                parent: std::path::PathBuf::from("/first"),
            },
        ));
        let second_id = queued_task_id(queue.enqueue(
            file_operations::QueuedFileOperation::CreateDirectory {
                parent: std::path::PathBuf::from("/second"),
            },
        ));

        let displayed_ids = operation_queue_display_tasks(&queue)
            .map(|task| task.id)
            .collect::<Vec<_>>();

        assert_eq!(displayed_ids, vec![second_id, first_id]);
        assert_eq!(queue.active_subscription().unwrap().id, first_id);
    }

    #[test]
    fn localsend_rows_mix_with_file_tasks_newest_first() {
        let (mut browser, _) = crate::app::FileBrowser::new(crate::config::default_user_config());
        let older = crate::app::transfer::tests::start_sending(&mut browser, "phone");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let file = queued_task_id(browser.operation_queue.enqueue(
            file_operations::QueuedFileOperation::CreateDirectory {
                parent: std::path::PathBuf::from("/first"),
            },
        ));
        std::thread::sleep(std::time::Duration::from_millis(5));
        let newer = crate::app::transfer::tests::start_sending(&mut browser, "tablet");

        let order: Vec<String> = panel_rows(&browser.operation_queue, &browser.transfer_tasks)
            .into_iter()
            .map(|row| match row {
                PanelRow::File(task) => format!("file {}", task.id),
                PanelRow::Transfer(task) => format!("transfer {}", task.id),
            })
            .collect();

        assert_eq!(
            order,
            [
                format!("transfer {newer}"),
                format!("file {file}"),
                format!("transfer {older}")
            ]
        );
        // 未读角标把两边加起来。
        assert_eq!(
            queue_indicator_badge(&browser.operation_queue, &browser.transfer_tasks),
            QueueIndicatorBadge::Count(3)
        );
    }

    #[test]
    fn indeterminate_indicator_animation_changes_only_dash_phase() {
        let first = indicator_svg(
            QueueIndicatorRing::ActiveIndeterminate { animation_frame: 0 },
            QueueIndicatorBadge::Hidden,
        );
        let second = indicator_svg(
            QueueIndicatorRing::ActiveIndeterminate { animation_frame: 1 },
            QueueIndicatorBadge::Hidden,
        );

        assert!(first.contains(r#"stroke-dasharray="7 5""#));
        assert!(second.contains(r#"stroke-dasharray="7 5""#));
        assert_ne!(first, second);
    }
}
