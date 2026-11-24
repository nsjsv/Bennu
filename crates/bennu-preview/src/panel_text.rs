//! 面板文本构造：主程序与文件选择器共用。

use iced::widget::text;
use iced::Theme;

/// 只收 `String`：界面文案先经 `tr!` 取好，文件名等用户数据原样传入；
/// 写成 `readable_text("Settings")` 编译不过，不会漏翻译。
pub fn readable_text<'a>(content: String) -> iced::widget::Text<'a, Theme> {
    let needs_advanced_shaping = needs_advanced_text_shaping(&content);
    let label = text(content);

    if needs_advanced_shaping {
        label.shaping(iced::widget::text::Shaping::Advanced)
    } else {
        label
    }
}

/// 一段界面文字大约多宽：全角字一个字号宽，其余 0.6 个字号，宁可估宽一点。给要按内容定宽、
/// 又不能让内容自己撑开的地方用——菜单项要铺满整行（悬停底色到右边），交给它们自己撑，
/// 弹出菜单就会铺满整个窗口；窄栏里也靠它决定先收起哪几项。
pub(crate) fn estimated_text_width(content: &str, size: f32) -> f32 {
    content
        .chars()
        .map(|character| {
            if is_wide_script(character) {
                size
            } else {
                size * 0.6
            }
        })
        .sum()
}

/// 非 ASCII（中文文件名、中文文案）要 Advanced shaping，否则显示成方框。
fn needs_advanced_text_shaping(content: &str) -> bool {
    !content.is_ascii()
}

/// 把文字切成全角 / 非全角交替的段（字节范围，是不是全角）。等宽排版（文本预览、Markdown
/// 代码）里全角文字（汉字、假名、谚文、全角标点）要换界面无衬线字体：等宽字体自带的 CJK
/// 字形常按「两格」排，一格 0.6 em 时一个汉字占 1.2 em，字间空隙明显过大。
pub(crate) fn wide_script_runs(content: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let mut runs = Vec::new();
    let mut run_start = 0;
    let mut run_is_wide = None;
    for (index, character) in content.char_indices() {
        let is_wide = is_wide_script(character);
        if run_is_wide.is_some_and(|wide| wide != is_wide) {
            runs.push((run_start..index, run_is_wide == Some(true)));
            run_start = index;
        }
        run_is_wide = Some(is_wide);
    }
    runs.push((run_start..content.len(), run_is_wide == Some(true)));
    runs
}

fn is_wide_script(character: char) -> bool {
    matches!(
        character,
        '\u{1100}'..='\u{11ff}'   // 谚文字母
            | '\u{2e80}'..='\u{2fdf}' // 部首
            | '\u{3000}'..='\u{30ff}' // 全角标点、假名
            | '\u{3100}'..='\u{31ff}' // 注音、谚文兼容、假名扩展
            | '\u{3400}'..='\u{4dbf}' // 汉字扩展 A
            | '\u{4e00}'..='\u{9fff}' // 汉字
            | '\u{a960}'..='\u{a97f}'
            | '\u{ac00}'..='\u{d7ff}' // 谚文音节
            | '\u{f900}'..='\u{faff}' // 兼容汉字
            | '\u{fe30}'..='\u{fe4f}' // 竖排标点
            | '\u{ff00}'..='\u{ffef}' // 全角/半角形式
            | '\u{20000}'..='\u{3134f}' // 汉字扩展 B–G
    )
}

#[cfg(test)]
mod tests {
    use super::{estimated_text_width, needs_advanced_text_shaping};

    #[test]
    fn full_width_characters_count_twice_as_wide_as_latin_ones() {
        assert_eq!(estimated_text_width("UTF-8", 10.0), 30.0);
        assert_eq!(estimated_text_width("简体中文", 10.0), 40.0);
        assert_eq!(
            estimated_text_width("第 1 行，共 45 行", 10.0),
            50.0 + 7.0 * 6.0
        );
    }

    #[test]
    fn ascii_text_keeps_fast_shaping() {
        assert!(!needs_advanced_text_shaping("Bennu"));
    }

    #[test]
    fn chinese_text_uses_advanced_shaping() {
        assert!(needs_advanced_text_shaping("设置"));
    }
}
