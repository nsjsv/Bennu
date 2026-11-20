//! 每种模式下框里列什么。

use super::{find, matching, mode, targets, BoxContext, BoxMode, BoxRow, FindStatus, SuperBox};
use super::{FOUND_IN_SEARCH, MOST_ROWS};

impl SuperBox {
    pub(super) fn compute_rows(&self, context: &BoxContext<'_>) -> Vec<BoxRow> {
        let text = self.input.trim();
        if text.is_empty() {
            return self
                .history
                .iter()
                .filter(|entry| mode::read_history(entry).0 == self.mode)
                .take(MOST_ROWS)
                .map(|entry| BoxRow::History(entry.clone()))
                .collect();
        }
        match self.mode {
            BoxMode::Search => matching::ranked(&self.entries, text)
                .into_iter()
                .map(BoxRow::Entry)
                .chain(self.found_rows(FOUND_IN_SEARCH))
                .collect(),
            BoxMode::Files => {
                let query = match find::read_find_query(text) {
                    Ok(query) => query,
                    Err(reason) => return vec![BoxRow::Problem(reason)],
                };
                if query.terms.is_empty() {
                    return vec![BoxRow::Problem(tr!(
                        "Type what to look for; filters only narrow it down"
                    ))];
                }
                if let FindStatus::Unavailable(reason) = &self.find_status {
                    return vec![BoxRow::Problem(reason.clone())];
                }
                self.found_rows(MOST_ROWS).collect()
            }
            BoxMode::Path => {
                let Some(target) = targets::jump_path(text, context.current_dir, context.home)
                else {
                    return vec![BoxRow::Problem(tr!(
                        "Relative paths need a folder; type a path starting with / or ~"
                    ))];
                };
                let suggestions = self
                    .suggestions
                    .iter()
                    .filter(|suggestion| **suggestion != target)
                    .take(MOST_ROWS - 1)
                    .cloned()
                    .map(BoxRow::JumpSuggestion)
                    .collect::<Vec<_>>();
                std::iter::once(BoxRow::JumpTo(target))
                    .chain(suggestions)
                    .collect()
            }
            BoxMode::New => {
                if let Some(reason) = &context.cannot_create {
                    return vec![BoxRow::Problem(reason.clone())];
                }
                match targets::read_new_entry(text, context.taken) {
                    Ok(entry) => vec![BoxRow::Create(entry)],
                    Err(reason) => vec![BoxRow::Problem(reason)],
                }
            }
            BoxMode::Shell => match context.current_dir {
                Some(_) => vec![BoxRow::Run(text.to_owned())],
                None => vec![BoxRow::Problem(tr!(
                    "Commands run in a folder; open one first"
                ))],
            },
        }
    }

    fn found_rows(&self, most: usize) -> impl Iterator<Item = BoxRow> + '_ {
        (0..self.found.len().min(most)).map(BoxRow::Found)
    }
}
