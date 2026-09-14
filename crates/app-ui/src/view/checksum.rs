use iced::widget::{button, column, container, row, scrollable, text_input, Column, Space};
use iced::{Alignment, Element, Length};

use file_core::{ChecksumAlgorithm, ALL_CHECKSUM_ALGORITHMS};

use crate::app::checksum::{
    ChecksumComputation, ChecksumExpectedVerdict, ChecksumFileVerdict, ChecksumMessage,
    ChecksumState,
};
use crate::appearance::{context_menu_style, muted_text_color};
use crate::formatting::format_middle_ellipsized_text;
use crate::icons::IconSymbol;
use crate::model::Message;
use crate::typography::readable_text;

use super::option_controls::secondary_action_button;
use super::settings_group::settings_card;
use super::{themed_icon, IconTone, MENU_ICON_SIZE};

const CHECKSUM_PANEL_WIDTH: f32 = 560.0;
const SECTION_SPACING: f32 = 12.0;
/// SHA-512 摘要 128 位太长,面板内显示中间省略,完整值走复制按钮。
const DIGEST_MAX_CHARS: usize = 52;
const FILE_CHIP_MAX_CHARS: usize = 26;
const ACTIVE_PATH_MAX_CHARS: usize = 62;
const CHECKSUM_FILE_PATH_MAX_CHARS: usize = 44;
const FILE_LIST_MAX_HEIGHT: f32 = 110.0;
/// 单个文件 chip 的高度(文本 12 + 上下 padding 4×2 + 行距 2 + 边距)。
const FILE_CHIP_ROW_HEIGHT: f32 = 26.0;
const NOTICE_ICON_SIZE: f32 = 14.0;

pub(super) fn checksum_panel(state: &ChecksumState) -> Element<'_, Message> {
    let title = row![
        themed_icon(IconSymbol::Hash, IconTone::Normal, MENU_ICON_SIZE),
        readable_text("File Checksum").size(16).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    // 标题、路径与关闭按钮钉死在面板两端;中间内容用 Shrink 滚动区:
    // 窗口够高时按内容自然伸缩,窗口太矮时中间出滚动条,关闭按钮
    // 永远可见可达(iced flex 会把剩余高度传给 Shrink 子元素)。
    let mut body = column![]
        .spacing(SECTION_SPACING)
        .width(Length::Fill);

    if state.files().len() > 1 {
        body = body.push(file_chip_list(state));
    }

    body = body.push(computation_section(state));
    body = body.push(verify_section(state));

    let content = column![
        title,
        readable_text(format_middle_ellipsized_text(
            &state.active_file().to_string_lossy(),
            ACTIVE_PATH_MAX_CHARS,
        ))
        .size(11)
        .width(Length::Fill),
        scrollable(body).width(Length::Fill).height(Length::Shrink),
        row![
            Space::new().width(Length::Fill),
            secondary_action_button("Close", Message::DismissFloating),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(SECTION_SPACING)
    .width(Length::Fill);

    container(content)
        .padding(16)
        .width(Length::Fixed(CHECKSUM_PANEL_WIDTH))
        .style(context_menu_style)
        .into()
}

/// 多选文件切换列表:当前文件带对勾标记,点击其它文件切换并重新计算。
fn file_chip_list(state: &ChecksumState) -> Element<'static, Message> {
    let mut chips = Column::new().spacing(2).width(Length::Fill);
    for (index, path) in state.files().iter().enumerate() {
        let chip_icon = if index == state.active_index() {
            IconSymbol::Check
        } else {
            IconSymbol::File
        };
        let label = path
            .file_name()
            .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
        chips = chips.push(
            button(
                row![
                    themed_icon(chip_icon, IconTone::Normal, 12.0),
                    readable_text(format_middle_ellipsized_text(&label, FILE_CHIP_MAX_CHARS,))
                        .size(12)
                        .width(Length::Fill),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .on_press(Message::Checksum(ChecksumMessage::FileSelected(index)))
            .padding([4, 6])
            .width(Length::Fill)
            .style(crate::appearance::context_menu_item_button_style()),
        );
    }

    // 高度随文件数伸缩,超出上限才滚动。
    let list_height = (state.files().len() as f32 * FILE_CHIP_ROW_HEIGHT).min(FILE_LIST_MAX_HEIGHT);
    scrollable(chips)
        .height(Length::Fixed(list_height))
        .width(Length::Fill)
        .into()
}

/// 计算区:四行校验卡常驻渲染,取消/失败提示行挂其下。
/// 卡片布局不随状态增删元素,否则切换文件时面板高度反复跳动。
fn computation_section(state: &ChecksumState) -> Element<'static, Message> {
    let mut body = column![digest_card(state)]
        .spacing(6)
        .width(Length::Fill);

    // 计算中不渲染任何进度/取消行:该行的显隐引起面板高度抖动,值位 "—" 已表达
    // 未就绪;要中止直接关面板,dismiss_floating 会取消仍在跑的计算。
    let status_line: Option<Element<'static, Message>> = match state.computation() {
        ChecksumComputation::Computing { .. } | ChecksumComputation::Completed(_) => None,
        ChecksumComputation::Canceled => Some(
            retry_notice(crate::localization::translate_current(
                "Checksum computation canceled",
            ))
            .into(),
        ),
        ChecksumComputation::Failed(error) => Some(retry_notice(error.clone()).into()),
    };
    if let Some(line) = status_line {
        body = body.push(line);
    }

    section(body.into())
}

fn digest_card(state: &ChecksumState) -> Element<'static, Message> {
    let digests = match state.computation() {
        ChecksumComputation::Completed(digests) => Some(digests),
        _ => None,
    };
    let rows = ALL_CHECKSUM_ALGORITHMS
        .into_iter()
        .map(|algorithm| digest_row(state, digests.map(|d| d.digest(algorithm)), algorithm))
        .collect();
    settings_card(rows)
}

