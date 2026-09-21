use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WindowControlKind {
    Minimize,
    MaximizeRestore,
    Close,
}

impl WindowControlKind {
    pub(crate) const ALL: [Self; 3] = [Self::Minimize, Self::MaximizeRestore, Self::Close];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize",
            Self::MaximizeRestore => "Maximize / Restore",
            Self::Close => "Close",
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::MaximizeRestore => "maximize_restore",
            Self::Close => "close",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "minimize" => Some(Self::Minimize),
            "maximize_restore" => Some(Self::MaximizeRestore),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

/// One-slot reorder inside a side's row sequence, driven by the row's
/// up/down arrow buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowControlMoveDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowControlSide {
    Left,
    Right,
}

impl WindowControlSide {
    pub(crate) const ALL: [Self; 2] = [Self::Left, Self::Right];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowControlVisibility {
    Visible,
    Hidden,
}

impl WindowControlVisibility {
    pub(crate) fn is_visible(self) -> bool {
        self == Self::Visible
    }

    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Visible => Self::Hidden,
            Self::Hidden => Self::Visible,
        }
    }
}

impl From<bool> for WindowControlVisibility {
    fn from(visible: bool) -> Self {
        if visible {
            Self::Visible
        } else {
            Self::Hidden
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowFrameState {
    Restored,
    Maximized,
}

pub(crate) const WINDOW_TITLE_BAR_HEIGHT: f32 = 40.0;
pub(crate) const WINDOW_TOP_BAR_HEIGHT: f32 = 48.0;
/// 全窗工具栏顶栏高度(上下 padding 18 + 控制行)。顶栏自身用此值
/// 定高,侧栏/窗格/面板的 y 向几何偏移才能与渲染严格一致。
pub(crate) const MAIN_TOOLBAR_ROW_HEIGHT: f32 = 72.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowChromeLayout {
    IntegratedNavigation,
    SeparateTitleBar,
}

impl WindowChromeLayout {
    pub(crate) const ALL: [Self; 2] = [Self::IntegratedNavigation, Self::SeparateTitleBar];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::IntegratedNavigation => "Integrated navigation",
            Self::SeparateTitleBar => "Separate title bar",
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::IntegratedNavigation => "integrated_navigation",
            Self::SeparateTitleBar => "separate_title_bar",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "integrated_navigation" => Some(Self::IntegratedNavigation),
            "separate_title_bar" => Some(Self::SeparateTitleBar),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowControlPlacement {
    kind: WindowControlKind,
    side: WindowControlSide,
    visibility: WindowControlVisibility,
}

impl WindowControlPlacement {
    pub(crate) fn new(
        kind: WindowControlKind,
        side: WindowControlSide,
        visibility: WindowControlVisibility,
    ) -> Self {
        Self {
            kind,
            side,
            visibility: normalized_visibility(kind, visibility),
        }
    }

    pub(crate) fn kind(self) -> WindowControlKind {
        self.kind
    }

    pub(crate) fn side(self) -> WindowControlSide {
        self.side
    }

    pub(crate) fn visibility(self) -> WindowControlVisibility {
        self.visibility
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowControlsConfig {
    layout: WindowChromeLayout,
    placements: Vec<WindowControlPlacement>,
}

impl WindowControlsConfig {
    pub(crate) fn from_partial_placements(
        layout: WindowChromeLayout,
        placements: Vec<WindowControlPlacement>,
    ) -> Self {
        let mut seen = HashSet::with_capacity(WindowControlKind::ALL.len());
        let mut normalized = Vec::with_capacity(WindowControlKind::ALL.len());
        for placement in placements {
            if seen.insert(placement.kind) {
                normalized.push(WindowControlPlacement::new(
                    placement.kind,
                    placement.side,
                    placement.visibility,
                ));
            }
        }
        for kind in WindowControlKind::ALL {
            if seen.insert(kind) {
                normalized.push(default_placement(kind));
            }
        }
        Self {
            layout,
            placements: normalized,
        }
    }

    pub(crate) fn layout(&self) -> WindowChromeLayout {
        self.layout
    }

    pub(crate) fn placements(&self) -> &[WindowControlPlacement] {
        &self.placements
    }

    pub(crate) fn placements_on(
        &self,
        side: WindowControlSide,
    ) -> impl Iterator<Item = WindowControlPlacement> + '_ {
        self.placements
            .iter()
            .copied()
            .filter(move |placement| placement.side == side)
    }

    pub(crate) fn placement(&self, kind: WindowControlKind) -> WindowControlPlacement {
        self.placements
            .iter()
            .copied()
            .find(|placement| placement.kind == kind)
            .expect("normalized window controls contain every kind")
    }

    pub(crate) fn select_layout(&mut self, layout: WindowChromeLayout) -> bool {
        if self.layout == layout {
            return false;
        }
        self.layout = layout;
        true
    }

    pub(crate) fn set_visibility(
        &mut self,
        kind: WindowControlKind,
        visibility: WindowControlVisibility,
    ) -> bool {
        let normalized = normalized_visibility(kind, visibility);
        let placement = self
            .placements
            .iter_mut()
            .find(|placement| placement.kind == kind)
            .expect("normalized window controls contain every kind");
        if placement.visibility == normalized {
            return false;
        }
        placement.visibility = normalized;
        true
    }

    pub(crate) fn move_to_side(
        &mut self,
        kind: WindowControlKind,
        side: WindowControlSide,
    ) -> bool {
        let index = self
            .placements
            .iter()
            .position(|placement| placement.kind == kind)
            .expect("normalized window controls contain every kind");
        if self.placements[index].side == side {
            return false;
        }
        let mut placement = self.placements.remove(index);
        placement.side = side;
        self.placements.push(placement);
        true
    }

    // Both sides share one placements Vec, so a side-local neighbour is the
    // nearest same-side entry scanned in the move direction, not the
    // Vec-adjacent slot. Swapping the two entries leaves every other slot
    // untouched, so "swapped" is exactly "order changed" and drives
    // persistence.
    pub(crate) fn move_within_side(
        &mut self,
        kind: WindowControlKind,
        direction: WindowControlMoveDirection,
    ) -> bool {
        let index = self
            .placements
            .iter()
            .position(|placement| placement.kind == kind)
            .expect("normalized window controls contain every kind");
        let side = self.placements[index].side;
        let neighbour = match direction {
            WindowControlMoveDirection::Up => (0..index)
                .rev()
                .find(|candidate| self.placements[*candidate].side == side),
            WindowControlMoveDirection::Down => (index + 1..self.placements.len())
                .find(|candidate| self.placements[*candidate].side == side),
        };
        let Some(neighbour) = neighbour else {
            return false;
        };
        self.placements.swap(index, neighbour);
        true
    }

    pub(crate) fn reset(&mut self) -> bool {
        let default = Self::default();
        if *self == default {
            return false;
        }
        *self = default;
        true
    }
}

impl Default for WindowControlsConfig {
    fn default() -> Self {
        Self {
            layout: WindowChromeLayout::IntegratedNavigation,
            placements: WindowControlKind::ALL
                .into_iter()
                .map(default_placement)
                .collect(),
        }
    }
}

fn default_placement(kind: WindowControlKind) -> WindowControlPlacement {
    WindowControlPlacement::new(
        kind,
        WindowControlSide::Right,
        WindowControlVisibility::Visible,
    )
}

fn normalized_visibility(
    kind: WindowControlKind,
    visibility: WindowControlVisibility,
) -> WindowControlVisibility {
    if kind == WindowControlKind::Close {
        WindowControlVisibility::Visible
    } else {
        visibility
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_on(config: &WindowControlsConfig, side: WindowControlSide) -> Vec<WindowControlKind> {
        config
            .placements_on(side)
            .map(WindowControlPlacement::kind)
            .collect()
    }

    #[test]
    fn default_uses_integrated_right_standard_order() {
        let config = WindowControlsConfig::default();

        assert_eq!(config.layout(), WindowChromeLayout::IntegratedNavigation);
        assert!(kinds_on(&config, WindowControlSide::Left).is_empty());
        assert_eq!(
            kinds_on(&config, WindowControlSide::Right),
            WindowControlKind::ALL
        );
        assert!(config
            .placements()
            .iter()
            .all(|placement| placement.visibility().is_visible()));
    }

    #[test]
    fn partial_placements_drop_duplicates_fill_missing_and_keep_close_visible() {
        let config = WindowControlsConfig::from_partial_placements(
            WindowChromeLayout::SeparateTitleBar,
            vec![
                WindowControlPlacement::new(
                    WindowControlKind::Close,
                    WindowControlSide::Left,
                    WindowControlVisibility::Hidden,
                ),
                WindowControlPlacement::new(
                    WindowControlKind::Close,
                    WindowControlSide::Right,
                    WindowControlVisibility::Visible,
                ),
                WindowControlPlacement::new(
                    WindowControlKind::Minimize,
                    WindowControlSide::Left,
                    WindowControlVisibility::Hidden,
                ),
            ],
        );

        assert_eq!(config.placements().len(), 3);
        assert_eq!(
            config.placement(WindowControlKind::Close).side(),
            WindowControlSide::Left
        );
        assert!(config
            .placement(WindowControlKind::Close)
            .visibility()
            .is_visible());
        assert_eq!(
            config.placement(WindowControlKind::MaximizeRestore).side(),
            WindowControlSide::Right
        );
    }

    #[test]
    fn moving_to_side_appends_to_target_side() {
        let mut config = WindowControlsConfig::default();

        assert!(config.move_to_side(WindowControlKind::MaximizeRestore, WindowControlSide::Left));
        assert!(config.move_to_side(WindowControlKind::Minimize, WindowControlSide::Left));

        assert_eq!(
            kinds_on(&config, WindowControlSide::Left),
            vec![
                WindowControlKind::MaximizeRestore,
                WindowControlKind::Minimize,
            ]
        );
        assert_eq!(
            kinds_on(&config, WindowControlSide::Right),
            vec![WindowControlKind::Close]
        );
    }

    #[test]
    fn move_down_swaps_with_next_same_side_control() {
        let mut config = WindowControlsConfig::default();

        assert!(config.move_within_side(
            WindowControlKind::Minimize,
            WindowControlMoveDirection::Down
        ));

        assert_eq!(
            kinds_on(&config, WindowControlSide::Right),
            vec![
                WindowControlKind::MaximizeRestore,
                WindowControlKind::Minimize,
                WindowControlKind::Close,
            ]
        );
    }

    #[test]
    fn move_up_swaps_with_previous_same_side_control() {
        let mut config = WindowControlsConfig::default();

        assert!(config.move_within_side(WindowControlKind::Close, WindowControlMoveDirection::Up));

        assert_eq!(
            kinds_on(&config, WindowControlSide::Right),
            vec![
                WindowControlKind::Minimize,
                WindowControlKind::Close,
                WindowControlKind::MaximizeRestore,
            ]
        );
    }

    #[test]
    fn move_past_side_boundary_reports_false() {
        let mut config = WindowControlsConfig::default();

        // The side's first row has no up neighbour and the last row no down
        // neighbour: the arrows are not rendered, so the model must refuse
        // too, otherwise a hidden click path could still mutate state.
        assert!(
            !config.move_within_side(WindowControlKind::Minimize, WindowControlMoveDirection::Up)
        );
        assert!(
            !config.move_within_side(WindowControlKind::Close, WindowControlMoveDirection::Down)
        );
        assert_eq!(
            kinds_on(&config, WindowControlSide::Right),
            WindowControlKind::ALL
        );
    }

    #[test]
    fn move_within_side_skips_foreign_side_entries() {
        // Pin the exact interleaved Vec [Close(L), Min(R), Max(R)]; plain
        // move_to_side appends to the Vec tail and cannot reach this order.
        let mut config = WindowControlsConfig::from_partial_placements(
            WindowChromeLayout::IntegratedNavigation,
            vec![
                WindowControlPlacement::new(
                    WindowControlKind::Close,
                    WindowControlSide::Left,
                    WindowControlVisibility::Visible,
                ),
                WindowControlPlacement::new(
                    WindowControlKind::Minimize,
                    WindowControlSide::Right,
                    WindowControlVisibility::Visible,
                ),
                WindowControlPlacement::new(
                    WindowControlKind::MaximizeRestore,
                    WindowControlSide::Right,
                    WindowControlVisibility::Visible,
                ),
            ],
        );

        // Ahead of Minimize the Vec holds only the foreign-side Close: the
        // nearest same-side scan must skip it and refuse, never swap across
        // sides.
        assert!(
            !config.move_within_side(WindowControlKind::Minimize, WindowControlMoveDirection::Up)
        );

        assert!(config.move_within_side(
            WindowControlKind::Minimize,
            WindowControlMoveDirection::Down
        ));

        // The swap happens between the two right-side entries; the left-side
        // Close keeps its Vec slot and order.
        assert_eq!(
            config
                .placements()
                .iter()
                .copied()
                .map(WindowControlPlacement::kind)
                .collect::<Vec<_>>(),
            vec![
                WindowControlKind::Close,
                WindowControlKind::MaximizeRestore,
                WindowControlKind::Minimize,
            ]
        );
        assert_eq!(
            kinds_on(&config, WindowControlSide::Left),
            vec![WindowControlKind::Close]
        );
    }

    #[test]
    fn move_within_side_leaves_other_side_untouched() {
        let mut config = WindowControlsConfig::default();
        assert!(config.move_to_side(WindowControlKind::Close, WindowControlSide::Left));
        // The shared Vec is now [Min(R), Max(R), Close(L)]: MaximizeRestore's
        // only Vec entry behind it is the left-side Close, so the down move
        // must be refused instead of crossing sides.
        assert!(!config.move_within_side(
            WindowControlKind::MaximizeRestore,
            WindowControlMoveDirection::Down
        ));

        assert!(config.move_within_side(
            WindowControlKind::MaximizeRestore,
            WindowControlMoveDirection::Up
        ));

        assert_eq!(
            kinds_on(&config, WindowControlSide::Left),
            vec![WindowControlKind::Close]
        );
        assert_eq!(
            kinds_on(&config, WindowControlSide::Right),
            vec![
                WindowControlKind::MaximizeRestore,
                WindowControlKind::Minimize,
            ]
        );
    }

    #[test]
    fn close_visibility_cannot_be_hidden() {
        let mut config = WindowControlsConfig::default();

        assert!(!config.set_visibility(WindowControlKind::Close, WindowControlVisibility::Hidden));
        assert!(config
            .placement(WindowControlKind::Close)
            .visibility()
            .is_visible());
    }
}
