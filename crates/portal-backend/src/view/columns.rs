//! 多栏（访达式栏链）视图：横向栏容器（固定可见 3 栏等宽、超出横向
//! 滚动）+ 逐栏纵向滚动的条目行（图标/行内缩略图 + 名称 + 目录行尾
//! chevron）。交互消息与列表同源（ColumnEntryClicked /
//! ColumnEntryDoubleClicked / ColumnEntryHovered）；视觉词汇（样式、
//! 图标、滚动条）全部来自 bennu-theme 共享层。

use bennu_theme::column_geometry::{ColumnEntryGeometry, COLUMN_ENTRY_TEXT_SIZE, COLUMN_PADDING};
use bennu_theme::icons::{file_entry_icon_symbol, IconSymbol};
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{
    enhanced_horizontal_scrollbar_direction, enhanced_scrollbar_style,
    enhanced_vertical_scrollbar_direction, hovered_row_style, muted_icon_svg_style,
    selected_icon_svg_style, selected_row_style, subtle_border_color,
};
use file_core::entry::FileKind;
use file_core::is_supported_image_path;
use iced::widget::{container, image, mouse_area, row, scrollable, Space};
use iced::{alignment, Element, Length, Theme};

use crate::picker_session::scrollbar::{scroll_id, scrollbar_on_scroll};
use crate::picker_session::{
    PickerSession, SessionMessage, SessionScrollRegion, COLUMNS_SCALE, MIN_LANE_WIDTH,
};

use super::{readable_label, smooth_scroll_region};

/// 多栏滚动条静态宽度：与列表同值（mac 式细滚动条）。
const COLUMNS_SCROLLBAR_WIDTH: f32 = 8.0;
/// 行内图标/缩略图边长：与列表行同尺寸（prd：行内小缩略图同列表）。
const LANE_ICON_EDGE: f32 = 18.0;

/// 多栏主体：横向栏容器 + 逐栏。栏宽由会话按栏容器视口宽给出
/// （÷ 可见栏数，最小 96；首帧探针未回时按估算基准）。
pub(super) fn columns_body(
    session: &PickerSession,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let lane_width = session.columns_lane_width();
    let lane_count = session.columns_chain().len();
    let mut rail = row![].height(Length::Fill);
    for lane in 0..lane_count {
        if lane > 0 {
            // 1px 栏间分隔线（subtle 边框色）。
            rail = rail.push(
                container(Space::new().height(Length::Fill))
                    .width(Length::Fixed(1.0))
                    .height(Length::Fill)
                    .style(|theme: &Theme| container::Style {
                        background: Some(subtle_border_color(theme).into()),
                        ..container::Style::default()
                    }),
            );
        }
        rail = rail.push(lane_view(session, lane, lane_width, emit.clone()));
    }

    let region = SessionScrollRegion::ColumnsRail;
    let scrollbar_visibility = session.scrollbar_visibility_for(&region);
    let scroller = scrollable(smooth_scroll_region(
        rail,
        region,
        session.scroll_shift_pressed(),
    ))
    .id(scroll_id(session.request_path(), region))
    .width(Length::Fill)
    .height(Length::Fill)
    .direction(enhanced_horizontal_scrollbar_direction(
        scrollbar_visibility,
        COLUMNS_SCROLLBAR_WIDTH,
    ))
    .style(enhanced_scrollbar_style(scrollbar_visibility))
    .on_scroll(scrollbar_on_scroll(region, |_| {
        SessionMessage::ScrollbarEngaged {
            region: SessionScrollRegion::ColumnsRail,
        }
    }));
    enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        session.scrollbar_viewport_for(&region),
        ScrollbarAxis::Horizontal,
        COLUMNS_SCROLLBAR_WIDTH,
    )
}

