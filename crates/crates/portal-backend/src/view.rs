//! 选择窗口视图：导航栏（上级 + 面包屑 + 过滤器）、SaveFile 文件名输入
//! 与确认栏、目录列表。纯展示，交互全部转成 `SessionMessage`。
//!
//! 视觉词汇（颜色角色、图标、按钮/行/滚动条/输入框样式）全部来自共享
//! crate `bennu-theme`，与主程序消费同一份活动主题，保证两个进程的
//! FileChooser 窗口看起来是同一个应用。

use std::path::Path;

use bennu_theme::icons::{file_entry_icon_symbol, rotated_chevron_right_view, IconSymbol};
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::smooth_scroll::SmoothScrollArea;
use bennu_theme::styles::{
    base_text_color, enhanced_horizontal_scrollbar_direction, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, hovered_row_style, list_row_style, muted_icon_svg_style,
    muted_text_color, navigation_text_input_style, primary_action_button_style,
    selected_icon_svg_style, selected_row_style, surface_button_style,
    transparent_icon_button_style,
};
use bennu_theme::ui_colors;
use file_core::entry::FileKind;
use iced::widget::Space;
use iced::widget::{
    button, column, container, mouse_area, pick_list, row, scrollable, text, text_input,
};
use iced::{alignment, mouse, Border, Color, Element, Length, Padding, Size, Theme};

use crate::picker_request::PickerKind;
use crate::picker_session::scrollbar::{scroll_axis, scroll_id, scrollbar_on_scroll};
use crate::picker_session::{
    breadcrumb_chain, DirectoryListing, PickerRow, PickerSession, SessionMessage,
    SessionScrollRegion,
};

/// 每请求窗口的初始尺寸。
pub(crate) fn window_size() -> Size {
    Size::new(820.0, 560.0)
}

pub(crate) fn window_min_size() -> Size {
    Size::new(640.0, 420.0)
}

/// 列表滚动条静态宽度：与主软件列表一致（mac 式细滚动条）。
const LIST_SCROLLBAR_WIDTH: f32 = 8.0;

/// 面包屑横向滚动条静态宽度：与主软件地址栏一致。
const BREADCRUMB_SCROLLBAR_WIDTH: f32 = 8.0;

/// 地址栏高度（与主程序 ADDRESS_BAR_HEIGHT 一致）。
const ADDRESS_BAR_HEIGHT: f32 = 34.0;

/// 子级行相对父级的缩进。
const EXPANSION_INDENT: f32 = 16.0;

/// 条目行固定高度：展开动画需要数值行高做裁剪（行高 × 进度），内容
/// 垂直居中；取值与既有 padding 撑出的视觉高度一致。
const ENTRY_ROW_HEIGHT: f32 = 28.0;

/// 滚动内容包一层滚轮捕获（SmoothScrollArea）：内容先处理事件，未
/// 被吞的滚轮按区域轴向发 `WheelScrolled`（增量换算在会话滚动子模块）。
fn smooth_scroll_region(
    content: impl Into<Element<'static, SessionMessage>>,
    region: SessionScrollRegion,
    shift_pressed: bool,
) -> Element<'static, SessionMessage> {
    Element::new(SmoothScrollArea::new(
        content,
        scroll_axis(region),
        shift_pressed,
        move |delta| SessionMessage::WheelScrolled { region, delta },
    ))
}

/// 窗口内容。`emit` 由上层提供，负责把会话消息与窗口关联。
pub(crate) fn picker_window_view(
    session: &PickerSession,
    theme: &Theme,
    address_input_id: iced::widget::Id,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let mut layout = column![].spacing(10);
    layout = layout.push(navigation_bar(
        session,
        theme,
        address_input_id,
        emit.clone(),
    ));
    if let Some(name_input) = name_input_row(session, emit.clone()) {
        layout = layout.push(name_input);
    }
    layout = layout.push(listing_body(session, theme, emit.clone()));
    layout = layout.push(confirm_footer(session, emit));

    container(layout)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(12)
        .style(|theme: &Theme| container::Style {
            background: Some(bennu_theme::ui_colors(theme).background.into()),
            text_color: Some(base_text_color(theme)),
            ..container::Style::default()
        })
        .into()
}

