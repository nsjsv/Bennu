use super::{BrowserPaneId, BrowserPaneLayout};

pub(crate) const SPLIT_DIVIDER_WIDTH: f32 = 8.0;
pub(crate) const SPLIT_MIN_PANE_SIZE: f32 = 160.0;
pub(crate) const SPLIT_PORTION_TOTAL: u16 = 1000;

impl BrowserPaneLayout {
    pub(crate) fn with_first_portion(self, first_portion: u16) -> Self {
        match self {
            Self::Single { .. } => self,
            Self::Split {
                axis,
                first,
                second,
                active,
                ..
            } => Self::Split {
                axis,
                first,
                second,
                active,
                first_portion: normalized_split_portion(first_portion),
            },
        }
    }

    pub(crate) fn first_portion(self) -> u16 {
        match self {
            Self::Single { .. } => SPLIT_PORTION_TOTAL / 2,
            Self::Split { first_portion, .. } => normalized_split_portion(first_portion),
        }
    }

    pub(crate) fn effective_split_portions(self, axis_extent: f32) -> (u16, u16) {
        let available = split_available_extent(axis_extent);
        if available < SPLIT_MIN_PANE_SIZE * 2.0 {
            return (SPLIT_PORTION_TOTAL / 2, SPLIT_PORTION_TOTAL / 2);
        }

        let minimum =
            ((SPLIT_MIN_PANE_SIZE / available) * SPLIT_PORTION_TOTAL as f32).ceil() as u16;
        let first = self
            .first_portion()
            .clamp(minimum, SPLIT_PORTION_TOTAL - minimum);
        (first, SPLIT_PORTION_TOTAL - first)
    }

    pub(crate) fn pane_extent(self, pane_id: BrowserPaneId, axis_extent: f32) -> f32 {
        split_available_extent(axis_extent) * self.portion_for_pane(pane_id, axis_extent) as f32
            / SPLIT_PORTION_TOTAL as f32
    }

    pub(crate) fn split_divider_center(self, axis_extent: f32) -> f32 {
        let (first, _) = self.effective_split_portions(axis_extent);
        split_available_extent(axis_extent) * first as f32 / SPLIT_PORTION_TOTAL as f32
            + SPLIT_DIVIDER_WIDTH / 2.0
    }

    fn portion_for_pane(self, pane_id: BrowserPaneId, axis_extent: f32) -> u16 {
        let (first, second) = self.effective_split_portions(axis_extent);
        match self {
            Self::Split {
                first: first_id, ..
            } if pane_id == first_id => first,
            Self::Split {
                second: second_id, ..
            } if pane_id == second_id => second,
            _ => SPLIT_PORTION_TOTAL,
        }
    }
}

fn split_available_extent(axis_extent: f32) -> f32 {
    (axis_extent - SPLIT_DIVIDER_WIDTH).max(0.0)
}

pub(super) fn normalized_split_portion(portion: u16) -> u16 {
    portion.clamp(1, SPLIT_PORTION_TOTAL - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SplitAxis;

    fn split(first_portion: u16) -> BrowserPaneLayout {
        BrowserPaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first: BrowserPaneId::PRIMARY,
            second: BrowserPaneId(1),
            active: BrowserPaneId::PRIMARY,
            first_portion,
        }
    }

    #[test]
    fn split_portions_enforce_minimum_pane_size() {
        let layout = split(1);
        let (first, second) = layout.effective_split_portions(1_000.0);
        let available = 1_000.0 - SPLIT_DIVIDER_WIDTH;

        assert!(available * first as f32 / SPLIT_PORTION_TOTAL as f32 >= SPLIT_MIN_PANE_SIZE);
        assert!(available * second as f32 / SPLIT_PORTION_TOTAL as f32 >= SPLIT_MIN_PANE_SIZE);
        assert_eq!(first + second, SPLIT_PORTION_TOTAL);
    }

    #[test]
    fn split_geometry_uses_one_pane_extent_and_divider_boundary() {
        let layout = split(700);
        let first_extent = layout.pane_extent(BrowserPaneId::PRIMARY, 1_000.0);
        let second_extent = layout.pane_extent(BrowserPaneId(1), 1_000.0);

        assert!((first_extent + second_extent + SPLIT_DIVIDER_WIDTH - 1_000.0).abs() < 0.01);
        assert_eq!(
            layout.split_divider_center(1_000.0),
            first_extent + SPLIT_DIVIDER_WIDTH / 2.0
        );
    }

    #[test]
    fn split_portions_fall_back_to_equal_when_window_is_too_small() {
        assert_eq!(split(900).effective_split_portions(300.0), (500, 500));
    }
}
