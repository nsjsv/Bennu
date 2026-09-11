use std::collections::HashMap;

use iced::{Point, Task};

use crate::app::FileBrowser;
use crate::commands::save_column_width_overrides_command;
use crate::config::{self, ColumnWidthAdjustMode};
use crate::model::Message;
use crate::three_column_view::{column_directories, COLUMN_RESIZE_DIVIDER_WIDTH};

#[derive(Debug, Clone, Copy)]
pub(super) struct ColumnResizeDrag {
    pub(super) column_index: usize,
    pub(super) cursor_start_x: f32,
    pub(super) width_start: f32,
}

impl FileBrowser {
    pub(super) fn start_column_resize_drag(&mut self, column_index: usize) -> Task<Message> {
        if self.renaming.is_some() {
            return self.commit_rename_if_active();
        }

        self.rebaseline_column_width_overrides(self.column_browser_content_width());
        self.column_resize_drag = Some(ColumnResizeDrag {
            column_index,
            cursor_start_x: self.cursor_position.x,
            width_start: self.column_width(column_index),
        });
        self.drag_selection_anchor = None;
        self.selection_marquee = None;
        self.clear_preview();
        self.context_menu = None;
        Task::none()
    }

    pub(super) fn update_column_resize_drag(&mut self, position: Point) {
        let Some(drag) = self.column_resize_drag else {
            return;
        };

        let resized_width =
            config::normalize_column_width(drag.width_start + position.x - drag.cursor_start_x);
        if self.user_config.column_width_adjust_mode == ColumnWidthAdjustMode::Uniform {
            for column_index in 0..self.adjustable_column_count() {
                self.column_width_overrides.insert(column_index, resized_width);
            }
            return;
        }
        self.column_width_overrides
            .insert(drag.column_index, resized_width);
    }

    pub(super) fn finish_column_resize_drag(&mut self) -> bool {
        self.column_resize_drag.take().is_some()
    }

    pub(crate) fn column_width(&self, column_index: usize) -> f32 {
        if let Some(width) = self.column_width_overrides.get(&column_index).copied() {
            return scale_column_width_to_content_width(
                width,
                self.column_width_reference_content_width,
                self.column_browser_content_width(),
            );
        }

        self.default_column_width()
    }

    fn default_column_width(&self) -> f32 {
        let width =
            self.column_browser_content_width() / self.user_config.visible_column_count as f32;
        if width.is_finite() {
            width.max(config::MIN_COLUMN_WIDTH)
        } else {
            config::MIN_COLUMN_WIDTH
        }
    }

    fn column_browser_content_width(&self) -> f32 {
        column_browser_content_width_for_window(
            self.main_window_width,
            self.sidebar_width,
            self.user_config.visible_column_count,
        )
    }

    /// 把全部 override 换算到新的参考内容宽度,显示宽度保持不变。
    /// 拖拽、持久化、模式切换之前先调它,保证"override 永远是参考宽度下的宽度"。
    fn rebaseline_column_width_overrides(&mut self, target_content_width: f32) {
        let current_reference = self.column_width_reference_content_width;
        for width in self.column_width_overrides.values_mut() {
            *width = scale_column_width_to_content_width(
                *width,
                current_reference,
                target_content_width,
            );
        }
        self.column_width_reference_content_width = target_content_width;
    }

    /// 把参考宽度重置为当前窗口内容宽度,不换算 override。
    /// 仅用于没有 override 需要保真的场景(初始化、无参考信息的旧数据兜底)。
    pub(super) fn refresh_column_width_reference_content_width(&mut self) {
        self.column_width_reference_content_width = self.column_browser_content_width();
    }

    pub(super) fn apply_column_width_overrides(
        &mut self,
        widths: HashMap<usize, f32>,
        reference_content_width: Option<f32>,
    ) {
        self.column_width_overrides = widths
            .into_iter()
            .map(|(column_index, width)| (column_index, config::normalize_column_width(width)))
            .collect();
        match reference_content_width {
            Some(reference) if reference.is_finite() && reference > 0.0 => {
                self.column_width_reference_content_width = reference;
            }
            _ => self.refresh_column_width_reference_content_width(),
        }
    }

