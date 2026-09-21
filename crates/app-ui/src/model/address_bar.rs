use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{BrowserPaneId, BrowserViewMode};

// 地址栏纯算法与共享状态（面包屑分段、宽度分配、编辑会话、过渡状态机）
// 已下沉 bennu-theme 共享；这里 re-export 保持 crate 内引用路径不变。
// 共享层不带窗格概念（portal 永远单窗格），主程序的窗格归属走下方旁路。
pub(crate) use bennu_theme::address_bar::model::{
    breadcrumb_segments, AddressBarTransition, AddressEditingSession, AddressEditingSessionId,
    AddressSuggestionRequest, BreadcrumbSegment, BreadcrumbSegmentKind,
};

pub(crate) fn displayed_address_directory<'a>(
    current_dir: &'a Path,
    view_mode: BrowserViewMode,
    deepest_open_column_directory: Option<&'a PathBuf>,
) -> &'a Path {
    match view_mode {
        BrowserViewMode::Columns => deepest_open_column_directory
            .map(PathBuf::as_path)
            .unwrap_or(current_dir),
        BrowserViewMode::List | BrowserViewMode::Icons => current_dir,
    }
}

/// 主程序专属旁路：共享编辑会话不含窗格归属，归属窗格在这里与会话
/// 合并为单一字段——两者必须同生共死，拆成平行字段会留下失同步的口子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneAddressEditingSession {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) session: AddressEditingSession,
}

/// 地址栏过渡的窗格归属旁路，理由同上。
pub(crate) struct PaneAddressBarTransition {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) transition: AddressBarTransition,
}

impl PaneAddressBarTransition {
    /// 共享 retarget 只看连续性；跨窗格的旧过渡与本窗格无关，必须先按
    /// 窗格过滤再传入（与下沉前共享实现内部的过滤语义一致）。
    pub(crate) fn retarget(
        previous: Option<&Self>,
        pane_id: BrowserPaneId,
        target_fraction: f32,
        exit_snapshot: Option<String>,
        now: Instant,
    ) -> Self {
        Self {
            pane_id,
            transition: AddressBarTransition::retarget(
                previous
                    .filter(|owned| owned.pane_id == pane_id)
                    .map(|owned| &owned.transition),
                target_fraction,
                exit_snapshot,
                now,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn columns_display_the_deepest_open_directory() {
        let current_dir = Path::new("/workspace");
        let deepest_open_directory = PathBuf::from("/workspace/project/src");

        assert_eq!(
            displayed_address_directory(
                current_dir,
                BrowserViewMode::Columns,
                Some(&deepest_open_directory),
            ),
            deepest_open_directory
        );
    }

    #[test]
    fn list_view_ignores_column_open_directory() {
        let current_dir = Path::new("/workspace");
        let deepest_open_directory = PathBuf::from("/workspace/project/src");

        assert_eq!(
            displayed_address_directory(
                current_dir,
                BrowserViewMode::List,
                Some(&deepest_open_directory),
            ),
            current_dir
        );
    }

    #[test]
    fn icon_view_ignores_hidden_column_open_directory() {
        let current_dir = Path::new("/workspace");
        let deepest_open_directory = PathBuf::from("/workspace/project/src");

        assert_eq!(
            displayed_address_directory(
                current_dir,
                BrowserViewMode::Icons,
                Some(&deepest_open_directory),
            ),
            current_dir
        );
    }
}
