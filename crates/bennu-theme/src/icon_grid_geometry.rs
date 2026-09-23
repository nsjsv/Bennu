//! 图标网格几何的唯一共享实现：主软件（app-ui）与 portal 大图视图
//! 共用同一份常量与纯函数，禁止任何一侧复制副本。函数入参一律为
//! 原生类型（图标边长 u32、视口宽/偏移 f32），不依赖任何调用方的
//! 私有视口模型；依赖调用方视口结构体的换算留在各自 crate。

/// 基准图标边长：辅助几何按它同比例缩放（主软件默认档
/// DEFAULT_ICON_GRID_SIZE；portal 固定用这一档）。
const BASE_ICON_EDGE: u32 = 96;

pub const ICON_GRID_CONTENT_PADDING: f32 = 12.0;
pub const ICON_GRID_GAP: f32 = 12.0;
pub const ICON_GRID_OVERSCAN_ROWS: usize = 3;
pub const ICON_GRID_TILE_VERTICAL_PADDING: u16 = 4;
pub const ICON_GRID_TILE_HORIZONTAL_PADDING: u16 = 8;
pub const ICON_GRID_ICON_LABEL_SPACING: u32 = 8;
pub const ICON_GRID_LABEL_LINES: usize = 3;
pub const ICON_GRID_LABEL_SIZE: f32 = 14.0;
pub const ICON_GRID_LABEL_LINE_HEIGHT_PX: f32 = 17.0;
pub const ICON_GRID_LABEL_HEIGHT: f32 =
    ICON_GRID_LABEL_LINE_HEIGHT_PX * ICON_GRID_LABEL_LINES as f32;
const ICON_GRID_TILE_EXTRA_WIDTH: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconGridDirection {
    Up,
    Down,
    Left,
    Right,
}

// 图标边长是档位的唯一输入：卡片内边距、标签、间距等辅助几何按
// 96px 基准档同比例缩放，保证所有调用点传入同一 icon_edge 即得到一致几何。
// 缩放结果一律取整到整数像素：整数几何让整行对齐的 viewport 运算保持精确，
// 避免虚拟范围与缩略图调度在浮点临界点上各舍入到不同行。
pub fn icon_grid_scale(icon_edge: u32) -> f32 {
    icon_edge as f32 / BASE_ICON_EDGE as f32
}

fn scaled_by_icon_grid(base: f32, icon_edge: u32) -> f32 {
    (base * icon_grid_scale(icon_edge)).round()
}

pub fn grid_gap(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_GAP, icon_edge)
}

pub fn tile_padding_vertical(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(f32::from(ICON_GRID_TILE_VERTICAL_PADDING), icon_edge)
}

pub fn tile_padding_horizontal(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(f32::from(ICON_GRID_TILE_HORIZONTAL_PADDING), icon_edge)
}

pub fn icon_label_spacing(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_ICON_LABEL_SPACING as f32, icon_edge)
}

pub fn label_size(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_LABEL_SIZE, icon_edge)
}

pub fn label_line_height(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_LABEL_LINE_HEIGHT_PX, icon_edge)
}

pub fn label_height(icon_edge: u32) -> f32 {
    scaled_by_icon_grid(ICON_GRID_LABEL_HEIGHT, icon_edge)
}

pub fn tile_width(icon_edge: u32) -> f32 {
    icon_edge as f32 + scaled_by_icon_grid(ICON_GRID_TILE_EXTRA_WIDTH, icon_edge)
}

pub fn tile_visual_height(icon_edge: u32) -> f32 {
    icon_edge as f32
        + tile_padding_vertical(icon_edge) * 2.0
        + icon_label_spacing(icon_edge)
        + label_height(icon_edge)
}

pub fn row_height(icon_edge: u32) -> f32 {
    tile_visual_height(icon_edge) + grid_gap(icon_edge)
}

pub fn column_count_for_width(viewport_width: f32, icon_edge: u32) -> usize {
    let available_width = (viewport_width - ICON_GRID_CONTENT_PADDING * 2.0).max(0.0);
    let column_slot_width = tile_width(icon_edge) + grid_gap(icon_edge);
    ((available_width + grid_gap(icon_edge)) / column_slot_width)
        .floor()
        .max(1.0) as usize
}

pub fn row_count_for_entries(entry_count: usize, column_count: usize) -> usize {
    entry_count.div_ceil(column_count.max(1))
}

pub fn keyboard_target_index(
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

pub fn thumbnail_edge(icon_edge: u32) -> u32 {
    icon_edge.saturating_mul(2)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn default_edge_keeps_legacy_geometry_and_edges_scale_auxiliary_dimensions() {
        let default_edge = 96;
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

        let max_edge = 192;
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

        let min_edge = 64;
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
