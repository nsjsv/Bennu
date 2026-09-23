//! 大图（图标网格）视图：按视口宽度用共享几何排行，tile = 96px
//! 图标/缩略图 + 最多 3 行居中文件名。交互消息与列表同源
//! （EntryClicked/EntryDoubleClicked/EntryHovered），索引即根行索引；
//! 视觉词汇（样式、图标）全部来自 bennu-theme 共享层。

use bennu_theme::icon_grid_geometry::{
    grid_gap, icon_label_spacing, label_height, label_size, tile_padding_horizontal,
    tile_padding_vertical, tile_visual_height, tile_width, ICON_GRID_CONTENT_PADDING,
};
use bennu_theme::icons::file_entry_icon_symbol;
use bennu_theme::scrollbar::{enhanced_scrollbar, ScrollbarAxis};
use bennu_theme::styles::{
    enhanced_scrollbar_style, enhanced_vertical_scrollbar_direction, hovered_row_style,
    muted_icon_svg_style, selected_icon_svg_style, selected_row_style,
};
use file_core::entry::FileKind;
use file_core::is_supported_image_path;
use iced::widget::{column, container, image, mouse_area, row, scrollable};
use iced::{alignment, Element, Length, Padding, Theme};

use crate::picker_session::scrollbar::{scroll_id, scrollbar_on_scroll};
use crate::picker_session::{
    PickerRow, PickerSession, SessionMessage, SessionScrollRegion, ICON_GRID_EDGE,
};

use super::{readable_label, smooth_scroll_region};

/// 大图网格滚动条静态宽度：与列表同值（mac 式细滚动条）。
const GRID_SCROLLBAR_WIDTH: f32 = 8.0;

/// 大图固定档：portal 不做图标尺寸调节（prd out of scope）。
const ICON_EDGE: u32 = ICON_GRID_EDGE;

/// 网格主体：滚动容器 + 逐行 tile。行几何全部来自共享 icon_grid_geometry，
/// 列数由会话按视口宽度给出（首帧探针未回时按窗口默认宽估算）。
pub(super) fn icon_grid_body(
    session: &PickerSession,
    emit: impl Fn(SessionMessage) -> SessionMessage + Clone + 'static,
) -> Element<'static, SessionMessage> {
    let column_count = session.icon_grid_column_count().max(1);
    let gap = grid_gap(ICON_EDGE);
    let mut grid = column![].spacing(gap);
    for (row_index, chunk) in session.rows().chunks(column_count).enumerate() {
        let mut grid_row = row![].spacing(gap);
        for (offset, row) in chunk.iter().enumerate() {
            let index = row_index * column_count + offset;
            grid_row = grid_row.push(tile(
                index,
                row,
                session,
                session.selection().contains(&index),
                session.hovered_index() == Some(index),
                emit.clone(),
            ));
        }
        grid = grid.push(grid_row);
    }

    let region = SessionScrollRegion::List;
    let scrollbar_visibility = session.scrollbar_visibility_for(&region);
    let scroller = scrollable(smooth_scroll_region(
        container(grid).padding(ICON_GRID_CONTENT_PADDING),
        region,
        session.scroll_shift_pressed(),
    ))
    .id(scroll_id(session.request_path(), region))
    .width(Length::Fill)
    .height(Length::Fill)
    .direction(enhanced_vertical_scrollbar_direction(
        scrollbar_visibility,
        GRID_SCROLLBAR_WIDTH,
    ))
    .style(enhanced_scrollbar_style(scrollbar_visibility))
    .on_scroll(scrollbar_on_scroll(region, |_| {
        SessionMessage::ScrollbarEngaged {
            region: SessionScrollRegion::List,
        }
    }));
    // mac 式滚动条与列表同一份包装（透明原生拇指 + canvas 浮层拇指）。
    enhanced_scrollbar(
        scroller,
        scrollbar_visibility,
        session.scrollbar_viewport_for(&region),
        ScrollbarAxis::Vertical,
        GRID_SCROLLBAR_WIDTH,
    )
}

/// 单个 tile：96px 图标/缩略图槽 + 最多 3 行居中文件名，交互与列表行
/// 同一套消息（单击选中 / 双击激活 / 悬停高亮）。
fn tile(
    index: usize,
    row: &PickerRow,
    session: &PickerSession,
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
    // 图片 tile 有就绪缩略图时用 PNG 替换类型图标（96×96，image 默认
    // ContentFit::Contain 保持纵横比）。Handle::from_path 只持路径不驻
    // 留像素，每帧重建成本可忽略。
    let icon: Element<'static, SessionMessage> =
        if entry.kind == FileKind::File && is_supported_image_path(&entry.path) {
            match session.thumbnail_ready(&entry.path) {
                Some(cached) => image(image::Handle::from_path(&cached.output))
                    .width(Length::Fixed(ICON_EDGE as f32))
                    .height(Length::Fixed(ICON_EDGE as f32))
                    .into(),
                // 未就绪/失败 backoff 期间保持类型图标占位。
                None => file_entry_icon_symbol(entry.kind, &entry.name)
                    .view(ICON_EDGE as f32)
                    .style(icon_tone)
                    .into(),
            }
        } else {
            file_entry_icon_symbol(entry.kind, &entry.name)
                .view(ICON_EDGE as f32)
                .style(icon_tone)
                .into()
        };

    // 文件名最多 3 行：容器高度钉在共享标签几何上并裁剪溢出。
    // iced 0.14 container 默认左上对齐，水平居中必须显式设在容器上
    // （文字自身宽度 Shrink，文字级 align_x 无空间可居中）。
    let label = container(
        readable_label(entry.name.to_string_lossy().into_owned()).size(label_size(ICON_EDGE)),
    )
    .width(Length::Fill)
    .height(Length::Fixed(label_height(ICON_EDGE)))
    .align_x(alignment::Horizontal::Center)
    .align_y(alignment::Vertical::Top)
    .clip(true);

    let content = column![icon, label]
        .spacing(icon_label_spacing(ICON_EDGE))
        .align_x(alignment::Horizontal::Center)
        .width(Length::Fixed(tile_width(ICON_EDGE)))
        .height(Length::Fixed(tile_visual_height(ICON_EDGE)))
        .padding(
            Padding::new(0.0)
                .top(tile_padding_vertical(ICON_EDGE))
                .bottom(tile_padding_vertical(ICON_EDGE))
                .left(tile_padding_horizontal(ICON_EDGE))
                .right(tile_padding_horizontal(ICON_EDGE)),
        );

    // 选中/悬停样式与列表行同源（primary container / hover 面）；常态
    // 透明，网格直接坐在窗口背景上（容器底色契约同列表）。
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
            .width(Length::Shrink)
            .height(Length::Shrink)
            .style(style),
    )
    .on_enter(emit(SessionMessage::EntryHovered { index: Some(index) }))
    .on_exit(emit(SessionMessage::EntryHovered { index: None }))
    .on_press(emit(SessionMessage::EntryClicked {
        index,
        ctrl: false,
        shift: false,
    }))
    .on_double_click(emit(SessionMessage::EntryDoubleClicked { index }))
    .into()
}
