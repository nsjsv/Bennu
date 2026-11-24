#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use bennu_theme::smooth_scroll::{SmoothScroll, SmoothScrollAxis};
use iced::{mouse, Task};

use super::FileBrowser;
use crate::config::ViewDensityLevel;
use crate::model::{BrowserViewMode, Message, ScrollbarRegion};

impl FileBrowser {
    /// 浏览内容区的 Ctrl+滚轮：每个非零事件走一档密度，边界档位静默保持；立即走用户偏好保存队列。
    pub(super) fn handle_view_density_wheel(
        &mut self,
        region: ScrollbarRegion,
        delta: mouse::ScrollDelta,
    ) -> Task<Message> {
        let Some(target) = view_density_target(&region) else {
            return Task::none();
        };
        let Some(step) = view_density_step_from_wheel(delta) else {
            return Task::none();
        };
        self.change_view_density(target, |current| current.step(step))
    }

    /// Ctrl+= / Ctrl+- / Ctrl+0：调活动标签页当前视图的档位，和 Ctrl+滚轮是同一组档位。
    pub(super) fn change_active_view_density(
        &mut self,
        next: impl FnOnce(ViewDensityLevel) -> ViewDensityLevel,
    ) -> Task<Message> {
        let target = match self.tab().view_mode() {
            BrowserViewMode::Columns => ViewDensityTarget::Columns,
            BrowserViewMode::List => ViewDensityTarget::List,
            BrowserViewMode::Icons => ViewDensityTarget::Icons,
        };
        self.change_view_density(target, next)
    }

    fn change_view_density(
        &mut self,
        target: ViewDensityTarget,
        next: impl FnOnce(ViewDensityLevel) -> ViewDensityLevel,
    ) -> Task<Message> {
        let density = match target {
            ViewDensityTarget::Columns => &mut self.user_config.columns_view_density,
            ViewDensityTarget::List => &mut self.user_config.list_view_density,
            ViewDensityTarget::Icons => &mut self.user_config.icons_view_density,
        };
        let changed = next(*density);
        if changed == *density {
            return Task::none();
        }
        *density = changed;
        // 换档改变可见范围，三种视图统一重新调度可见缩略图。
        Task::batch([
            self.persist_user_preferences_command(),
            self.clamp_visible_pane_viewports(),
            self.schedule_thumbnail_refresh(),
        ])
    }
}

/// 一个滚动区的惯性滚动（滚轮在控件里按帧推进，不经过应用）：先 `capture` 内容、放进
/// `scrollable`，再 `drive` 这个滚动区。有密度档的浏览区 Ctrl+滚轮交给应用调档。
pub(crate) fn smooth_scroll<'a>(region: &ScrollbarRegion) -> SmoothScroll<'a, Message> {
    let scroll = bennu_theme::smooth_scroll::smooth_scroll(smooth_scroll_axis(region));
    if view_density_target(region).is_none() {
        return scroll;
    }
    let region = region.clone();
    scroll.on_ctrl_wheel(move |delta| Message::ViewDensityWheel(region.clone(), delta))
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
        // 分页文档（放大后超宽）、SQLite 数据网格和文本查看器两个方向都能滚：用竖向轴会
        // 丢掉触控板、小红点的横向分量。文本查看器自己推进滚轮，这里只表明它是两向区。
        ScrollbarRegion::PreviewDocument
        | ScrollbarRegion::PreviewSqliteData
        | ScrollbarRegion::TextPreview => SmoothScrollAxis::Both,
        _ => SmoothScrollAxis::Vertical,
    }
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
    use bennu_theme::smooth_scroll::{
        wheel_delta_for_axis, SmoothScrollDelta, WheelScrollDelta, WheelScrollMode, MOS_SCROLL_STEP,
    };

    fn wheel_delta_for_region(
        region: &ScrollbarRegion,
        shift_pressed: bool,
        delta: mouse::ScrollDelta,
    ) -> Option<WheelScrollDelta> {
        wheel_delta_for_axis(smooth_scroll_axis(region), shift_pressed, delta)
    }

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

    /// 两向区（放大后超宽的分页文档、SQLite 数据网格）保留触控板、小红点的横向分量；斜着划
    /// 两个方向一起动，纯横划也由它们认领（否则落回 iced 自带的滚动，没有惯性）。
    #[test]
    fn two_way_preview_regions_keep_the_sideways_part_of_a_touchpad_swipe() {
        for region in [
            ScrollbarRegion::PreviewDocument,
            ScrollbarRegion::PreviewSqliteData,
        ] {
            let diagonal = wheel_delta_for_region(
                &region,
                false,
                mouse::ScrollDelta::Pixels { x: -6.0, y: -4.0 },
            );
            assert_eq!(
                diagonal,
                Some(WheelScrollDelta {
                    delta: SmoothScrollDelta { x: 6.0, y: 4.0 },
                    mode: WheelScrollMode::Direct,
                })
            );
            let tilt = wheel_delta_for_region(
                &region,
                false,
                mouse::ScrollDelta::Lines { x: 1.0, y: 0.0 },
            );
            assert_eq!(
                tilt,
                Some(WheelScrollDelta {
                    delta: SmoothScrollDelta {
                        x: -MOS_SCROLL_STEP,
                        y: 0.0,
                    },
                    mode: WheelScrollMode::MosAnimated,
                })
            );
            // shift+竖向滚轮仍转成横向。
            let shifted = wheel_delta_for_region(
                &region,
                true,
                mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 },
            );
            assert_eq!(
                shifted.map(|wheel| wheel.delta),
                Some(SmoothScrollDelta {
                    x: MOS_SCROLL_STEP * 2.0,
                    y: 0.0,
                })
            );
        }
    }

    /// 竖向区只取竖向分量、不认领纯横向，行为和以前一样。
    #[test]
    fn vertical_regions_still_drop_the_sideways_part() {
        for region in [
            ScrollbarRegion::PaneList(BrowserPaneId::PRIMARY),
            ScrollbarRegion::PreviewDirectory,
            ScrollbarRegion::PreviewArchive,
            ScrollbarRegion::MarkdownPreview,
            ScrollbarRegion::PreviewSqliteTables,
        ] {
            let diagonal = wheel_delta_for_region(
                &region,
                false,
                mouse::ScrollDelta::Pixels { x: -6.0, y: -4.0 },
            );
            assert_eq!(
                diagonal,
                Some(WheelScrollDelta {
                    delta: SmoothScrollDelta { x: 0.0, y: 4.0 },
                    mode: WheelScrollMode::Direct,
                })
            );
            let sideways = wheel_delta_for_region(
                &region,
                false,
                mouse::ScrollDelta::Pixels { x: -6.0, y: 0.0 },
            );
            assert_eq!(sideways, None);
        }
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
}
