#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use iced::{mouse, Element, Task};

use super::FileBrowser;
use crate::model::{Message, ScrollbarRegion};

// Mos 惯性状态机、滚轮换算与捕获滚轮事件的包装 widget 均已下沉到共享
// crate `bennu-theme::smooth_scroll`（portal-backend 复用同一实现）；
// 这里重导出公共项，调用点保持原名零改动。
pub(crate) use bennu_theme::smooth_scroll::{
    wheel_delta_for_axis, SmoothScrollArea, SmoothScrollAxis, SmoothScrollDelta, WheelScrollDelta,
    WheelScrollMode,
};

// FileBrowser 全局只挂主软件的区域枚举：类型别名把共享状态机钉到具体
// Region，`MosScrollState` 在调用点保持原名与原推导路径。
pub(crate) type MosScrollState = bennu_theme::smooth_scroll::MosScrollState<ScrollbarRegion>;

impl FileBrowser {
    pub(crate) fn smooth_scroll_shift_pressed(&self) -> bool {
        self.keyboard_modifiers.shift()
    }

    pub(super) fn handle_smooth_scroll_wheel(
        &mut self,
        region: ScrollbarRegion,
        delta: mouse::ScrollDelta,
    ) -> Task<Message> {
        // Ctrl+滚轮在浏览内容区统一走密度档位路径；其他区域保持原滚动语义。
        if self.keyboard_modifiers.control() {
            if let Some(target) = view_density_target(&region) {
                return self.adjust_view_density(target, delta);
            }
        }

        let Some(scroll_delta) = self.smooth_scroll_delta_for_region(&region, delta) else {
            return Task::none();
        };

        match scroll_delta.mode {
            WheelScrollMode::MosAnimated => {
                self.smooth_scroll
                    .push_wheel_delta(region.clone(), scroll_delta.delta);

                self.show_scrollbars_temporarily(region)
            }
            WheelScrollMode::Direct => {
                self.smooth_scroll.stop();
                let scroll_task = iced::widget::operation::scroll_by(
                    smooth_scroll_id(&region),
                    scroll_frame_offset(scroll_delta.delta),
                );

                Task::batch([self.show_scrollbars_temporarily(region), scroll_task])
            }
        }
    }

    // Ctrl+滚轮调档的唯一状态入口：每个非零事件走一档，停止旧惯性，
    // 立即走用户偏好保存队列；边界档位静默保持，不产生滚动位移。
    fn adjust_view_density(
        &mut self,
        target: ViewDensityTarget,
        delta: mouse::ScrollDelta,
    ) -> Task<Message> {
        self.smooth_scroll.stop();
        let Some(step) = view_density_step_from_wheel(delta) else {
            return Task::none();
        };
        let current = match target {
            ViewDensityTarget::Columns => self.user_config.columns_view_density,
            ViewDensityTarget::List => self.user_config.list_view_density,
            ViewDensityTarget::Icons => self.user_config.icons_view_density,
        };
        let next = current.step(step);
        if next == current {
            return Task::none();
        }
        match target {
            ViewDensityTarget::Columns => self.user_config.columns_view_density = next,
            ViewDensityTarget::List => self.user_config.list_view_density = next,
            // icon_grid_size 是旧配置的兼容镜像，必须与档位同步写回。
            ViewDensityTarget::Icons => self.user_config.set_icons_view_density(next),
        }
        // 换档改变可见范围，三种视图统一重新调度可见缩略图。
        Task::batch([
            self.persist_user_preferences_command(),
            self.schedule_thumbnail_refresh(),
        ])
    }

    pub(super) fn smooth_scroll_animation_is_active(&self) -> bool {
        self.smooth_scroll.is_active()
    }

    pub(super) fn advance_smooth_scroll_animation(&mut self) -> Task<Message> {
        let scroll_task = self
            .smooth_scroll
            .next_frame_delta()
            .map(|(region, delta)| {
                iced::widget::operation::scroll_by(
                    smooth_scroll_id(&region),
                    scroll_frame_offset(delta),
                )
            });

        if !self.smooth_scroll.is_active() {
            self.smooth_scroll.stop();
        }

        scroll_task.unwrap_or_else(Task::none)
    }