fn navigation_bar(
    session: &PickerSession,
    theme: &Theme,
    address_input_id: iced::widget::Id,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let up_button = button(
        IconSymbol::ArrowUp
            .view(16.0)
            .style(bennu_theme::styles::icon_svg_style()),
    )
    .padding(6.0)
    .style(transparent_icon_button_style)
    .on_press(emit(SessionMessage::NavigateUp));

    // 地址栏主体：面包屑态 / 路径编辑态。整栏包一层 mouse_area，
    // 点击空白处进入编辑（面包屑按钮捕获自身点击，不受影响）——
    // 与主程序 address_bar_surface 同一结构。
    let surface: Element<'static, SessionMessage> = if let Some(draft) = session.address_edit() {
        text_input("输入路径，回车跳转", draft)
            .id(address_input_id)
            .on_input({
                let emit = emit.clone();
                move |value| emit(SessionMessage::AddressEditChanged(value))
            })
            .on_submit(emit(SessionMessage::AddressEditingSubmitted))
            .size(13)
            .padding(Padding::new(6.0).top(7.0).bottom(7.0))
            .style(navigation_text_input_style)
            .width(Length::Fill)
            .into()
    } else {
        let region = SessionScrollRegion::Breadcrumb;
        let scrollbar_visibility = session.scrollbar_visibility_for(&region);
        let mut breadcrumb = row![].spacing(2).align_y(alignment::Vertical::Center);
        for (position, segment) in breadcrumb_chain(session.directory()).iter().enumerate() {
            if position > 0 {
                breadcrumb = breadcrumb.push(
                    IconSymbol::ChevronRight
                        .view(12.0)
                        .style(muted_icon_svg_style()),
                );
            }
            breadcrumb = breadcrumb.push(
                button(readable_label(segment_label(segment)).size(13))
                    .padding(Padding::new(6.0).top(4.0).bottom(4.0))
                    .style(breadcrumb_segment_style)
                    .on_press(emit(SessionMessage::BreadcrumbActivated {
                        ancestor: position,
                    })),
            );
        }
        let breadcrumb_scroller = scrollable(smooth_scroll_region(
            breadcrumb,
            region,
            session.scroll_shift_pressed(),
        ))
        .id(scroll_id(session.request_path(), region))
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
        .direction(enhanced_horizontal_scrollbar_direction(
            scrollbar_visibility,
            BREADCRUMB_SCROLLBAR_WIDTH,
        ))
        .style(enhanced_scrollbar_style(scrollbar_visibility))
        .on_scroll(scrollbar_on_scroll(region, |_| {
            SessionMessage::ScrollbarEngaged {
                region: SessionScrollRegion::Breadcrumb,
            }
        }));
        // mac 式滚动条：透明原生命中层 + canvas 浮层拇指，替换默认常显
        // 的原生横向滚动条。
        enhanced_scrollbar(
            breadcrumb_scroller,
            scrollbar_visibility,
            session.scrollbar_viewport_for(&region),
            ScrollbarAxis::Horizontal,
            BREADCRUMB_SCROLLBAR_WIDTH,
        )
        .into()
    };

    let address_bar = mouse_area(
        container(surface)
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .align_y(alignment::Vertical::Center),
    )
    .on_press(emit(SessionMessage::AddressEditingStarted))
    .interaction(mouse::Interaction::Text);

    let mut bar = row![].spacing(6).align_y(alignment::Vertical::Center);
    bar = bar.push(up_button);
    bar = bar.push(address_bar);

    if session.filters().len() > 1 {
        let labels: Vec<String> = session
            .filters()
            .iter()
            .map(|rule| rule.name.clone())
            .collect();
        let known_labels = labels.clone();
        let current = Some(session.active_filter_label());
        bar = bar.push(Space::new().width(Length::Fill));
        bar = bar.push(
            pick_list(labels, current, move |picked: String| {
                match known_labels.iter().position(|label| *label == picked) {
                    Some(rule) => emit(SessionMessage::FilterSelected { rule }),
                    None => emit(SessionMessage::FilterSelectionIgnored),
                }
            })
            .text_size(13.0)
            .padding(Padding::new(6.0)),
        );
    } else {
        bar = bar.push(Space::new().width(Length::Fill));
        bar = bar.push(
            readable_label(session.active_filter_label())
                .size(13)
                .color(muted_text_color(theme)),
        );
    }

    container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
        .align_y(alignment::Vertical::Center)
        .into()
}

/// 面包屑段按钮：surface 按钮的三档状态，圆角 6。
fn breadcrumb_segment_style(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = surface_button_style(theme, status);
    style.border.radius = 6.0.into();
    style
}

