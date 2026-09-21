//! 地址栏视图：导航按钮组（后退/前进/上级）+ 面包屑 ↔ 编辑渐变主体 +
//! 路径补全浮层 + 过滤器。拼装结构从主软件 `view/address_bar.rs` 裁掉
//! 拖放/trash/主软件滚动区特判后移植；消息换 `SessionMessage`，视觉
//! 词汇（弹性面包屑、样式、图标）全部来自 bennu-theme 共享层。

use std::path::Path;

use bennu_theme::address_bar::{
    breadcrumb_segments, BreadcrumbMeasurement, BreadcrumbSegmentKind, ElasticBreadcrumbs,
    ADDRESS_BAR_HEIGHT, ADDRESS_TEXT_SIZE, BREADCRUMB_HORIZONTAL_PADDING, BREADCRUMB_ICON_SIZE,
    BREADCRUMB_SEPARATOR_SIZE,
};
use bennu_theme::anchored_popup::anchored_popup;
use bennu_theme::icons::IconSymbol;
use bennu_theme::measured_text::{format_middle_ellipsized_text, measured_middle_ellipsized_text};
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::ScrollbarVisibility;
use bennu_theme::styles::{
    address_bar_style, base_text_color, button_hover_surface_color, button_pressed_surface_color,
    button_surface_color, faded_button_style, faded_text_input_style, icon_svg_style,
    muted_icon_svg_style, muted_text_color, path_suggestion_item_style, path_suggestions_style,
    scale_color_alpha, selected_path_suggestion_item_style, subtle_border_color,
};
use iced::widget::{
    button, container, mouse_area, opaque, pick_list, responsive, scrollable, stack, text_input,
    Column, Space,
};
use iced::{mouse, Alignment, Background, Border, Color, Element, Length, Padding, Theme};

use crate::picker_session::scrollbar::{scroll_id, scrollbar_on_scroll, ScrollbarViewport};
use crate::picker_session::{PickerSession, SessionMessage, SessionScrollRegion};

use super::readable_label;

/// 面包屑横向滚动条静态宽度：与主软件地址栏一致。
const BREADCRUMB_SCROLLBAR_WIDTH: f32 = 8.0;

/// 补全建议行的中间省略上限：与主软件同值。
const PATH_SUGGESTION_MAX_CHARS: usize = 72;

/// 导航按钮组图标尺寸：与主软件 TOOLBAR_ICON_SIZE 同值。
const NAVIGATION_ICON_SIZE: f32 = 16.0;

/// 地址输入框的 widget Id：与 scrollbar::scroll_id 同风格按请求路径
/// 命名空间——widget 操作（focus/select_all/move_cursor_to_end）遍历
/// 进程内所有窗口，两个弹窗同处编辑态时不会互相命中（portal spec 的
/// widget Id 命名空间契约）。
pub(crate) fn address_input_id(request_path: &str) -> iced::widget::Id {
    iced::widget::Id::from(format!("portal-address-input#{request_path}"))
}

pub(super) fn navigation_bar(
    session: &PickerSession,
    theme: &Theme,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let mut bar = iced::widget::row![].spacing(6).align_y(Alignment::Center);
    bar = bar.push(navigation_button_group(session, emit.clone()));
    bar = bar.push(address_bar(session, emit.clone()));

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
        .align_y(Alignment::Center)
        .into()
}

/// [←][→][↑] 分段按钮组：hover/pressed 底色与禁用态数值对齐主软件
/// toolbar_segment_button；历史到头的方向按钮无 on_press 且图标 muted。
fn navigation_button_group(
    session: &PickerSession,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let group = iced::widget::row![
        navigation_segment_button(
            IconSymbol::ArrowLeft,
            session.can_navigate_back(),
            emit(SessionMessage::NavigateBack),
        ),
        navigation_segment_button(
            IconSymbol::ArrowRight,
            session.can_navigate_forward(),
            emit(SessionMessage::NavigateForward),
        ),
        navigation_segment_button(IconSymbol::ArrowUp, true, emit(SessionMessage::NavigateUp)),
    ]
    .spacing(0)
    .align_y(Alignment::Center);

    container(group)
        .clip(true)
        .style(navigation_button_group_style)
        .into()
}

fn navigation_segment_button(
    symbol: IconSymbol,
    enabled: bool,
    message: SessionMessage,
) -> iced::widget::Button<'static, SessionMessage> {
    // 禁用态不做灰底只换 muted 图标：与主软件到头按钮的视觉一致。
    let icon_tone = if enabled {
        icon_svg_style()
    } else {
        muted_icon_svg_style()
    };
    let mut segment = button(symbol.view(NAVIGATION_ICON_SIZE).style(icon_tone))
        .padding([8, 10])
        .style(navigation_segment_button_style);
    if enabled {
        segment = segment.on_press(message);
    }
    segment
}

