//! 文本预览底部状态栏：编码（可选）、自动换行开关、缩放百分比、当前行与总行数。
//! 面板窄时按宽度收起次要项：先收缩放，再收换行，再收编码，位置一直在。

use bennu_localization::tr;
use bennu_theme::controls::{dropdown, icon_button_style, toggle_button_style};
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;
use encoding_rs::{
    Encoding, BIG5, EUC_JP, EUC_KR, GB18030, GBK, IBM866, ISO_2022_JP, ISO_8859_10, ISO_8859_13,
    ISO_8859_14, ISO_8859_15, ISO_8859_16, ISO_8859_2, ISO_8859_3, ISO_8859_4, ISO_8859_5,
    ISO_8859_6, ISO_8859_7, ISO_8859_8, KOI8_R, KOI8_U, MACINTOSH, SHIFT_JIS, UTF_16BE, UTF_16LE,
    UTF_8, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255,
    WINDOWS_1256, WINDOWS_1257, WINDOWS_1258, WINDOWS_874, X_MAC_CYRILLIC,
};
use iced::widget::{button, container, responsive, row, text, Space};
use iced::{Alignment, Element, Length, Padding};

use crate::preview_message::PreviewMessage;
use crate::text_document::{LineCountStatus, TextPreviewDocument, TextViewerEvent};
use crate::text_view::{TextViewChange, TextViewSettings};

const STATUS_BAR_HEIGHT: f32 = 30.0;
/// 各项大致占的宽度（含间距），用来决定窄面板里收起哪些。
const POSITION_WIDTH: f32 = 190.0;
const ENCODING_WIDTH: f32 = 170.0;
const WRAP_WIDTH: f32 = 96.0;
const ZOOM_WIDTH: f32 = 64.0;

/// 编码下拉里的一项：名称 + 语言说明。
#[derive(Debug, Clone, PartialEq)]
struct EncodingOption {
    encoding: &'static Encoding,
    label: String,
}

impl std::fmt::Display for EncodingOption {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label)
    }
}

pub(crate) fn text_status_bar<'a, Message>(
    document: &'a TextPreviewDocument,
    view: TextViewSettings,
    show_wrap: bool,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    responsive(move |size| {
        let mut available = size.width - POSITION_WIDTH;
        let mut fits = |width: f32| {
            let fits = available >= width;
            if fits {
                available -= width;
            }
            fits
        };
        let show_encoding = fits(ENCODING_WIDTH);
        let show_wrap = show_wrap && fits(WRAP_WIDTH);
        let show_zoom = fits(ZOOM_WIDTH);

        let mut bar = row![].spacing(6).align_y(Alignment::Center);
        if show_encoding {
            bar = bar.push(encoding_picker(document));
        }
        if show_wrap {
            bar = bar.push(
                button(text(tr!("Wrap Lines")).size(CAPTION_SIZE))
                    .padding([3, 8])
                    .style(toggle_button_style(view.wrap))
                    .on_press(change_view(TextViewChange::ToggleWrap)),
            );
        }
        if show_zoom {
            bar = bar.push(
                button(text(format!("{}%", view.zoom_percent)).size(CAPTION_SIZE))
                    .padding([3, 8])
                    .style(icon_button_style)
                    .on_press(change_view(TextViewChange::ZoomReset)),
            );
        }
        bar = bar
            .push(Space::new().width(Length::Fill))
            .push(text(position_label(document)).size(CAPTION_SIZE));
        container(bar)
            .padding(Padding::from([0, 8]))
            .height(Length::Fill)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .style(|theme| container::Style {
                background: Some(ui_colors(theme).surface_container_low.into()),
                text_color: Some(ui_colors(theme).on_surface_variant),
                ..container::Style::default()
            })
            .into()
    })
    .height(Length::Fixed(STATUS_BAR_HEIGHT))
    .into()
}

fn change_view<Message: From<PreviewMessage>>(change: TextViewChange) -> Message {
    Message::from(PreviewMessage::TextViewer(TextViewerEvent::ChangeView(
        change,
    )))
}

fn encoding_picker<'a, Message>(document: &TextPreviewDocument) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let options = encoding_options();
    let selected = options
        .iter()
        .find(|option| option.encoding == document.encoding())
        .cloned();
    dropdown(options, selected, |option: EncodingOption| {
        Message::from(PreviewMessage::TextPreviewEncodingSelected(option.encoding))
    })
    .text_size(CAPTION_SIZE)
    .padding([2, 8])
    .into()
}

