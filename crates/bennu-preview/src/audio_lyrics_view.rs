//! 歌词区。带时间轴的不用滚动区：面板知道自己有多高，就显示放得下的那几行、以当前句
//! 为中心（引擎不必知道视口高度去算滚动偏移）；滚轮按整行翻，停手 3 秒回到当前句；
//! 点一句从这句开始放。没有时间轴的就是一个普通滚动区。

use std::time::Duration;

use iced::alignment::Horizontal;
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, scrollable, Column};
use iced::{mouse, Element, Length};

use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, muted_text_color,
    transparent_button_style,
};
use bennu_theme::typography::BOLD;
use bennu_theme::ui_colors;

use crate::audio_preview::{AudioLyrics, LyricLine};
use crate::panel_text::readable_text;
use crate::preview::LyricsBrowse;
use crate::preview_message::PreviewMessage;

/// 一句歌词最多占两行（双语歌词常把原文和译文写在同一行，一行放不下）。iced 0.14 的文字
/// 没有省略号，超过两行的部分被截掉；行高固定，按面板高度算放得下几句才算得准。
pub(crate) const LYRIC_ROW_HEIGHT: f32 = 44.0;
/// 触控板的像素滚动按多少像素折一句。
const WHEEL_PIXELS_PER_LINE: f32 = 28.0;

pub(crate) fn lyrics_view<Message>(
    lyrics: &AudioLyrics,
    position: Duration,
    browse: LyricsBrowse,
    height: f32,
    align: Horizontal,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    match lyrics {
        AudioLyrics::Synced(lines) => {
            synced_lyrics(lines, lyrics.current_line(position), browse, height, align)
        }
        AudioLyrics::Plain(lines) => plain_lyrics(lines, height, align),
    }
}

/// 要显示的那一段：`rows` 行，以「当前句 + 翻看偏移」为中心，贴住开头和结尾不留空。
pub(crate) fn visible_window(
    line_count: usize,
    current: Option<usize>,
    offset_lines: i32,
    rows: usize,
) -> std::ops::Range<usize> {
    if line_count == 0 || rows == 0 {
        return 0..0;
    }
    let last = line_count as i64 - 1;
    let center = (current.unwrap_or(0) as i64 + i64::from(offset_lines)).clamp(0, last);
    let rows = rows.min(line_count) as i64;
    let first = (center - rows / 2).clamp(0, line_count as i64 - rows);
    first as usize..(first + rows) as usize
}

fn synced_lyrics<Message>(
    lines: &[LyricLine],
    current: Option<usize>,
    browse: LyricsBrowse,
    height: f32,
    align: Horizontal,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let rows = (height / LYRIC_ROW_HEIGHT).floor().max(1.0) as usize;
    let mut column = Column::new().width(Length::Fill);
    for index in visible_window(lines.len(), current, browse.offset_lines, rows) {
        let is_current = current == Some(index);
        let mut text = readable_text(lines[index].text.clone())
            .wrapping(Wrapping::WordOrGlyph)
            .width(Length::Fill)
            .align_x(align);
        text = if is_current {
            text.font(BOLD)
                .style(|theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(ui_colors(theme).primary),
                })
        } else {
            text.style(|theme: &iced::Theme| iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            })
        };
        column = column.push(
            button(container(text).center_y(Length::Fill).clip(true))
                .width(Length::Fill)
                .height(Length::Fixed(LYRIC_ROW_HEIGHT))
                .padding([0, 4])
                .style(transparent_button_style())
                .on_press(Message::from(PreviewMessage::AudioLyricsLineSelected(
                    index,
                ))),
        );
    }
    mouse_area(
        container(column)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .center_y(Length::Fixed(height)),
    )
    .on_scroll(|delta| Message::from(PreviewMessage::AudioLyricsBrowsed(wheel_lines(delta))))
    .into()
}

/// 滚轮往上（y > 0）看前面的句子；触控板的像素增量按行高折算，至少一行。
fn wheel_lines(delta: mouse::ScrollDelta) -> i32 {
    let (y, per_line) = match delta {
        mouse::ScrollDelta::Lines { y, .. } => (y, 1.0),
        mouse::ScrollDelta::Pixels { y, .. } => (y, WHEEL_PIXELS_PER_LINE),
    };
    let lines = (y / per_line).round().abs().max(1.0) as i32;
    if y > 0.0 {
        -lines
    } else {
        lines
    }
}

fn plain_lyrics<Message>(
    lines: &[String],
    height: f32,
    align: Horizontal,
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    let text = lines.join("\n");
    let scroller = scrollable(column![readable_text(text)
        .width(Length::Fill)
        .align_x(align)])
    .direction(enhanced_vertical_scrollbar_direction())
    .style(enhanced_scrollbar_style)
    .height(Length::Fixed(height));
    enhanced_scrollbar(scroller, ScrollbarAxis::Vertical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_centres_on_the_current_line_and_sticks_to_the_ends() {
        assert_eq!(visible_window(20, Some(10), 0, 5), 8..13);
        assert_eq!(visible_window(20, Some(1), 0, 5), 0..5);
        assert_eq!(visible_window(20, Some(19), 0, 5), 15..20);
        // 第一句之前也从开头显示。
        assert_eq!(visible_window(20, None, 0, 5), 0..5);
        // 翻看偏移挪中心，越界夹住。
        assert_eq!(visible_window(20, Some(10), -6, 5), 2..7);
        assert_eq!(visible_window(20, Some(10), 100, 5), 15..20);
        // 行数比放得下的少：全显示。
        assert_eq!(visible_window(3, Some(1), 0, 5), 0..3);
        assert_eq!(visible_window(0, None, 0, 5), 0..0);
    }

    #[test]
    fn wheel_up_shows_earlier_lines() {
        assert_eq!(
            wheel_lines(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }),
            -1
        );
        assert_eq!(
            wheel_lines(mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 }),
            3
        );
        assert_eq!(
            wheel_lines(mouse::ScrollDelta::Pixels { x: 0.0, y: -5.0 }),
            1
        );
        assert_eq!(
            wheel_lines(mouse::ScrollDelta::Pixels { x: 0.0, y: 60.0 }),
            -2
        );
    }
}
