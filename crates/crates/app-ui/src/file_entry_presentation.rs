#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionRunPosition {
    Single,
    First,
    Middle,
    Last,
}

impl SelectionRunPosition {
    pub(crate) fn from_neighbors(previous_selected: bool, next_selected: bool) -> Self {
        match (previous_selected, next_selected) {
            (false, false) => Self::Single,
            (false, true) => Self::First,
            (true, true) => Self::Middle,
            (true, false) => Self::Last,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_selection_run_positions_are_classified() {
        assert_eq!(
            SelectionRunPosition::from_neighbors(false, true),
            SelectionRunPosition::First
        );
        assert_eq!(
            SelectionRunPosition::from_neighbors(true, true),
            SelectionRunPosition::Middle
        );
        assert_eq!(
            SelectionRunPosition::from_neighbors(true, false),
            SelectionRunPosition::Last
        );
    }

    #[test]
    fn isolated_selected_path_is_single_run() {
        assert_eq!(
            SelectionRunPosition::from_neighbors(false, false),
            SelectionRunPosition::Single
        );
    }
}
