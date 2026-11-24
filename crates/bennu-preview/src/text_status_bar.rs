//! 文本预览底部状态栏：编码（点开选别的）、自动换行开关、缩放百分比、当前行与总行数。
//! 面板窄时按宽度收起次要项：先收缩放，再收换行，再收编码，位置一直在。

use bennu_localization::tr;
use bennu_theme::anchored_popup::anchored_dropup_menu;
use bennu_theme::controls::{icon_button_style, toggle_button_style};
use bennu_theme::icons::{
    icon_tone_style, rotated_chevron_right_view, themed_icon, IconSymbol, IconTone,
};
use bennu_theme::styles::{context_menu_item_button_style, context_menu_style};
use bennu_theme::typography::CAPTION_SIZE;
use bennu_theme::ui_colors;
use encoding_rs::{
    Encoding, BIG5, EUC_JP, EUC_KR, GB18030, GBK, IBM866, ISO_2022_JP, ISO_8859_10, ISO_8859_13,
    ISO_8859_14, ISO_8859_15, ISO_8859_16, ISO_8859_2, ISO_8859_3, ISO_8859_4, ISO_8859_5,
    ISO_8859_6, ISO_8859_7, ISO_8859_8, KOI8_R, KOI8_U, MACINTOSH, SHIFT_JIS, UTF_16BE, UTF_16LE,
    UTF_8, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255,
    WINDOWS_1256, WINDOWS_1257, WINDOWS_1258, WINDOWS_874, X_MAC_CYRILLIC,
};
use iced::widget::{button, container, responsive, row, scrollable, text, Column, Space};
use iced::{Alignment, Element, Length, Padding};

use crate::preview_message::PreviewMessage;
use crate::text_document::{
    HighlightStatus, LineCountStatus, TextPreviewDocument, TextViewerEvent,
};
use crate::text_view::{TextViewChange, TextViewSettings};

const STATUS_BAR_HEIGHT: f32 = 30.0;
/// 状态栏左右各留的边距。
const BAR_PADDING_X: f32 = 8.0;
/// 栏里各项之间的间距。
const ITEM_SPACING: f32 = 6.0;
/// 按钮左右各留的内边距。
const BUTTON_PADDING_X: f32 = 8.0;
const CHEVRON_SIZE: f32 = 10.0;
const CHEVRON_GAP: f32 = 4.0;
const MENU_ITEM_PADDING: [u16; 2] = [4, 10];
const MENU_CHECK_SIZE: f32 = 12.0;

/// 编码菜单里的一项：名称 + 语言说明。
struct EncodingOption {
    encoding: &'static Encoding,
    label: String,
}

