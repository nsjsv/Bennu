//! 最近提交过的输入，最新的在前：去掉首尾空白、同样的只留最新一条、最多 `LIMIT` 条。
//! 搜索框和超级输入框各用一份，规则相同。

use std::collections::HashSet;

pub(crate) const SEARCH_HISTORY_LIMIT: usize = 10;

pub(crate) type SearchHistory = InputHistory<SEARCH_HISTORY_LIMIT>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InputHistory<const LIMIT: usize> {
    entries: Vec<String>,
}

impl<const LIMIT: usize> InputHistory<LIMIT> {
    pub(crate) fn from_persisted(entries: Vec<String>) -> Self {
        let mut seen = HashSet::new();
        let entries = entries
            .into_iter()
            .map(|entry| entry.trim().to_owned())
            .filter(|entry| !entry.is_empty())
            .filter(|entry| seen.insert(entry.clone()))
            .take(LIMIT)
            .collect();
        Self { entries }
    }

    pub(crate) fn entries(&self) -> &[String] {
        &self.entries
    }

    pub(crate) fn contains(&self, keyword: &str) -> bool {
        self.entries.iter().any(|entry| entry == keyword)
    }

    pub(crate) fn record_submission(&mut self, keyword: &str) -> bool {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            return false;
        }

        if self.entries.first().is_some_and(|entry| entry == keyword) {
            return false;
        }
        self.entries.retain(|entry| entry != keyword);
        self.entries.insert(0, keyword.to_owned());
        self.entries.truncate(LIMIT);
        true
    }

    pub(crate) fn remove(&mut self, keyword: &str) -> bool {
        let original_len = self.entries.len();
        self.entries.retain(|entry| entry != keyword);
        self.entries.len() != original_len
    }

    pub(crate) fn clear(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        self.entries.clear();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_submissions_are_trimmed_and_kept_in_recent_order() {
        let mut history = SearchHistory::default();

        assert!(history.record_submission("  report  "));
        assert!(history.record_submission("images"));
        assert_eq!(history.entries(), ["images", "report"]);
        assert!(!history.record_submission("   "));
        assert_eq!(history.entries(), ["images", "report"]);
    }

    #[test]
    fn repeated_exact_keyword_moves_to_front_without_folding_case() {
        let mut history = SearchHistory::default();
        history.record_submission("Report");
        history.record_submission("report");
        history.record_submission("images");

        assert!(history.record_submission("Report"));
        assert_eq!(history.entries(), ["Report", "images", "report"]);
        assert!(!history.record_submission("Report"));
    }

    #[test]
    fn recording_an_eleventh_keyword_evicts_the_oldest() {
        let mut history = SearchHistory::default();
        for index in 0..=SEARCH_HISTORY_LIMIT {
            history.record_submission(&format!("keyword-{index}"));
        }

        assert_eq!(history.entries().len(), SEARCH_HISTORY_LIMIT);
        assert_eq!(history.entries().first().unwrap(), "keyword-10");
        assert!(!history.contains("keyword-0"));
    }

    #[test]
    fn persisted_entries_are_normalized_once_at_the_boundary() {
        let entries = vec![
            " report ".to_owned(),
            String::new(),
            "report".to_owned(),
            "Report".to_owned(),
        ]
        .into_iter()
        .chain((0..SEARCH_HISTORY_LIMIT).map(|index| format!("keyword-{index}")))
        .collect();

        let history = SearchHistory::from_persisted(entries);

        assert_eq!(history.entries().len(), SEARCH_HISTORY_LIMIT);
        assert_eq!(history.entries()[..2], ["report", "Report"]);
    }

    #[test]
    fn remove_and_clear_report_real_changes_only() {
        let mut history =
            SearchHistory::from_persisted(vec!["report".to_owned(), "images".to_owned()]);

        assert!(!history.remove("missing"));
        assert!(history.remove("report"));
        assert_eq!(history.entries(), ["images"]);
        assert!(history.clear());
        assert!(!history.clear());
    }
}
