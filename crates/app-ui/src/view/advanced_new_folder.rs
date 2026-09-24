//! 高级新建文件夹弹窗视图：两种批量命名模式 + 移入勾选 + 最终名字预览。

use iced::widget::{
    button, checkbox, column, container, mouse_area, row, scrollable, space::Space, text_editor,
    text_input, Column,
};
use iced::{Alignment, Background, Border, Color, Element, Length};

use crate::anchored_popup::anchored_popup_expanding;
use crate::appearance::{
    context_menu_item_button_style, context_menu_style, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, muted_text_color, subtle_border_color,
};
use crate::icons::IconSymbol;
use crate::matugen_theme::ui_colors;
use crate::model::{
    AdvancedNewFolderAfter, AdvancedNewFolderMessage, AdvancedNewFolderMode,
    AdvancedNewFolderState, Message, ScrollbarVisibility,
};
use crate::typography::{localized_text, readable_text};

use super::option_controls::{secondary_action_button, segmented_choice_row, SegmentedChoice};

const PANEL_WIDTH: f32 = 480.0;
const PANEL_MAX_HEIGHT: f32 = 560.0;
const NAME_EDITOR_HEIGHT: f32 = 140.0;
const PREVIEW_HEIGHT: f32 = 160.0;

pub(super) fn advanced_new_folder_panel(state: &AdvancedNewFolderState) -> Element<'_, Message> {
    let planned = state.plan_detailed();

    let mode_row = segmented_choice_row(vec![
        SegmentedChoice {
            label: "Name list",
            selected: state.mode == AdvancedNewFolderMode::NameList,
            message: Message::AdvancedNewFolder(AdvancedNewFolderMessage::ModeSelected(
                AdvancedNewFolderMode::NameList,
            )),
            tooltip: None,
        },
        SegmentedChoice {
            label: "Numbered",
            selected: state.mode == AdvancedNewFolderMode::Numbered,
            message: Message::AdvancedNewFolder(AdvancedNewFolderMessage::ModeSelected(
                AdvancedNewFolderMode::Numbered,
            )),
            tooltip: None,
        },
    ]);

    let editor: Element<'_, Message> = match state.mode {
        AdvancedNewFolderMode::NameList => container(
            text_editor(&state.editor_content)
                .on_action(|action| {
                    Message::AdvancedNewFolder(AdvancedNewFolderMessage::DraftEdited(action))
                })
                .height(Length::Fixed(NAME_EDITOR_HEIGHT))
                .padding(6),
        )
        .width(Length::Fill)
        .style(name_editor_frame_style)
        .into(),
        AdvancedNewFolderMode::Numbered => row![
            number_input_column("Prefix", &state.prefix, |prefix| {
                Message::AdvancedNewFolder(AdvancedNewFolderMessage::PrefixChanged(prefix))
            }),
            number_input_column("Start", &state.start_number, |start| {
                Message::AdvancedNewFolder(AdvancedNewFolderMessage::StartNumberChanged(start))
            }),
            number_input_column("Count", &state.count, |count| {
                Message::AdvancedNewFolder(AdvancedNewFolderMessage::CountChanged(count))
            }),
        ]
        .spacing(8)
        .into(),
    };

    let gather_row: Option<Element<'_, Message>> = if state.gather_sources.is_empty() {
        None
    } else {
        Some(
            row![
                checkbox(state.gather).on_toggle(|gather| {
                    Message::AdvancedNewFolder(AdvancedNewFolderMessage::GatherToggled(gather))
                }),
                readable_text("Move selected items into the first folder").size(12),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        )
    };

    let preview_rows: Element<'_, Message> = if planned.is_empty() {
        readable_text("No folders to create").size(12).into()
    } else {
        let mut rows = Column::new().spacing(2);
        for (name, renamed) in &planned {
            let mut name_row = row![readable_text(name.clone()).size(13)].spacing(8);
            if *renamed {
                // 撞车被自动错开的名字整行淡化,尾部小字说明原因。
                name_row = name_row.push(
                    container(readable_text("Auto-renamed").size(11)).style(auto_renamed_style),
                );
            }
            let name_row = name_row.align_y(Alignment::Center);
            rows = rows.push(if *renamed {
                Element::from(container(name_row).style(auto_renamed_style))
            } else {
                Element::from(name_row)
            });
        }
        scrollable(rows)
            .height(Length::Fixed(PREVIEW_HEIGHT))
            .direction(enhanced_vertical_scrollbar_direction(
                ScrollbarVisibility::Visible,
                6.0,
            ))
            .style(enhanced_scrollbar_style(ScrollbarVisibility::Visible))
            .into()
    };

    let apply = create_split_button(
        planned.is_empty(),
        state.after_menu_open,
        state.split_button_hover,
    );

    let mut content_column = column![
        localized_text("Advanced New Folder").size(16),
        mode_row,
        editor,
    ];
    if let Some(gather_row) = gather_row {
        content_column = content_column.push(gather_row);
    }
    content_column = content_column.push(readable_text("Folders").size(12));
    content_column = content_column.push(preview_rows);
    let action_row = row![
        Space::new().width(Length::Fill),
        secondary_action_button(
            "Cancel",
            Message::AdvancedNewFolder(AdvancedNewFolderMessage::Cancel),
        ),
        apply,
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let after_menu: Option<Element<'_, Message>> = state
        .after_menu_open
        .then(|| after_create_menu(planned.is_empty()));
    let actions = anchored_popup_expanding(action_row, after_menu);
    content_column = content_column.push(actions);
    let content = content_column.spacing(12).width(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fill)
        .max_width(PANEL_WIDTH)
        .height(Length::Shrink)
        .max_height(PANEL_MAX_HEIGHT)
        .style(context_menu_style)
        .into()
}