    pub(super) fn finish_column_resize_drag_command(&mut self) -> Task<Message> {
        if self.finish_column_resize_drag() {
            self.persist_column_width_overrides_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn persist_column_width_overrides_command(&self) -> Task<Message> {
        let Some(task_queue_store) = self.operation_queue.task_queue_store().cloned() else {
            return Task::none();
        };
        let reference_content_width = (!self.column_width_overrides.is_empty())
            .then_some(self.column_width_reference_content_width);
        save_column_width_overrides_command(
            task_queue_store,
            self.column_width_overrides.clone(),
            reference_content_width,
        )
    }

    pub(crate) fn select_column_width_adjust_mode(
        &mut self,
        mode: ColumnWidthAdjustMode,
    ) -> Task<Message> {
        if self.user_config.column_width_adjust_mode == mode {
            return Task::none();
        }
        self.user_config.column_width_adjust_mode = mode;
        if mode == ColumnWidthAdjustMode::Uniform {
            self.uniformize_column_widths();
        }
        Task::batch([
            self.persist_user_preferences_command(),
            self.persist_column_width_overrides_command(),
        ])
    }

    /// 切到等宽模式时把所有可见栏统一为当前显示宽的平均值,总占用宽度保持不变。
    fn uniformize_column_widths(&mut self) {
        let column_count = self.adjustable_column_count();
        if column_count == 0 {
            return;
        }
        self.rebaseline_column_width_overrides(self.column_browser_content_width());
        let total: f32 = (0..column_count).map(|index| self.column_width(index)).sum();
        let uniform_width = config::normalize_column_width(total / column_count as f32);
        self.column_width_overrides = (0..column_count)
            .map(|column_index| (column_index, uniform_width))
            .collect();
    }

    /// 渲染层会画 max(已打开目录链长, 可见栏数) 根栏,联动宽度必须覆盖同样的范围。
    fn adjustable_column_count(&self) -> usize {
        column_directories(self)
            .len()
            .max(self.user_config.visible_column_count)
    }

    pub(super) fn toggle_show_hidden_files(&mut self) -> Task<Message> {
        self.options.include_hidden = !self.options.include_hidden;
        self.user_config.show_hidden_files = self.options.include_hidden;
        let persist_command = self.persist_user_preferences_command();
        let reload_command = self.reload_current();
        Task::batch([persist_command, reload_command])
    }
}

fn column_browser_content_width_for_window(
    main_window_width: f32,
    sidebar_width: f32,
    visible_column_count: usize,
) -> f32 {
    let divider_width = COLUMN_RESIZE_DIVIDER_WIDTH * (visible_column_count - 1) as f32;
    (main_window_width - sidebar_width - divider_width).max(1.0)
}

fn scale_column_width_to_content_width(
    width: f32,
    reference_content_width: f32,
    current_content_width: f32,
) -> f32 {
    let scaled_width = width * current_content_width.max(1.0) / reference_content_width.max(1.0);
    config::normalize_column_width(scaled_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::ui_thread_startup_config;

    const FLOAT_TOLERANCE: f32 = 0.01;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= FLOAT_TOLERANCE,
            "expected {actual} to be within {FLOAT_TOLERANCE} of {expected}"
        );
    }

    #[test]
    fn override_width_grows_with_content_width() {
        let width = scale_column_width_to_content_width(240.0, 720.0, 960.0);

        assert_close(width, 320.0);
    }

    #[test]
    fn override_width_shrinks_with_content_width() {
        let width = scale_column_width_to_content_width(360.0, 720.0, 540.0);

        assert_close(width, 270.0);
    }

    #[test]
    fn override_width_still_respects_minimum_width() {
        let width = scale_column_width_to_content_width(120.0, 720.0, 120.0);

        assert_close(width, config::MIN_COLUMN_WIDTH);
    }

    #[test]
    fn column_browser_content_width_excludes_sidebar_slot() {
        let visible_column_count = 3;
        let divider_width = COLUMN_RESIZE_DIVIDER_WIDTH * (visible_column_count - 1) as f32;
        let width = column_browser_content_width_for_window(900.0, 180.0, visible_column_count);

        assert_close(width, 900.0 - 180.0 - divider_width);
    }

    #[test]
    fn rebaseline_keeps_displayed_width_stable() {
        // 720 参考宽度下 override 240,显示为 240*960/720 = 320;
        // rebaseline 到 960 参考宽度后 override 变 320,显示必须仍是 320。
        let displayed_before = scale_column_width_to_content_width(240.0, 720.0, 960.0);
        let rebaselined_override = displayed_before;
        let displayed_after =
            scale_column_width_to_content_width(rebaselined_override, 960.0, 960.0);

        assert_close(displayed_before, 320.0);
        assert_close(displayed_after, 320.0);
    }

    #[test]
    fn restored_reference_scales_persisted_widths_to_smaller_window() {
        // 回归:全屏时保存的栏宽,重启后窗口变小必须等比例缩放,而不是原样展示。
        let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
        browser.main_window_width = 1_920.0;
        let saved_reference = browser.column_browser_content_width();

        browser.apply_column_width_overrides(
            HashMap::from([(0usize, 600.0)]),
            Some(saved_reference),
        );
        assert_close(browser.column_width(0), 600.0);

        browser.main_window_width = 1_000.0;
        let expected = scale_column_width_to_content_width(
            600.0,
            saved_reference,
            browser.column_browser_content_width(),
        );
        assert!(expected < 600.0);
        assert_close(browser.column_width(0), expected);
    }

    #[test]
    fn missing_reference_falls_back_to_current_window_width() {
        // 旧数据没有参考宽度:保持历史行为,按当前窗口宽度兜底。
        let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
        browser.main_window_width = 1_000.0;

        browser.apply_column_width_overrides(HashMap::from([(0usize, 600.0)]), None);

        assert_close(browser.column_width(0), 600.0);
    }

    #[test]
    fn uniform_drag_sets_every_visible_column_to_same_width() {
        let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
        browser.user_config.column_width_adjust_mode = ColumnWidthAdjustMode::Uniform;
        browser.main_window_width = 1_200.0;

        browser.start_column_resize_drag(0);
        browser.update_column_resize_drag(Point::new(340.0, 0.0));

        let visible_column_count = browser.user_config.visible_column_count;
        assert_eq!(browser.column_width_overrides.len(), visible_column_count);
        for column_index in 1..visible_column_count {
            assert_close(browser.column_width(column_index), browser.column_width(0));
        }
    }

    #[test]
    fn selecting_uniform_mode_averages_existing_widths() {
        let (mut browser, _) = FileBrowser::new(ui_thread_startup_config());
        browser.main_window_width = 1_200.0;
        browser.apply_column_width_overrides(HashMap::from([(0usize, 400.0)]), None);

        browser.select_column_width_adjust_mode(ColumnWidthAdjustMode::Uniform);

        let visible_column_count = browser.user_config.visible_column_count;
        assert_eq!(browser.column_width_overrides.len(), visible_column_count);
        for column_index in 1..visible_column_count {
            assert_close(browser.column_width(column_index), browser.column_width(0));
        }
    }
}
