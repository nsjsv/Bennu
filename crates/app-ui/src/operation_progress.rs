use std::path::PathBuf;

use file_operation_store::StoredProgress;
use iced::widget::{column, container, progress_bar, svg, Column, Svg};
use iced::{Element, Length};

use crate::formatting::{format_file_size, format_middle_ellipsized_text};
use crate::model::{Message, RemotePreviewDownload};
use crate::typography::readable_text;

const ACTIVE_PROGRESS_LIMIT: f32 = 0.999;
const INDETERMINATE_DASH_PERIOD: u8 = 16;
const PREVIEW_ENTRY_NAME_MAX_CHARS: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOperationWorkProgress {
    Indeterminate,
    Bytes {
        completed_bytes: u64,
        total_bytes: u64,
    },
    Completed {
        bytes: Option<(u64, u64)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileOperationProgress {
    work: FileOperationWorkProgress,
    completed_items: usize,
    total_items: Option<usize>,
}

impl FileOperationProgress {
    pub(crate) fn pending() -> Self {
        Self {
            work: FileOperationWorkProgress::Indeterminate,
            completed_items: 0,
            total_items: None,
        }
    }

    pub(crate) fn fraction(self) -> Option<f32> {
        match self.work {
            FileOperationWorkProgress::Indeterminate => None,
            FileOperationWorkProgress::Bytes {
                completed_bytes,
                total_bytes,
            } => active_byte_fraction(completed_bytes, total_bytes),
            FileOperationWorkProgress::Completed { .. } => Some(1.0),
        }
    }

    pub(crate) fn bytes(self) -> Option<(u64, u64)> {
        match self.work {
            FileOperationWorkProgress::Bytes {
                completed_bytes,
                total_bytes,
            } => Some((completed_bytes, total_bytes)),
            FileOperationWorkProgress::Completed { bytes } => bytes,
            FileOperationWorkProgress::Indeterminate => None,
        }
    }

    pub(crate) fn items(self) -> Option<(usize, usize)> {
        self.total_items
            .map(|total_items| (self.completed_items, total_items))
    }

    pub(crate) fn mark_complete(&mut self) {
        let bytes = match self.work {
            FileOperationWorkProgress::Bytes { total_bytes, .. } => {
                Some((total_bytes, total_bytes))
            }
            FileOperationWorkProgress::Indeterminate
            | FileOperationWorkProgress::Completed { .. } => None,
        };
        self.work = FileOperationWorkProgress::Completed { bytes };
        if let Some(total_items) = self.total_items {
            self.completed_items = total_items;
        }
    }

    pub(crate) fn to_stored(self) -> StoredProgress {
        match self.fraction() {
            Some(fraction) => StoredProgress::with_fraction(fraction as f64),
            None => StoredProgress::pending(),
        }
    }

    pub(crate) fn update(&mut self, update: FileOperationProgressUpdate) {
        match update {
            FileOperationProgressUpdate::Bytes {
                completed_bytes,
                total_bytes,
                completed_items,
                total_items,
            } => {
                self.update_items(completed_items, total_items);
                if total_bytes == 0 {
                    return;
                }
                let completed_bytes = completed_bytes.min(total_bytes);
                self.work = match self.work {
                    FileOperationWorkProgress::Indeterminate => FileOperationWorkProgress::Bytes {
                        completed_bytes,
                        total_bytes,
                    },
                    FileOperationWorkProgress::Bytes {
                        completed_bytes: current_completed_bytes,
                        total_bytes: current_total_bytes,
                    } if current_total_bytes == total_bytes => FileOperationWorkProgress::Bytes {
                        completed_bytes: current_completed_bytes.max(completed_bytes),
                        total_bytes,
                    },
                    current => current,
                };
            }
            FileOperationProgressUpdate::IndeterminateItems { completed, total } => {
                self.update_items(completed, total);
            }
            FileOperationProgressUpdate::Indeterminate => {}
        }
    }

    fn update_items(&mut self, completed: usize, total: usize) {
        if total == 0 {
            return;
        }
        let completed = completed.min(total);
        match self.total_items {
            Some(current_total) if current_total == total => {
                self.completed_items = self.completed_items.max(completed);
            }
            Some(_) => {}
            None => {
                self.completed_items = completed;
                self.total_items = Some(total);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum FileOperationProgressUpdate {
    Bytes {
        completed_bytes: u64,
        total_bytes: u64,
        completed_items: usize,
        total_items: usize,
    },
    IndeterminateItems {
        completed: usize,
        total: usize,
    },
    Indeterminate,
}

/// 单个传入条目的传输快照:占位行的单一事实源,由任务 `transfer_progress`
/// 全量替换持有,视图渲染时从队列派生,不另建占位状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferEntrySnapshot {
    /// 最终落地路径(含重名序号,入队后不变),`parent()` 用于匹配目标目录。
    pub(crate) target: PathBuf,
    pub(crate) is_directory: bool,
    /// `None` 表示未开始或字节数未知(排队中/清单缺失),占位画空环。
    pub(crate) completed_bytes: Option<u64>,
    pub(crate) total_bytes: Option<u64>,
    pub(crate) state: TransferEntrySnapshotState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferEntrySnapshotState {
    Queued,
    Active,
    Completed,
}

pub(crate) fn active_byte_fraction(completed_bytes: u64, total_bytes: u64) -> Option<f32> {
    if total_bytes == 0 {
        return None;
    }
    Some(
        ((completed_bytes.min(total_bytes) as f64 / total_bytes as f64) as f32)
            .min(ACTIVE_PROGRESS_LIMIT),
    )
}

pub(crate) fn active_indeterminate_track_handle(animation_frame: u8) -> svg::Handle {
    let dash_offset = animation_frame % INDETERMINATE_DASH_PERIOD;
    svg::Handle::from_memory(progress_track_svg("#3b82f6", Some(dash_offset)).into_bytes())
}

pub(crate) fn static_indeterminate_track_handle() -> svg::Handle {
    svg::Handle::from_memory(progress_track_svg("#64748b", None).into_bytes())
}

fn progress_track_svg(stroke: &str, dash_offset: Option<u8>) -> String {
    let dash_offset = dash_offset
        .map(|offset| format!(r#" stroke-dashoffset="-{offset}""#))
        .unwrap_or_default();
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="4" viewBox="0 0 100 4" preserveAspectRatio="none">
<line x1="1" y1="2" x2="99" y2="2" stroke="#64748b" stroke-opacity="0.22" stroke-width="2" stroke-linecap="round"/>
<line x1="1" y1="2" x2="99" y2="2" stroke="{stroke}" stroke-width="2.6" stroke-linecap="round" stroke-dasharray="8 8"{dash_offset}/>
</svg>"##
    )
}

pub(crate) fn remote_preview_download_panel(
    download: &RemotePreviewDownload,
    animation_frame: u8,
) -> Column<'static, Message> {
    let name = download
        .source_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| download.source_path.to_string_lossy().into_owned());
    let name = format_middle_ellipsized_text(&name, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let title = if crate::localization::current_language_is_chinese() {
        format!("正在下载 {name}")
    } else {
        format!("Downloading {name}")
    };
    let progress: Element<'static, Message> = match download.fraction() {
        Some(fraction) => container(progress_bar(0.0..=1.0, fraction))
            .width(Length::Fill)
            .into(),
        None => Svg::new(active_indeterminate_track_handle(animation_frame))
            .width(Length::Fill)
            .height(Length::Fixed(4.0))
            .into(),
    };
    let detail = download
        .bytes_total
        .map(|bytes_total| {
            format!(
                "{} / {}",
                format_file_size(download.bytes_done),
                format_file_size(bytes_total)
            )
        })
        .unwrap_or_else(|| crate::localization::translate_current("Preparing download..."));

    column![
        readable_text(title).size(14),
        progress,
        readable_text(detail).size(12),
    ]
    .spacing(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_byte_fraction_is_unknown_without_a_denominator_and_never_reaches_terminal() {
        assert_eq!(active_byte_fraction(0, 0), None);
        assert_eq!(active_byte_fraction(250, 1_000), Some(0.25));
        assert_eq!(
            active_byte_fraction(1_000, 1_000),
            Some(ACTIVE_PROGRESS_LIMIT)
        );
    }

    #[test]
    fn indeterminate_animation_changes_phase_without_changing_dash_density() {
        let first = progress_track_svg("#3b82f6", Some(0));
        let second = progress_track_svg("#3b82f6", Some(1));

        assert!(first.contains(r#"stroke-dasharray="8 8""#));
        assert!(second.contains(r#"stroke-dasharray="8 8""#));
        assert_ne!(first, second);
    }
}
