const SEARCH_LOG_DETAIL_CHAR_LIMIT: usize = 1_000;

pub(crate) fn bounded_search_log_detail(detail: &str) -> String {
    let mut characters = detail.chars();
    let prefix = characters
        .by_ref()
        .take(SEARCH_LOG_DETAIL_CHAR_LIMIT)
        .collect::<String>();
    if characters.next().is_none() {
        return prefix;
    }

    let mut truncated = prefix
        .chars()
        .take(SEARCH_LOG_DETAIL_CHAR_LIMIT - 1)
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_log_detail_is_unicode_safe_and_bounded() {
        let detail = "界".repeat(SEARCH_LOG_DETAIL_CHAR_LIMIT + 1);
        let bounded = bounded_search_log_detail(&detail);

        assert_eq!(bounded.chars().count(), SEARCH_LOG_DETAIL_CHAR_LIMIT);
        assert!(bounded.ends_with('…'));
    }
}
