//! 网格几何的唯一实现在 bennu-theme（共享单源）；本文件只保留依赖
//! app-ui 私有视口模型（IconGridViewport）与虚拟行范围求解器的换算。
//! 纯几何（常量、tile/行列/键盘目标索引等）全部再导出自共享层，
//! 既有调用点的 `crate::icon_grid_geometry::…` 路径保持不变。

#[cfg(test)]
use crate::virtual_range::{
    initial_rows_for_height, initial_virtual_range, vertical_scroll_delta_to_reveal,
    virtual_range_for_viewport,
};
pub use bennu_theme::icon_grid_geometry::{
    column_count_for_width, grid_gap, icon_label_spacing, keyboard_target_index, label_height,
    label_line_height, label_size, row_count_for_entries, row_height, thumbnail_edge,
    tile_padding_horizontal, tile_padding_vertical, tile_visual_height, tile_width,
    IconGridDirection, ICON_GRID_CONTENT_PADDING, ICON_GRID_LABEL_SIZE, ICON_GRID_OVERSCAN_ROWS,
};

use crate::model::IconGridViewport;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ViewDensityLevel, ViewDensityStep};
    use crate::config::{MAX_ICON_GRID_SIZE, MIN_ICON_GRID_SIZE};
    use bennu_theme::icon_grid_geometry::icon_grid_scale;

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
    fn default_edge_geometry_matches_view_density_levels() {
        // 档位端点与共享几何的缩放假设一致：默认 96 / 最大 192 / 最小 64。
        let default_edge = ViewDensityLevel::DEFAULT.icon_grid_size();
        assert_eq!(icon_grid_scale(default_edge), 1.0);

        let max_edge = MAX_ICON_GRID_SIZE;
        assert_eq!(icon_grid_scale(max_edge), 2.0);
        let min_edge = MIN_ICON_GRID_SIZE;
        assert!((icon_grid_scale(min_edge) - 64.0 / 96.0).abs() < f32::EPSILON);
    }
}
