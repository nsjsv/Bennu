use crate::config::DEFAULT_ICON_GRID_SIZE;
use crate::model::IconGridViewport;
#[cfg(test)]
use crate::virtual_range::{
    initial_rows_for_height, initial_virtual_range, vertical_scroll_delta_to_reveal,
    virtual_range_for_viewport,
};

pub(crate) const ICON_GRID_CONTENT_PADDING: f32 = 12.0;
pub(crate) const ICON_GRID_GAP: f32 = 12.0;
pub(crate) const ICON_GRID_OVERSCAN_ROWS: usize = 3;
pub(crate) const ICON_GRID_TILE_VERTICAL_PADDING: u16 = 4;
pub(crate) const ICON_GRID_TILE_HORIZONTAL_PADDING: u16 = 8;
pub(crate) const ICON_GRID_ICON_LABEL_SPACING: u32 = 8;
pub(crate) const ICON_GRID_LABEL_LINES: usize = 3;
pub(crate) const ICON_GRID_LABEL_SIZE: f32 = 14.0;
pub(crate) const ICON_GRID_LABEL_LINE_HEIGHT_PX: f32 = 17.0;
pub(crate) const ICON_GRID_LABEL_HEIGHT: f32 =
    ICON_GRID_LABEL_LINE_HEIGHT_PX * ICON_GRID_LABEL_LINES as f32;
const ICON_GRID_TILE_EXTRA_WIDTH: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconGridDirection {
    Up,
    Down,
    Left,
    Right,
}

// 图标边长是档位的唯一输入：卡片内边距、标签、间距等辅助几何按
// 96px 基准档同比例缩放，保证所有调用点传入同一 icon_edge 即得到一致几何。
// 缩放结果一律取整到整数像素：整数几何让整行对齐的 viewport 运算保持精确，
// 避免虚拟范围与缩略图调度在浮点临界点上各舍入到不同行。
fn icon_grid_scale(icon_edge: u32) -> f32 {
    icon_edge as f32 / DEFAULT_ICON_GRID_SIZE as f32
}

fn scaled_by_icon_grid(base: f32, icon_edge: u32) -> f32 {
    (base * icon_grid_scale(icon_edge)).round()
}

pub(crate) fn grid_gap(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_GAP, icon_edge)
}

pub(crate) fn tile_padding_vertical(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(f32::from(ICON_GRID_TILE_VERTICAL_PADDING), icon_edge)
}

pub(crate) fn tile_padding_horizontal(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(f32::from(ICON_GRID_TILE_HORIZONTAL_PADDING), icon_edge)
}

pub(crate) fn icon_label_spacing(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_ICON_LABEL_SPACING as f32, icon_edge)
}

pub(crate) fn label_size(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_LABEL_SIZE, icon_edge)
}

pub(crate) fn label_line_height(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_LABEL_LINE_HEIGHT_PX, icon_edge)
}

pub(crate) fn label_height(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_LABEL_HEIGHT, icon_edge)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IconGridVisibleRange {
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) start_entry: usize,
    pub(crate) end_entry: usize,
    pub(crate) before_height: f32,
    pub(crate) after_height: f32,
}

pub(crate) fn tile_width(icon_edge: u32) -> f32 {
    icon_edge as f32 + scaled_by_icon_grid(ICON_GRID_TILE_EXTRA_WIDTH, icon_edge)
}

pub(crate) fn tile_visual_height(icon_edge: u32) -> f32 {
    icon_edge as f32
        + tile_padding_vertical(icon_edge) * 2.0
        + icon_label_spacing(icon_edge)
        + label_height(icon_edge)
}

pub(crate) fn row_height(icon_edge: u32) -> f32 {
    tile_visual_height(icon_edge) + grid_gap(icon_edge)
}

pub(crate) fn column_count_for_width(viewport_width: f32, icon_edge: u32) -> usize {
    let available_width = (viewport_width - ICON_GRID_CONTENT_PADDING * 2.0).max(0.0);
    let column_slot_width = tile_width(icon_edge) + grid_gap(icon_edge);
    ((available_width + grid_gap(icon_edge)) / column_slot_width)
        .floor()
        .max(1.0) as usize
}

pub(crate) fn row_count_for_entries(entry_count: usize, column_count: usize) -> usize {
    entry_count.div_ceil(column_count.max(1))
}

