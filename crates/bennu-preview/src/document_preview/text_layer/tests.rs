use super::*;

const TWO_PAGES: &str = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd"><html xmlns="http://www.w3.org/1999/xhtml"><body><doc>
  <page width="500.000000" height="600.000000"><flow><block>
    <line><word xMin="50" yMin="60" xMax="150" yMax="90">Hello</word><word xMin="160" yMin="60" xMax="250" yMax="90">World&amp;Co</word></line>
    <line><word xMin="50" yMin="120" xMax="150" yMax="150">second</word><word xMin="160" yMin="120" xMax="250" yMax="150">line</word></line>
  </block></flow></page>
  <page width="600.000000" height="400.000000"><flow><block>
    <line><word xMin="100" yMin="100" xMax="200" yMax="130">hello</word><word xMin="210" yMin="100" xMax="300" yMax="130">again</word></line>
  </block></flow></page>
</doc></body></html>"#;

fn sample() -> DocumentText {
    // 第二页旋转过：显示出来 400×600，坐标已经是显示方向。
    parse_bbox_layout(TWO_PAGES, &[(500.0, 600.0), (400.0, 600.0)]).expect("parse")
}

#[test]
fn words_become_page_fractions_with_line_ends() {
    let text = sample();
    assert_eq!(text.pages.len(), 2);
    let first = &text.pages[0].words;
    assert_eq!(first[1].text, "World&Co");
    assert_eq!(
        first[0].bounds,
        PageRect {
            left: 0.1,
            top: 0.1,
            right: 0.3,
            bottom: 0.15
        }
    );
    assert!(!first[0].ends_line && first[1].ends_line && first[3].ends_line);
    assert_eq!(text.pages[1].words[0].bounds.left, 0.25);
}

#[test]
fn search_finds_phrases_across_words_and_respects_case() {
    let text = sample();
    let found = find_matches(&text, "hello", false);
    assert_eq!(
        found,
        vec![
            TextMatch {
                page: 0,
                first_word: 0,
                last_word: 0
            },
            TextMatch {
                page: 1,
                first_word: 0,
                last_word: 0
            }
        ]
    );
    assert_eq!(find_matches(&text, "hello", true).len(), 1);
    // 跨行的短语：上一行最后一个词接下一行第一个词。
    assert_eq!(
        find_matches(&text, "co  second", false),
        vec![TextMatch {
            page: 0,
            first_word: 1,
            last_word: 2
        }]
    );

    let mut search = DocumentSearch {
        query: "hello".to_owned(),
        ..DocumentSearch::default()
    };
    search.refresh(&text, 1);
    assert_eq!(search.current, Some(1));
    search.step(SearchDirection::Forward);
    assert_eq!(search.current, Some(0));
    search.step(SearchDirection::Backward);
    assert_eq!(search.current, Some(1));
}

#[test]
fn selection_copies_words_with_line_and_page_breaks() {
    let text = sample();
    let selection = DocumentSelection {
        anchor: (1, 0),
        focus: (0, 1),
    };
    assert_eq!(selection.words_on_page(0, &text), Some((1, 3)));
    assert_eq!(selection.words_on_page(1, &text), Some((0, 0)));
    assert_eq!(selection.text(&text), "World&Co\nsecond line\n\nhello");
    assert_eq!(word_near(&text.pages[0], 0.4, 0.12), Some(1));
}
