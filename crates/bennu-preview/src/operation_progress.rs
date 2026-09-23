// WHY：RemotePreviewDownload::fraction 的字节进度数学随预览模型下沉到
// 本 crate；app-ui 的文件操作进度条（operation_progress.rs）与传输占位
// 经 re-export 继续单源消费此实现，不在两侧各留一份。

use iced::widget::{column, container, progress_bar, svg, Column, Svg};
use iced::{Element, Length};

use bennu_theme::measured_text::format_middle_ellipsized_text;

use crate::formatting::format_file_size;
use crate::panel_text::readable_text;
use crate::preview::RemotePreviewDownload;

pub const ACTIVE_PROGRESS_LIMIT: f32 = 0.999;

const INDETERMINATE_DASH_PERIOD: u8 = 16;
const PREVIEW_ENTRY_NAME_MAX_CHARS: usize = 48;

pub fn active_byte_fraction(completed_bytes: u64, total_bytes: u64) -> Option<f32> {
    if total_bytes == 0 {
        return None;
    }
    Some(
        ((completed_bytes.min(total_bytes) as f64 / total_bytes as f64) as f32)
            .min(ACTIVE_PROGRESS_LIMIT),
    )
}

/// 活动中的不定进度轨道（负 dash 偏移动画）。app-ui 的任务队列视图
/// 经 re-export 消费同一实现。
pub fn active_indeterminate_track_handle(animation_frame: u8) -> svg::Handle {
    let dash_offset = animation_frame % INDETERMINATE_DASH_PERIOD;
    svg::Handle::from_memory(progress_track_svg("#3b82f6", Some(dash_offset)).into_bytes())
}

/// 静止的不定进度轨道（排队/未知总量）。app-ui 的任务队列视图经
/// re-export 消费同一实现。
pub fn static_indeterminate_track_handle() -> svg::Handle {
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

/// 远程文件下载中的预览面板：文件名 + 字节进度条/不定轨道 + 已下载
/// 明细。自 app-ui operation_progress.rs 下沉（预览窗口 Downloading 态
// 消费）；宿主任务队列视图只借用上面的轨道句柄。
pub fn remote_preview_download_panel<Message: 'static>(
    download: &RemotePreviewDownload,
    animation_frame: u8,
) -> Column<'static, Message> {
    let name = download
        .source_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| download.source_path.to_string_lossy().into_owned());
    let name = format_middle_ellipsized_text(&name, PREVIEW_ENTRY_NAME_MAX_CHARS);
    let title = if bennu_localization::current_language_is_chinese() {
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
        .unwrap_or_else(|| bennu_localization::translate_current("Preparing download..."));

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