fn digest_row(
    state: &ChecksumState,
    digest: Option<&str>,
    algorithm: ChecksumAlgorithm,
) -> Element<'static, Message> {
    let value_text = match digest {
        Some(digest) => format_middle_ellipsized_text(digest, DIGEST_MAX_CHARS),
        None => String::from("—"),
    };
    // 对勾只在"该算法当前确实显示着已复制的值"时出现,否则取消/重算后旧对勾会挂在空值行上。
    let copy_icon = if digest.is_some() && state.last_copied() == Some(algorithm) {
        IconSymbol::Check
    } else {
        IconSymbol::Copy
    };
    // 无值(未算完/取消/失败)时不挂 on_press,复制只在真值可复制时可用。
    let mut copy_button = button(themed_icon(copy_icon, IconTone::Normal, 13.0))
        .padding(4)
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .style(crate::appearance::navigation_icon_button_style());
    if digest.is_some() {
        copy_button = copy_button.on_press(Message::Checksum(
            ChecksumMessage::HashCopyRequested(algorithm),
        ));
    }
    row![
        readable_text(algorithm.label())
            .size(12)
            .width(Length::Fixed(64.0)),
        readable_text(value_text).size(12).width(Length::Fill),
        copy_button,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn verify_section(state: &ChecksumState) -> Element<'static, Message> {
    let mut rows = vec![text_input(
        &crate::localization::translate_current("Paste expected checksum"),
        state.expected_text(),
    )
    .on_input(|text| Message::Checksum(ChecksumMessage::ExpectedValueChanged(text)))
    .padding([6, 8])
    .size(13)
    .width(Length::Fill)
    .into()];

    if let Some(verdict) = expected_value_verdict_line(state.expected_value_verdict()) {
        rows.push(verdict);
    }

    let mut load_row = row![secondary_action_button(
        "Load checksum file...",
        Message::Checksum(ChecksumMessage::ChecksumFileLoadPressed)
    )]
    .spacing(8)
    .align_y(Alignment::Center);
    if let Some(verification) = state.checksum_file() {
        load_row = load_row.push(
            readable_text(format_middle_ellipsized_text(
                &verification.path().to_string_lossy(),
                CHECKSUM_FILE_PATH_MAX_CHARS,
            ))
            .size(11),
        );
    }
    rows.push(load_row.width(Length::Fill).into());

    if let Some(verdict) = state.checksum_file_verdict() {
        rows.push(checksum_file_verdict_line(verdict));
    }

    section(
        Column::with_children(rows)
            .spacing(6)
            .width(Length::Fill)
            .into(),
    )
}

