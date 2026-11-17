//! vendor 的 cosmic-text 从 fontconfig 的缓存建系统字体列表（`fontconfig_faces`）：
//! 建出来的必须和 fontdb 自己解析字体文件得到的一样，排版才认得出同样的字体。

use iced::advanced::graphics::text::cosmic_text::fontconfig_faces::{
    parse_font_list, FONT_LIST_FORMAT,
};
use iced::advanced::graphics::text::cosmic_text::fontdb;

/// 按 `FONT_LIST_FORMAT` 拼一条记录。
fn record(fields: [&str; 8], families: &[(&str, &str)]) -> String {
    let families: String = families
        .iter()
        .map(|(family, language)| format!("{family}\u{1d}{language}\u{1c}"))
        .collect();
    format!("{}\u{1f}{families}\u{1e}", fields.join("\u{1f}"))
}

/// 字重、样式、宽度、等宽从 fontconfig 的刻度换回字体里的值；英文名排第一，别的语言的名字也留着。
#[test]
fn a_listed_font_becomes_the_face_its_file_describes() {
    let list = [
        record(
            [
                "/fonts/wqy-zenhei.ttc",
                "0",
                "False",
                "0",
                "80",
                "100",
                "",
                "WenQuanYiZenHei",
            ],
            &[
                ("文泉驿正黑", "zh-cn"),
                ("WenQuanYi Zen Hei", "en"),
                ("文泉驛正黑", "zh-tw"),
            ],
        ),
        record(
            [
                "/fonts/Mono-BoldOblique.TTF",
                "2",
                "False",
                "110",
                "200",
                "75",
                "100",
                "Mono-BoldOblique",
            ],
            &[("Mono", "en"), ("Mono Bold", "en"), ("Mono", "en")],
        ),
    ]
    .concat();
    let faces = parse_font_list(&list);
    assert_eq!(faces.len(), 2);

    let han = &faces[0];
    assert_eq!(
        han.families,
        [
            (
                "WenQuanYi Zen Hei".to_owned(),
                fontdb::Language::English_UnitedStates
            ),
            ("文泉驿正黑".to_owned(), fontdb::Language::Unknown),
            ("文泉驛正黑".to_owned(), fontdb::Language::Unknown),
        ]
    );
    assert_eq!(han.post_script_name, "WenQuanYiZenHei");
    assert_eq!(han.index, 0);
    assert_eq!(han.weight, fontdb::Weight::NORMAL);
    assert_eq!(han.style, fontdb::Style::Normal);
    assert_eq!(han.stretch, fontdb::Stretch::Normal);
    assert!(!han.monospaced);
    assert!(matches!(&han.source, fontdb::Source::File(path) if path.ends_with("wqy-zenhei.ttc")));

    let mono = &faces[1];
    let names: Vec<&str> = mono
        .families
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["Mono", "Mono Bold"]);
    assert_eq!(mono.index, 2);
    assert_eq!(mono.weight, fontdb::Weight::BOLD);
    // 名叫 Oblique、字体头标斜体的（DejaVu），fontdb 算 Italic；要 Italic 的字得找得到它。
    assert_eq!(mono.style, fontdb::Style::Italic);
    assert_eq!(mono.stretch, fontdb::Stretch::Condensed);
    assert!(mono.monospaced);
}

/// fontdb 解析不了的格式、可变字体的整体和命名实例、残缺的记录都不进列表；
/// 可变字体只留默认那一款。
#[test]
fn only_faces_that_fontdb_would_load_are_listed() {
    let family = [("Inter", "en")];
    let list = [
        record(
            [
                "/fonts/misc/6x13.pcf.gz",
                "0",
                "False",
                "0",
                "80",
                "100",
                "110",
                "Fixed",
            ],
            &family,
        ),
        record(
            [
                "/fonts/Inter.ttf",
                "0",
                "True",
                "0",
                "[0 210]",
                "100",
                "",
                "",
            ],
            &family,
        ),
        record(
            [
                "/fonts/Inter.ttf",
                "131072",
                "False",
                "0",
                "40",
                "100",
                "",
                "Inter-ExtraLight",
            ],
            &family,
        ),
        record(
            [
                "/fonts/Inter.ttf",
                "0",
                "False",
                "0",
                "80",
                "100",
                "",
                "Inter-Regular",
            ],
            &family,
        ),
        "/fonts/broken.ttf\u{1f}0\u{1e}".to_owned(),
        record(
            [
                "/fonts/Nameless.otf",
                "0",
                "False",
                "0",
                "80",
                "100",
                "",
                "Nameless",
            ],
            &[],
        ),
    ]
    .concat();
    let faces = parse_font_list(&list);
    let listed: Vec<&str> = faces
        .iter()
        .map(|face| face.post_script_name.as_str())
        .collect();
    assert_eq!(listed, ["Inter-Regular"]);
}

/// 同一个字体文件，fontconfig 描述的（`fc-scan` 与它缓存里存的是同一份）和 fontdb 解析的一致：
/// 名字、字重、样式、宽度都一样，fontdb 认得的家族名都在。
#[test]
fn fontconfig_describes_a_font_file_the_way_fontdb_parses_it() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("FiraSans-Regular.ttf");
    std::fs::write(&path, iced::advanced::graphics::text::FIRA_SANS_REGULAR).expect("write font");
    let scanned = std::process::Command::new("fc-scan")
        .arg("--format")
        .arg(FONT_LIST_FORMAT)
        .arg(&path)
        .output()
        .expect("fc-scan (fontconfig) is installed");
    let listed = parse_font_list(&String::from_utf8_lossy(&scanned.stdout));

    let mut database = fontdb::Database::new();
    database
        .load_font_file(&path)
        .expect("fontdb parses the font");
    let parsed: Vec<&fontdb::FaceInfo> = database.faces().collect();

    assert_eq!(listed.len(), parsed.len());
    for (listed, parsed) in listed.iter().zip(parsed) {
        assert_eq!(listed.families[0], parsed.families[0]);
        for (family, _) in &parsed.families {
            assert!(
                listed.families.iter().any(|(known, _)| known == family),
                "{family}"
            );
        }
        assert_eq!(listed.post_script_name, parsed.post_script_name);
        assert_eq!(listed.index, parsed.index);
        assert_eq!(listed.weight, parsed.weight);
        assert_eq!(listed.style, parsed.style);
        assert_eq!(listed.stretch, parsed.stretch);
        assert_eq!(listed.monospaced, parsed.monospaced);
    }
}
