#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VirtualRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) before_height: f32,
    pub(crate) after_height: f32,
}

impl VirtualRange {
    pub(crate) fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            before_height: 0.0,
            after_height: 0.0,
        }
    }
}

pub(crate) fn virtual_range_for_viewport(
    total_rows: usize,
    row_height: f32,
    offset_y: f32,
    viewport_height: f32,
    overscan_rows: usize,
) -> VirtualRange {
    if total_rows == 0 || row_height <= f32::EPSILON {
        return VirtualRange::empty();
    }

    let first_visible = (offset_y.max(0.0) / row_height).floor() as usize;
    let visible_rows = (viewport_height.max(row_height) / row_height).ceil() as usize;
    let start = first_visible.saturating_sub(overscan_rows).min(total_rows);
    let end = first_visible
        .saturating_add(visible_rows)
        .saturating_add(overscan_rows)
        .min(total_rows)
        .max(start);

    heights_for_range(total_rows, row_height, start, end)
}

pub(crate) fn vertical_scroll_delta_to_reveal(
    viewport_offset: f32,
    viewport_height: f32,
    item_offset: f32,
    item_height: f32,
) -> f32 {
    if viewport_height <= f32::EPSILON || item_height <= f32::EPSILON {
        return 0.0;
    }

    let viewport_top = viewport_offset.max(0.0);
    let viewport_bottom = viewport_top + viewport_height;
    let item_top = item_offset.max(0.0);
    let item_bottom = item_top + item_height;

    if item_top < viewport_top {
        item_top - viewport_top
    } else if item_bottom > viewport_bottom {
        item_bottom - viewport_bottom
    } else {
        0.0
    }
}

pub(crate) fn initial_virtual_range(
    total_rows: usize,
    row_height: f32,
    initial_rows: usize,
) -> VirtualRange {
    if total_rows == 0 || row_height <= f32::EPSILON {
        return VirtualRange::empty();
    }
    heights_for_range(total_rows, row_height, 0, initial_rows.min(total_rows))
}

// 初始虚拟窗口行数必须由真实视口高度上界推导。固定行数常数在内容
// 恰好不溢出视口时（无滚动事件记录 viewport）会永久遮蔽折叠线以下
// 的行，且无法通过滚动自愈。
pub(crate) fn initial_rows_for_height(height: f32, row_height: f32, overscan_rows: usize) -> usize {
    if row_height <= f32::EPSILON {
        return overscan_rows;
    }
    (height.max(0.0) / row_height).ceil() as usize + overscan_rows
}

fn heights_for_range(total_rows: usize, row_height: f32, start: usize, end: usize) -> VirtualRange {
    VirtualRange {
        start,
        end,
        before_height: start as f32 * row_height,
        after_height: total_rows.saturating_sub(end) as f32 * row_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_at_start_includes_overscan() {
        let range = virtual_range_for_viewport(100, 10.0, 0.0, 30.0, 2);

        assert_eq!(range.start, 0);
        assert_eq!(range.end, 5);
        assert_eq!(range.before_height, 0.0);
        assert_eq!(range.after_height, 950.0);
    }

    #[test]
    fn viewport_in_middle_has_spacers() {
        let range = virtual_range_for_viewport(100, 10.0, 500.0, 30.0, 2);

        assert_eq!(range.start, 48);
        assert_eq!(range.end, 55);
        assert_eq!(range.before_height, 480.0);
        assert_eq!(range.after_height, 450.0);
    }

    #[test]
    fn viewport_near_end_clamps_end() {
        let range = virtual_range_for_viewport(10, 10.0, 95.0, 30.0, 2);

        assert_eq!(range.start, 7);
        assert_eq!(range.end, 10);
        assert_eq!(range.after_height, 0.0);
    }

    #[test]
    fn initial_rows_cover_viewport_height_plus_overscan() {
        assert_eq!(initial_rows_for_height(300.0, 10.0, 2), 32);
        assert_eq!(initial_rows_for_height(301.0, 10.0, 2), 33);
        assert_eq!(initial_rows_for_height(0.0, 10.0, 2), 2);
        assert_eq!(initial_rows_for_height(-40.0, 10.0, 2), 2);
        assert_eq!(initial_rows_for_height(300.0, 0.0, 2), 2);
    }

    #[test]
    fn separated_row_stride_keeps_rendered_content_height_stable() {
        let row_height = 24.0;
        let row_gap = 2.0;
        let row_stride = row_height + row_gap;
        let first_range = virtual_range_for_viewport(120, row_stride, row_stride * 40.0, 180.0, 16);
        let second_range =
            virtual_range_for_viewport(120, row_stride, row_stride * 85.0, 180.0, 16);

        assert_eq!(
            rendered_height_with_spacer_gaps(first_range, row_height, row_gap),
            rendered_height_with_spacer_gaps(second_range, row_height, row_gap)
        );
    }

    #[test]
    fn reveal_delta_handles_above_visible_and_below_items() {
        assert_eq!(
            vertical_scroll_delta_to_reveal(30.0, 40.0, 10.0, 10.0),
            -20.0
        );
        assert_eq!(vertical_scroll_delta_to_reveal(30.0, 40.0, 45.0, 10.0), 0.0);
        assert_eq!(
            vertical_scroll_delta_to_reveal(30.0, 40.0, 75.0, 10.0),
            15.0
        );
    }

    #[test]
    fn initial_range_limits_first_render() {
        let range = initial_virtual_range(100, 10.0, 12);

        assert_eq!(range.start, 0);
        assert_eq!(range.end, 12);
        assert_eq!(range.after_height, 880.0);
    }

    #[test]
    fn empty_rows_return_empty_range() {
        assert_eq!(
            virtual_range_for_viewport(0, 10.0, 0.0, 30.0, 2),
            VirtualRange::empty()
        );
        assert_eq!(initial_virtual_range(0, 10.0, 12), VirtualRange::empty());
    }

    #[test]
    fn invalid_row_height_returns_empty_range() {
        assert_eq!(
            virtual_range_for_viewport(100, 0.0, 0.0, 30.0, 2),
            VirtualRange::empty()
        );
        assert_eq!(initial_virtual_range(100, 0.0, 12), VirtualRange::empty());
    }

    fn rendered_height_with_spacer_gaps(range: VirtualRange, row_height: f32, row_gap: f32) -> f32 {
        let rendered_rows = range.end.saturating_sub(range.start) as f32;
        range.before_height
            + range.after_height
            + rendered_rows * row_height
            + (rendered_rows + 1.0) * row_gap
    }
}