pub(crate) fn text_status_bar<'a, Message>(
    document: &'a TextPreviewDocument,
    view: TextViewSettings,
) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    // Markdown 渲染模式不按行排版：换行开关不适用，位置只报总行数。
    let rendered = document.shows_rendered_markdown();
    responsive(move |size| {
        let position = position_label(document);
        let too_large = *document.highlight_status() == HighlightStatus::TooLarge;
        let too_large_label = tr!("Too large to highlight");
        let wrap_label = tr!("Wrap Lines");
        let zoom_label = format!("{}%", view.zoom_percent);
        let mut always_shown = caption_width(&position);
        if too_large {
            always_shown += caption_width(&too_large_label) + ITEM_SPACING;
        }
        let shown = shown_items(
            size.width,
            always_shown,
            document.encoding(),
            (!rendered).then_some(wrap_label.as_str()),
            &zoom_label,
        );

        let mut bar = row![].spacing(ITEM_SPACING).align_y(Alignment::Center);
        if shown.encoding {
            bar = bar.push(encoding_menu(document));
        }
        if shown.wrap {
            bar = bar.push(
                button(text(wrap_label).size(CAPTION_SIZE))
                    .padding([3.0, BUTTON_PADDING_X])
                    .style(toggle_button_style(view.wrap))
                    .on_press(change_view(TextViewChange::ToggleWrap)),
            );
        }
        if shown.zoom {
            bar = bar.push(
                button(text(zoom_label).size(CAPTION_SIZE))
                    .padding([3.0, BUTTON_PADDING_X])
                    .style(icon_button_style)
                    .on_press(change_view(TextViewChange::ZoomReset)),
            );
        }
        bar = bar.push(Space::new().width(Length::Fill));
        if too_large {
            bar = bar.push(text(too_large_label).size(CAPTION_SIZE));
        }
        bar = bar.push(text(position).size(CAPTION_SIZE));
        container(bar)
            .padding(Padding::from([0.0, BAR_PADDING_X]))
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

/// 窄面板里放得下的那几项。
#[derive(Debug, PartialEq)]
struct ShownItems {
    encoding: bool,
    wrap: bool,
    zoom: bool,
}

/// 位置（和“太大不着色”）总在，占 `always_shown` 宽；剩下的宽度按编码、换行、缩放的次序分，
/// 放不下的收起。`wrap_label` 是 `None` 时没有换行开关（Markdown 渲染模式）。
fn shown_items(
    bar_width: f32,
    always_shown: f32,
    encoding: &'static Encoding,
    wrap_label: Option<&str>,
    zoom_label: &str,
) -> ShownItems {
    let mut available = bar_width - BAR_PADDING_X * 2.0 - always_shown;
    let mut fits = |width: f32| {
        let fits = available >= width + ITEM_SPACING;
        if fits {
            available -= width + ITEM_SPACING;
        }
        fits
    };
    ShownItems {
        encoding: fits(encoding_button_width(encoding)),
        wrap: wrap_label.is_some_and(|label| fits(button_width(label))),
        zoom: fits(button_width(zoom_label)),
    }
}

/// 一段说明字大约多宽，只用来决定窄面板里收起哪几项：中日韩文字、全角标点按一个字号宽，
/// 其余按 0.6 个字号。宁可估宽一点，收起一项也比挤出栏外好。
fn caption_width(label: &str) -> f32 {
    label
        .chars()
        .map(|character| {
            if character.is_ascii() {
                CAPTION_SIZE * 0.6
            } else {
                CAPTION_SIZE
            }
        })
        .sum()
}

fn button_width(label: &str) -> f32 {
    caption_width(label) + BUTTON_PADDING_X * 2.0
}

/// 编码按钮只写当前编码的名字和一个箭头，语言说明放在菜单里：按钮要是和下拉框一样按最长的
/// 一项（“ISO-8859-16 · 东南欧”）定宽，窄面板里就把换行、缩放都挤掉了。
fn encoding_button_width(encoding: &'static Encoding) -> f32 {
    button_width(encoding.name()) + CHEVRON_GAP + CHEVRON_SIZE
}

/// 编码按钮，点开在上方弹出全部编码（贴着窗口底边，只能往上开），当前的打勾；点外面收起。
fn encoding_menu<'a, Message>(document: &TextPreviewDocument) -> Element<'a, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let current = document.encoding();
    let anchor = button(
        row![
            text(current.name()).size(CAPTION_SIZE),
            rotated_chevron_right_view(90.0, CHEVRON_SIZE).style(icon_tone_style(IconTone::Normal)),
        ]
        .spacing(CHEVRON_GAP)
        .align_y(Alignment::Center),
    )
    .padding([3.0, BUTTON_PADDING_X])
    .style(icon_button_style)
    .on_press(Message::from(PreviewMessage::TextEncodingMenuToggled));
    let popup = document.encoding_menu_open().then(|| {
        let mut list = Column::new().spacing(2);
        for option in encoding_options() {
            let check: Element<'a, Message> = if option.encoding == current {
                themed_icon(IconSymbol::Check, IconTone::Normal, MENU_CHECK_SIZE).into()
            } else {
                Space::new().width(Length::Fixed(MENU_CHECK_SIZE)).into()
            };
            list = list.push(
                button(
                    row![check, text(option.label).size(CAPTION_SIZE)]
                        .spacing(8)
                        .align_y(Alignment::Center),
                )
                .padding(MENU_ITEM_PADDING)
                .width(Length::Fill)
                .style(context_menu_item_button_style())
                .on_press(Message::from(
                    PreviewMessage::TextPreviewEncodingSelected(option.encoding),
                )),
            );
        }
        container(scrollable(list))
            .padding(4)
            .style(context_menu_style)
            .into()
    });
    anchored_dropup_menu(anchor, popup, || {
        Message::from(PreviewMessage::TextEncodingMenuDismissed)
    })
}

fn change_view<Message: From<PreviewMessage>>(change: TextViewChange) -> Message {
    Message::from(PreviewMessage::TextViewer(TextViewerEvent::ChangeView(
        change,
    )))
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
    // 渲染模式没有「当前行」，只报总行数。
    let viewport = document
        .viewport()
        .filter(|_| !document.shows_rendered_markdown());
    let line = viewport.and_then(|viewport| {
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

    /// 窄面板（右侧面板常见的四百来宽）里四项都放得下：编码按钮只按当前编码的名字算宽，
    /// 不再按最长的那一项（“ISO-8859-16 · 东南欧”）把换行、缩放挤掉。
    #[test]
    fn a_narrow_panel_still_fits_every_item() {
        let position = caption_width("第 1 行，共 45 行");
        let everything = ShownItems {
            encoding: true,
            wrap: true,
            zoom: true,
        };
        assert_eq!(
            shown_items(400.0, position, UTF_8, Some("自动换行"), "100%"),
            everything
        );
        assert_eq!(
            shown_items(400.0, position, ISO_8859_16, Some("自动换行"), "100%"),
            everything
        );
    }

    #[test]
    fn items_fold_away_zoom_first_then_wrap_then_encoding() {
        let position = caption_width("第 1 行，共 45 行");
        let shown = |width| shown_items(width, position, UTF_8, Some("自动换行"), "100%");
        assert_eq!(
            shown(300.0),
            ShownItems {
                encoding: true,
                wrap: true,
                zoom: false,
            }
        );
        assert_eq!(
            shown(230.0),
            ShownItems {
                encoding: true,
                wrap: false,
                zoom: false,
            }
        );
        assert_eq!(
            shown(140.0),
            ShownItems {
                encoding: false,
                wrap: false,
                zoom: false,
            }
        );
        // Markdown 渲染模式没有换行开关，空出来的给缩放。
        assert!(shown_items(300.0, position, UTF_8, None, "100%").zoom);
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