/// 期望值比对结论行;没有输入时不占位。
fn expected_value_verdict_line(
    verdict: ChecksumExpectedVerdict,
) -> Option<Element<'static, Message>> {
    let localize = crate::localization::translate_current;
    let line = match verdict {
        ChecksumExpectedVerdict::Empty => return None,
        ChecksumExpectedVerdict::Invalid => verdict_line(
            IconSymbol::TriangleAlert,
            IconTone::Normal,
            localize("Enter a valid hex checksum."),
        )
        .into(),
        ChecksumExpectedVerdict::Pending => {
            muted_info_line(localize("Waiting for the computation to finish...")).into()
        }
        ChecksumExpectedVerdict::NoMatch => verdict_line(
            IconSymbol::TriangleAlert,
            IconTone::Warning,
            localize("No checksum matches the expected value."),
        )
        .into(),
        ChecksumExpectedVerdict::Matched(algorithms) => verdict_line(
            IconSymbol::Check,
            IconTone::Normal,
            matched_text(&localize("Checksum matches"), &algorithms),
        )
        .into(),
    };
    Some(line)
}

fn checksum_file_verdict_line(verdict: ChecksumFileVerdict) -> Element<'static, Message> {
    let localize = crate::localization::translate_current;
    match verdict {
        ChecksumFileVerdict::Pending => {
            muted_info_line(localize("Waiting for the computation to finish...")).into()
        }
        ChecksumFileVerdict::BareMatched => verdict_line(
            IconSymbol::Check,
            IconTone::Normal,
            localize("Checksum file value matches."),
        )
        .into(),
        ChecksumFileVerdict::BareMismatch => verdict_line(
            IconSymbol::TriangleAlert,
            IconTone::Warning,
            localize("Checksum file value does not match."),
        )
        .into(),
        ChecksumFileVerdict::EntryMatched => verdict_line(
            IconSymbol::Check,
            IconTone::Normal,
            localize("File entry in the checksum file matches."),
        )
        .into(),
        ChecksumFileVerdict::EntryMismatch => verdict_line(
            IconSymbol::TriangleAlert,
            IconTone::Warning,
            localize("File entry in the checksum file does not match."),
        )
        .into(),
        ChecksumFileVerdict::EntryNotFound => {
            muted_info_line(localize("The checksum file has no entry for this file.")).into()
        }
    }
}

fn matched_text(prefix: &str, algorithms: &[ChecksumAlgorithm]) -> String {
    let labels = algorithms
        .iter()
        .map(|algorithm| algorithm.label())
        .collect::<Vec<_>>()
        .join(" / ");
    format!("{prefix} ({labels})")
}

/// 分区:muted 标题 + 卡片行组;与设置窗口的分组观感一致。
fn section(body: Element<'static, Message>) -> Element<'static, Message> {
    column![
        container(readable_text("Verify"))
            .width(Length::Fill)
            .style(muted_container_style),
        body,
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// 取消/失败后的提示行,附重新计算按钮。
fn retry_notice(message: String) -> iced::widget::Row<'static, Message> {
    row![
        themed_icon(
            IconSymbol::TriangleAlert,
            IconTone::Warning,
            NOTICE_ICON_SIZE
        ),
        readable_text(message).size(12).width(Length::Fill),
        secondary_action_button("Retry", Message::Checksum(ChecksumMessage::RetryPressed)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
}

fn verdict_line(
    icon: IconSymbol,
    tone: IconTone,
    message: String,
) -> iced::widget::Row<'static, Message> {
    row![
        themed_icon(icon, tone, NOTICE_ICON_SIZE),
        readable_text(message).size(12).width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
}

fn muted_info_line(message: String) -> iced::widget::Text<'static> {
    readable_text(message).size(12)
}

fn muted_container_style(theme: &iced::Theme) -> container::Style {
    container::Style {
        text_color: Some(muted_text_color(theme)),
        ..container::Style::default()
    }
}
