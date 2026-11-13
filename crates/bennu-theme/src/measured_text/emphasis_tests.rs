// 区间数组就是“要加粗的片段”，只有一个片段的写法是本意，不是误写的序列。
#![allow(clippy::single_range_in_vec_init)]

use super::displayed_emphasis;
use crate::measured_text::MiddleEllipsisKind;

#[test]
fn whole_text_keeps_ranges() {
    assert_eq!(
        displayed_emphasis(
            "report.txt",
            "report.txt",
            MiddleEllipsisKind::GeneralText,
            &[0..3]
        ),
        vec![0..3]
    );
}

#[test]
fn ranges_follow_the_kept_head_and_tail() {
    // 8 个可见字符：头 4 尾 4。
    let content = "abcdefghijklmnop";
    let displayed = "abcd...mnop";
    assert_eq!(
        displayed_emphasis(
            content,
            displayed,
            MiddleEllipsisKind::GeneralText,
            &[1..3, 13..15]
        ),
        vec![1..3, 8..10]
    );
}

#[test]
fn range_inside_the_elided_middle_disappears() {
    let content = "abcdefghijklmnop";
    assert!(displayed_emphasis(
        content,
        "abcd...mnop",
        MiddleEllipsisKind::GeneralText,
        &[6..9]
    )
    .is_empty());
}

#[test]
fn range_across_the_cut_keeps_both_visible_parts() {
    let content = "abcdefghijklmnop";
    assert_eq!(
        displayed_emphasis(
            content,
            "abcd...mnop",
            MiddleEllipsisKind::GeneralText,
            &[2..14]
        ),
        vec![2..4, 7..9]
    );
}

#[test]
fn multibyte_text_maps_by_bytes() {
    // “项目文件夹名称”保留 4 个字：头 2 尾 2。
    let content = "项目文件夹名称";
    let displayed = "项目...名称";
    let emphasized = [content.find("名").unwrap()..content.len()];
    assert_eq!(
        displayed_emphasis(
            content,
            displayed,
            MiddleEllipsisKind::GeneralText,
            &emphasized
        ),
        vec!["项目...".len()..displayed.len()]
    );
}

#[test]
fn file_name_cut_keeps_the_extension_in_the_tail() {
    // 5 个可见字符时文件名省略至少留下整个“.txt”：头 1 尾 4。
    let content = "very-long-report-name.txt";
    let candidate = MiddleEllipsisKind::FileName.candidate(content, 5);
    assert_eq!(candidate, "v....txt");
    let emphasized = [content.len() - 3..content.len()];
    assert_eq!(
        displayed_emphasis(
            content,
            &candidate,
            MiddleEllipsisKind::FileName,
            &emphasized
        ),
        vec![candidate.len() - 3..candidate.len()]
    );
}