/// 中文等非 ASCII 文案需要 Advanced shaping（与主程序 typography 同规则）。
fn readable_label(label: String) -> iced::widget::Text<'static, Theme> {
    let needs_advanced_shaping = !label.is_ascii();
    let label = text(label);
    if needs_advanced_shaping {
        label.shaping(iced::widget::text::Shaping::Advanced)
    } else {
        label
    }
}

fn segment_label(segment: &Path) -> String {
    segment
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string())
}

fn listing_body(
    session: &PickerSession,
    theme: &Theme,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let body: Element<'static, SessionMessage> = match session.listing() {
        DirectoryListing::Pending => centered_hint("正在读取目录…", 14.0, muted_text_color(theme)),
        DirectoryListing::Failed(details) => container(
            column![
                readable_label("目录读取失败".to_string())
                    .size(15)
                    .color(ui_colors(theme).error),
                readable_label(details.clone())
                    .size(13)
                    .color(muted_text_color(theme)),
            ]
            .spacing(6),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .padding(24)
        .into(),
        DirectoryListing::Ready(_) => {
            if session.rows().is_empty() {
                centered_hint("空目录", 14.0, muted_text_color(theme))
            } else {
                let region = SessionScrollRegion::List;
                let scrollbar_visibility = session.scrollbar_visibility_for(&region);
                let mut list = column![].spacing(2);
                for (index, row) in session.rows().iter().enumerate() {
                    list = list.push(entry_row(
                        index,
                        row,
                        theme,
                        session.selection().contains(&index),
                        session.hovered_index() == Some(index),
                        emit.clone(),
                    ));
                }
                let list_scroller = scrollable(smooth_scroll_region(
                    list,
                    region,
                    session.scroll_shift_pressed(),
                ))
                .id(scroll_id(session.request_path(), region))
                .width(Length::Fill)
                .height(Length::Fill)
                .direction(enhanced_vertical_scrollbar_direction(
                    scrollbar_visibility,
                    LIST_SCROLLBAR_WIDTH,
                ))
                .style(enhanced_scrollbar_style(scrollbar_visibility))
                .on_scroll(scrollbar_on_scroll(region, |_| {
                    SessionMessage::ScrollbarEngaged {
                        region: SessionScrollRegion::List,
                    }
                }));
                // mac 式滚动条：透明原生拇指 + canvas 浮层拇指（此前
                // enhanced_scrollbar_style 把原生拇指透明化而浮层缺失，
                // 滚动条整体隐形）。
                enhanced_scrollbar(
                    list_scroller,
                    scrollbar_visibility,
                    session.scrollbar_viewport_for(&region),
                    ScrollbarAxis::Vertical,
                    LIST_SCROLLBAR_WIDTH,
                )
                .into()
            }
        }
    };
    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|theme: &Theme| {
            let colors = bennu_theme::ui_colors(theme);
            container::Style {
                // 条纹偶数行画的就是 background：面板底色必须同为
                // background，偶数行才能像主应用一样隐形（凹槽色会让
                // 每一行都变成可见的盒子）。
                background: Some(colors.background.into()),
                border: Border {
                    color: bennu_theme::styles::subtle_border_color(theme),
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..container::Style::default()
            }
        })
        .padding(6)
        .into()
}