fn navigation_segment_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => Some(Background::Color(button_hover_surface_color(theme))),
        button::Status::Pressed => Some(Background::Color(button_pressed_surface_color(theme))),
        button::Status::Active | button::Status::Disabled => None,
    };

    button::Style {
        background,
        text_color: if matches!(status, button::Status::Disabled) {
            muted_text_color(theme)
        } else {
            base_text_color(theme)
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..button::Style::default()
    }
}

fn navigation_button_group_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(button_surface_color(theme))),
        text_color: Some(base_text_color(theme)),
        border: Border {
            color: subtle_border_color(theme),
            width: 1.0,
            radius: 7.0.into(),
        },
        ..container::Style::default()
    }
}

/// 地址栏主体：面包屑层恒在渲染，编辑态用 opaque 输入框按过渡分数
/// 覆盖其上；退出编辑后由快照层渐出。补全面板经 anchored_popup 挂在
/// 锚点下方。整栏包 mouse_area：点击空白进编辑、光标呈文本形态。
fn address_bar(
    session: &PickerSession,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let editing_fraction = session.address_transition_fraction().clamp(0.0, 1.0);
    let breadcrumb_fraction = 1.0 - editing_fraction;
    let breadcrumbs = breadcrumb_layer(session, breadcrumb_fraction, emit.clone());

    let anchor: Element<'static, SessionMessage> = if let Some(editing) = session.address_editing()
    {
        let input = text_input("输入路径，回车跳转", &editing.draft)
            .id(address_input_id(session.request_path()))
            .on_input({
                let emit = emit.clone();
                move |value| emit(SessionMessage::AddressEditChanged(value))
            })
            .on_submit(emit(SessionMessage::AddressEditingSubmitted))
            .padding([7, 10])
            .size(16)
            .style(move |theme, status| faded_text_input_style(theme, status, editing_fraction))
            .width(Length::Fill);

        stack([breadcrumbs, opaque(input)])
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .into()
    } else if let Some(snapshot) = session.address_exit_snapshot() {
        let snapshot_layer = address_exit_snapshot(snapshot, editing_fraction);
        stack([breadcrumbs, snapshot_layer])
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .into()
    } else {
        breadcrumbs
    };

    let popup = session.address_editing().and_then(|editing| {
        (!editing.suggestions.is_empty()).then(|| path_suggestions_panel(editing, emit.clone()))
    });

    address_bar_surface(anchored_popup(anchor, popup), emit)
}

fn address_bar_surface(
    content: Element<'static, SessionMessage>,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    mouse_area(
        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .style(address_bar_style),
    )
    .on_press(emit(SessionMessage::AddressEditingStarted))
    .interaction(mouse::Interaction::Text)
    .into()
}

/// 面包屑层：共享 breadcrumb_segments 折叠 Home 段后交给弹性布局；
/// responsive 先量视口再分配段宽，横向滚动沿用 portal 的滚动接线。
fn breadcrumb_layer(
    session: &PickerSession,
    opacity: f32,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let segments = breadcrumb_segments(session.directory(), session.home_dir());
    let region = SessionScrollRegion::Breadcrumb;
    let scrollbar_visibility = session.scrollbar_visibility_for(&region);
    let scrollbar_viewport = session.scrollbar_viewport_for(&region);
    let request_path = session.request_path().to_string();
    let shift_pressed = session.scroll_shift_pressed();

    responsive(move |viewport_size| {
        breadcrumb_scroller(
            segments.clone(),
            opacity,
            viewport_size.width,
            request_path.clone(),
            shift_pressed,
            scrollbar_visibility,
            scrollbar_viewport,
            emit.clone(),
        )
    })
    .width(Length::Fill)
    .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
    .into()
}

fn breadcrumb_scroller(
    segments: Vec<bennu_theme::address_bar::BreadcrumbSegment>,
    opacity: f32,
    viewport_width: f32,
    request_path: String,
    shift_pressed: bool,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let region = SessionScrollRegion::Breadcrumb;
    let breadcrumbs = elastic_breadcrumbs(segments, opacity, viewport_width, emit);
    let scroller = scrollable(super::smooth_scroll_region(
        breadcrumbs,
        region,
        shift_pressed,
    ))
    .id(scroll_id(&request_path, region))
    .width(Length::Fill)
    .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
    .direction(
        bennu_theme::styles::enhanced_horizontal_scrollbar_direction(
            scrollbar_visibility,
            BREADCRUMB_SCROLLBAR_WIDTH,
        ),
    )
    .style(bennu_theme::styles::enhanced_scrollbar_style(
        scrollbar_visibility,
    ))
    .on_scroll(scrollbar_on_scroll(region, move |_| {
        SessionMessage::ScrollbarEngaged { region }
    }));

    enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        scrollbar_viewport,
        ScrollbarAxis::Horizontal,
        BREADCRUMB_SCROLLBAR_WIDTH,
    )
    .into()
}