/// 可以手动选的编码：先是 Unicode，再按语言分组。
fn encoding_options() -> Vec<EncodingOption> {
    let entries: [(&'static Encoding, String); 38] = [
        (UTF_8, String::new()),
        (UTF_16LE, String::new()),
        (UTF_16BE, String::new()),
        (GBK, tr!("Simplified Chinese")),
        (GB18030, tr!("Simplified Chinese")),
        (BIG5, tr!("Traditional Chinese")),
        (SHIFT_JIS, tr!("Japanese")),
        (EUC_JP, tr!("Japanese")),
        (ISO_2022_JP, tr!("Japanese")),
        (EUC_KR, tr!("Korean")),
        (WINDOWS_1252, tr!("Western European")),
        (ISO_8859_15, tr!("Western European")),
        (MACINTOSH, tr!("Western European")),
        (WINDOWS_1250, tr!("Central European")),
        (ISO_8859_2, tr!("Central European")),
        (WINDOWS_1251, tr!("Cyrillic")),
        (ISO_8859_5, tr!("Cyrillic")),
        (KOI8_R, tr!("Cyrillic")),
        (KOI8_U, tr!("Cyrillic")),
        (IBM866, tr!("Cyrillic")),
        (X_MAC_CYRILLIC, tr!("Cyrillic")),
        (WINDOWS_1253, tr!("Greek")),
        (ISO_8859_7, tr!("Greek")),
        (WINDOWS_1254, tr!("Turkish")),
        (WINDOWS_1255, tr!("Hebrew")),
        (ISO_8859_8, tr!("Hebrew")),
        (WINDOWS_1256, tr!("Arabic")),
        (ISO_8859_6, tr!("Arabic")),
        (WINDOWS_1257, tr!("Baltic")),
        (ISO_8859_4, tr!("Baltic")),
        (ISO_8859_13, tr!("Baltic")),
        (WINDOWS_1258, tr!("Vietnamese")),
        (WINDOWS_874, tr!("Thai")),
        (ISO_8859_3, tr!("South European")),
        (ISO_8859_10, tr!("Nordic")),
        (ISO_8859_14, tr!("Celtic")),
        (ISO_8859_16, tr!("South-Eastern European")),
        (encoding_rs::ISO_8859_8_I, tr!("Hebrew")),
    ];
    entries
        .into_iter()
        .map(|(encoding, language)| EncodingOption {
            encoding,
            label: if language.is_empty() {
                encoding.name().to_owned()
            } else {
                format!("{} · {language}", encoding.name())
            },
        })
        .collect()
}

/// 「第 120 行，共 35,201 行」；统计中「第 120 行，正在统计…」；行号还不知道时只说统计中。
pub(crate) fn position_label(document: &TextPreviewDocument) -> String {
    let line = document.viewport().and_then(|viewport| {
        let row = viewport
            .cursor
            .map_or(viewport.first_row, |cursor| cursor.row);
        let index = document.row_index_containing(row)?;
        document.rows()[index].line_number
    });
    match (line, document.line_count_status()) {
        (Some(line), LineCountStatus::Done { total }) => {
            tr!("Line %1 of %2", group_digits(line + 1), group_digits(total))
        }
        (Some(line), LineCountStatus::Counting { .. }) => {
            tr!("Line %1, counting lines…", group_digits(line + 1))
        }
        (None, LineCountStatus::Done { total }) => tr!("%1 lines", group_digits(total)),
        (None, LineCountStatus::Counting { .. }) => tr!("Counting lines…"),
    }
}

/// 千分位：35201 → 35,201。
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_document::{document_from_bytes, TextJump};

    #[test]
    fn digits_are_grouped_by_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(35_201), "35,201");
        assert_eq!(group_digits(20_000_000), "20,000,000");
    }

    #[test]
    fn every_encoding_option_has_a_distinct_label() {
        let options = encoding_options();
        for (index, option) in options.iter().enumerate() {
            assert!(options[index + 1..]
                .iter()
                .all(|other| other.encoding != option.encoding));
        }
        assert!(options.iter().any(|option| option.label == "UTF-8"));
    }

    #[test]
    fn position_says_counting_until_the_count_is_done() {
        let bytes: Vec<u8> = (0..50)
            .flat_map(|index| format!("line {index}\n").into_bytes())
            .collect();
        let document = document_from_bytes(bytes, TextJump::Start);
        // 测试文档没有跑统计：行号来自第一个窗口，总数还在统计中。
        assert_eq!(position_label(&document), "Counting lines…");
    }
}