    fn smooth_scroll_delta_for_region(
        &self,
        region: &ScrollbarRegion,
        delta: mouse::ScrollDelta,
    ) -> Option<WheelScrollDelta> {
        wheel_delta_for_region(region, self.keyboard_modifiers.shift(), delta)
    }
}

pub(crate) fn smooth_scroll_content<'a>(
    content: impl Into<Element<'a, Message>>,
    region: ScrollbarRegion,
) -> Element<'a, Message> {
    smooth_scroll_content_with_shift(content, region, false)
}

pub(crate) fn smooth_scroll_content_with_shift<'a>(
    content: impl Into<Element<'a, Message>>,
    region: ScrollbarRegion,
    shift_pressed: bool,
) -> Element<'a, Message> {
    Element::new(SmoothScrollArea::new(
        content,
        smooth_scroll_axis(&region),
        shift_pressed,
        move |delta| Message::SmoothScrollWheel(region.clone(), delta),
    ))
}

pub(crate) fn smooth_scroll_id(region: &ScrollbarRegion) -> iced::widget::Id {
    match region {
        ScrollbarRegion::Sidebar => iced::widget::Id::new("sidebar"),
        ScrollbarRegion::AddressBar(pane_id) => {
            iced::widget::Id::from(format!("address-bar-{}", pane_id.key()))
        }
        ScrollbarRegion::PaneList(pane_id) => {
            iced::widget::Id::from(format!("pane-list-{}", pane_id.key()))
        }
        ScrollbarRegion::PaneIcons(pane_id) => {
            iced::widget::Id::from(format!("pane-icons-{}", pane_id.key()))
        }
        ScrollbarRegion::ColumnBrowser(pane_id) => {
            iced::widget::Id::from(format!("column-browser-{}", pane_id.key()))
        }
        ScrollbarRegion::Column { pane_id, directory } => iced::widget::Id::from(format!(
            "column-scroll-{}-{}",
            pane_id.key(),
            path_hash(directory)
        )),
        ScrollbarRegion::Settings => iced::widget::Id::new("settings"),
        ScrollbarRegion::Properties => iced::widget::Id::new("properties"),
        ScrollbarRegion::OpenWithApplications => iced::widget::Id::new("open-with-applications"),
        ScrollbarRegion::OperationQueue => iced::widget::Id::new("operation-queue"),
        ScrollbarRegion::BatchRenamePreview => iced::widget::Id::new("batch-rename-preview"),
        ScrollbarRegion::SearchHistory => iced::widget::Id::new("search-history"),
        ScrollbarRegion::SearchResults => iced::widget::Id::new("search-results"),
        ScrollbarRegion::PreviewDirectory => iced::widget::Id::new("preview-directory"),
        ScrollbarRegion::PreviewArchive => iced::widget::Id::new("preview-archive"),
        ScrollbarRegion::PreviewDocument => iced::widget::Id::new("preview-document"),
        ScrollbarRegion::TextPreview => iced::widget::Id::new("text-preview"),
        ScrollbarRegion::MarkdownPreview => iced::widget::Id::new("markdown-preview"),
        ScrollbarRegion::PreviewSqliteTables => iced::widget::Id::new("preview-sqlite-tables"),
        ScrollbarRegion::PreviewSqliteData => iced::widget::Id::new("preview-sqlite-data"),
    }
}

fn smooth_scroll_axis(region: &ScrollbarRegion) -> SmoothScrollAxis {
    match region {
        ScrollbarRegion::AddressBar(_) => SmoothScrollAxis::HorizontalFromPrimaryWheel,
        ScrollbarRegion::ColumnBrowser(_) => SmoothScrollAxis::Horizontal,
        // 多栏单列是竖向滚动区：shift 横滚的目标是外层整排列排
        // (ColumnBrowser)，本区在 shift 下不认领滚轮，让事件冒泡。
        ScrollbarRegion::Column { .. } => SmoothScrollAxis::VerticalIgnoringShift,
        _ => SmoothScrollAxis::Vertical,
    }
}

fn wheel_delta_for_region(
    region: &ScrollbarRegion,
    shift_pressed: bool,
    delta: mouse::ScrollDelta,
) -> Option<WheelScrollDelta> {
    // 区域 → 轴向的换算在 smooth_scroll_axis：Column 用
    // VerticalIgnoringShift 表达 "shift 横滚冒泡到外层" 的区域语义。
    wheel_delta_for_axis(smooth_scroll_axis(region), shift_pressed, delta)
}