/// 单栏：纵向滚动的过滤条目行。栏头无 chrome，行几何来自共享
/// column_geometry（portal 固定基准档）。
fn lane_view(
    session: &PickerSession,
    lane: usize,
    lane_width: f32,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let geometry = ColumnEntryGeometry::for_scale(COLUMNS_SCALE);
    let mut content = iced::widget::column![]
        .spacing(geometry.content_spacing)
        .padding(COLUMN_PADDING)
        .width(Length::Fill);
    for (index, entry) in session.column_entries_iter(lane).enumerate() {
        content = content.push(lane_entry_row(session, lane, index, entry, emit.clone()));
    }

    let region = SessionScrollRegion::ColumnsLane(lane);
    let scrollbar_visibility = session.scrollbar_visibility_for(&region);
    let scroller = scrollable(smooth_scroll_region(
        content,
        region,
        session.scroll_shift_pressed(),
    ))
    .id(scroll_id(session.request_path(), region))
    .width(Length::Fill)
    .height(Length::Fill)
    .direction(enhanced_vertical_scrollbar_direction(
        scrollbar_visibility,
        COLUMNS_SCROLLBAR_WIDTH,
    ))
    .style(enhanced_scrollbar_style(scrollbar_visibility))
    .on_scroll(scrollbar_on_scroll(region, move |_| {
        SessionMessage::ScrollbarEngaged {
            region: SessionScrollRegion::ColumnsLane(lane),
        }
    }));
    let scrolled = enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        session.scrollbar_viewport_for(&region),
        ScrollbarAxis::Vertical,
        COLUMNS_SCROLLBAR_WIDTH,
    );
    container(scrolled)
        .width(Length::Fixed(lane_width.max(MIN_LANE_WIDTH)))
        .height(Length::Fill)
        .into()
}

/// 单个条目行：图标/行内缩略图 + 名称 + 目录行尾 chevron；高亮 =
/// 本栏选中集 ∪ 光标（row_highlighted 唯一规则），悬停 = hover 面。
fn lane_entry_row(
    session: &PickerSession,
    lane: usize,
    index: usize,
    entry: &file_core::entry::DirectoryEntry,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let geometry = ColumnEntryGeometry::for_scale(COLUMNS_SCALE);
    let selected = session.columns_row_highlighted(lane, index);
    let hovered = session.columns_hovered(lane) == Some(index);
    let icon_tone = if selected {
        selected_icon_svg_style()
    } else {
        muted_icon_svg_style()
    };
    // 图片行有就绪缩略图时用行内 PNG 替换类型图标（与列表行同尺寸；
    // image 默认 ContentFit::Contain 保持纵横比）。
    let icon: Element<'static, SessionMessage> =
        if entry.kind == FileKind::File && is_supported_image_path(&entry.path) {
            match session.thumbnail_ready(&entry.path) {
                Some(cached) => image(image::Handle::from_path(&cached.output))
                    .width(Length::Fixed(LANE_ICON_EDGE))
                    .height(Length::Fixed(LANE_ICON_EDGE))
                    .into(),
                None => file_entry_icon_symbol(entry.kind, &entry.name)
                    .view(LANE_ICON_EDGE)
                    .style(icon_tone)
                    .into(),
            }
        } else {
            file_entry_icon_symbol(entry.kind, &entry.name)
                .view(LANE_ICON_EDGE)
                .style(icon_tone)
                .into()
        };

    // 目录行尾 chevron；文件行用等宽占位保证名称列对齐。
    let trailing: Element<'static, SessionMessage> = if entry.kind == FileKind::Directory {
        IconSymbol::ChevronRight
            .view(geometry.chevron_icon_size)
            .style(icon_tone)
            .into()
    } else {
        Space::new()
            .width(Length::Fixed(geometry.chevron_icon_size))
            .into()
    };

    let content = row![
        icon,
        readable_label(entry.name.to_string_lossy().into_owned()).size(COLUMN_ENTRY_TEXT_SIZE,),
        trailing
    ]
    .spacing(geometry.entry_spacing)
    .align_y(alignment::Vertical::Center);

    // 常态透明：栏直接坐在窗口背景上（条纹容器底色契约同列表面板）。
    let style = move |theme: &Theme| {
        if selected {
            selected_row_style(theme)
        } else if hovered {
            hovered_row_style(theme)
        } else {
            container::Style::default()
        }
    };

    mouse_area(
        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(geometry.entry_height))
            .center_y(Length::Fixed(geometry.entry_height))
            .padding(geometry.entry_padding)
            .style(style),
    )
    .on_enter(emit(SessionMessage::ColumnEntryHovered {
        lane,
        index: Some(index),
    }))
    .on_exit(emit(SessionMessage::ColumnEntryHovered {
        lane,
        index: None,
    }))
    .on_press(emit(SessionMessage::ColumnEntryClicked {
        lane,
        index,
        ctrl: false,
        shift: false,
    }))
    .on_double_click(emit(SessionMessage::ColumnEntryDoubleClicked {
        lane,
        index,
    }))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}