/// 弹性面包屑：段按钮与分隔符按「段/分隔符交错」交给共享 widget 布局；
/// Home 段是 House 图标，Root/Name 段按自然宽度测量（主软件同构）。
fn elastic_breadcrumbs(
    segments: Vec<bennu_theme::address_bar::BreadcrumbSegment>,
    opacity: f32,
    viewport_width: f32,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let mut children = Vec::with_capacity(segments.len().saturating_mul(2).saturating_sub(1));
    let mut measurements = Vec::with_capacity(segments.len());

    for (index, segment) in segments.into_iter().enumerate() {
        if index > 0 {
            children.push(faded_icon(
                IconSymbol::ChevronRight,
                BREADCRUMB_SEPARATOR_SIZE,
                opacity,
            ));
        }

        let display_text = segment.display_text();
        let content: Element<'static, SessionMessage> = match &segment.kind {
            BreadcrumbSegmentKind::Home => {
                faded_icon(IconSymbol::House, BREADCRUMB_ICON_SIZE, opacity)
            }
            BreadcrumbSegmentKind::Root | BreadcrumbSegmentKind::Name(_) => {
                measured_middle_ellipsized_text(display_text.clone(), ADDRESS_TEXT_SIZE)
            }
        };
        let target = segment.target;
        let segment_button = button(content)
            .padding([7, BREADCRUMB_HORIZONTAL_PADDING as u16])
            .width(Length::Fill)
            .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
            .style(move |theme, status| faded_button_style(theme, status, opacity, false))
            .on_press(emit(SessionMessage::BreadcrumbActivated { target }));

        measurements.push(match &segment.kind {
            BreadcrumbSegmentKind::Home => BreadcrumbMeasurement::Home,
            BreadcrumbSegmentKind::Root | BreadcrumbSegmentKind::Name(_) => {
                BreadcrumbMeasurement::Text(display_text)
            }
        });
        children.push(segment_button.into());
    }

    Element::new(ElasticBreadcrumbs::new(
        children,
        measurements,
        viewport_width,
    ))
}

fn path_suggestions_panel(
    editing: &bennu_theme::address_bar::AddressEditingSession,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let mut suggestions = Column::new().spacing(3).padding(4);
    for (index, suggestion) in editing.suggestions.iter().enumerate() {
        suggestions = suggestions.push(path_suggestion_row(
            suggestion,
            editing.suggestion_selection == Some(index),
            emit.clone(),
        ));
    }

    container(suggestions)
        .width(Length::Fill)
        .style(path_suggestions_style)
        .into()
}

fn path_suggestion_row(
    path: &Path,
    is_selected: bool,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let label = path.to_string_lossy();
    let label = format_middle_ellipsized_text(label.as_ref(), PATH_SUGGESTION_MAX_CHARS);
    let item = container(readable_label(label).size(13).width(Length::Fill))
        .padding([5, 8])
        .width(Length::Fill);
    let item = if is_selected {
        item.style(selected_path_suggestion_item_style)
    } else {
        item.style(path_suggestion_item_style)
    };

    mouse_area(item)
        .on_press(emit(SessionMessage::AddressSuggestionSelected {
            path: path.to_path_buf(),
        }))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// 退出过渡期间的草稿快照层：复用输入框样式保证渐出时边框/底色与
/// 编辑态完全连续（主软件 address_exit_snapshot 同构）。
fn address_exit_snapshot(snapshot: &str, opacity: f32) -> Element<'static, SessionMessage> {
    container(measured_middle_ellipsized_text(snapshot.to_owned(), 16))
        .padding([7, 10])
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
        .style(move |theme| {
            let input_style = faded_text_input_style(theme, text_input::Status::Active, opacity);
            container::Style {
                background: Some(input_style.background),
                text_color: Some(input_style.value),
                border: input_style.border,
                ..container::Style::default()
            }
        })
        .into()
}

/// 随过渡分数淡出的图标（面包屑分隔符与 Home 图标）。
fn faded_icon(symbol: IconSymbol, size: f32, opacity: f32) -> Element<'static, SessionMessage> {
    symbol
        .view(size)
        .style(move |theme, status| {
            let mut style = icon_svg_style()(theme, status);
            style.color = style.color.map(|color| scale_color_alpha(color, opacity));
            style
        })
        .into()
}