// 浏览内容区到密度档位的映射：侧栏、地址栏、预览和设置不参与密度缩放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewDensityTarget {
    Columns,
    List,
    Icons,
}

fn view_density_target(region: &ScrollbarRegion) -> Option<ViewDensityTarget> {
    match region {
        ScrollbarRegion::PaneList(_) => Some(ViewDensityTarget::List),
        ScrollbarRegion::PaneIcons(_) => Some(ViewDensityTarget::Icons),
        ScrollbarRegion::Column { .. } | ScrollbarRegion::ColumnBrowser(_) => {
            Some(ViewDensityTarget::Columns)
        }
        _ => None,
    }
}

// 优先竖轴，竖轴为零时回退横轴；正方向放大，负方向缩小，零增量不移动档位。
fn view_density_step_from_wheel(
    delta: mouse::ScrollDelta,
) -> Option<crate::config::ViewDensityStep> {
    let primary_delta = match delta {
        mouse::ScrollDelta::Lines { x, y } | mouse::ScrollDelta::Pixels { x, y } => {
            if y.abs() > f32::EPSILON {
                y
            } else {
                x
            }
        }
    };
    if primary_delta > f32::EPSILON {
        Some(crate::config::ViewDensityStep::Increase)
    } else if primary_delta < -f32::EPSILON {
        Some(crate::config::ViewDensityStep::Decrease)
    } else {
        None
    }
}

fn scroll_frame_offset(delta: SmoothScrollDelta) -> iced::widget::scrollable::AbsoluteOffset {
    iced::widget::scrollable::AbsoluteOffset {
        x: delta.x,
        y: delta.y,
    }
}

pub(crate) fn path_hash(path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    #[cfg(unix)]
    {
        hasher.update(path.as_os_str().as_bytes());
    }
    #[cfg(not(unix))]
    {
        hasher.update(path.to_string_lossy().as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod icon_grid_tests;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::BrowserPaneId;
    // 换算常量随状态机下沉到共享层；app-ui 代码已不直接消费，仅
    // region 换算用例引用。
    use bennu_theme::smooth_scroll::MOS_SCROLL_STEP;

    #[test]
    fn horizontal_region_ignores_unshifted_vertical_wheel() {
        let delta = wheel_delta_for_region(
            &ScrollbarRegion::ColumnBrowser(BrowserPaneId::PRIMARY),
            false,
            mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
        );

        assert_eq!(delta, None);
    }

    #[test]
    fn address_bar_uses_primary_wheel_for_horizontal_scroll() {
        let delta = wheel_delta_for_region(
            &ScrollbarRegion::AddressBar(BrowserPaneId::PRIMARY),
            false,
            mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 },
        );

        assert_eq!(
            delta,
            Some(WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            })
        );
    }

    #[test]
    fn column_region_ignores_shifted_vertical_wheel() {
        // None 让 shift 事件冒泡:多栏单列的 shift 横滚由外层
        // ColumnBrowser 认领,发布给竖轴单列只会被静默丢弃。
        let delta = wheel_delta_for_region(
            &ScrollbarRegion::Column {
                pane_id: BrowserPaneId::PRIMARY,
                directory: PathBuf::from("/tmp"),
            },
            true,
            mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
        );

        assert_eq!(delta, None);
    }

    #[test]
    fn shifted_column_browser_uses_vertical_wheel_for_horizontal_delta() {
        let delta = wheel_delta_for_region(
            &ScrollbarRegion::ColumnBrowser(BrowserPaneId::PRIMARY),
            true,
            mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 },
        );

        assert_eq!(
            delta,
            Some(WheelScrollDelta {
                delta: SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                },
                mode: WheelScrollMode::MosAnimated,
            })
        );
    }

    #[test]
    fn frame_offset_preserves_relative_scroll_delta() {
        let offset = scroll_frame_offset(SmoothScrollDelta { x: -3.0, y: 6.0 });

        assert_eq!(offset.x, -3.0);
        assert_eq!(offset.y, 6.0);
    }
}