fn number_input_column<'a>(
    label: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    let translated = crate::localization::translate_current(label);
    column![
        readable_text(label).size(11),
        text_input(&translated, value)
            .on_input(on_input)
            .padding([6, 8])
            .size(13)
            .width(Length::Fill),
    ]
    .spacing(3)
    .width(Length::Fill)
    .into()
}

/// 创建分裂按钮:主区执行默认创建,箭头区展开收尾动作菜单;
/// 两区共处同一个 primary 圆角容器,中间一条分隔线。
fn create_split_button(
    disabled: bool,
    menu_open: bool,
    button_hover: bool,
) -> Element<'static, Message> {
    let close = Message::AdvancedNewFolder(AdvancedNewFolderMessage::AfterMenuOpenChanged(false));
    let arrow_icon = move || {
        IconSymbol::ArrowDown
            .view(11.0)
            .style(move |theme, _status| {
                let colors = ui_colors(theme);
                iced::widget::svg::Style {
                    color: Some(if disabled {
                        colors.on_surface_variant
                    } else {
                        colors.on_primary_container
                    }),
                }
            })
    };
    let primary_section = button(readable_text("Create").size(12))
        .padding(iced::Padding {
            top: 6.0,
            right: 4.0,
            bottom: 6.0,
            left: 12.0,
        })
        .style(move |theme, status| {
            split_button_section_style(theme, status, disabled, menu_open, button_hover)
        });
    let primary_section: Element<'static, Message> = if disabled {
        Element::from(primary_section)
    } else {
        Element::from(primary_section.on_press(Message::AdvancedNewFolder(
            AdvancedNewFolderMessage::Confirmed,
        )))
    };
    let arrow_button = button(arrow_icon())
        .padding(iced::Padding {
            top: 6.0,
            right: 10.0,
            bottom: 6.0,
            left: 4.0,
        })
        .style(move |theme, status| {
            split_button_section_style(theme, status, disabled, menu_open, button_hover)
        });
    let open = Message::AdvancedNewFolder(AdvancedNewFolderMessage::AfterMenuOpenChanged(true));
    let arrow_section: Element<'static, Message> =
        if disabled {
            Element::from(mouse_area(arrow_button.on_press(close.clone())).on_enter(close))
        } else {
            Element::from(mouse_area(arrow_button).on_enter(open).on_press(
                Message::AdvancedNewFolder(AdvancedNewFolderMessage::AfterMenuOpenChanged(true)),
            ))
        };
    let highlight = button_hover || menu_open;
    mouse_area(
        container(
            row![primary_section, divider(), arrow_section]
                .spacing(2)
                .align_y(Alignment::Center),
        )
        .style(move |theme| split_button_container_style(theme, disabled, highlight)),
    )
    .on_enter(Message::AdvancedNewFolder(
        AdvancedNewFolderMessage::SplitButtonHoverChanged(true),
    ))
    .on_exit(Message::AdvancedNewFolder(
        AdvancedNewFolderMessage::SplitButtonHoverChanged(false),
    ))
    .into()
}

