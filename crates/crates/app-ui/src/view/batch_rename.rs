use crate::app::scrollbar::{enhanced_scrollbar, scrollbar_on_scroll, ScrollbarAxis};
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use iced::widget::{
    button, checkbox, column, container, mouse_area, pick_list, row, scrollable, text, text_input,
    Column, Row, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::anchored_popup::anchored_popup;
use crate::appearance::{
    base_text_color, context_menu_item_button_style, context_menu_style, dragged_row_style,
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, muted_text_color,
    path_suggestion_item_style, subtle_border_color,
};
use crate::formatting::format_middle_ellipsized_text;
use crate::matugen_theme::ui_colors;
use crate::model::{
    BatchRenameExtensionMode, BatchRenameInsertMode, BatchRenameMessage, BatchRenamePreviewRow,
    BatchRenameRemoveClass, BatchRenameRemoveMode, BatchRenameReplaceRule, BatchRenameReplaceScope,
    BatchRenameRule, BatchRenameRuleKind, BatchRenameRuleParams, BatchRenameSliceMode,
    BatchRenameSortMode, BatchRenameState, BatchRenameTemplateToken, Message, ScrollbarRegion,
    ScrollbarViewport, ScrollbarVisibility,
};
use crate::typography::{localized_text, readable_text};

use super::batch_rename_preview_name_input_id;
use super::option_controls::{
    inactive_primary_action_button, primary_action_button, secondary_action_button,
};

const BATCH_RENAME_PANEL_WIDTH: f32 = 720.0;
// 面板自然高度上限(规则区+预览区满配)。浮层把窗口高-2×边距作为 max 传入,
// 面板 Fill 高度在该上限与窗口之间取小者,保证小窗口收缩、大窗口外观不变。
const BATCH_RENAME_PANEL_MAX_HEIGHT: f32 = 700.0;
const BATCH_RENAME_PREVIEW_HEIGHT: f32 = 240.0;
const BATCH_RENAME_PATH_MAX_CHARS: usize = 38;
const BATCH_RENAME_RULES_AREA_HEIGHT: f32 = 170.0;

fn scrollbar_visibility_stub() -> ScrollbarVisibility {
    ScrollbarVisibility::Visible
}

pub(super) fn batch_rename_panel(
    state: &BatchRenameState,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let header = row![
        localized_text("Batch Rename").size(16).width(Length::Fill),
        localized_text("Items").size(12),
        readable_text(state.items.len().to_string()).size(12),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let sort_row = row![
        localized_text("Sort order")
            .size(12)
            .width(Length::Fixed(84.0)),
        pick_list(
            BatchRenameSortMode::options(),
            Some(state.sort.mode),
            |mode| { Message::BatchRename(BatchRenameMessage::SortModeSelected(mode)) }
        )
        .width(Length::Fill)
        .text_size(12)
        .padding([5, 8]),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    // 按钮钉底:表单区(标题/排序/规则/编辑器/预览)整体滚动,取消/应用留在
    // 滚动区外。窗口矮时滚动区收缩,按钮行永远贴底可见;大窗口下受
    // BATCH_RENAME_PANEL_MAX_HEIGHT 兜底,外观与固定高度时代一致。
    let form = column![
        header,
        sort_row,
        rules_section(state),
        selected_rule_editor(state),
        preview_rows(state, scrollbar_visibility, scrollbar_viewport),
    ]
    .spacing(12)
    .width(Length::Fill);

    let form_scroller = scrollable(form)
        .width(Length::Fill)
        .height(Length::Fill)
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility_stub(),
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility_stub()));

    let content = column![form_scroller, action_row(state)]
        .spacing(12)
        .width(Length::Fill)
        .height(Length::Fill);

    container(content)
        .padding(14)
        .width(Length::Fill)
        .max_width(BATCH_RENAME_PANEL_WIDTH)
        .height(Length::Fill)
        .max_height(BATCH_RENAME_PANEL_MAX_HEIGHT)
        .style(context_menu_style)
        .into()
}

fn rules_section(state: &BatchRenameState) -> Element<'_, Message> {
    let mut chips = Column::new().spacing(4);
    for (index, rule) in state.rules.iter().enumerate() {
        chips = chips.push(rule_chip_row(
            rule,
            state.selected_rule == Some(rule.id),
            index == 0,
            index == state.rules.len() - 1,
        ));
    }

    let chips_area: Element<'_, Message> = if state.rules.is_empty() {
        localized_text("No rules yet. Add a rule to rename files.")
            .size(12)
            .into()
    } else {
        scrollable(chips)
            .height(Length::Fixed(BATCH_RENAME_RULES_AREA_HEIGHT))
            .direction(enhanced_vertical_scrollbar_direction(
                scrollbar_visibility_stub(),
                6.0,
            ))
            .style(enhanced_scrollbar_style(scrollbar_visibility_stub()))
            .into()
    };

    let add_button = button(
        row![
            readable_text("+").size(13),
            localized_text("Add rule").size(12),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding([5, 10])
    .style(add_rule_button_style())
    .on_press(Message::BatchRename(BatchRenameMessage::AddRuleMenuToggled));

    let add_menu: Option<Element<'_, Message>> = if state.add_rule_menu_open {
        Some(add_rule_menu())
    } else {
        None
    };

    column![
        section_title("Rules"),
        chips_area,
        anchored_popup(add_button, add_menu),
    ]
    .spacing(6)
    .into()
}

