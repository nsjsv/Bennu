//! 视图模式（列表/大图/多栏）状态与跨模式不变量。当前目录与确认集
//! 的唯一读取口（`target_directory` / `selected_paths`）、切换时的派生
//! 状态清理（展开/悬停/type-ahead）与主选中项揭示、以及随模式变化的
//! 几何（可见缩略图区间、翻页步长、网格光标位移、滚动揭示行矩形）
//! 全部集中在本模块：其余模块消费这里的方法，禁止各自按模式分支。

use std::path::Path;

use bennu_theme::icon_grid_geometry::{self, IconGridDirection};

use super::expansion::{LIST_ROW_HEIGHT, LIST_ROW_STRIDE};
use super::scrollbar::{ScrollbarViewport, SessionScrollRegion};
use super::thumbnails::VISIBLE_MARGIN_ROWS;
use super::{PickerSession, SessionEffect};

/// portal 大图固定默认档图标边长（主软件 DEFAULT_ICON_GRID_SIZE 同值）。
pub(crate) const ICON_GRID_EDGE: u32 = 96;
/// 列表行内缩略图请求档位：与主软件 normal 桶同 key 布局。
const LIST_THUMBNAIL_EDGE: u32 = 128;
/// 列表视口宽度未测量（首帧探针未回）时的列数估算基准：窗口默认宽。
const FALLBACK_VIEWPORT_WIDTH: f32 = 820.0;

/// portal 视图模式。多栏视图由 09-23-filechooser-columns-view 实现：
/// 状态先行（含持久化枚举值），按钮暂不渲染。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PickerViewMode {
    #[default]
    List,
    Icons,
    #[allow(dead_code)] // 多栏视图子任务 2 放开；先占住持久化枚举值。
    Columns,
}

impl PickerViewMode {
    /// portal.toml 的 view_mode 字段值 → 模式。未知值与 Columns 一律
    /// 回落 List：子任务 2 放开前不渲染半成品视图。
    pub(crate) fn from_storage_value(value: Option<&str>) -> Self {
        match value {
            Some("icons") => Self::Icons,
            _ => Self::List,
        }
    }

    pub(crate) fn storage_value(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Icons => "icons",
            Self::Columns => "columns",
        }
    }
}

impl PickerSession {
    pub(crate) fn view_mode(&self) -> PickerViewMode {
        self.view_mode
    }

    /// 确认/保存/记忆的目标目录唯一读取口：列表与大图 = 浏览目录；
    /// 多栏视图落地后改为焦点栏目录（子任务 2 只改这一处）。
    pub(crate) fn target_directory(&self) -> &Path {
        match self.view_mode {
            PickerViewMode::List | PickerViewMode::Icons | PickerViewMode::Columns => {
                &self.directory
            }
        }
    }

    /// 确认集唯一读取口：选中行路径（大图与列表共用行索引语义）。
    pub(crate) fn selected_paths(&self) -> Vec<std::path::PathBuf> {
        self.selection
            .iter()
            .filter_map(|&index| self.rows.get(index))
            .map(|row| row.entry.path.clone())
            .collect()
    }