/// 分裂按钮中段的分隔线。
fn divider() -> Element<'static, Message> {
    Element::from(
        container(Space::new().width(1.0).height(16.0)).style(|theme| {
            iced::widget::container::Style {
                background: Some(iced::Background::Color(iced::Color {
                    a: 0.35,
                    ..ui_colors(theme).on_surface
                })),
                ..Default::default()
            }
        }),
    )
}

fn split_button_section_style(
    theme: &iced::Theme,
    _status: iced::widget::button::Status,
    disabled: bool,
    _menu_open: bool,
    _button_hover: bool,
) -> iced::widget::button::Style {
    let colors = ui_colors(theme);
    // 悬停高亮由整个按钮的容器统一处理,分区内不再单独提亮。
    button::Style {
        background: None,
        text_color: if disabled {
            colors.on_surface_variant
        } else {
            colors.on_primary_container
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn split_button_container_style(
    theme: &iced::Theme,
    disabled: bool,
    highlight: bool,
) -> iced::widget::container::Style {
    let colors = ui_colors(theme);
    iced::widget::container::Style {
        background: Some(Background::Color(match (disabled, highlight) {
            (true, _) => colors.surface_container_highest,
            (_, true) => Color {
                a: 0.12,
                ..colors.on_primary_container
            },
            (false, false) => colors.primary_container,
        })),
        text_color: Some(if disabled {
            colors.on_surface_variant
        } else {
            colors.on_primary_container
        }),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// 收尾动作菜单:创建后进入 / 新标签进入。鼠标移出菜单即收起。
/// 按翻译后文字估菜单宽度:全角≈字号、半角≈0.55 字号,加内边距余量。
fn advanced_menu_width(labels: &[&str], font_size: f32, padding: f32) -> f32 {
    let max_units = labels
        .iter()
        .map(|label| {
            crate::localization::translate_current(label)
                .chars()
                .map(|ch| if ch.is_ascii() { 0.55 } else { 1.0 })
                .sum::<f32>()
        })
        .fold(0.0, f32::max);
    max_units * font_size + padding
}

fn after_create_menu(disabled: bool) -> Element<'static, Message> {
    let labels = ["Enter after creation", "Open in new tab after creation"];
    // 容器宽 = 最长项文字宽:各项 Fill 后等宽,整行都是可点热区。
    let width = advanced_menu_width(&labels, 12.0, 44.0);
    let mut items = Column::new().spacing(2);
    if !disabled {
        for (label, after) in [
            ("Enter after creation", AdvancedNewFolderAfter::Enter),
            (
                "Open in new tab after creation",
                AdvancedNewFolderAfter::EnterInNewTab,
            ),
        ] {
            items = items.push(
                button(readable_text(label).size(12))
                    .padding([5, 10])
                    .width(Length::Fixed(width))
                    .style(context_menu_item_button_style())
                    .on_press(Message::AdvancedNewFolder(
                        AdvancedNewFolderMessage::ConfirmedWithAfter(after),
                    )),
            );
        }
    }
    mouse_area(container(items).padding(4).style(context_menu_style))
        .on_enter(Message::AdvancedNewFolder(
            AdvancedNewFolderMessage::MenuHoverChanged(true),
        ))
        .on_exit(Message::AdvancedNewFolder(
            AdvancedNewFolderMessage::MenuHoverChanged(false),
        ))
        .into()
}

/// 被自动错开的名字:整行淡化提示。
fn auto_renamed_style(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        text_color: Some(muted_text_color(theme)),
        ..Default::default()
    }
}

/// 编辑器外框：内容透明，只画 1px 边框提示可输入区域。
fn name_editor_frame_style(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: None,
        border: iced::Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}