fn add_rule_menu() -> Element<'static, Message> {
    let mut items = Column::new().spacing(2);
    for kind in BatchRenameRuleKind::ALL {
        items = items.push(
            button(readable_text(kind.label_key()).size(12))
                .padding([5, 10])
                .width(Length::Fill)
                .style(context_menu_item_button_style())
                .on_press(Message::BatchRename(BatchRenameMessage::AddRuleSelected(
                    kind,
                ))),
        );
    }

    container(items)
        .padding(4)
        .width(Length::Fixed(190.0))
        .style(context_menu_style)
        .into()
}

fn rule_chip_row(
    rule: &BatchRenameRule,
    is_selected: bool,
    is_first: bool,
    is_last: bool,
) -> Element<'static, Message> {
    let id = rule.id;
    let enabled_toggle = checkbox(rule.enabled)
        .on_toggle(move |_| Message::BatchRename(BatchRenameMessage::RuleEnabledToggled(id)));

    let summary = button(readable_text(rule_summary(rule)).size(12))
        .padding([4, 8])
        .width(Length::Fill)
        .style(rule_chip_style(is_selected))
        .on_press(Message::BatchRename(BatchRenameMessage::RuleSelected(id)));

    let move_up = button(readable_text("↑").size(12))
        .padding([4, 7])
        .style(chip_action_button_style());
    let move_up = if is_first {
        move_up
    } else {
        move_up.on_press(Message::BatchRename(BatchRenameMessage::RuleMoved(id, -1)))
    };

    let move_down = button(readable_text("↓").size(12))
        .padding([4, 7])
        .style(chip_action_button_style());
    let move_down = if is_last {
        move_down
    } else {
        move_down.on_press(Message::BatchRename(BatchRenameMessage::RuleMoved(id, 1)))
    };

    let remove = button(readable_text("×").size(12))
        .padding([4, 7])
        .style(chip_action_button_style())
        .on_press(Message::BatchRename(BatchRenameMessage::RuleRemoved(id)));

    let chip = row![enabled_toggle, summary, move_up, move_down, remove,]
        .spacing(6)
        .align_y(Alignment::Center);

    let chip: Element<'static, Message> = if rule.enabled {
        chip.into()
    } else {
        // 禁用规则整行弱化，参数保留但管道跳过
        container(chip).style(disabled_chip_container_style).into()
    };

    chip
}

fn rule_summary(rule: &BatchRenameRule) -> String {
    let label = crate::localization::translate_current(rule.params.kind().label_key());
    match &rule.params {
        BatchRenameRuleParams::Template(params) => format!(
            "{label}: {}",
            crate::localization::translate_current(&params.template)
        ),
        BatchRenameRuleParams::Replace(params) => {
            format!("{label}: {} → {}", params.find, params.replacement)
        }
        BatchRenameRuleParams::Insert(params) => format!("{label}: {}", params.text),
        BatchRenameRuleParams::Slice(_) => label,
        BatchRenameRuleParams::Remove(params) => format!("{label}: {}", params.text),
        BatchRenameRuleParams::Case(params) => {
            format!(
                "{label}: {}",
                crate::localization::translate_current(params.label())
            )
        }
        BatchRenameRuleParams::Sequence(params) => {
            let padding = params.padding_input.trim().parse::<usize>().unwrap_or(2);
            format!("{label}: {}{:0padding$}+", params.prefix, 1)
        }
        BatchRenameRuleParams::Random(params) => {
            format!(
                "{label}: {}",
                crate::localization::translate_current(match params.mode {
                    crate::model::BatchRenameRandomMode::Off => "Off",
                    crate::model::BatchRenameRandomMode::ReplaceStem => "Replace stem",
                    crate::model::BatchRenameRandomMode::Prefix => "Prefix",
                    crate::model::BatchRenameRandomMode::Suffix => "Suffix",
                })
            )
        }
        BatchRenameRuleParams::Extension(params) => {
            format!(
                "{label}: {}",
                crate::localization::translate_current(match params.mode {
                    BatchRenameExtensionMode::Preserve => "Preserve",
                    BatchRenameExtensionMode::Remove => "Remove",
                    BatchRenameExtensionMode::Replace => "Replace",
                    BatchRenameExtensionMode::Lowercase => "lowercase",
                    BatchRenameExtensionMode::Uppercase => "UPPERCASE",
                })
            )
        }
        BatchRenameRuleParams::Regex(params) => format!("{label}: {}", params.pattern),
        BatchRenameRuleParams::List(params) => {
            let lines = params
                .names
                .lines()
                .count()
                .max(if params.names.trim().is_empty() { 0 } else { 1 });
            format!("{label}: {lines}")
        }
    }
}