    /// 视图切换：进入大图清空目录展开（行集合回到根条目，深层选中
    /// 丢弃、根级选中保留）；悬停与 type-ahead 缓冲作废；主选中项在
    /// 新几何的探针回信后揭示（`complete_view_switch_reveal`）。
    pub(crate) fn select_view_mode(&mut self, mode: PickerViewMode) -> SessionEffect {
        if mode == self.view_mode {
            return SessionEffect::None;
        }
        self.view_mode = mode;
        // 根级选中按路径保留（行索引随展开行收缩整体位移，按索引截
        // 断会错杀根级选中）；depth > 0 的深层选中丢弃。
        let kept_root_paths: Vec<std::path::PathBuf> = self
            .selection
            .iter()
            .filter_map(|&index| self.rows.get(index))
            .filter(|row| row.depth == 0)
            .map(|row| row.entry.path.clone())
            .collect();
        self.expansions.clear();
        self.refresh_rows();
        self.selection = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| kept_root_paths.contains(&row.entry.path))
            .map(|(index, _)| index)
            .collect();
        self.selection_anchor = self.selection.first().copied();
        self.hovered_index = None;
        self.keyboard_nav.reset();
        // 此刻视口缓存（offset/content_height）仍是旧视图几何，按它钳位
        // 会把揭示落点算错：只记下目标，等下面的探针回信再滚动。
        self.pending_view_switch_reveal = self
            .selection
            .first()
            .map(|&index| self.rows[index].entry.path.clone());
        // 行几何随模式整体更换：核实滚动条溢出并刷新视口缓存（探针
        // 回信按新几何重算缩略图可见区间并完成主选中项揭示）。
        SessionEffect::VerifyScrollbarLayout
    }

    /// List 探针回信后完成视图切换的主选中项揭示：只落光标并滚动跟随，
    /// 不重选（多选集必须完整保留——click_entry 的单选语义会把它压成一项）。
    /// 返回需要执行的绝对滚动偏移（已可见则 None）。
    pub(crate) fn complete_view_switch_reveal(&mut self) -> Option<f32> {
        let path = self.pending_view_switch_reveal.take()?;
        let index = self.rows.iter().position(|row| row.entry.path == path)?;
        match self.reveal_cursor_on(index) {
            SessionEffect::ScrollListTo { offset_y } => Some(offset_y),
            _ => None,
        }
    }

    /// 大图网格列数：视口宽（探针缓存）不足时按窗口默认宽估算。
    pub(crate) fn icon_grid_column_count(&self) -> usize {
        icon_grid_geometry::column_count_for_width(self.icon_grid_viewport_width(), ICON_GRID_EDGE)
    }

    /// 当前视图每行条目数：列表 1；大图 = 网格列数。
    fn entries_per_row(&self) -> usize {
        match self.view_mode {
            PickerViewMode::Icons => self.icon_grid_column_count(),
            PickerViewMode::List | PickerViewMode::Columns => 1,
        }
    }

    /// 当前视图的纵向步长（滚动偏移 ↔ 行号换算与翻页共用）。
    fn view_stride(&self) -> f32 {
        match self.view_mode {
            PickerViewMode::Icons => icon_grid_geometry::row_height(ICON_GRID_EDGE),
            PickerViewMode::List | PickerViewMode::Columns => LIST_ROW_STRIDE,
        }
    }

    /// 键盘光标条目的纵向矩形（滚动揭示用）：列表 = 行×步长；大图 =
    /// 所在网格行（含内容顶 padding）×网格行高 + tile 视觉高。
    pub(crate) fn cursor_row_span(&self, index: usize) -> (f32, f32) {
        match self.view_mode {
            PickerViewMode::Icons => {
                let columns = self.entries_per_row().max(1);
                let row = index / columns;
                let top = icon_grid_geometry::ICON_GRID_CONTENT_PADDING
                    + row as f32 * icon_grid_geometry::row_height(ICON_GRID_EDGE);
                (top, icon_grid_geometry::tile_visual_height(ICON_GRID_EDGE))
            }
            PickerViewMode::List | PickerViewMode::Columns => {
                (index as f32 * LIST_ROW_STRIDE, LIST_ROW_HEIGHT)
            }
        }
    }

    /// 翻页步长（条目数）：一屏行数 × 每行条目数。
    pub(crate) fn page_step_entries(&self, viewport_height: f32) -> isize {
        (viewport_height / self.view_stride()).max(1.0) as isize * self.entries_per_row() as isize
    }

    /// 方向键的光标位移（条目数）：大图 ↑↓ = ∓列数、←→ = ±1；
    /// 列表 ↑↓ = ∓1。列表的 ←→ 返回 None（折叠开关归键盘路由的
    /// 既有逻辑，大图无展开语义）。
    pub(crate) fn cursor_move_delta(&self, direction: IconGridDirection) -> Option<isize> {
        match self.view_mode {
            PickerViewMode::Icons => {
                let columns = self.entries_per_row().max(1) as isize;
                Some(match direction {
                    IconGridDirection::Up => -columns,
                    IconGridDirection::Down => columns,
                    IconGridDirection::Left => -1,
                    IconGridDirection::Right => 1,
                })
            }
            PickerViewMode::List | PickerViewMode::Columns => match direction {
                IconGridDirection::Up => Some(-1),
                IconGridDirection::Down => Some(1),
                IconGridDirection::Left | IconGridDirection::Right => None,
            },
        }
    }

    /// 可见条目索引区间与缩略图请求档位（缩略图调度用）：列表按行
    /// 步长换算；大图按网格行高×列数换算并请求大边长。
    pub(crate) fn visible_thumbnail_window(
        &self,
        viewport: &ScrollbarViewport,
    ) -> Option<(usize, usize, u32)> {
        if self.rows.is_empty() {
            return None;
        }
        let columns = self.entries_per_row().max(1);
        let stride = self.view_stride();
        // 大图内容有顶部 padding（列表行从 0 开始）：换算同一口径。
        let top_offset = match self.view_mode {
            PickerViewMode::Icons => icon_grid_geometry::ICON_GRID_CONTENT_PADDING,
            PickerViewMode::List | PickerViewMode::Columns => 0.0,
        };
        let row_count = self.rows.len().div_ceil(columns);
        let first_row = (((viewport.offset_y - top_offset) / stride).floor() as isize)
            .saturating_sub(VISIBLE_MARGIN_ROWS)
            .max(0) as usize;
        if first_row >= row_count {
            return None;
        }
        // 退化视口（高度 0）时 ceil 为 0，防 usize 回绕出巨大区间。
        let last_row = ((((viewport.offset_y + viewport.viewport_height - top_offset) / stride)
            .ceil() as usize)
            .saturating_sub(1)
            .saturating_add(VISIBLE_MARGIN_ROWS as usize))
        .min(row_count - 1);
        if last_row < first_row {
            return None;
        }
        let first = (first_row * columns).min(self.rows.len() - 1);
        let last = ((last_row + 1) * columns)
            .saturating_sub(1)
            .min(self.rows.len() - 1);
        let edge = match self.view_mode {
            PickerViewMode::Icons => icon_grid_geometry::thumbnail_edge(ICON_GRID_EDGE),
            PickerViewMode::List | PickerViewMode::Columns => LIST_THUMBNAIL_EDGE,
        };
        Some((first, last, edge))
    }

    fn icon_grid_viewport_width(&self) -> f32 {
        self.scrollbar_viewport_for(&SessionScrollRegion::List)
            .map(|viewport| viewport.viewport_width)
            .filter(|width| *width > f32::EPSILON)
            .unwrap_or(FALLBACK_VIEWPORT_WIDTH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus_file_chooser::PickerResolution;
    use crate::picker_request::{PickerKind, PickerRequestSpec};
    use crate::picker_session::scan::{DirectoryScanOutcome, DirectoryScanResult};
    use crate::picker_session::{PickerSession, SessionMessage};
    use file_core::entry::{DirectoryEntry, EntryMetadata, FileKind};

    fn session_with_entries(names: &[(&str, FileKind)]) -> PickerSession {
        let (reply, _receiver) = tokio::sync::oneshot::channel::<PickerResolution>();
        let base = tempfile::tempdir().unwrap();
        let mut session = PickerSession::new(
            &PickerRequestSpec {
                kind: PickerKind::OpenFile {
                    multiple: true,
                    directory: false,
                },
                accept_label: None,
                title: None,
                filters: Vec::new(),
                active_filter: None,
                start_folder: None,
                choices: Vec::new(),
            },
            "/req/view-mode".to_string(),
            base.keep(),
            PickerViewMode::List,
            reply,
        );
        let entries = names
            .iter()
            .map(|&(name, kind)| {
                DirectoryEntry::new(
                    session.directory.join(name),
                    kind,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                )
            })
            .collect();
        session.apply_scan(DirectoryScanResult {
            directory: session.directory.clone(),
            outcome: Ok(DirectoryScanOutcome { entries }),
        });
        session
    }

    fn storage_value_of(memory_toml: &str) -> Option<String> {
        crate::location::PortalMemory::parse_for_test(memory_toml)
            .stored_view_mode()
            .map(str::to_owned)
    }

    #[test]
    fn storage_values_round_trip_and_unknown_values_fall_back_to_list() {
        assert_eq!(
            PickerViewMode::from_storage_value(None),
            PickerViewMode::List
        );
        assert_eq!(
            PickerViewMode::from_storage_value(
                storage_value_of("view_mode = \"list\"\n").as_deref()
            ),
            PickerViewMode::List
        );
        assert_eq!(
            PickerViewMode::from_storage_value(
                storage_value_of("view_mode = \"icons\"\n").as_deref()
            ),
            PickerViewMode::Icons
        );
        // Columns 已持久化但视图未放开：回落 List 避免半成品视图。
        assert_eq!(
            PickerViewMode::from_storage_value(
                storage_value_of("view_mode = \"columns\"\n").as_deref()
            ),
            PickerViewMode::List
        );
        assert_eq!(
            PickerViewMode::from_storage_value(
                storage_value_of("view_mode = \"gibberish\"\n").as_deref()
            ),
            PickerViewMode::List
        );
        assert_eq!(PickerViewMode::List.storage_value(), "list");
        assert_eq!(PickerViewMode::Icons.storage_value(), "icons");
    }

    #[test]
    fn switching_to_icons_clears_expansions_but_keeps_root_selection() {
        let mut session = session_with_entries(&[
            ("branch", FileKind::Directory),
            ("deep.txt", FileKind::File),
            ("kept.txt", FileKind::File),
        ]);
        // 展开第一行后同时选中深层子条目与根级条目。
        session.update(SessionMessage::EntryExpandToggled { index: 0 });
        session.apply_scan(DirectoryScanResult {
            directory: session.directory.join("branch"),
            outcome: Ok(DirectoryScanOutcome {
                entries: vec![DirectoryEntry::new(
                    session.directory.join("branch/deep.txt"),
                    FileKind::File,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                )],
            }),
        });
        assert!(session.rows().len() > 3);
        // 行序：branch(0)、branch/deep.txt(1)、deep.txt(2)、kept.txt(3)。
        session.update(SessionMessage::EntryClicked {
            index: 1,
            ctrl: false,
            shift: false,
        });
        session.update(SessionMessage::EntryClicked {
            index: 3,
            ctrl: true,
            shift: false,
        });
        assert_eq!(session.selected_paths().len(), 2);
        let deep_path = session.directory.join("branch/deep.txt");

        session.update(SessionMessage::ViewModeSelected {
            mode: PickerViewMode::Icons,
        });

        // 行集合回到根条目；深层选中丢弃、根级选中保留。
        assert_eq!(session.rows().len(), 3);
        let paths = session.selected_paths();
        assert!(!paths.contains(&deep_path));
        assert_eq!(paths, vec![session.directory.join("kept.txt")]);
    }

    #[test]
    fn switching_view_keeps_multi_selection_and_places_cursor_on_first_selection() {
        let mut session = session_with_entries(&[
            ("a.txt", FileKind::File),
            ("b.txt", FileKind::File),
            ("c.txt", FileKind::File),
        ]);
        session.update(SessionMessage::EntryClicked {
            index: 0,
            ctrl: false,
            shift: false,
        });
        session.update(SessionMessage::EntryClicked {
            index: 2,
            ctrl: true,
            shift: false,
        });

        session.update(SessionMessage::ViewModeSelected {
            mode: PickerViewMode::Icons,
        });

        // 多选完整保留（光标揭示不得走单选重选路径）。
        assert_eq!(session.selected_paths().len(), 2);
        // 揭示等探针回信（新几何）才落光标。
        assert_eq!(session.list_cursor(), None);
        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: ScrollbarViewport {
                offset_x: 0.0,
                offset_y: 0.0,
                viewport_width: 600.0,
                viewport_height: 400.0,
                content_width: 600.0,
                content_height: 400.0,
            },
        });
        assert_eq!(session.list_cursor(), Some(0));
        // 切回列表同样保留。
        session.update(SessionMessage::ViewModeSelected {
            mode: PickerViewMode::List,
        });
        assert_eq!(session.selected_paths().len(), 2);
        assert_eq!(session.view_mode(), PickerViewMode::List);
    }

    #[test]
    fn switch_reveal_waits_for_new_geometry_then_scrolls_primary_into_view() {
        let names: Vec<String> = (0..60).map(|index| format!("f{index:03}.txt")).collect();
        let files: Vec<(&str, FileKind)> = names
            .iter()
            .map(|name| (name.as_str(), FileKind::File))
            .collect();
        let mut session = session_with_entries(&files);
        session.update(SessionMessage::EntryClicked {
            index: 59,
            ctrl: false,
            shift: false,
        });
        session.update(SessionMessage::ViewModeSelected {
            mode: PickerViewMode::Icons,
        });
        // 大图新几何（528 宽 = 3 列，20 行）的探针回信：顶部视口看不到末项。
        let row_height = icon_grid_geometry::row_height(ICON_GRID_EDGE);
        let content_height =
            icon_grid_geometry::ICON_GRID_CONTENT_PADDING * 2.0 + 20.0 * row_height;
        let tasks = session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: ScrollbarViewport {
                offset_x: 0.0,
                offset_y: 0.0,
                viewport_width: 528.0,
                viewport_height: 400.0,
                content_width: 528.0,
                content_height,
            },
        });
        assert!(!tasks.is_empty());
        assert_eq!(session.list_cursor(), Some(59));
        // 视口缓存同步到揭示落点：末项所在网格行在视口内。
        let viewport = session
            .scrollbar_viewport_for(&SessionScrollRegion::List)
            .unwrap();
        let (top, height) = session.cursor_row_span(59);
        assert!(top >= viewport.offset_y);
        assert!(top + height <= viewport.offset_y + viewport.viewport_height + 0.5);
        // 只揭示一次：后续探针回信不再滚动。
        assert!(session.complete_view_switch_reveal().is_none());
    }

    #[test]
    fn switching_to_same_mode_is_a_no_op() {
        let mut session = session_with_entries(&[("a.txt", FileKind::File)]);
        session.update(SessionMessage::EntryClicked {
            index: 0,
            ctrl: false,
            shift: false,
        });
        let effect = session.update(SessionMessage::ViewModeSelected {
            mode: PickerViewMode::List,
        });
        assert!(matches!(effect, crate::picker_session::SessionEffect::None));
    }

    #[test]
    fn grid_cursor_delta_follows_column_count() {
        let mut session = session_with_entries(&[("a.txt", FileKind::File)]);
        session.update(SessionMessage::ViewModeSelected {
            mode: PickerViewMode::Icons,
        });
        // 窄视口（无缓存 → 820 估算走不到；直接按窄宽注入视口缓存）。
        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: crate::picker_session::scrollbar::ScrollbarViewport {
                offset_x: 0.0,
                offset_y: 0.0,
                viewport_width: 200.0,
                viewport_height: 400.0,
                content_width: 200.0,
                content_height: 2000.0,
            },
        });
        let columns = session.icon_grid_column_count() as isize;
        assert!(columns >= 1);
        assert_eq!(
            session.cursor_move_delta(IconGridDirection::Up),
            Some(-columns)
        );
        assert_eq!(
            session.cursor_move_delta(IconGridDirection::Down),
            Some(columns)
        );
        assert_eq!(session.cursor_move_delta(IconGridDirection::Left), Some(-1));
        assert_eq!(session.cursor_move_delta(IconGridDirection::Right), Some(1));
    }

    #[test]
    fn list_left_right_delegate_to_expansion_toggle() {
        let session = session_with_entries(&[("a.txt", FileKind::File)]);
        // 列表的 ←→ 不产生位移（键盘路由转目录行折叠开关）。
        assert_eq!(session.cursor_move_delta(IconGridDirection::Left), None);
        assert_eq!(session.cursor_move_delta(IconGridDirection::Right), None);
        assert_eq!(session.cursor_move_delta(IconGridDirection::Up), Some(-1));
        assert_eq!(session.cursor_move_delta(IconGridDirection::Down), Some(1));
    }

    #[test]
    fn visible_thumbnail_window_follows_grid_geometry_and_requests_large_edge() {
        let names: Vec<String> = (0..60).map(|index| format!("img{index:03}.png")).collect();
        let files: Vec<(&str, FileKind)> = names
            .iter()
            .map(|name| (name.as_str(), FileKind::File))
            .collect();
        let mut session = session_with_entries(&files);
        session.update(SessionMessage::ViewModeSelected {
            mode: PickerViewMode::Icons,
        });
        // 视口宽 528：内容可用 504 / slot(128+12) = 3 列。
        session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
            region: SessionScrollRegion::List,
            viewport: crate::picker_session::scrollbar::ScrollbarViewport {
                offset_x: 0.0,
                offset_y: 0.0,
                viewport_width: 528.0,
                viewport_height: 800.0,
                content_width: 528.0,
                content_height: 4000.0,
            },
        });
        assert_eq!(session.icon_grid_column_count(), 3);

        let (first, last, edge) = session
            .visible_thumbnail_window(&crate::picker_session::scrollbar::ScrollbarViewport {
                offset_x: 0.0,
                offset_y: 0.0,
                viewport_width: 528.0,
                viewport_height: 400.0,
                content_width: 528.0,
                content_height: 4000.0,
            })
            .unwrap();
        // 首屏 400px ≈ 2 行 × 3 列，±8 行余量 → 前 11 行 = 33 条。
        assert_eq!(first, 0);
        assert_eq!(last, 32);
        // 大图请求大边长（thumbnail_edge(96)），与列表 128 档区分。
        assert_eq!(edge, 192);
    }

    #[test]
    fn selected_paths_and_target_directory_are_the_single_read_path() {
        let mut session = session_with_entries(&[("a.txt", FileKind::File)]);
        session.update(SessionMessage::EntryClicked {
            index: 0,
            ctrl: false,
            shift: false,
        });
        assert_eq!(session.target_directory(), session.directory());
        assert_eq!(
            session.selected_paths(),
            vec![session.directory.join("a.txt")]
        );
    }
}
