use super::markup_text;

#[test]
fn keeps_the_words_between_tags() {
    assert_eq!(
        markup_text("<html><head><title>季度报告</title></head><body><p>第一段</p><p>第二段</p></body></html>"),
        "季度报告 第一段 第二段"
    );
}

#[test]
fn inline_tags_do_not_split_words() {
    assert_eq!(
        markup_text("<p>he<b>ll</b>o <a href=\"x\">world</a></p>"),
        "hello world"
    );
}

#[test]
fn scripts_styles_comments_and_declarations_are_dropped() {
    let page = "<!DOCTYPE html><?xml version=\"1.0\"?><!-- hidden --><style>p{color:red}</style>\
                <p>shown</p><SCRIPT type=\"text/javascript\">var hidden = '<p>';</SCRIPT>after";
    assert_eq!(markup_text(page), "shown after");
}

#[test]
fn quoted_angle_brackets_stay_inside_the_tag() {
    assert_eq!(markup_text("<p title=\"a > b\">text</p>"), "text");
}

#[test]
fn entities_are_decoded_and_unknown_ones_kept() {
    assert_eq!(
        markup_text("<p>Tom &amp; Jerry &lt;3 &#20013;&#x6587; &copy; &unknown; &</p>"),
        "Tom & Jerry <3 中文 © &unknown; &"
    );
}

#[test]
fn cdata_keeps_its_text_verbatim() {
    assert_eq!(
        markup_text("<note><![CDATA[a <raw> & text]]></note>"),
        "a <raw> & text"
    );
}

#[test]
fn stray_angle_brackets_are_text() {
    assert_eq!(markup_text("1 < 2 and 3 <4"), "1 < 2 and 3 <4");
}

#[test]
fn whitespace_collapses_and_keeps_line_breaks() {
    assert_eq!(markup_text("<p>a   b</p>\n\n<p>c\td</p>"), "a b\nc d");
}

#[test]
fn svg_text_survives_and_path_data_does_not() {
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M0 0 L10 10\"/>\
               <text x=\"1\">图例</text></svg>";
    assert_eq!(markup_text(svg), "图例");
}

#[test]
fn unclosed_constructs_end_at_the_input() {
    assert_eq!(markup_text("before<!-- never closed"), "before");
    assert_eq!(markup_text("before<script>never closed"), "before");
    assert_eq!(markup_text("before<p class=\"x"), "before");
}