#[cfg(test)]
pub(crate) fn visible_entry_range(
    viewport: IconGridViewport,
    entry_count: usize,
    height_bound: f32,
    icon_edge: u32,
) -> IconGridVisibleRange {
    let column_count = column_count_for_width(viewport.width, icon_edge);
    let total_rows = row_count_for_entries(entry_count, column_count);
    let row_height = row_height(icon_edge);
    let rows = if viewport.width > f32::EPSILON && viewport.height > f32::EPSILON {
        virtual_range_for_viewport(
            total_rows,
            row_height,
            (viewport.offset_y - ICON_GRID_CONTENT_PADDING).max(0.0),
            viewport.height,
            ICON_GRID_OVERSCAN_ROWS,
        )
    } else {
        initial_virtual_range(
            total_rows,
            row_height,
            initial_rows_for_height(height_bound, row_height, ICON_GRID_OVERSCAN_ROWS),
        )
    };

    IconGridVisibleRange {
        start_row: rows.start,
        end_row: rows.end,
        start_entry: rows.start.saturating_mul(column_count).min(entry_count),
        end_entry: rows.end.saturating_mul(column_count).min(entry_count),
        before_height: rows.before_height,
        after_height: rows.after_height,
    }
}

pub(crate) fn keyboard_target_index(
    current_index: Option<usize>,
    direction: IconGridDirection,
    entry_count: usize,
    column_count: usize,
) -> Option<usize> {
    if entry_count == 0 {
        return None;
    }

    let last_index = entry_count - 1;
    let Some(current_index) = current_index.filter(|index| *index < entry_count) else {
        return Some(match direction {
            IconGridDirection::Up | IconGridDirection::Left => last_index,
            IconGridDirection::Down | IconGridDirection::Right => 0,
        });
    };
    let column_count = column_count.max(1);

    Some(match direction {
        IconGridDirection::Up => current_index.saturating_sub(column_count),
        IconGridDirection::Down => current_index.saturating_add(column_count).min(last_index),
        IconGridDirection::Left => current_index.saturating_sub(1),
        IconGridDirection::Right => current_index.saturating_add(1).min(last_index),
    })
}

/// 可见窗口的纵向范围(含上下 overscan):viewport 无效时退化为
/// "从顶部到一屏高",供首帧/未测量场景兜底。
pub(crate) fn visible_vertical_window(
    viewport: IconGridViewport,
    icon_edge: u32,
    height_bound: f32,
) -> (f32, f32) {
    let overscan = ICON_GRID_OVERSCAN_ROWS as f32 * row_height(icon_edge);
    if viewport.width > f32::EPSILON && viewport.height > f32::EPSILON {
        (
            (viewport.offset_y - overscan).max(0.0),
            viewport.offset_y + viewport.height + overscan,
        )
    } else {
        (
            0.0,
            ICON_GRID_CONTENT_PADDING + height_bound.max(0.0) + overscan,
        )
    }
}

#[cfg(test)]
pub(crate) fn scroll_delta_to_reveal_row(
    viewport: IconGridViewport,
    target_row: usize,
    icon_edge: u32,
) -> f32 {
    vertical_scroll_delta_to_reveal(
        viewport.offset_y,
        viewport.height,
        ICON_GRID_CONTENT_PADDING + target_row as f32 * row_height(icon_edge),
        tile_visual_height(icon_edge),
    )
}