fn selected_rule_editor(state: &BatchRenameState) -> Element<'_, Message> {
    let Some(rule) = state.selected_rule() else {
        return Space::new().height(Length::Fixed(0.0)).into();
    };

    let editor: Element<'_, Message> = match &rule.params {
        BatchRenameRuleParams::Template(params) => template_editor(rule.id, params),
        BatchRenameRuleParams::Replace(params) => replace_editor(rule.id, params),
        BatchRenameRuleParams::Insert(params) => insert_editor(rule.id, params),
        BatchRenameRuleParams::Slice(params) => slice_editor(rule.id, params),
        BatchRenameRuleParams::Remove(params) => remove_editor(rule.id, params),
        BatchRenameRuleParams::Case(params) => column![
            section_title("Letter case"),
            pick_list(
                crate::model::BatchRenameCaseRule::options(),
                Some(*params),
                move |value| Message::BatchRename(BatchRenameMessage::CaseSelected(rule.id, value)),
            )
            .width(Length::Fill)
            .text_size(12)
            .padding([5, 8]),
        ]
        .spacing(6)
        .into(),
        BatchRenameRuleParams::Sequence(params) => sequence_editor(rule.id, params),
        BatchRenameRuleParams::Random(params) => column![
            section_title("Random"),
            row![
                pick_list(
                    crate::model::BatchRenameRandomMode::options(),
                    Some(params.mode),
                    move |value| Message::BatchRename(BatchRenameMessage::RandomModeSelected(
                        rule.id, value
                    )),
                )
                .width(Length::Fill)
                .text_size(12)
                .padding([5, 8]),
                input_column("Length", &params.length_input, move |value| {
                    Message::BatchRename(BatchRenameMessage::RandomLengthChanged(rule.id, value))
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            input_column("Alphabet", &params.alphabet, move |value| {
                Message::BatchRename(BatchRenameMessage::RandomAlphabetChanged(rule.id, value))
            }),
        ]
        .spacing(6)
        .into(),
        BatchRenameRuleParams::Extension(params) => {
            let mode = pick_list(
                crate::model::BatchRenameExtensionMode::options(),
                Some(params.mode),
                move |value| {
                    Message::BatchRename(BatchRenameMessage::ExtensionModeSelected(rule.id, value))
                },
            )
            .width(Length::Fill)
            .text_size(12)
            .padding([5, 8]);
            let controls = column![section_title("Extension")];
            if params.mode == BatchRenameExtensionMode::Replace {
                controls
                    .push(
                        row![
                            mode,
                            input_column("New extension", &params.replacement, move |value| {
                                Message::BatchRename(
                                    BatchRenameMessage::ExtensionReplacementChanged(rule.id, value),
                                )
                            }),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .spacing(6)
                    .into()
            } else {
                controls.push(mode).spacing(6).into()
            }
        }
        BatchRenameRuleParams::Regex(params) => column![
            section_title("Regex"),
            row![
                input_column("Pattern", &params.pattern, move |value| {
                    Message::BatchRename(BatchRenameMessage::RegexPatternChanged(rule.id, value))
                }),
                input_column("Replacement", &params.replacement, move |value| {
                    Message::BatchRename(BatchRenameMessage::RegexReplacementChanged(
                        rule.id, value,
                    ))
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(6)
        .into(),
        BatchRenameRuleParams::List(params) => column![
            section_title("List"),
            input_column("Names", &params.names, move |value| {
                Message::BatchRename(BatchRenameMessage::ListNamesChanged(rule.id, value))
            }),
        ]
        .spacing(6)
        .into(),
    };

    container(editor)
        .padding([8, 10])
        .width(Length::Fill)
        .style(rule_editor_container_style)
        .into()
}

fn template_editor<'a>(
    id: u64,
    params: &'a crate::model::BatchRenameTemplateRule,
) -> Element<'a, Message> {
    let token_button = button(localized_text("Tokens").size(12))
        .padding([6, 10])
        .style(add_rule_button_style())
        .on_press(Message::BatchRename(
            BatchRenameMessage::TemplateTokenMenuToggled(id),
        ));
    let token_menu: Option<Element<'a, Message>> = if params.token_menu_open {
        let mut items = Column::new().spacing(2);
        for token in BatchRenameTemplateToken::ALL {
            items = items.push(
                button(readable_text(token.label()).size(12))
                    .padding([5, 10])
                    .width(Length::Fill)
                    .style(context_menu_item_button_style())
                    .on_press(Message::BatchRename(
                        BatchRenameMessage::TemplateTokenSelected(id, token),
                    )),
            );
        }
        Some(
            container(items)
                .padding(4)
                .width(Length::Fixed(230.0))
                .style(context_menu_style)
                .into(),
        )
    } else {
        None
    };

    column![
        section_title("Template"),
        row![
            input_column("Template", &params.template, move |value| {
                Message::BatchRename(BatchRenameMessage::TemplateChanged(id, value))
            }),
            anchored_popup(token_button, token_menu),
        ]
        .spacing(8)
        .align_y(Alignment::End),
    ]
    .spacing(6)
    .into()
}

fn replace_editor<'a>(id: u64, params: &'a BatchRenameReplaceRule) -> Element<'a, Message> {
    let mut controls = column![
        section_title("Replace"),
        pick_list(
            crate::model::BatchRenameReplaceScope::options(),
            Some(params.scope),
            move |value| Message::BatchRename(BatchRenameMessage::ReplaceScopeSelected(id, value)),
        )
        .width(Length::Fill)
        .text_size(12)
        .padding([5, 8]),
        row![
            input_column("Find", &params.find, move |value| {
                Message::BatchRename(BatchRenameMessage::ReplaceFindChanged(id, value))
            }),
            input_column("With", &params.replacement, move |value| {
                Message::BatchRename(BatchRenameMessage::ReplaceWithChanged(id, value))
            }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ];
    if params.scope == BatchRenameReplaceScope::Range {
        controls = controls.push(
            row![
                input_column("Range start", &params.range_start_input, move |value| {
                    Message::BatchRename(BatchRenameMessage::ReplaceRangeStartChanged(id, value))
                }),
                input_column("Range length", &params.range_length_input, move |value| {
                    Message::BatchRename(BatchRenameMessage::ReplaceRangeLengthChanged(id, value))
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    controls
        .push(
            checkbox(params.ignore_case)
                .label(crate::localization::translate_current("Ignore case"))
                .on_toggle(move |value| {
                    Message::BatchRename(BatchRenameMessage::ReplaceIgnoreCaseToggled(id, value))
                }),
        )
        .spacing(6)
        .into()
}

fn insert_editor<'a>(
    id: u64,
    params: &'a crate::model::BatchRenameInsertRule,
) -> Element<'a, Message> {
    let mut fields = row![input_column("Text", &params.text, move |value| {
        Message::BatchRename(BatchRenameMessage::InsertTextChanged(id, value))
    })]
    .spacing(8)
    .align_y(Alignment::Center);
    match params.mode {
        BatchRenameInsertMode::Position => {
            fields = fields.push(input_column(
                "Position",
                &params.position_input,
                move |value| {
                    Message::BatchRename(BatchRenameMessage::InsertPositionChanged(id, value))
                },
            ));
        }
        BatchRenameInsertMode::AfterAnchor => {
            fields = fields.push(input_column("After text", &params.anchor, move |value| {
                Message::BatchRename(BatchRenameMessage::InsertAnchorChanged(id, value))
            }));
        }
        BatchRenameInsertMode::Before | BatchRenameInsertMode::After => {}
    }

    let mut controls = column![
        section_title("Insert"),
        pick_list(
            crate::model::BatchRenameInsertMode::options(),
            Some(params.mode),
            move |value| Message::BatchRename(BatchRenameMessage::InsertModeSelected(id, value)),
        )
        .width(Length::Fill)
        .text_size(12)
        .padding([5, 8]),
        fields,
    ];
    if params.mode != BatchRenameInsertMode::Before {
        controls = controls.push(
            checkbox(params.ignore_extension)
                .label(crate::localization::translate_current("Ignore extension"))
                .on_toggle(move |value| {
                    Message::BatchRename(BatchRenameMessage::InsertIgnoreExtensionToggled(
                        id, value,
                    ))
                }),
        );
    }

    controls.spacing(6).into()
}

fn slice_editor<'a>(
    id: u64,
    params: &'a crate::model::BatchRenameSliceRule,
) -> Element<'a, Message> {
    let location = match params.mode {
        BatchRenameSliceMode::Position => {
            input_column("Start", &params.start_input, move |value| {
                Message::BatchRename(BatchRenameMessage::SliceStartChanged(id, value))
            })
        }
        BatchRenameSliceMode::AfterAnchor => {
            input_column("After text", &params.anchor, move |value| {
                Message::BatchRename(BatchRenameMessage::SliceAnchorChanged(id, value))
            })
        }
    };

    column![
        section_title("Slice"),
        pick_list(
            crate::model::BatchRenameSliceMode::options(),
            Some(params.mode),
            move |value| Message::BatchRename(BatchRenameMessage::SliceModeSelected(id, value)),
        )
        .width(Length::Fill)
        .text_size(12)
        .padding([5, 8]),
        row![
            location,
            input_column("Length", &params.length_input, move |value| {
                Message::BatchRename(BatchRenameMessage::SliceLengthChanged(id, value))
            }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(6)
    .into()
}

fn remove_editor<'a>(
    id: u64,
    params: &'a crate::model::BatchRenameRemoveRule,
) -> Element<'a, Message> {
    let mode_controls: Element<'a, Message> = match params.mode {
        BatchRenameRemoveMode::TextAndRange => column![
            input_column("Text", &params.text, move |value| {
                Message::BatchRename(BatchRenameMessage::RemoveTextChanged(id, value))
            }),
            row![
                input_column("Start", &params.start_input, move |value| {
                    Message::BatchRename(BatchRenameMessage::RemoveStartChanged(id, value))
                }),
                input_column("Length", &params.length_input, move |value| {
                    Message::BatchRename(BatchRenameMessage::RemoveLengthChanged(id, value))
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(6)
        .into(),
        BatchRenameRemoveMode::CharacterClasses => column![
            row![
                remove_class_toggle(id, params, BatchRenameRemoveClass::Lowercase),
                remove_class_toggle(id, params, BatchRenameRemoveClass::Uppercase),
                remove_class_toggle(id, params, BatchRenameRemoveClass::Digits),
                remove_class_toggle(id, params, BatchRenameRemoveClass::Symbols),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                remove_class_toggle(id, params, BatchRenameRemoveClass::Brackets),
                remove_class_toggle(id, params, BatchRenameRemoveClass::Whitespace),
                remove_class_toggle(id, params, BatchRenameRemoveClass::Hanzi),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(6)
        .into(),
    };

    column![
        section_title("Remove characters"),
        pick_list(
            crate::model::BatchRenameRemoveMode::options(),
            Some(params.mode),
            move |value| Message::BatchRename(BatchRenameMessage::RemoveModeSelected(id, value)),
        )
        .width(Length::Fill)
        .text_size(12)
        .padding([5, 8]),
        mode_controls,
    ]
    .spacing(6)
    .into()
}

fn remove_class_toggle<'a>(
    id: u64,
    params: &'a crate::model::BatchRenameRemoveRule,
    class: BatchRenameRemoveClass,
) -> Element<'a, Message> {
    checkbox(params.classes.contains(&class))
        .label(class.to_string())
        .on_toggle(move |value| {
            Message::BatchRename(BatchRenameMessage::RemoveClassToggled(id, class, value))
        })
        .into()
}

fn sequence_editor<'a>(
    id: u64,
    params: &'a crate::model::BatchRenameSequenceRule,
) -> Element<'a, Message> {
    column![
        section_title("Numbering"),
        row![
            input_column("Prefix", &params.prefix, move |value| {
                Message::BatchRename(BatchRenameMessage::SequencePrefixChanged(id, value))
            }),
            input_column("Start", &params.start_input, move |value| {
                Message::BatchRename(BatchRenameMessage::SequenceStartChanged(id, value))
            }),
            input_column("Step", &params.step_input, move |value| {
                Message::BatchRename(BatchRenameMessage::SequenceStepChanged(id, value))
            }),
            input_column("Padding", &params.padding_input, move |value| {
                Message::BatchRename(BatchRenameMessage::SequencePaddingChanged(id, value))
            }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            checkbox(params.include_original_stem)
                .label(crate::localization::translate_current("Original stem"))
                .on_toggle(move |value| {
                    Message::BatchRename(BatchRenameMessage::SequenceIncludeOriginalToggled(
                        id, value,
                    ))
                }),
            checkbox(params.preserve_extension)
                .label(crate::localization::translate_current("Extension"))
                .on_toggle(move |value| {
                    Message::BatchRename(BatchRenameMessage::SequencePreserveExtensionToggled(
                        id, value,
                    ))
                }),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ]
    .spacing(6)
    .into()
}

fn preview_rows(
    state: &BatchRenameState,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let mut rows = Column::new().spacing(4);
    let dragging_active = state.dragging_preview_source().is_some();
    let editing_source = state.editing_target_name_source();
    let editing_input = state.editing_target_name_input();
    for (index, row_item) in state.preview.rows.iter().enumerate() {
        let is_dragging = state.dragging_preview_source() == Some(row_item.source.as_path());
        let is_editing = editing_source == Some(row_item.source.as_path());
        rows = rows.push(preview_row(
            index + 1,
            row_item,
            is_dragging,
            dragging_active,
            is_editing,
            editing_input,
        ));
    }

    let scroll_region = ScrollbarRegion::BatchRenamePreview;
    let scroller = scrollable(smooth_scroll_content(rows, scroll_region.clone()))
        .id(smooth_scroll_id(&scroll_region))
        .direction(enhanced_vertical_scrollbar_direction(
            scrollbar_visibility,
            6.0,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .height(Length::Fixed(BATCH_RENAME_PREVIEW_HEIGHT))
        .on_scroll(scrollbar_on_scroll(scroll_region.clone(), |_| {
            Message::BatchRenamePreviewScrolled
        }));
    let scroller = enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Vertical,
        6.0,
    );

    // 表头/行单元文本一律单行测量(Wrapping::None)+ cell clip 水平裁剪:
    // iced 默认 Word 换行,窄面板压窄列时会把行高撑到两倍以上(无头布局
    // 探针实测 400px 窗口约 43px vs 单行 28px),clip 只裁绘制不救换行。
    let header = row![
        readable_text("#").size(11).width(Length::Fixed(24.0)),
        container(
            localized_text("Original name")
                .size(11)
                .wrapping(text::Wrapping::None)
        )
        .width(Length::FillPortion(3))
        .clip(true),
        container(
            localized_text("New name")
                .size(11)
                .wrapping(text::Wrapping::None)
        )
        .width(Length::FillPortion(3))
        .clip(true),
        container(
            localized_text("Status")
                .size(11)
                .wrapping(text::Wrapping::None)
        )
        .width(Length::FillPortion(1))
        .clip(true),
    ]
    .spacing(8);

    column![section_title("Preview"), header, scroller]
        .spacing(6)
        .into()
}

fn preview_row<'a>(
    index: usize,
    row_state: &'a BatchRenamePreviewRow,
    is_dragging: bool,
    dragging_active: bool,
    is_editing: bool,
    editing_input: &'a str,
) -> Element<'a, Message> {
    let source =
        format_middle_ellipsized_text(&row_state.original_name, BATCH_RENAME_PATH_MAX_CHARS);
    let status = row_state.status.label();
    let index_cell: Element<'a, Message> = readable_text(index.to_string())
        .size(11)
        .width(Length::Fixed(24.0))
        .into();
    let source_cell: Element<'a, Message> = mouse_area(
        container(
            readable_text(source)
                .size(12)
                .wrapping(text::Wrapping::None),
        )
        .width(Length::FillPortion(3))
        .padding([1, 0])
        .clip(true),
    )
    .on_press(Message::BatchRename(
        BatchRenameMessage::PreviewDragStarted(row_state.source.clone()),
    ))
    .interaction(iced::mouse::Interaction::Grab)
    .into();
    let target_cell: Element<'a, Message> = if is_editing {
        text_input(
            &crate::localization::translate_current("New name"),
            editing_input,
        )
        .id(batch_rename_preview_name_input_id(&row_state.source))
        .on_input(|value| Message::BatchRename(BatchRenameMessage::PreviewNameChanged(value)))
        .on_submit(Message::BatchRename(
            BatchRenameMessage::PreviewNameEditCommitted,
        ))
        .padding([5, 6])
        .size(12)
        .width(Length::FillPortion(3))
        .into()
    } else {
        mouse_area(
            container(diff_highlighted_target(row_state))
                .width(Length::FillPortion(3))
                .padding([1, 0]),
        )
        .on_press(Message::BatchRename(
            BatchRenameMessage::PreviewNameEditStarted(row_state.source.clone()),
        ))
        .interaction(iced::mouse::Interaction::Text)
        .into()
    };
    let row_container = container(
        row![
            index_cell,
            source_cell,
            target_cell,
            // 状态列与新名列一致:单行测量 + clip 裁剪,窄列不撑高行。
            container(
                readable_text(status)
                    .size(11)
                    .wrapping(text::Wrapping::None)
                    .style(move |theme: &Theme| {
                        iced::widget::text::Style {
                            color: Some(status_color(theme, row_state.status)),
                        }
                    })
            )
            .width(Length::FillPortion(1))
            .clip(true),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(if is_dragging {
        dragged_row_style
    } else {
        path_suggestion_item_style
    });

    let row_area = mouse_area(row_container)
        .on_release(Message::BatchRename(
            BatchRenameMessage::PreviewDragFinished,
        ))
        .interaction(iced::mouse::Interaction::Pointer);

    let row_area = if dragging_active {
        row_area.on_enter(Message::BatchRename(
            BatchRenameMessage::PreviewDragEntered(row_state.source.clone()),
        ))
    } else {
        row_area
    };

    row_area.into()
}

/// 新文件名按公共前缀/后缀切分，变更段高亮；超长名按预算收缩并保留变更段。
fn diff_highlighted_target(row_state: &BatchRenamePreviewRow) -> Element<'_, Message> {
    let segments = diff_name_display(
        &row_state.original_name,
        &row_state.target_name,
        BATCH_RENAME_PATH_MAX_CHARS,
    );
    let mut cells = Row::new().spacing(0);
    for (segment, changed) in segments {
        // 变更段同样单行测量,窄面板下交给外层 container.clip 裁剪。
        cells = cells.push(
            readable_text(segment)
                .size(12)
                .wrapping(text::Wrapping::None)
                .style(move |theme: &Theme| iced::widget::text::Style {
                    color: Some(if changed {
                        changed_segment_color(theme)
                    } else {
                        base_text_color(theme)
                    }),
                }),
        );
    }

    container(cells).clip(true).width(Length::Fill).into()
}

fn diff_name_display(original: &str, target: &str, max_chars: usize) -> Vec<(String, bool)> {
    if original == target {
        return vec![(target.to_owned(), false)];
    }

    let original_chars: Vec<char> = original.chars().collect();
    let target_chars: Vec<char> = target.chars().collect();
    let prefix = original_chars
        .iter()
        .zip(target_chars.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let max_suffix = (original_chars.len() - prefix).min(target_chars.len() - prefix);
    let mut suffix = 0;
    while suffix < max_suffix
        && original_chars[original_chars.len() - 1 - suffix]
            == target_chars[target_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let middle = target_chars.len() - prefix - suffix;

    let span = |start: usize, end: usize, changed: bool| {
        (target_chars[start..end].iter().collect::<String>(), changed)
    };
    let mut segments = Vec::new();
    if total_chars(prefix, middle, suffix) <= max_chars {
        if prefix > 0 {
            segments.push(span(0, prefix, false));
        }
        if middle > 0 {
            segments.push(span(prefix, prefix + middle, true));
        }
        if suffix > 0 {
            segments.push(span(prefix + middle, target_chars.len(), false));
        }
        return segments;
    }

    // 超长名：收缩未变更的前后缀，尽量保住变更段可见
    let head = prefix.min(max_chars / 3);
    let tail = suffix.min(max_chars / 3);
    let mid_budget = max_chars.saturating_sub(head + tail).saturating_sub(2);
    if mid_budget == 0 {
        return vec![(target_chars.iter().collect::<String>(), false)];
    }

    if head > 0 {
        segments.push(span(0, head, false));
        if prefix > head {
            segments.push(("…".to_owned(), false));
        }
    }
    if middle > 0 {
        if middle > mid_budget {
            let mid_head = mid_budget / 2;
            let mid_tail = mid_budget - mid_head;
            segments.push(span(prefix, prefix + mid_head, true));
            if mid_tail > 0 {
                segments.push(("…".to_owned(), true));
                segments.push(span(prefix + middle - mid_tail, prefix + middle, true));
            }
        } else {
            segments.push(span(prefix, prefix + middle, true));
        }
    }
    if tail > 0 {
        if suffix > tail {
            segments.push(("…".to_owned(), false));
        }
        segments.push(span(target_chars.len() - tail, target_chars.len(), false));
    }

    segments
}

fn total_chars(prefix: usize, middle: usize, suffix: usize) -> usize {
    prefix + middle + suffix
}

fn changed_segment_color(theme: &Theme) -> Color {
    ui_colors(theme).primary
}

fn status_color(theme: &Theme, status: crate::model::BatchRenamePreviewStatus) -> Color {
    use crate::model::BatchRenamePreviewStatus::*;
    let colors = ui_colors(theme);
    match status {
        Ready => base_text_color(theme),
        Unchanged => muted_text_color(theme),
        DuplicateTarget | ExistingTarget | EmptyName | RuleError => colors.error,
    }
}

fn action_row(state: &BatchRenameState) -> Element<'static, Message> {
    let apply = if state.can_apply() {
        primary_action_button("Apply", Message::BatchRename(BatchRenameMessage::Apply))
    } else {
        inactive_primary_action_button("Apply")
    };

    row![
        Space::new().width(Length::Fill),
        secondary_action_button("Cancel", Message::BatchRename(BatchRenameMessage::Cancel)),
        apply,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn input_column<'a>(
    label: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    column![
        readable_text(translate_current(label)).size(11),
        text_input(&translate_current(label), value)
            .on_input(on_input)
            .padding([6, 8])
            .size(13)
            .width(Length::Fill),
    ]
    .spacing(3)
    .width(Length::Fill)
    .into()
}

fn translate_current(label: &'static str) -> String {
    crate::localization::translate_current(label)
}

fn section_title(label: &'static str) -> Element<'static, Message> {
    readable_text(label).size(12).into()
}

fn rule_chip_style(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style + Clone {
    move |theme, status| {
        let colors = ui_colors(theme);
        let accent = colors.primary;
        let hover = colors.surface_container_high;
        let surface = colors.surface_container;

        button::Style {
            background: Some(Background::Color(if selected {
                Color { a: 0.18, ..accent }
            } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                hover
            } else {
                surface
            })),
            text_color: if selected {
                accent
            } else if matches!(status, button::Status::Disabled) {
                muted_text_color(theme)
            } else {
                base_text_color(theme)
            },
            border: Border {
                color: if selected {
                    accent
                } else {
                    subtle_border_color(theme)
                },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

fn chip_action_button_style() -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let colors = ui_colors(theme);
        button::Style {
            background: Some(Background::Color(
                if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    colors.surface_container_high
                } else {
                    colors.surface_container
                },
            )),
            text_color: muted_text_color(theme),
            border: Border {
                color: subtle_border_color(theme),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

fn add_rule_button_style() -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let colors = ui_colors(theme);
        button::Style {
            background: Some(Background::Color(
                if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    colors.surface_container_high
                } else {
                    colors.surface_container
                },
            )),
            text_color: base_text_color(theme),
            border: Border {
                color: subtle_border_color(theme),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

fn rule_editor_container_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_container)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

fn disabled_chip_container_style(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(muted_text_color(theme)),
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::diff_name_display;

    #[test]
    fn identical_names_render_single_unchanged_segment() {
        let segments = diff_name_display("photo.jpg", "photo.jpg", 38);
        assert_eq!(segments, vec![("photo.jpg".to_owned(), false)]);
    }

    #[test]
    fn changed_middle_is_highlighted() {
        let segments = diff_name_display("photo_old.jpg", "photo_new.jpg", 38);
        assert_eq!(
            segments,
            vec![
                ("photo_".to_owned(), false),
                ("new".to_owned(), true),
                (".jpg".to_owned(), false),
            ]
        );
    }

    #[test]
    fn appended_suffix_is_highlighted() {
        let segments = diff_name_display("file.txt", "file_backup.txt", 38);
        assert_eq!(
            segments,
            vec![
                ("file".to_owned(), false),
                ("_backup".to_owned(), true),
                (".txt".to_owned(), false)
            ]
        );
    }

    #[test]
    fn long_names_keep_highlight_within_budget() {
        let segments = diff_name_display("aaaaaaaaaa_old_end", "aaaaaaaaaa_NEW_end", 12);
        let rendered: String = segments.iter().map(|(text, _)| text.as_str()).collect();
        assert!(rendered.chars().count() <= 12 + 3);
        assert!(segments.iter().any(|(_, changed)| *changed));
    }
}