fn centered_hint(label: &str, size: f32, color: Color) -> Element<'static, SessionMessage> {
    container(readable_label(label.to_string()).size(size).color(color))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn entry_row(
    index: usize,
    row: &PickerRow,
    theme: &Theme,
    selected: bool,
    hovered: bool,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let entry = &row.entry;
    let icon_tone = if selected {
        selected_icon_svg_style()
    } else {
        muted_icon_svg_style()
    };
    let icon = file_entry_icon_symbol(entry.kind, &entry.name)
        .view(18.0)
        .style(icon_tone);

    let meta = match entry.kind {
        FileKind::Directory => "文件夹".to_string(),
        _ => readable_size(entry.metadata.len),
    };

    // 目录行首的展开开关（访达列表语义）：箭头随自身展开进度旋转，
    // 收起时同步转回；文件行用等宽占位保证名称列对齐。
    let disclosure: Element<'static, SessionMessage> = if entry.kind == FileKind::Directory {
        button(rotated_chevron_right_view(row.expand_progress * 90.0, 12.0).style(icon_tone))
            .padding(2.0)
            .style(transparent_icon_button_style)
            .on_press(emit(SessionMessage::EntryExpandToggled { index }))
            .into()
    } else {
        Space::new()
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(12.0))
            .into()
    };

    let mut content = row![].spacing(10).align_y(alignment::Vertical::Center);
    content = content.push(disclosure);
    content = content.push(icon);
    content = content.push(readable_label(entry.name.to_string_lossy().into_owned()).size(14));
    content = content.push(Space::new().width(Length::Fill));
    content = content.push(readable_label(meta).size(12).color(muted_text_color(theme)));

    // 条纹是常态底色（与主应用同一份实现），选中/悬停态在其上覆盖。
    let stripe_style = list_row_style(row.depth, index);
    let style = move |theme: &Theme| {
        if selected {
            selected_row_style(theme)
        } else if hovered {
            hovered_row_style(theme)
        } else {
            stripe_style(theme)
        }
    };

    let depth_indent = row.depth as f32 * EXPANSION_INDENT;
    let row_surface = mouse_area(
        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(ENTRY_ROW_HEIGHT))
            .center_y(Length::Fixed(ENTRY_ROW_HEIGHT))
            .padding(Padding::new(0.0).left(6.0 + depth_indent).right(6.0))
            .style(style),
    )
    .on_enter(emit(SessionMessage::EntryHovered { index: Some(index) }))
    .on_exit(emit(SessionMessage::EntryHovered { index: None }))
    .on_press(emit(SessionMessage::EntryClicked {
        index,
        ctrl: false,
        shift: false,
    }))
    .on_double_click(emit(SessionMessage::EntryDoubleClicked { index }));

    // 展开动画中行高按祖先级联进度裁剪；完成后走普通路径，避免
    // 浮点残差让满高行底部留缝。
    if row.height_progress >= 1.0 {
        row_surface.into()
    } else {
        container(row_surface)
            .width(Length::Fill)
            .height(Length::Fixed(
                ENTRY_ROW_HEIGHT * row.height_progress.clamp(0.0, 1.0),
            ))
            .clip(true)
            .into()
    }
}

fn readable_size(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    let mut value = bytes as f64;
    let mut unit = "B";
    for candidate in ["KB", "MB", "GB", "TB"] {
        if value < UNIT {
            break;
        }
        value /= UNIT;
        unit = candidate;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn name_input_row(
    session: &PickerSession,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Option<Element<'static, SessionMessage>> {
    if !matches!(session.kind(), PickerKind::SaveFile { .. }) {
        return None;
    }
    let input = text_input("文件名", session.name_input())
        .on_input({
            let emit = emit.clone();
            move |value| emit(SessionMessage::NameInputChanged(value))
        })
        .on_submit(emit(SessionMessage::ConfirmPressed))
        .size(14)
        .padding(Padding::new(6.0).top(7.0).bottom(7.0))
        .style(navigation_text_input_style);
    Some(Element::from(
        row![readable_label("文件名".to_string()).size(13), input,]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
    ))
}

fn confirm_footer(
    session: &PickerSession,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let mut layout = column![].spacing(8);

    if let Some(target) = session.overwrite_target() {
        layout = layout.push(
            container(
                row![
                    readable_label(format!(
                        "“{}” 已存在，确认覆盖？",
                        target
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ))
                    .size(13),
                    Space::new().width(Length::Fill),
                    button(readable_label("换个名字".to_string()).size(13))
                        .padding([5, 10])
                        .style(surface_button_style)
                        .on_press(emit(SessionMessage::OverwriteDeclined)),
                ]
                .spacing(10)
                .align_y(alignment::Vertical::Center),
            )
            .width(Length::Fill)
            .padding([8, 12])
            .style(bennu_theme::styles::error_notification_style),
        );
    }

    let mut footer = row![].spacing(8).align_y(alignment::Vertical::Center);
    footer = footer.push(Space::new().width(Length::Fill));

    let cancel = button(readable_label("取消".to_string()).size(13))
        .padding([6, 14])
        .style(surface_button_style)
        .on_press(emit(SessionMessage::DismissPressed));

    let mut confirm = button(readable_label(session.accept_button_label()).size(13))
        .padding([6, 14])
        .style(primary_action_button_style());
    if session.can_confirm() {
        confirm = confirm.on_press(emit(SessionMessage::ConfirmPressed));
    }

    footer = footer.push(cancel).push(confirm);
    layout = layout.push(footer);

    container(layout).width(Length::Fill).into()
}