pub(crate) fn thumbnail_edge(icon_edge: u32) -> u32 {
    icon_edge.saturating_mul(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ViewDensityLevel, ViewDensityStep};
    use crate::config::{MAX_ICON_GRID_SIZE, MIN_ICON_GRID_SIZE};

    #[test]
    fn narrow_width_always_keeps_one_column() {
        assert_eq!(column_count_for_width(1.0, 96), 1);
    }

    #[test]
    fn column_count_changes_at_exact_slot_boundary() {
        let two_columns_width =
            ICON_GRID_CONTENT_PADDING * 2.0 + tile_width(96) * 2.0 + ICON_GRID_GAP;

        assert_eq!(column_count_for_width(two_columns_width - 0.1, 96), 1);
        assert_eq!(column_count_for_width(two_columns_width, 96), 2);
        assert_eq!(column_count_for_width(500.0, 96), 3);
    }

    #[test]
    fn incomplete_last_row_is_counted_once() {
        assert_eq!(row_count_for_entries(7, 3), 3);
        assert_eq!(row_count_for_entries(0, 3), 0);
    }

    #[test]
    fn tile_height_is_derived_from_the_shared_three_line_label_geometry() {
        assert_eq!(
            ICON_GRID_LABEL_HEIGHT,
            ICON_GRID_LABEL_LINE_HEIGHT_PX * ICON_GRID_LABEL_LINES as f32
        );
        assert_eq!(
            tile_visual_height(96),
            96.0 + f32::from(ICON_GRID_TILE_VERTICAL_PADDING) * 2.0
                + ICON_GRID_ICON_LABEL_SPACING as f32
                + ICON_GRID_LABEL_HEIGHT
        );
        assert_eq!(row_height(96), tile_visual_height(96) + ICON_GRID_GAP);
    }

    #[test]
    fn visible_range_uses_row_overscan_and_clamps_partial_row() {
        let viewport = IconGridViewport {
            offset_y: ICON_GRID_CONTENT_PADDING + row_height(96) * 10.0,
            width: 500.0,
            height: row_height(96) * 2.0,
        };
        let range = visible_entry_range(viewport, 44, 800.0, 96);

        assert_eq!(range.start_row, 7);
        assert_eq!(range.end_row, 15);
        assert_eq!(range.start_entry, 21);
        assert_eq!(range.end_entry, 44);
    }

    #[test]
    fn keyboard_navigation_clamps_to_entry_boundaries() {
        assert_eq!(
            keyboard_target_index(Some(5), IconGridDirection::Up, 8, 3),
            Some(2)
        );
        assert_eq!(
            keyboard_target_index(Some(5), IconGridDirection::Down, 8, 3),
            Some(7)
        );
        assert_eq!(
            keyboard_target_index(Some(0), IconGridDirection::Left, 8, 3),
            Some(0)
        );
        assert_eq!(
            keyboard_target_index(Some(7), IconGridDirection::Right, 8, 3),
            Some(7)
        );
    }

    #[test]
    fn keyboard_navigation_without_selection_uses_directional_edge() {
        assert_eq!(
            keyboard_target_index(None, IconGridDirection::Up, 8, 3),
            Some(7)
        );
        assert_eq!(
            keyboard_target_index(None, IconGridDirection::Right, 8, 3),
            Some(0)
        );
    }

    #[test]
    fn scroll_delta_only_reveals_rows_outside_viewport() {
        let viewport = IconGridViewport {
            offset_y: 160.0,
            width: 500.0,
            height: 320.0,
        };

        assert!(scroll_delta_to_reveal_row(viewport, 0, 96) < 0.0);
        assert_eq!(scroll_delta_to_reveal_row(viewport, 1, 96), 0.0);
        assert!(scroll_delta_to_reveal_row(viewport, 3, 96) > 0.0);
    }

    #[test]
    fn zoom_steps_and_clamp_at_limits() {
        assert_eq!(ViewDensityLevel::DEFAULT.icon_grid_size(), 96);
        assert_eq!(
            ViewDensityLevel::from_index(8)
                .step(ViewDensityStep::Increase)
                .icon_grid_size(),
            MAX_ICON_GRID_SIZE
        );
        assert_eq!(
            ViewDensityLevel::from_index(0)
                .step(ViewDensityStep::Decrease)
                .icon_grid_size(),
            MIN_ICON_GRID_SIZE
        );
    }

    #[test]
    fn default_edge_keeps_legacy_geometry_and_edges_scale_auxiliary_dimensions() {
        let default_edge = ViewDensityLevel::DEFAULT.icon_grid_size();
        assert_eq!(icon_grid_scale(default_edge), 1.0);
        assert_eq!(tile_width(default_edge), 96.0 + ICON_GRID_TILE_EXTRA_WIDTH);
        assert_eq!(grid_gap(default_edge), ICON_GRID_GAP);
        assert_eq!(label_size(default_edge), ICON_GRID_LABEL_SIZE);
        assert_eq!(
            tile_visual_height(default_edge),
            96.0 + f32::from(ICON_GRID_TILE_VERTICAL_PADDING) * 2.0
                + ICON_GRID_ICON_LABEL_SPACING as f32
                + ICON_GRID_LABEL_HEIGHT
        );

        let max_edge = MAX_ICON_GRID_SIZE;
        assert_eq!(icon_grid_scale(max_edge), 2.0);
        assert_eq!(
            tile_width(max_edge),
            192.0 + ICON_GRID_TILE_EXTRA_WIDTH * 2.0
        );
        assert_eq!(grid_gap(max_edge), ICON_GRID_GAP * 2.0);
        assert_eq!(
            label_line_height(max_edge),
            ICON_GRID_LABEL_LINE_HEIGHT_PX * 2.0
        );
        assert_eq!(
            row_height(max_edge),
            tile_visual_height(max_edge) + ICON_GRID_GAP * 2.0
        );

        let min_edge = MIN_ICON_GRID_SIZE;
        assert!((icon_grid_scale(min_edge) - 64.0 / 96.0).abs() < f32::EPSILON);
        // 缩放尺寸四舍五入到整数像素。
        assert_eq!(
            tile_width(min_edge),
            64.0 + (ICON_GRID_TILE_EXTRA_WIDTH * 2.0 / 3.0).round()
        );
    }

    #[test]
    fn thumbnail_request_uses_double_display_edge() {
        assert_eq!(thumbnail_edge(96), 192);
        assert_eq!(thumbnail_edge(192), 384);
    }
}
